// local/shells.rs — which shells this machine can open, and where they live (PLAN §103).
//
// The home screen's Local bar is a row of buttons, one per shell cmote can start HERE. That row is
// not a fixed list: a machine with no PowerShell 7 installed must not offer a button that fails, and
// a machine without Git for Windows must not offer Git Bash. So the bar is built from
// [`catalogue`], which looks for each candidate and returns only the ones it found.
//
// Two rules shape the search, and both are about not launching the wrong program.
//
//   1. **Known locations and `PATH`, never user text.** A shell is found by looking where Windows
//      and the installers put it, or by walking `PATH` for a known file name. Nothing the user types
//      reaches this — the connect form has no field for it, and a target cannot carry one — so
//      there is no place a crafted string could name a program of its own choosing. The path that
//      crosses the channel to the session task is one cmote resolved itself.
//   2. **`System32\bash.exe` is excluded on purpose.** That name is WSL's launcher, not Git Bash:
//      running it starts a Linux distribution in a VM, whose filesystem the local file panes beside
//      it would be describing wrongly (they show THIS machine's drives). A `bash.exe` found on
//      `PATH` is therefore only accepted when it sits in a Git installation's `bin`, which is what
//      [`git_bash`] checks. Finding nothing is the right answer; guessing is not.
//
// The catalogue is built on the GUI side rather than in the session task, which is deliberate: the
// bar can then only offer what exists, and the task never has to search — it is handed a program and
// an argument list and spawns exactly that.

use std::path::{Path, PathBuf};

/// What every shell cmote can start understands as "leave now" (§104).
///
/// One word for all six, which is the rare case where the four dialects agree — `cd` needed a method
/// per shell and this needs none. `exit` at a prompt is the shell's own way out, so it runs whatever
/// that shell runs on the way: PSReadLine's history flush, a `~/.bash_logout`, an `EXIT` trap the user
/// set. (cmd's `exit` takes a `/b` for leaving a batch file without leaving the interpreter; a shell at
/// its prompt is not in one, so the bare word is right.)
///
/// It is TYPED, so it is only ever sent when typing makes sense — see `Tab::end_session`, which will not
/// type at a full-screen program.
pub const QUIT_COMMAND: &str = "exit";

/// Which shell a button opens. The label and the session's endpoint text come off this, so it is
/// what tells `pwsh` from `powershell` on screen — the two are different programs with different
/// syntax, and a status bar that called both "PowerShell" would be lying about which one is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
	/// PowerShell 7+ (`pwsh.exe`), the cross-platform one — installed separately, so often absent.
	Pwsh,
	/// Windows PowerShell 5.1 (`powershell.exe`), the one that ships with Windows.
	PowerShell,
	/// The Windows command interpreter (`cmd.exe`).
	Cmd,
	/// Git for Windows' bash, the MSYS2 one — NOT WSL's (see the module note).
	GitBash,
	/// The macOS default shell (`zsh`).
	Zsh,
	/// `bash` on macOS, where the name is unambiguous (there is no WSL to shadow it).
	Bash,
}

impl Kind {
	/// What the button says. Spelled the way the vendor spells it, so "Windows PowerShell" and
	/// "PowerShell 7" read as the two distinct programs they are.
	pub fn label(self) -> &'static str {
		match self {
			Self::Pwsh => "PowerShell 7",
			Self::PowerShell => "Windows PowerShell",
			Self::Cmd => "Command Prompt",
			Self::GitBash => "Git Bash",
			Self::Zsh => "zsh",
			Self::Bash => "bash",
		}
	}

	/// The short name the terminal's status bar shows in place of an endpoint (§10). A remote session
	/// puts `user@host:port` there; a local one has no such thing to say, so it says which machine
	/// ("local") and which shell — the one fact that distinguishes two local tabs from each other.
	pub fn slug(self) -> &'static str {
		match self {
			Self::Pwsh => "pwsh",
			Self::PowerShell => "powershell",
			Self::Cmd => "cmd",
			Self::GitBash => "git bash",
			Self::Zsh => "zsh",
			Self::Bash => "bash",
		}
	}

	/// Whether this shell ends itself when it is sent EOF — the `0x04` a Ctrl+D produces (§104).
	///
	/// On a POSIX shell that is how you log out: at an empty prompt, "no more input" means there is
	/// nothing left to read, so the shell exits, the session ends, and the tab lands back on the home
	/// screen — where a second Ctrl+D closes it (§30). The three Windows shells do not play: their EOF
	/// is Ctrl+Z, and it only ever means EOF to a program *reading a stream*, never to the interpreter
	/// itself. `0x04` reaches them and is simply dropped.
	///
	/// That was measured rather than assumed. A probe drove a real ConPTY child of each of `pwsh`,
	/// `powershell` and `cmd`, waited for the prompt, wrote one `0x04`, and watched the child handle for
	/// six seconds: none of the three exited, and none printed anything either. So on those three the
	/// key does nothing at all, which is what `app` fills in — see `Tab::end_local_shell`.
	pub fn quits_on_eof(self) -> bool {
		match self {
			// MSYS bash and the macOS shells are POSIX shells and log out on EOF.
			Self::GitBash | Self::Zsh | Self::Bash => true,
			// The interpreters that ignore the byte.
			Self::Pwsh | Self::PowerShell | Self::Cmd => false,
		}
	}

	/// The command line that moves THIS shell to `pane` — the Sync button, the tree's and the pane's
	/// "Open in terminal", the tree's Enter key (§19, §103).
	///
	/// This is the one place a local session cannot borrow the remote behaviour. `app` types
	/// `cd '<pane path>'` for a remote, which is right because the remote is a POSIX shell and the pane
	/// path is already its dialect. Here neither half holds: the path has to be translated
	/// (`local::path`), and the four shells disagree about how to spell the command AND about what a
	/// path even looks like.
	///
	///   * **cmd** needs `/d`, or a `cd` to another drive changes the current directory ON that drive
	///     and leaves the prompt where it was — a no-op that looks like a bug.
	///   * **PowerShell** gets `Set-Location -LiteralPath` rather than `cd`: `cd` there is an alias for
	///     the same cmdlet but goes through `-Path`, which treats `[`, `]` and `?` as wildcards. A folder
	///     with a bracket in its name is not exotic, and a wildcard that matches nothing is an error
	///     rather than a move.
	///   * **Git Bash** is an MSYS shell, so it wants `/c/Users/cme` and not `C:\Users\cme`.
	///   * **zsh / bash** on macOS take the path as it already is.
	///
	/// `None` when the path will not translate at all — the virtual root has no directory to move to,
	/// which is the honest answer rather than a `cd` to somewhere invented.
	///
	/// Every one of these puts the path in QUOTES, and that is the whole of the injection story: the
	/// path comes from a listing of the user's own disk, and it is typed at the user's own shell, so
	/// there is no privilege boundary being crossed — but a folder named `a & del b` would still run as
	/// two commands unquoted, and the user would not have asked for that.
	pub fn cd(self, pane: &str) -> Option<String> {
		let native = super::path::to_native(pane)?;
		let path = native.to_string_lossy().into_owned();
		Some(match self {
			// Doubling `"` is not an escape in cmd — there is none — but `"` is not legal in a Windows
			// file name either, so a quoted path cannot contain one and nothing needs escaping.
			Self::Cmd => format!("cd /d \"{path}\""),
			// A single-quoted PowerShell string takes everything literally; the only escape it has is a
			// doubled `'`, which cannot occur in a path Windows allows but costs nothing to honour.
			Self::Pwsh | Self::PowerShell => {
				format!("Set-Location -LiteralPath '{}'", path.replace('\'', "''"))
			}
			// MSYS spelling, and then the ordinary POSIX quoting every other shell here uses.
			Self::GitBash => format!("cd {}", crate::explorer::shell_quote(&msys(&path))),
			Self::Zsh | Self::Bash => format!("cd {}", crate::explorer::shell_quote(&path)),
		})
	}
}

/// A Windows path in the spelling an MSYS shell (Git Bash) understands: `C:\Users\cme` becomes
/// `/c/Users/cme`.
///
/// Only the drive letter moves; the separators flip and nothing else changes. A path that is not
/// drive-rooted is handed back with its separators flipped and no invented prefix — it cannot arrive
/// here (`local::path::to_native` only ever produces drive-rooted paths) and guessing would be worse
/// than passing it through for the shell itself to reject.
fn msys(path: &str) -> String {
	let flipped = path.replace('\\', "/");
	let bytes = flipped.as_bytes();
	if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
		let letter = flipped[..1].to_lowercase();
		return format!("/{letter}{}", &flipped[2..]);
	}
	flipped
}

/// One shell this machine can actually start: what it is, the program to run, and the arguments it
/// wants. Owned and `Clone`, because it crosses the command channel to the session task (§4).
///
/// The arguments are part of the entry rather than derived from the kind at spawn time, because only
/// one shell needs any and the reason is specific to it (see [`git_bash`]).
#[derive(Debug, Clone)]
pub struct Shell {
	pub kind: Kind,
	pub program: PathBuf,
	pub args: Vec<String>,
}

impl Shell {
	/// A shell with no arguments — the ordinary case: an interactive shell started with a pty needs
	/// nothing told to it, because the pty is what makes it interactive.
	fn plain(kind: Kind, program: PathBuf) -> Self {
		Self {
			kind,
			program,
			args: Vec::new(),
		}
	}

	/// What the status bar shows for a session running this shell (§10). Not a real endpoint and
	/// deliberately not shaped like one: `user@host:port` would invite the reader to believe there is
	/// a machine on the other end of a connection.
	pub fn endpoint(&self) -> String {
		format!("local — {}", self.kind.slug())
	}
}

/// Every shell found on this machine, in the order the Local bar shows them.
///
/// The order is fixed rather than sorted: the buttons should not move between machines, and the
/// leftmost one is the one most people want. On Windows that is PowerShell 7 when it is installed
/// and Windows PowerShell when it is not — which is why the missing ones are simply left out of the
/// row rather than shown disabled. A button that cannot work teaches nothing.
///
/// Searched **once per run** and kept. That is not an optimisation, it is a correctness requirement:
/// the home screen is redrawn on every frame, and a search that touched the disk each time would put
/// a dozen `is_file` probes into the paint loop. A shell installed while cmote is running therefore
/// needs a restart to appear, which is a price worth naming and not worth paying per frame.
pub fn catalogue() -> &'static [Shell] {
	static FOUND: std::sync::OnceLock<Vec<Shell>> = std::sync::OnceLock::new();
	FOUND.get_or_init(|| candidates().into_iter().flatten().collect())
}

/// The Windows candidates, each `Some` only if its program is really there.
#[cfg(windows)]
fn candidates() -> Vec<Option<Shell>> {
	vec![pwsh(), windows_powershell(), cmd(), git_bash()]
}

/// The macOS candidates. `pwsh` is offered here too — PowerShell 7 is cross-platform, and someone
/// who installed it on a Mac means to use it.
#[cfg(target_os = "macos")]
fn candidates() -> Vec<Option<Shell>> {
	vec![
		unix_shell(Kind::Zsh, "zsh"),
		unix_shell(Kind::Bash, "bash"),
		pwsh(),
	]
}

/// PowerShell 7 (`pwsh`). Installed separately from Windows, and by several routes — the MSI, the
/// Store, `winget`, a portable unzip on `PATH` — so `PATH` is tried first and the MSI's own
/// directories after it.
fn pwsh() -> Option<Shell> {
	let name = if cfg!(windows) { "pwsh.exe" } else { "pwsh" };
	if let Some(found) = on_path(name) {
		return Some(Shell::plain(Kind::Pwsh, found));
	}
	// The MSI's layout: `%ProgramFiles%\PowerShell\7\pwsh.exe`. The major version is in the path, so
	// it is walked rather than guessed — 7 today, 8 tomorrow, and a machine can hold both.
	for base in program_files() {
		let versions = base.join("PowerShell");
		let Ok(entries) = std::fs::read_dir(&versions) else {
			continue;
		};
		// Highest version first, so a machine with 7 and 8 installed opens the newer one. The names
		// are plain integers in practice; sorting them as text is close enough and never panics.
		let mut found: Vec<PathBuf> = entries
			.filter_map(Result::ok)
			.map(|entry| entry.path().join(name))
			.filter(|candidate| candidate.is_file())
			.collect();
		found.sort();
		if let Some(newest) = found.pop() {
			return Some(Shell::plain(Kind::Pwsh, newest));
		}
	}
	None
}

/// Windows PowerShell 5.1, the one in the box. Its path is fixed by Windows itself, so it is named
/// rather than searched for — and `%SystemRoot%` rather than a hard-coded `C:\Windows`, because the
/// drive and the folder are both installer choices.
#[cfg(windows)]
fn windows_powershell() -> Option<Shell> {
	let root = std::env::var_os("SystemRoot").map(PathBuf::from)?;
	let program = root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
	program
		.is_file()
		.then(|| Shell::plain(Kind::PowerShell, program))
}

/// `cmd.exe`. `%ComSpec%` is Windows' own answer to "what is the command interpreter", so it is
/// asked first and `%SystemRoot%` is the fallback for the rare environment that unsets it.
#[cfg(windows)]
fn cmd() -> Option<Shell> {
	let from_env = std::env::var_os("ComSpec").map(PathBuf::from);
	let from_root = std::env::var_os("SystemRoot")
		.map(PathBuf::from)
		.map(|root| root.join(r"System32\cmd.exe"));
	let program = from_env
		.into_iter()
		.chain(from_root)
		.find(|candidate| candidate.is_file())?;
	Some(Shell::plain(Kind::Cmd, program))
}

/// Git for Windows' bash — the MSYS2 one, never WSL's (see the module note).
///
/// Two ways in, and both end at a path INSIDE a Git installation, which is the whole check:
///   * the installer's own directories, and
///   * `git.exe` on `PATH`, whose grandparent is the install root — the route that finds a Git
///     installed somewhere cmote does not know to look.
///
/// A bare `bash.exe` on `PATH` is deliberately never accepted: on a default Windows that name
/// resolves to `System32\bash.exe`, which is WSL.
///
/// `--login -i` because that is what the Git Bash shortcut passes. Without `--login` the MSYS2
/// startup files never run, so `PATH` has no Unix tools on it and the shell is a bash that cannot
/// find `ls` — a shell that starts and is useless is worse than no button.
#[cfg(windows)]
fn git_bash() -> Option<Shell> {
	let program = git_bash_path()?;
	Some(Shell {
		kind: Kind::GitBash,
		program,
		args: vec!["--login".to_owned(), "-i".to_owned()],
	})
}

/// Where Git for Windows' `bash.exe` is, or `None`.
#[cfg(windows)]
fn git_bash_path() -> Option<PathBuf> {
	// The installer's places: per-machine under either Program Files, and per-user under
	// `%LOCALAPPDATA%\Programs\Git` (the "install for me only" option).
	let mut roots: Vec<PathBuf> = program_files()
		.into_iter()
		.map(|base| base.join("Git"))
		.collect();
	if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
		roots.push(local.join(r"Programs\Git"));
	}
	// A Git that is on `PATH` but installed elsewhere: `<root>\cmd\git.exe`, so the root is two
	// levels up. This is the only place `PATH` takes part, and it takes part by naming `git`, never
	// `bash` — so WSL's launcher can never be what is found.
	if let Some(git) = on_path("git.exe")
		&& let Some(root) = git.parent().and_then(Path::parent)
	{
		roots.push(root.to_path_buf());
	}
	// And the root its own installer wrote down (§105), which is the only search that finds a Git
	// installed somewhere of the user's choosing and kept off `PATH`.
	roots.extend(recorded_git_roots());
	roots
		.into_iter()
		.map(|root| root.join(r"bin\bash.exe"))
		.find(|candidate| candidate.is_file())
}

/// Where the Git for Windows installer says it put itself, per machine and per user (§105).
///
/// The other two searches both miss a Git that is neither in a Program Files folder nor reachable
/// through `PATH` — and that is not a corner case: the installer lets you choose any directory, and its
/// "use Git from Git Bash only" PATH option deliberately leaves `git.exe` off `PATH`. On the machine this
/// was written for, Git lives in `C:\git` and nothing about it is on `PATH`, so the Local bar had no Git
/// Bash button on a machine that has Git Bash — the one shell whose Ctrl+D behaviour §104 was written
/// against could not be opened at all.
///
/// This is still a **known location**, in the sense the module note means: one fixed key, written by the
/// installer and not by anything the user types at cmote, and whatever comes out of it still has to be a
/// `bin\bash.exe` that exists before a button is offered. `HKLM` is a per-machine install and `HKCU` the
/// "for me only" one; a machine with both gets the per-machine one first, which is the order the
/// installer itself prefers.
#[cfg(windows)]
fn recorded_git_roots() -> Vec<PathBuf> {
	use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

	[HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER]
		.into_iter()
		.filter_map(|root| registry_string(root, r"SOFTWARE\GitForWindows", "InstallPath"))
		.collect()
}

/// One `REG_SZ` value, as a path — `None` if the key is absent, the value is absent, it is not a string,
/// or it is empty.
///
/// A missing key is the ordinary answer here (a machine with no Git for Windows), so nothing is logged:
/// this is a search, and finding nothing is a result.
#[cfg(windows)]
fn registry_string(
	root: windows_sys::Win32::System::Registry::HKEY,
	subkey: &str,
	value: &str,
) -> Option<PathBuf> {
	use std::os::windows::ffi::OsStringExt;
	use windows_sys::Win32::Foundation::ERROR_SUCCESS;
	use windows_sys::Win32::System::Registry::{RRF_RT_REG_SZ, RegGetValueW};

	let subkey = wide(subkey);
	let value = wide(value);
	// Room to spare for a path: `MAX_PATH` is 260 units, and an install root long enough to overflow
	// this could not be typed into the installer's own dialog. An oversize value fails the call with
	// `ERROR_MORE_DATA` and is treated as "not found", which is the honest answer for a value cmote
	// cannot read rather than a truncated path it would then try to run.
	let mut buffer = [0u16; 512];
	let mut bytes = std::mem::size_of_val(&buffer) as u32;
	// SAFETY: both names are NUL-terminated wide strings owned by this frame, so they outlive the call.
	// The data pointer and `bytes` describe `buffer` exactly — `RegGetValueW` writes at most that many
	// bytes and replaces `bytes` with what it actually wrote — and `RRF_RT_REG_SZ` makes it refuse any
	// value whose type is not a string, so what lands in the buffer is UTF-16 or nothing. The null
	// `pdwtype` says the type is not wanted back, which the API allows.
	let status = unsafe {
		RegGetValueW(
			root,
			subkey.as_ptr(),
			value.as_ptr(),
			RRF_RT_REG_SZ,
			std::ptr::null_mut(),
			buffer.as_mut_ptr().cast(),
			&raw mut bytes,
		)
	};
	if status != ERROR_SUCCESS {
		return None;
	}
	// The length comes back in BYTES and counts the terminator, and the API guarantees one is there —
	// so the string is whatever precedes the first NUL, and the `min` is belt and braces against a
	// length longer than the buffer that was just handed in.
	let units = (bytes as usize / 2).min(buffer.len());
	let text: Vec<u16> = buffer[..units]
		.iter()
		.copied()
		.take_while(|unit| *unit != 0)
		.collect();
	(!text.is_empty()).then(|| PathBuf::from(std::ffi::OsString::from_wide(&text)))
}

/// A NUL-terminated UTF-16 copy of `text`, which is what a Windows `PCWSTR` parameter wants.
#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
	text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A shell named on `PATH` (or at its usual `/bin` home), for the macOS entries where the name is
/// not ambiguous.
#[cfg(target_os = "macos")]
fn unix_shell(kind: Kind, name: &str) -> Option<Shell> {
	let fallback = PathBuf::from("/bin").join(name);
	let program = on_path(name).or_else(|| fallback.is_file().then_some(fallback))?;
	Some(Shell::plain(kind, program))
}

/// The `Program Files` directories to look in: the 64-bit one and the 32-bit one, in that order.
///
/// Both are read from the environment rather than composed from a drive letter — Windows can be
/// installed anywhere, and `%ProgramFiles%` is what it itself uses. On macOS this is empty, which
/// leaves the `pwsh` search to `PATH` alone.
fn program_files() -> Vec<PathBuf> {
	["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
		.iter()
		.filter_map(std::env::var_os)
		.map(PathBuf::from)
		.collect()
}

/// The first `PATH` entry holding a file with this exact name, or `None`.
///
/// Written out rather than shelled out to `where` / `which`: spawning a process to find a program
/// costs more than reading one environment variable, and it would put a command line where none is
/// needed. Only the NAME is matched — no extension guessing, no `PATHEXT` walk — because every
/// caller here already knows the exact file it wants.
fn on_path(name: &str) -> Option<PathBuf> {
	let path = std::env::var_os("PATH")?;
	std::env::split_paths(&path)
		.map(|dir| dir.join(name))
		.find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
	use super::{Kind, Shell, catalogue, on_path, program_files};
	use std::path::PathBuf;

	#[test]
	fn a_local_session_is_labelled_by_its_shell_and_never_shaped_like_an_endpoint() {
		// Two local tabs are told apart by the shell, so the shell has to be in the label. And the
		// label must not read as `user@host:port`: there is no machine on the other end of anything.
		let shell = Shell::plain(Kind::Cmd, PathBuf::from(r"C:\Windows\System32\cmd.exe"));
		assert_eq!(shell.endpoint(), "local — cmd");
		assert!(!shell.endpoint().contains('@'));
	}

	#[test]
	fn the_two_powershells_are_never_given_the_same_name() {
		// They are different programs with different syntax. A bar that called both "PowerShell"
		// would leave the user unable to tell which one they had opened.
		assert_ne!(Kind::Pwsh.label(), Kind::PowerShell.label());
		assert_ne!(Kind::Pwsh.slug(), Kind::PowerShell.slug());
	}

	#[test]
	fn git_bash_is_started_the_way_its_own_shortcut_starts_it() {
		// Without `--login` the MSYS2 profile never runs, so the shell comes up with no Unix tools on
		// `PATH` — a bash that cannot find `ls`. Only this entry carries arguments; a pty is what
		// makes the others interactive.
		for shell in catalogue() {
			match shell.kind {
				Kind::GitBash => assert_eq!(shell.args, vec!["--login", "-i"]),
				_ => assert!(
					shell.args.is_empty(),
					"{} needs no arguments",
					shell.kind.label()
				),
			}
		}
	}

	#[test]
	fn every_offered_shell_is_a_program_that_is_really_there() {
		// The bar's whole contract: a button that appears can be pressed. Nothing is listed on a
		// guess, so this holds on a machine with none of them installed too — the row is then empty.
		for shell in catalogue() {
			assert!(
				shell.program.is_file(),
				"{} was offered but {} is not a file",
				shell.kind.label(),
				shell.program.display()
			);
		}
	}

	#[cfg(windows)]
	#[test]
	fn each_shell_is_moved_by_a_command_it_actually_understands() {
		// The one place a local session cannot borrow the remote behaviour. A `cd '/C:/Users/cme'` typed
		// at any of these is an error, not a move — the pane path is not a path here.
		let pane = "/C:/Users/cme";
		// cmd needs `/d`: without it a `cd` to another drive moves the directory ON that drive and
		// leaves the prompt where it was, which looks like nothing happened.
		assert_eq!(
			Kind::Cmd.cd(pane).as_deref(),
			Some(r#"cd /d "C:\Users\cme""#)
		);
		// PowerShell gets `-LiteralPath`, so a folder with a bracket in its name is a name and not a
		// wildcard that matches nothing.
		for shell in [Kind::Pwsh, Kind::PowerShell] {
			assert_eq!(
				shell.cd(pane).as_deref(),
				Some(r"Set-Location -LiteralPath 'C:\Users\cme'")
			);
		}
		// Git Bash is an MSYS shell, so the drive becomes a top-level directory.
		assert_eq!(Kind::GitBash.cd(pane).as_deref(), Some("cd '/c/Users/cme'"));
	}

	#[cfg(windows)]
	#[test]
	fn a_git_installed_where_it_liked_is_still_offered() {
		// The gap §105 closed. Git for Windows takes any directory and can be told to keep `git.exe` off
		// `PATH` — the machine this was found on has it in `C:\git` with nothing of it on `PATH` — so the
		// Program Files search and the `PATH` search both missed it, and the bar offered no Git Bash on a
		// machine that has Git Bash.
		//
		// Skipped rather than failed where there is nothing to find: what is asserted is that the search
		// AGREES with the installer's own record, not that Git is installed.
		let Some(root) = super::recorded_git_roots().into_iter().next() else {
			eprintln!("skipped: no Git for Windows install is recorded in the registry");
			return;
		};
		if !root.join(r"bin\bash.exe").is_file() {
			eprintln!(
				"skipped: the recorded root {} holds no bash",
				root.display()
			);
			return;
		}
		assert!(
			catalogue().iter().any(|shell| shell.kind == Kind::GitBash),
			"the install recorded at {} is offered on the bar",
			root.display()
		);
	}

	#[test]
	fn only_the_posix_shells_end_themselves_on_eof() {
		// Which side of this split a shell falls on decides whether Ctrl+D is the shell's key or cmote's
		// (§104), so it is written down per kind rather than inferred from a name at the call site.
		for shell in [Kind::GitBash, Kind::Zsh, Kind::Bash] {
			assert!(
				shell.quits_on_eof(),
				"{} logs out on EOF, the way every POSIX shell does",
				shell.label()
			);
		}
		// Measured against real ConPTY children of all three: one `0x04` at the prompt, six seconds of
		// watching the process handle, no exit and no output. Their EOF is Ctrl+Z, and even that means
		// EOF only to a program reading a stream — never to the interpreter itself.
		for shell in [Kind::Pwsh, Kind::PowerShell, Kind::Cmd] {
			assert!(
				!shell.quits_on_eof(),
				"{} drops the byte, so cmote is the one that has to end the session",
				shell.label()
			);
		}
	}

	#[cfg(windows)]
	#[test]
	fn a_path_with_nowhere_to_go_types_nothing() {
		// The virtual root is not a directory, so there is no `cd` that reaches it. Typing one anyway
		// would put a failing command in the user's own shell history.
		for shell in [Kind::Cmd, Kind::Pwsh, Kind::PowerShell, Kind::GitBash] {
			assert_eq!(shell.cd("/").as_deref(), None, "{}", shell.label());
			// And a path that is not on this machine at all is refused by the same translation.
			assert_eq!(shell.cd("/etc").as_deref(), None, "{}", shell.label());
		}
	}

	#[test]
	fn the_msys_spelling_moves_the_drive_and_nothing_else() {
		// `C:\Users\cme` -> `/c/Users/cme`: the letter goes to the front, lowercased, and the separators
		// flip. Nothing else about the path is touched.
		assert_eq!(super::msys(r"C:\Users\cme"), "/c/Users/cme");
		assert_eq!(super::msys(r"D:\"), "/d/");
		// A path with no drive keeps its shape rather than being given an invented prefix — it cannot
		// arrive here, and guessing would be worse than letting the shell say no.
		assert_eq!(super::msys(r"relative\path"), "relative/path");
	}

	#[test]
	fn the_search_looks_only_where_the_platform_puts_things() {
		// `on_path` matches the exact name and never invents an extension, which is what keeps the
		// Git Bash search from ever resolving a bare `bash`.
		assert!(on_path("cmote-no-such-program-anywhere").is_none());
		// And every `Program Files` root comes from the environment, so a Windows installed off C:
		// is searched correctly rather than missed.
		for base in program_files() {
			assert!(base.is_absolute(), "{} is not absolute", base.display());
		}
	}

	#[cfg(windows)]
	#[test]
	fn wsls_launcher_is_never_taken_for_git_bash() {
		// `System32\bash.exe` is WSL. Starting it would run a Linux distribution in a VM while the
		// file panes beside it described this machine's drives — so the Git Bash search reaches a
		// `bash.exe` only through a Git installation, and this asserts the one that is found is not
		// the one in System32.
		if let Some(path) = super::git_bash_path() {
			let lowered = path.to_string_lossy().to_lowercase();
			assert!(
				!lowered.contains("system32"),
				"{lowered} is WSL's launcher, not Git Bash"
			);
			assert!(lowered.ends_with(r"bin\bash.exe"));
		}
	}
}
