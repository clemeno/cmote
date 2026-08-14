// term/pointer.rs — the mouse pointer shape a remote asks for over its own grid (PLAN §77).
//
//   OSC 22   ESC ] 22 ; name    BEL | ST        name is a CSS cursor keyword: text, pointer, …
//
// Not to be confused with the row two below it in the matrix. OSC 50 sets the TEXT CARET's shape —
// block, bar, underline, the thing that marks where the next character lands. This one sets the
// MOUSE POINTER: the arrow or I-beam under the user's hand. X11 called the mouse pointer a "cursor"
// (`XC_xterm`), which is why `vte` names its two handlers `set_mouse_cursor_icon` and
// `set_cursor_shape` and why the two rows read as near-duplicates. They share no state at all.
//
// WHY THIS IS CMOTE'S TO ANSWER AND NOT THE ENGINE'S. `vte` does parse OSC 22 — it resolves the name
// through `cursor_icon::CursorIcon::from_str` and calls `Handler::set_mouse_cursor_icon`. That
// method is left at its empty default body and `alacritty_terminal` never overrides it, so the
// sequence dies inside the engine and cmote is never offered it. Reading the bytes here is the same
// way in as the cwd (§17), the prompt marks (§34) and the icon name (§69) took, and for the same
// reason: the engine drops it, so scan it out beside the grid.
//
// WHY THE OLD REFUSAL DID NOT SURVIVE RE-READING. Until §77 this row was 🤷, on the grounds that a
// pointer shape is WINDOW-WIDE chrome and would fight the shapes cmote's own widgets ask for. That
// is true of `winit::window::Window::set_cursor`, which is what a terminal built straight on the
// windowing layer would have to call — and what `vte`'s handler name suggests. cmote never goes near
// it. The grid is an iced `mouse_area`, and `mouse_area::interaction` applies WHILE THE POINTER IS
// INSIDE THAT WIDGET and nowhere else. So "scoped to the terminal, not the window" is not a thing
// this file had to build; it is the only thing the toolkit offers. And the four shapes the old note
// said would be contested — `ResizingHorizontally` on the explorer splitter, `ResizingVertically` on
// the files splitter, `Grab`/`Grabbing` on the dialog and tab-strip drags — all sit on widgets that
// are SIBLINGS of the grid and never over it. There was no contest to arbitrate.
//
// WHAT IS REFUSED, AND WHY IT IS A REFUSAL RATHER THAN AN OMISSION. `Shape` names five keywords and
// `from_css` matches those five and nothing else, so the allow-list is the parser — the same
// construction `term/iterm.rs` uses for OSC 1337 and `link.rs` for URI schemes. The wire can spell
// 35 different shapes (`cursor_icon`'s full CSS set); the line between the five and the rest is not
// "what iced can draw" but WHO THE SHAPE MAKES A CLAIM ABOUT:
//
//  * The five kept — `default`, `text`, `pointer`, `crosshair`, `cell` — describe the CONTENT under
//    the mouse. What is under the mouse on the grid is the remote's output, so the remote is the one
//    that knows. That is the whole of what this row buys, and it is worth having: a TUI with
//    clickable regions can say so, and `text` over a pager is honest in a way cmote cannot work out
//    for itself.
//
//  * `grab`, `grabbing`, `move` and the fourteen resize shapes are refused because they are CMOTE'S
//    OWN VOCABULARY. Those exact shapes are what the two splitters and the two drag handles say, and
//    §51 goes as far as drawing custom art for two of them. A remote painting `col-resize` over the
//    grid teaches the user that a grid edge drags when it does not, and `grab` impersonates a cmote
//    handle outright. Same class of attack as a spoofed window title (§55, §69), one surface over.
//
//  * `wait`, `progress`, `not-allowed` and `no-drop` are refused because they make a claim about
//    THE CLIENT, not about the text. `wait` says cmote is busy; `progress` says cmote is working;
//    `not-allowed` says cmote is declining the user's input. A remote must not be able to make the
//    local application look hung or broken — that is a shape only cmote's own state may ask for.
//
//  * `help` and `context-menu` are refused for the same reason one step softer: cmote really does
//    open a context menu on a right-press over the grid, and that menu is cmote's, so a remote must
//    not get to announce it. Everything else left over (`alias`, `copy`, `zoom-in`, `all-scroll`,
//    `vertical-text`, …) is refused for want of a meaning inside a text grid — the tight list is the
//    point, and a shape can always be added later with a reason attached.
//
// ONE HAZARD DOES NOT EXIST HERE AT ALL, which is worth writing down so nobody goes looking for the
// guard against it: there is no way to HIDE the pointer. CSS has `cursor: none` and iced has
// `Interaction::Hidden`, but `cursor_icon::CursorIcon` has no such variant, so no OSC 22 payload any
// terminal accepts can spell it. A remote cannot make the user's mouse pointer disappear over the
// grid because the vocabulary has no word for it, not because this file checks.
//
// A REFUSED NAME LEAVES THE CURRENT SHAPE ALONE rather than resetting to `Default` — the same rule
// `icon` keeps when a payload is not an icon name. The shape that survives is one this same remote
// asked for and this same allow-list already passed, so the refusal cannot be turned into a way of
// clearing a shape; and `default` is spelled out in the list precisely so that giving the pointer
// back is something a program can say on purpose.

/// The longest OSC payload this scanner will buffer. `22;` plus the longest keyword in the CSS
/// cursor set (`context-menu`, thirteen characters) is sixteen bytes, so sixty-four is generous and
/// still refuses to hold anything that could not possibly be a shape name (§12). A payload past it
/// is abandoned and the framer resumes hunting, so a flood costs the flooded sequence and no more.
const MAX_PAYLOAD: usize = 64;

/// A mouse pointer shape cmote will let a remote ask for. Five variants, and the fact that there are
/// only five IS the security boundary — see the module header for what the other thirty are and
/// which of three reasons each is refused for.
///
/// Deliberately cmote's own enum rather than a re-export of `cursor_icon::CursorIcon` or of
/// `iced::mouse::Interaction`. Re-exporting either would mean the refused shapes were still
/// *nameable* here and kept out by a check somewhere downstream; naming only five means a refused
/// shape has no value to be carried in, and the allow-list cannot be got round by a later caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Shape {
	/// Whatever the toolkit would have drawn — the shape cmote uses when no remote has asked, and
	/// the one a program spells out to hand the pointer back.
	#[default]
	Default,
	/// An I-beam: the text under here can be selected.
	Text,
	/// A hand: the thing under here can be clicked.
	Pointer,
	/// A crosshair: a two-dimensional selection is being made.
	Crosshair,
	/// A thick plus: the cell under here is selectable, as in a spreadsheet.
	Cell,
}

impl Shape {
	/// The allow-list, as a function. `Some` for the five shapes a remote may ask for, `None` for
	/// every other CSS cursor keyword and for anything that is not one at all.
	///
	/// Matched exactly and in lower case, which is how `cursor_icon::CursorIcon::from_str` reads the
	/// same names and therefore how every program that sends this sequence spells them. Being strict
	/// costs a remote nothing — there is one spelling of each keyword — and keeps the list a list.
	fn from_css(name: &str) -> Option<Self> {
		match name {
			"default" => Some(Self::Default),
			"text" => Some(Self::Text),
			"pointer" => Some(Self::Pointer),
			"crosshair" => Some(Self::Crosshair),
			"cell" => Some(Self::Cell),
			_ => None,
		}
	}
}

/// The pointer shape the remote last asked for over this terminal's grid (§77). Feed it every byte
/// of shell output; it keeps the most recent shape from the allow-list and ignores everything else.
#[derive(Debug, Default)]
pub struct Pointer {
	framer: super::osc::Framer<MAX_PAYLOAD>,
	shape: Shape,
}

impl Pointer {
	/// Scan a chunk of shell output for a pointer shape. Safe at any chunk boundary — the framer's
	/// state carries over between calls.
	pub fn feed(&mut self, bytes: &[u8]) {
		// Every finished OSC arrives here — titles, colour queries, prompt marks and all — and
		// `parse` keeps only the OSC 22s whose payload survives the allow-list. Anything else
		// leaves the current shape where it is.
		let shape = &mut self.shape;
		self.framer.feed(bytes, |_offset, payload| {
			if let Some(found) = parse(payload) {
				*shape = found;
			}
		});
	}

	/// The shape the remote last asked for, or `Shape::Default` if none has.
	pub fn shape(&self) -> Shape {
		self.shape
	}

	/// Give the pointer back to cmote, whatever the remote last asked for.
	///
	/// Called on both directions of the alternate-screen swap (§77), which is the moment a
	/// full-screen program starts or ends: a TUI that asked for `pointer` over its own buttons must
	/// not leave that hand hovering over the shell prompt it quit back to, and a TUI starting up
	/// must be shown a pointer in its default state rather than the last program's.
	pub fn clear(&mut self) {
		self.shape = Shape::Default;
	}
}

/// Pull a pointer shape out of an OSC payload, or `None` if this OSC is not one — which leaves the
/// current shape alone rather than resetting it.
///
/// The `22;` prefix is matched whole, so no OSC code that merely starts with those digits can be
/// mistaken for it. `from_utf8` is strict rather than lossy, unlike the icon name's: a shape name is
/// matched against a fixed list rather than drawn, so a payload with a bad byte in it can never be
/// one of the five and there is nothing to be gained by repairing it into a replacement character.
fn parse(payload: &[u8]) -> Option<Shape> {
	let rest = payload.strip_prefix(b"22;")?;
	Shape::from_css(std::str::from_utf8(rest).ok()?.trim())
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Feed one byte slice to a fresh scanner and read the shape it left.
	fn track(bytes: &[u8]) -> Shape {
		let mut pointer = Pointer::default();
		pointer.feed(bytes);
		pointer.shape()
	}

	#[test]
	fn nothing_asked_for_is_the_default_shape() {
		assert_eq!(track(b"ordinary output"), Shape::Default);
	}

	#[test]
	fn osc22_sets_each_of_the_five_allowed_shapes() {
		// The whole allow-list, one assertion each — a list is worth nothing if only its first
		// entry is ever exercised.
		assert_eq!(track(b"\x1b]22;text\x07"), Shape::Text);
		assert_eq!(track(b"\x1b]22;pointer\x07"), Shape::Pointer);
		assert_eq!(track(b"\x1b]22;crosshair\x07"), Shape::Crosshair);
		assert_eq!(track(b"\x1b]22;cell\x07"), Shape::Cell);
		assert_eq!(track(b"\x1b]22;default\x07"), Shape::Default);
	}

	#[test]
	fn the_st_terminator_is_accepted_too() {
		assert_eq!(track(b"\x1b]22;text\x1b\\"), Shape::Text);
	}

	#[test]
	fn a_sequence_split_across_chunks_is_still_read() {
		// Output arrives in arbitrary chunks, including a split between the ESC and the `]`.
		let mut pointer = Pointer::default();
		pointer.feed(b"text \x1b");
		pointer.feed(b"]22;point");
		pointer.feed(b"er\x07more text");
		assert_eq!(pointer.shape(), Shape::Pointer);
	}

	#[test]
	fn a_program_can_hand_the_pointer_back() {
		// `default` is on the list on purpose: it is how a program says it is finished owning the
		// shape, and without it the only way back would be for cmote to guess when to reset.
		let mut pointer = Pointer::default();
		pointer.feed(b"\x1b]22;pointer\x07");
		assert_eq!(pointer.shape(), Shape::Pointer);
		pointer.feed(b"\x1b]22;default\x07");
		assert_eq!(pointer.shape(), Shape::Default);
	}

	#[test]
	fn the_shapes_that_are_cmotes_own_vocabulary_are_refused() {
		// The spoof this allow-list exists for (§77). These are the exact shapes cmote's splitters
		// and drag handles ask for, so a remote painting one over the grid would teach the user an
		// affordance that is not there — and `grab` impersonates a cmote handle outright.
		for name in [
			"grab",
			"grabbing",
			"move",
			"col-resize",
			"row-resize",
			"ew-resize",
			"ns-resize",
			"nwse-resize",
			"all-scroll",
		] {
			let sequence = format!("\x1b]22;{name}\x07");
			assert_eq!(
				track(sequence.as_bytes()),
				Shape::Default,
				"{name} must not reach the pointer"
			);
		}
	}

	#[test]
	fn the_shapes_that_would_speak_for_cmote_are_refused() {
		// The other half of the refusal: these say something about the CLIENT rather than about the
		// text under the pointer. A remote must not be able to make cmote look hung, busy, or as
		// though it were refusing the user's input.
		for name in ["wait", "progress", "not-allowed", "no-drop"] {
			let sequence = format!("\x1b]22;{name}\x07");
			assert_eq!(
				track(sequence.as_bytes()),
				Shape::Default,
				"{name} must not reach the pointer"
			);
		}
	}

	#[test]
	fn a_refused_shape_does_not_disturb_the_one_already_set() {
		// A refusal is not a back door to clearing: the shape that survives is one the same remote
		// asked for and this same list already passed.
		let mut pointer = Pointer::default();
		pointer.feed(b"\x1b]22;text\x07");
		pointer.feed(b"\x1b]22;wait\x07\x1b]22;grab\x07");
		assert_eq!(pointer.shape(), Shape::Text);
	}

	#[test]
	fn a_name_that_is_not_a_cursor_keyword_at_all_is_refused() {
		// Including the empty payload, which for the icon name means "clear" and here means
		// nothing — there is a keyword for handing the pointer back and this is not it.
		assert_eq!(track(b"\x1b]22;\x07"), Shape::Default);
		assert_eq!(track(b"\x1b]22;banana\x07"), Shape::Default);
		let mut pointer = Pointer::default();
		pointer.feed(b"\x1b]22;text\x07\x1b]22;\x07");
		assert_eq!(pointer.shape(), Shape::Text);
	}

	#[test]
	fn the_match_is_exact_and_lower_case() {
		// How `cursor_icon::CursorIcon::from_str` reads the same names, so this is what every
		// program that sends the sequence actually spells. Surrounding whitespace is forgiven
		// because it costs nothing; a different spelling is not, because a list that accepts
		// near-misses is not a list.
		assert_eq!(track(b"\x1b]22; text \x07"), Shape::Text);
		assert_eq!(track(b"\x1b]22;TEXT\x07"), Shape::Default);
		assert_eq!(track(b"\x1b]22;Pointer\x07"), Shape::Default);
		assert_eq!(track(b"\x1b]22;text2\x07"), Shape::Default);
	}

	#[test]
	fn no_other_osc_code_is_mistaken_for_a_pointer_shape() {
		// The prefix is matched whole. OSC 2 is the window title and could otherwise carry a `2;`
		// of its own into the match; the rest are codes that merely begin with the digits.
		assert_eq!(track(b"\x1b]2;text\x07"), Shape::Default);
		assert_eq!(track(b"\x1b]220;text\x07"), Shape::Default);
		assert_eq!(track(b"\x1b]1;text\x07"), Shape::Default);
		assert_eq!(track(b"\x1b]50;CursorShape=1\x07"), Shape::Default);
	}

	#[test]
	fn another_osc_does_not_forget_the_shape_we_have() {
		// A shell sets the title and announces its cwd on every prompt. Neither is a pointer shape
		// and neither may clear one — the rule every scanner on this framer keeps.
		let mut pointer = Pointer::default();
		pointer.feed(b"\x1b]22;text\x07");
		pointer.feed(b"\x1b]0;user@host: ~\x07\x1b]7;file://host/home\x07");
		assert_eq!(pointer.shape(), Shape::Text);
	}

	#[test]
	fn a_non_utf8_payload_is_refused_rather_than_repaired() {
		// Strict rather than lossy, unlike the icon name's: this payload is matched against a fixed
		// list rather than drawn, so repairing a bad byte could only ever produce a non-match.
		assert_eq!(track(b"\x1b]22;te\xffxt\x07"), Shape::Default);
	}

	#[test]
	fn an_overlong_payload_is_dropped_not_buffered() {
		// A hostile or broken stream must not grow our memory: past the cap the payload is
		// abandoned and the scanner keeps hunting, so the flood costs the flooded sequence alone.
		let mut pointer = Pointer::default();
		pointer.feed(b"\x1b]22;");
		pointer.feed(&[b'x'; MAX_PAYLOAD + 10]);
		pointer.feed(b"\x07");
		assert_eq!(pointer.shape(), Shape::Default);

		pointer.feed(b"\x1b]22;text\x07");
		assert_eq!(pointer.shape(), Shape::Text);
	}

	#[test]
	fn clearing_gives_the_pointer_back() {
		// What the alternate-screen swap does with it, in isolation.
		let mut pointer = Pointer::default();
		pointer.feed(b"\x1b]22;crosshair\x07");
		pointer.clear();
		assert_eq!(pointer.shape(), Shape::Default);
	}
}
