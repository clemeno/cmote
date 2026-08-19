// taskbar.rs — mirror the active tab's command progress onto the Windows taskbar button (PLAN §54).
//
// A remote command reports how far along it is with OSC 9;4 (`term::progress`), and cmote draws that
// on the tab's own chip. This puts the SAME reading on the taskbar button, so a minimised window
// still shows a long build filling up — which is the reason Windows Terminal and ConEmu read the
// sequence at all.
//
// Only the ACTIVE tab is mirrored. There is one taskbar button and there can be many tabs, and no
// rule for combining them explains itself to the person looking at it; "the tab you are looking at"
// does. Every tab keeps its own bar on its own chip, which is where a background job is watched.
//
// ─── why this file is full of hand-written COM ───────────────────────────────────────────────────
//
// The taskbar's progress lives on `ITaskbarList3`, a COM interface. cmote depends on `windows-sys`,
// which ships bindings for plain functions, structs and constants — and NO COM interfaces at all: it
// has the `TaskbarList` CLSID and not one method to call on it. So the interface is declared here:
// the IID, and a vtable laid out in its true inheritance order.
//
// Getting that order right IS the correctness of this file, because a wrong slot calls a different
// method with our arguments:
//
//   IUnknown        QueryInterface, AddRef, Release              slots 0,1,2
//   ITaskbarList    HrInit, AddTab, DeleteTab, ActivateTab,
//                   SetActiveAlt                                 slots 3..7
//   ITaskbarList2   MarkFullscreenWindow                         slot  8
//   ITaskbarList3   SetProgressValue                              slot  9
//                   SetProgressState                              slot 10
//                   (RegisterTab, UnregisterTab, SetTabOrder, … after, unused)
//
// The alternative was taking the full `windows` crate — which generates those wrappers — for exactly
// two calls. That is a large dependency and a second Windows-bindings crate in a tree that already
// has one, against ~40 lines of declaration whose only risk is the table above.
//
// iced exposes no HWND, so `app`'s boot task hands it over — the same handle, from the same place, as
// `cursor::install` is given for §51's hand cursors. This module keeps its own copy rather than asking
// `cursor` for that one: a progress bar has no business depending on the cursor module, and the
// dependency would only exist to avoid storing one integer twice.

/// Take the window the progress will be shown on, at boot. A no-op off Windows.
pub fn attach(hwnd: isize) {
	#[cfg(windows)]
	platform::attach(hwnd);
	#[cfg(not(windows))]
	let _ = hwnd;
}

/// Put a reading on the taskbar button. Called on every update with the active tab's progress; the
/// platform layer drops a repeat, so this is cheap to call unconditionally and there is no need for
/// any caller to work out whether something changed.
///
/// A no-op off Windows. macOS has no equivalent (the Dock shows no per-app progress) and Linux would
/// mean a Unity/DBus launcher entry, which is a desktop-specific thing to build if either platform is
/// ever really targeted.
pub fn show(progress: crate::term::progress::Progress) {
	#[cfg(windows)]
	platform::show(progress);
	#[cfg(not(windows))]
	let _ = progress;
}

#[cfg(windows)]
mod platform {
	use std::ffi::c_void;
	use std::ptr;
	use std::sync::{Mutex, OnceLock};

	use windows_sys::Win32::Foundation::{HWND, S_OK};
	use windows_sys::Win32::System::Com::{
		CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
	};
	use windows_sys::Win32::UI::Shell::TaskbarList;
	use windows_sys::core::GUID;

	use crate::term::progress::Progress;

	/// `IID_ITaskbarList3`, from `ShObjIdl_core.h`. The CLSID that names the object comes from
	/// `windows-sys` (`TaskbarList`); only the interface id has to be written out, because the crate
	/// declares no interfaces.
	const IID_ITASKBARLIST3: GUID = GUID::from_u128(0xea1afb91_9e28_4b86_90e9_9e9f8a5eefaf);

	// The taskbar's progress states (`TBPFLAG`). They map onto OSC 9;4's `st` values one for one,
	// which is unsurprising — the sequence was invented to drive exactly this.
	const TBPF_NOPROGRESS: u32 = 0x0;
	const TBPF_INDETERMINATE: u32 = 0x1;
	const TBPF_NORMAL: u32 = 0x2;
	const TBPF_ERROR: u32 = 0x4;
	const TBPF_PAUSED: u32 = 0x8;

	/// The share the bar is scaled against. `SetProgressValue` takes a completed/total pair rather
	/// than a percentage, so the total is simply 100 and the value is the share itself.
	const TOTAL: u64 = 100;

	/// The slice of `ITaskbarList3`'s vtable this file calls, in the interface's true layout order —
	/// see the table in the module header. Every entry up to the ones we use must be present and the
	/// right shape, because a vtable is addressed by POSITION: dropping an unused earlier slot would
	/// silently shift `SetProgressValue` onto `MarkFullscreenWindow`.
	///
	/// The unused slots are typed as bare pointers rather than as their real signatures. They are
	/// never called, so their argument lists would be decoration that could only be wrong.
	#[repr(C)]
	struct Vtbl {
		query_interface: *const c_void, // IUnknown, slot 0
		add_ref: *const c_void,         // slot 1
		release: unsafe extern "system" fn(*mut Interface) -> u32, // slot 2
		hr_init: unsafe extern "system" fn(*mut Interface) -> i32, // ITaskbarList, slot 3
		add_tab: *const c_void,         // slot 4
		delete_tab: *const c_void,      // slot 5
		activate_tab: *const c_void,    // slot 6
		set_active_alt: *const c_void,  // slot 7
		mark_fullscreen_window: *const c_void, // ITaskbarList2, slot 8
		set_progress_value: unsafe extern "system" fn(*mut Interface, HWND, u64, u64) -> i32, // ITaskbarList3, slot 9
		set_progress_state: unsafe extern "system" fn(*mut Interface, HWND, u32) -> i32,      // slot 10
	}

	/// A COM object is a pointer to a pointer to its vtable, which is all this needs to be.
	#[repr(C)]
	struct Interface {
		vtable: *const Vtbl,
	}

	/// The interface pointer and the last reading we sent, together under one lock.
	///
	/// They are together on purpose: sending a repeat is not merely wasteful but visible — Windows
	/// restarts the bar's shimmer each time a state is set, so re-sending "normal, 40%" every update
	/// would make a stalled command look busy. Held as `isize` because a raw pointer is not `Send`;
	/// the object is created once and lives as long as the process.
	static TASKBAR: Mutex<(Option<isize>, Option<Progress>)> = Mutex::new((None, None));

	/// The window the bar belongs to, handed over once at boot. Held as `isize` for the same reason
	/// the interface is: a raw `HWND` is a pointer and so not `Sync`.
	static WINDOW: OnceLock<isize> = OnceLock::new();

	/// Remember the window. Called from the boot task, on the thread that owns it.
	pub fn attach(hwnd: isize) {
		if hwnd != 0 {
			let _ = WINDOW.set(hwnd);
		}
	}

	/// Mirror a reading, skipping one that is already showing.
	pub fn show(progress: Progress) {
		let Ok(mut held) = TASKBAR.lock() else {
			// A poisoned lock means another thread panicked mid-call. The taskbar is decoration, so
			// giving up on it is the whole of the right response.
			return;
		};
		if held.1 == Some(progress) {
			return;
		}
		let Some(&hwnd) = WINDOW.get() else {
			// The window is not up yet — this is reachable before the boot task has run. Nothing is
			// recorded, so the reading is sent again on the next update, once there is a window.
			return;
		};
		let Some(taskbar) = interface(&mut held.0) else {
			return;
		};
		if apply(taskbar, hwnd as HWND, progress) {
			held.1 = Some(progress);
		}
	}

	/// Set the state, and the value when there is one to set.
	///
	/// The order matters: the state is set FIRST, because setting a value on a button still in
	/// `TBPF_NOPROGRESS` is discarded — the bar has to exist before it can be filled. Reports whether
	/// both calls succeeded, so a failure is not remembered as the reading now on screen.
	fn apply(taskbar: *mut Interface, hwnd: HWND, progress: Progress) -> bool {
		let state = state_for(progress);
		// SAFETY: `taskbar` came from a successful `CoCreateInstance` for `IID_ITaskbarList3` and is
		// only ever used behind the mutex above, so the vtable is that interface's and the calls are
		// serialized. `hwnd` is iced's own window, taken from the handle `cursor` stashed. The two
		// slots called are the ones the module header's table names.
		unsafe {
			let vtable = &*(*taskbar).vtable;
			if (vtable.set_progress_state)(taskbar, hwnd, state) != S_OK {
				return false;
			}
			// `Indeterminate` and `None` have no share to send — a value would contradict the state.
			match progress.percent() {
				Some(share) => {
					(vtable.set_progress_value)(taskbar, hwnd, u64::from(share), TOTAL) == S_OK
				}
				None => true,
			}
		}
	}

	/// Which `TBPFLAG` a reading becomes. Split out from `apply` for one reason: it is the part of this
	/// file a test can actually reach. Everything around it needs a live window and a shell to talk to,
	/// so the COM calls themselves are only exercised by running the program — but a wrong constant
	/// here would show a red error bar for a paused command, or nothing at all for a working one, and
	/// that failure is silent rather than loud.
	fn state_for(progress: Progress) -> u32 {
		match progress {
			Progress::None => TBPF_NOPROGRESS,
			Progress::Indeterminate => TBPF_INDETERMINATE,
			Progress::Working(_) => TBPF_NORMAL,
			Progress::Failed(_) => TBPF_ERROR,
			Progress::Paused(_) => TBPF_PAUSED,
		}
	}

	/// The interface, created on first use and kept. `None` when COM or the shell will not give it to
	/// us, which is not worth retrying every update — but is not cached as a failure either, since the
	/// cost of trying again is one call that returns an error.
	fn interface(held: &mut Option<isize>) -> Option<*mut Interface> {
		if let Some(pointer) = held {
			return Some(*pointer as *mut Interface);
		}
		// SAFETY: both calls are the documented way to obtain this object. `CoInitializeEx` is called
		// on the thread that will use it (the UI thread, which is where `update` runs) and its result
		// is deliberately unchecked: `S_FALSE` means COM was already initialised here — by winit, as
		// it happens — which is a success for our purposes, and `RPC_E_CHANGED_MODE` means someone
		// chose a different apartment, under which these calls still work.
		let taskbar = unsafe {
			let _ = CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED as u32);
			let mut created: *mut c_void = ptr::null_mut();
			let status = CoCreateInstance(
				&TaskbarList,
				ptr::null_mut(),
				CLSCTX_INPROC_SERVER,
				&IID_ITASKBARLIST3,
				&raw mut created,
			);
			if status != S_OK || created.is_null() {
				return None;
			}
			created.cast::<Interface>()
		};
		// SAFETY: `taskbar` is the interface just created. `HrInit` is slot 3 and must be called
		// before any other method on it; a failure means the shell is not ready to talk to us, and
		// the object is released rather than kept in a state where nothing would work.
		unsafe {
			let vtable = &*(*taskbar).vtable;
			if (vtable.hr_init)(taskbar) != S_OK {
				(vtable.release)(taskbar);
				return None;
			}
		}
		*held = Some(taskbar as isize);
		Some(taskbar)
	}

	#[cfg(test)]
	mod tests {
		use super::*;

		#[test]
		fn every_reading_maps_to_its_own_taskbar_state() {
			// The five `TBPFLAG` values are a bit field, so a typo lands on a real-but-wrong state
			// rather than failing: 0x4 (error) and 0x8 (paused) are one digit apart and mean opposite
			// things to the person looking at the button.
			assert_eq!(state_for(Progress::None), 0x0);
			assert_eq!(state_for(Progress::Indeterminate), 0x1);
			assert_eq!(state_for(Progress::Working(40)), 0x2);
			assert_eq!(state_for(Progress::Failed(40)), 0x4);
			assert_eq!(state_for(Progress::Paused(40)), 0x8);
		}

		#[test]
		fn the_states_with_no_share_send_no_value() {
			// `apply` sends a value only when there is one, because a value contradicts
			// `TBPF_NOPROGRESS` and `TBPF_INDETERMINATE` — neither has a position on the bar.
			assert_eq!(Progress::None.percent(), None);
			assert_eq!(Progress::Indeterminate.percent(), None);
			assert_eq!(Progress::Working(40).percent(), Some(40));
		}

		#[test]
		fn a_window_that_was_never_given_is_not_remembered() {
			// A null HWND means iced gave us no Win32 handle; storing it would leave every later call
			// addressing window 0.
			attach(0);
			assert_eq!(WINDOW.get(), None);
		}
	}
}
