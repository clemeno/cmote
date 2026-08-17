// term/gate.rs — the one place cmote sits BETWEEN the parser and the engine (PLAN §102).
//
// Every other module under `term/` reads the byte stream a SECOND time, beside the engine, and acts
// on what the engine dropped. That shape was chosen deliberately and it is still the right one for
// almost everything: a scanner cannot break what the engine does, because it never stands in the
// engine's way.
//
// This module is the exception, and it exists because two things cannot be done from beside the
// stream:
//
//   READING BACK WHAT THE ENGINE DECIDED. The vertical scrolling region is private, with no
//   accessor and no reply arm (`term/region.rs` says what that cost). A scanner could watch DECSTBM
//   go past on the wire — but not the RESETS, which happen inside the engine on RIS and on resize.
//   Sitting on the `Handler` boundary catches all of them, because that is where the engine is TOLD.
//
//   STANDING IN FRONT OF A DECISION. Left and right margins change what PRINTING a character does:
//   the line breaks at the margin instead of at the screen edge. There is no sequence to translate
//   that into (§72's route) and no way to repair it afterwards, because by the time the text is on
//   the grid it is at the wrong columns. The only place to be is in front of `Handler::input`.
//
// `Processor::advance<H: Handler>` is generic over the handler and `Term` merely IMPLEMENTS
// `Handler`, so a type that holds `&mut Term` and implements the trait itself can be passed in its
// place. No fork, no patch, no reimplementation: the gate answers a dozen calls itself and hands the
// engine the other sixty.
//
// WHY THIS WAS REFUSED FOR SEVEN SECTIONS, AND WHAT CHANGED
//
// TERMINAL_COMPATIBILITY_PLAN §5 costed this build twice and turned it down both times, on one
// argument: **every method of `Handler` has a default empty body**, so a method the gate forgets to
// forward — or one a future `alacritty_terminal` ADDS — compiles cleanly and silently drops a
// sequence. §5 called that "the same class of hazard as §57's borrowed flag bit, except §57's could
// be caught at build time with a `const` assertion and this one cannot: a trait growing a defaulted
// method breaks nothing."
//
// That last clause was wrong, and the attribute on the `impl` below is why:
//
//   #[deny(clippy::missing_trait_methods)]
//
// The lint reports every method an `impl` leaves to its default. Denied on this one block, a method
// missing from the list — today's oversight or tomorrow's addition — is a **build error**, and
// cmote's gate runs `clippy --all-targets -- -D warnings`. The failure mode §5 refused the build
// over is now the one thing that cannot happen quietly.
//
// It fails loud from the other end too. If a future clippy DROPS the lint, the attribute names a
// lint that no longer exists, which is an `unknown_lints` warning, which `-D warnings` turns into an
// error. There is no version of the future where this file goes quiet on its own.
//
// What the guard does NOT catch is a method that is present and forwards WRONGLY, or one whose
// meaning changes inside the engine while its signature stays. The first is ordinary code and is
// tested like ordinary code; the second is the same exposure every other module here already runs,
// since all of them read engine state.
//
// THE FORWARDS ARE GENERATED, AND THAT IS A CORRECTNESS ARGUMENT
//
// Sixty-odd forwards written out by hand would be sixty chances to pass `count` where `mode` was
// meant. The `forward!` macro below takes a name and a signature and writes the body, so the only
// thing that can be got wrong is the signature — and a wrong signature does not compile, because the
// trait's does not match. The macro is not a saving of typing. It is the removal of a class of bug.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::vte::ansi::cursor_icon::CursorIcon;
use alacritty_terminal::vte::ansi::{
	Attr, CharsetIndex, ClearMode, CursorShape, CursorStyle, Handler, Hyperlink, KeyboardModes,
	KeyboardModesApplyBehavior, LineClearMode, Mode, ModifyOtherKeys, PrivateMode, Rgb,
	ScpCharPath, ScpUpdateMode, StandardCharset, TabulationClearMode,
};

use super::Engine;
use super::region::Region;

/// Write one `Handler` method that hands its arguments straight to the engine.
///
/// Each entry is the method's name and its parameter list, spelled exactly as the trait spells it.
/// Every method of `Handler` returns `()` and takes `&mut self`, so that shape is baked in and the
/// entry carries only what varies. See the module header for why this is generated rather than
/// written out: a wrong signature fails to compile, and a hand-written body that forwards the wrong
/// argument does not.
macro_rules! forward {
	($($name:ident($($argument:ident: $type:ty),*)),* $(,)?) => {
		$(
			fn $name(&mut self $(, $argument: $type)*) {
				self.term.$name($($argument),*);
			}
		)*
	};
}

/// The handler cmote passes to `Processor::advance` in the engine's place (§102).
///
/// Borrowed for the length of one advance and thrown away, so it holds no state of its own — the
/// state it maintains lives on `Terminal` and is borrowed in alongside the engine.
pub struct Gate<'a> {
	/// The engine, which still does all the work this gate does not do itself.
	term: &'a mut Engine,
	/// cmote's mirror of the engine's private vertical scrolling region (`term/region.rs`).
	region: &'a mut Region,
}

impl<'a> Gate<'a> {
	/// Borrow an engine and the state kept beside it for the length of one advance.
	pub fn new(term: &'a mut Engine, region: &'a mut Region) -> Self {
		Self { term, region }
	}
}

// The forwarding table and the handful of methods cmote answers itself. The `deny` is the whole
// reason this build was possible at all — see the module header.
#[deny(clippy::missing_trait_methods)]
impl Handler for Gate<'_> {
	/// DECSTBM — mirrored on the way past, then performed by the engine as it always was (§102).
	///
	/// The mirror is updated FIRST and unconditionally, including for a request the engine will
	/// reject: `Region::set` applies the engine's own `top >= bottom` test and leaves itself alone
	/// exactly where the engine leaves itself alone, so the two agree on malformed input as well as
	/// on good input. Then the call goes through untouched, because the engine is still the only
	/// thing that scrolls and it needs its own copy to do it with.
	fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
		self.region.set(top, bottom, self.term.screen_lines());
		self.term.set_scrolling_region(top, bottom);
	}

	/// RIS — the engine puts the region back to the whole page, so the mirror does too (§102).
	///
	/// This is one of the two reset paths a scanner beside the stream could never have seen. The
	/// bytes on the wire are `ESC c`, which says nothing about a scrolling region; that it clears one
	/// is a fact about the engine's insides, and the only place it surfaces is here.
	fn reset_state(&mut self) {
		self.term.reset_state();
		self.region.reset(self.term.screen_lines());
	}

	forward! {
		set_title(title: Option<String>),
		set_cursor_style(style: Option<CursorStyle>),
		set_cursor_shape(shape: CursorShape),
		input(c: char),
		goto(line: i32, col: usize),
		goto_line(line: i32),
		goto_col(col: usize),
		insert_blank(count: usize),
		move_up(lines: usize),
		move_down(lines: usize),
		identify_terminal(intermediate: Option<char>),
		device_status(argument: usize),
		move_forward(cols: usize),
		move_backward(cols: usize),
		move_down_and_cr(rows: usize),
		move_up_and_cr(rows: usize),
		put_tab(count: u16),
		backspace(),
		carriage_return(),
		linefeed(),
		bell(),
		substitute(),
		newline(),
		set_horizontal_tabstop(),
		scroll_up(lines: usize),
		scroll_down(lines: usize),
		insert_blank_lines(lines: usize),
		delete_lines(lines: usize),
		erase_chars(count: usize),
		delete_chars(count: usize),
		move_backward_tabs(count: u16),
		move_forward_tabs(count: u16),
		save_cursor_position(),
		restore_cursor_position(),
		clear_line(mode: LineClearMode),
		clear_screen(mode: ClearMode),
		clear_tabs(mode: TabulationClearMode),
		set_tabs(interval: u16),
		reverse_index(),
		terminal_attribute(attribute: Attr),
		set_mode(mode: Mode),
		unset_mode(mode: Mode),
		report_mode(mode: Mode),
		set_private_mode(mode: PrivateMode),
		unset_private_mode(mode: PrivateMode),
		report_private_mode(mode: PrivateMode),
		set_keypad_application_mode(),
		unset_keypad_application_mode(),
		set_active_charset(index: CharsetIndex),
		configure_charset(index: CharsetIndex, charset: StandardCharset),
		set_color(index: usize, color: Rgb),
		dynamic_color_sequence(prefix: String, index: usize, terminator: &str),
		reset_color(index: usize),
		clipboard_store(clipboard: u8, payload: &[u8]),
		clipboard_load(clipboard: u8, terminator: &str),
		decaln(),
		push_title(),
		pop_title(),
		text_area_size_pixels(),
		text_area_size_chars(),
		set_hyperlink(link: Option<Hyperlink>),
		set_mouse_cursor_icon(icon: CursorIcon),
		report_keyboard_mode(),
		push_keyboard_mode(mode: KeyboardModes),
		pop_keyboard_modes(to_pop: u16),
		set_keyboard_mode(mode: KeyboardModes, behavior: KeyboardModesApplyBehavior),
		set_modify_other_keys(mode: ModifyOtherKeys),
		report_modify_other_keys(),
		set_scp(char_path: ScpCharPath, update_mode: ScpUpdateMode),
	}
}
