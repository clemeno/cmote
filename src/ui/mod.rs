// ui/mod.rs — view helpers (PLAN §10).
//
// Views are pure functions from state to an iced `Element`. Keeping them out of
// app.rs stops that file from growing without bound and groups the widget code
// by screen. Each submodule owns one screen's layout.

pub mod connect; // the connection form
pub mod dialog; // shared modal-dialog chrome (header / body / footer)
pub mod editor; // the in-tab text editor: toolbar, line-number gutter, changed marks (§32)
pub mod elevate; // the "Log in as…" dialog: the session's accounts, and becoming another (§47)
pub mod explorer; // the remote folder tree beside the terminal (§18)
pub mod files; // the remote file grid under the terminal (§19)
pub mod forward; // the port-forwards management dialog (§27)
pub mod grid; // the terminal screen itself, drawn cell-exact as one widget (§9)
pub mod home; // the home screen: saved connection targets (§14)
pub mod menu; // shared right-click context-menu chrome: panel / items / dismiss layer (§10)
pub mod preview; // the in-tab picture viewer: toolbar, the image on its ground, the refusal (§53)
pub mod richcopy; // serialise a selection to styled HTML for a rich Ctrl+C (§10)
pub mod scrollbar; // one scrollbar look, shared by the terminal's own bar and every pane's (§118)
pub mod selection; // mouse text selection over the grid
pub mod snackbar; // the copy-confirmation toast (§10)
pub mod split; // the window cut into side-by-side / stacked regions, and their dividers (§48)
pub mod syntax; // syntect-backed syntax highlighting for the editor's CME theme (§32)
pub mod tabs; // the tab strip across the top of the window (§26)
pub mod terminal; // the live shell grid

use iced::widget::{button, column, text, text_editor, text_input};
use iced::{Color, Element};

use crate::app::Message;

/// The body copy for the host-key dialog (§8). The fingerprint is appended (on its own
/// line) when the dialog opens, so the whole message — fingerprint included — is one
/// selectable block. Public so `app` can seed it into the dialog buffer.
pub const HOST_KEY_DIALOG_BODY: &str = "This is the first connection to this server. Verify the fingerprint below matches the server you expect before trusting it.";

/// The body copy for the CHANGED host-key dialog (§8) — the loud override warning. The two
/// fingerprints (stored vs presented, each on its own line) are appended when the dialog opens, so
/// the whole warning is one selectable, copyable block for out-of-band comparison. Public so `app`
/// can seed it into the dialog buffer.
pub const HOST_KEY_CHANGED_DIALOG_BODY: &str = "WARNING: the host key for this server has CHANGED since it was last trusted. This can mean the server's key was legitimately rotated — or that the connection is being intercepted (a man-in-the-middle attack). Compare the two fingerprints below against the server you expect, out-of-band, before overriding.";

/// The body copy for the passphrase dialog (§7). Public so `app` can seed it into the
/// dialog buffer when the encrypted-key prompt opens.
pub const PASSPHRASE_DIALOG_BODY: &str =
	"This private key is protected by a passphrase. Enter it to unlock the key and continue.";

/// The widget id of the passphrase field. It is stable and shared: `passphrase_view`
/// tags the field with it, and `app` hands the same id to `text_input`'s focus
/// operation so the field is focused the instant the prompt appears — the user can
/// type immediately without first clicking it (§7). A plain `&'static str` is enough
/// because iced's widget `Id` is `From<&'static str>`.
pub const PASSPHRASE_INPUT_ID: &str = "passphrase-input";

/// The widget ids of the master-passphrase prompt's fields (§16). The first is always shown;
/// the confirm field appears only when creating the vault (see `vault_view`). Stable `&'static`
/// strings so `app` can move focus to them the instant the prompt opens.
pub const VAULT_INPUT_ID: &str = "vault-input";
pub const VAULT_CONFIRM_INPUT_ID: &str = "vault-confirm";

/// The body copy for the vault prompt when UNLOCKING an existing vault (§16). Public so `app`
/// can seed it into the dialog buffer when the prompt opens.
pub const VAULT_UNLOCK_BODY: &str =
	"Enter your master passphrase to unlock the saved credentials in this portable vault.";

/// The body copy for the vault prompt when CREATING the vault for the first time (§16). It
/// spells out the one hard rule of a portable encrypted store: forget the passphrase and the
/// secrets are gone — there is no recovery, by design.
pub const VAULT_CREATE_BODY: &str = "Choose a master passphrase to protect saved credentials. It encrypts the vault so the whole store stays portable across machines. There is no way to recover it if you forget it, so keep it safe.";

/// The intro line for the keyboard-interactive prompt (§7). The server's own heading and
/// instructions (either may be empty) are appended to it when the prompt opens, so the whole
/// message is one selectable block the user can copy.
pub const INTERACTIVE_DIALOG_BODY: &str =
	"The server is asking for additional authentication. Answer each prompt below to continue.";

/// The widget id of the keyboard-interactive field at `index` (§7). Keeping the derivation in
/// one place means the view (which tags each field) and `app` (which focuses the first field
/// when the prompt opens) build the exact same id. The fields are dynamic — one per prompt in
/// the server's request — so a per-index id is needed rather than a single constant.
pub fn interactive_field_id(index: usize) -> String {
	format!("interactive-{index}")
}

// --- the layout boundary: integers on one side, `f32` pixels on the other (§111) ---
//
// iced measures everything in `f32`, and cmote counts everything in `usize`: rows, columns,
// characters, entries. So every layout expression crosses between them, twice — a count multiplied
// by a pitch to get pixels, and a pixel measurement divided by a pitch to get back to an index.
//
// There is no way to write either conversion without `as`, and this was checked rather than assumed:
// std has no `TryFrom<f32> for usize` and no `From<u32> for f32`, and clippy flags even a provably
// bounded `px.clamp(0.0, 4096.0) as usize` for both truncation and sign loss. `f32::from(u16)` is the
// one exact spelling available, and a `u16` ceiling is wrong here — an editor buffer can hold more
// than 65,535 lines, and clamping the gutter at that would break a file this program can open.
//
// So the boundary is crossed HERE, in these seven functions, and nowhere else. Each carries an
// `#[expect]` naming the lint it answers, which is not the same thing as an `allow`: the lint stays
// enabled for the whole crate, so a bare float cast written anywhere else is still a build error, and
// `expect` fails the build if the lint ever stops firing here — the suppression cannot outlive its
// reason. What the functions buy beyond the conversion is the arithmetic: the floor, the clamp and the
// negative case were repeated at thirty-one call sites and are now written once each.
//
// The limits are real and stated where they bite. `f32` represents integers exactly up to 2^24
// (16,777,216); above that a row index rounds, and its pixel top is wrong by less than the row is
// tall. That is above every count this program can reach — the editor's own 8 MiB ceiling is about
// 8.4 million lines of one byte — so the inexactness is unreachable rather than tolerated.

/// `count` rows, columns or characters, each `pitch` pixels, as pixels.
#[expect(
	clippy::cast_precision_loss,
	reason = "std offers no exact usize-to-f32; exact below 2^24, above every count reachable here"
)]
pub fn pixels(count: usize, pitch: f32) -> f32 {
	count as f32 * pitch
}

/// The same for a row offset that may be NEGATIVE — a picture whose top edge has scrolled above the
/// viewport still has to be placed, at a negative `y`, so that its visible part lands correctly (§41).
#[expect(
	clippy::cast_precision_loss,
	reason = "std offers no exact i64-to-f32; a row offset is bounded by the document, far below 2^24"
)]
pub fn signed_pixels(rows: i64, pitch: f32) -> f32 {
	rows as f32 * pitch
}

/// How many whole `pitch`-sized units fit inside `pixels` — a measurement turned back into a count.
///
/// Negative and NaN both answer 0, which is what every caller wants: a viewport scrolled to a
/// negative offset, or measured before its first layout, has no rows to show rather than a wrapped
/// number of them.
#[expect(
	clippy::cast_possible_truncation,
	clippy::cast_sign_loss,
	reason = "std offers no TryFrom<f32>; floored and floored at zero on the line above"
)]
pub fn cells(pixels: f32, pitch: f32) -> usize {
	if pitch <= 0.0 {
		return 0;
	}
	let count = (pixels / pitch).floor();
	// `is_finite` rather than `!is_nan`: an INFINITE measurement floors to infinity, which is
	// positive, and `as usize` then saturates at `usize::MAX` — an unmeasured `Length::Fill` would
	// hand a caller four billion billion rows to walk. A test found this; the first version of the
	// guard only refused NaN.
	if !count.is_finite() || count <= 0.0 {
		return 0;
	}
	count as usize
}

/// The same, rounded UP: how many units it takes to COVER `pixels`. What a viewport asks when it
/// needs the partly-visible row at its bottom edge drawn as well.
#[expect(
	clippy::cast_possible_truncation,
	clippy::cast_sign_loss,
	reason = "std offers no TryFrom<f32>; rounded up and floored at zero on the line above"
)]
pub fn cells_covering(pixels: f32, pitch: f32) -> usize {
	if pitch <= 0.0 {
		return 0;
	}
	let count = (pixels / pitch).ceil();
	// Finite, for the same reason as `cells` above.
	if !count.is_finite() || count <= 0.0 {
		return 0;
	}
	count as usize
}

/// `cells`, as the terminal's own `u16` geometry (§9) — clamped, since a grid wider than 65,535
/// columns can be neither drawn nor asked for.
pub fn cell_index(pixels: f32, pitch: f32) -> u16 {
	u16::try_from(cells(pixels, pitch)).unwrap_or(u16::MAX)
}

/// `part` of `whole` as a fraction in `0.0..=1.0`, for a progress bar (§17). A zero whole is 0.0
/// rather than a NaN, because "nothing to do" draws as an empty bar and not as a hole.
#[expect(
	clippy::cast_precision_loss,
	reason = "std offers no exact u64-to-f32; a byte count above 2^24 loses precision far below one \
	          pixel of a progress bar"
)]
pub fn fraction(part: u64, whole: u64) -> f32 {
	if whole == 0 {
		return 0.0;
	}
	part.min(whole) as f32 / whole as f32
}

/// A wheel movement in whole lines (§23), rounded to the nearest rather than truncated so that a
/// small flick still moves one line instead of none.
#[expect(
	clippy::cast_possible_truncation,
	reason = "std offers no TryFrom<f32>; clamped into an i32's range on the line above"
)]
pub fn lines_scrolled(amount: f32) -> i32 {
	if amount.is_nan() {
		return 0;
	}
	let lines = amount
		.round()
		.clamp(f32::from(i16::MIN), f32::from(i16::MAX));
	lines as i32
}

/// The colour of the "wrong passphrase" hint (§7). A muted red that reads clearly on
/// the default light theme. This is about a *local* key-file passphrase, not remote
/// auth, so it is not a credential oracle (§12) — the key is decrypted and MAC-checked
/// on this machine, and telling the user their local passphrase was wrong is expected.
const PASSPHRASE_ERROR: Color = Color::from_rgb8(0xb0, 0x00, 0x00);

/// The colour of the changed-host-key warning line (§8). A bright red that reads clearly on the
/// dialog's DARK card — unlike `PASSPHRASE_ERROR`, which is a muted red for the light form — so the
/// man-in-the-middle warning stands out from the body copy it sits above.
const WARNING_FG: Color = Color::from_rgb8(0xff, 0x5c, 0x5c);

/// Shorten `text` to at most `max_chars`, dropping the MIDDLE and marking the cut with a
/// single `…`, so both the start and the end survive rather than the tail being lost to a
/// hard clip. `text` already short enough passes through untouched.
///
/// Splits by CHARACTERS, not bytes, so a multi-byte string is never cut through a glyph;
/// the tail keeps the odd character on an uneven budget, since the end — a file's
/// extension, a path's leaf folder — is usually the more worth-showing half.
///
/// Shared by every place that shows a name or a path in a fixed number of lines: the file
/// grid's cells (§19), the pane headers' current directory (§22) and the connect form's
/// key file (§14). Each caller passes the char budget its own width and line count come to
/// — this owns only the cut, not the "how many fit" estimate, so one rule keeps them all
/// consistent.
pub(crate) fn elide_middle(text: &str, max_chars: usize) -> String {
	let chars: Vec<char> = text.chars().collect();
	if chars.len() <= max_chars {
		return text.to_owned();
	}

	// One glyph goes to the `…`; the rest is split head/tail, the tail taking the odd one.
	let budget = max_chars.saturating_sub(1);
	let head = budget / 2;
	let tail = budget - head;
	let start: String = chars[..head].iter().collect();
	let end: String = chars[chars.len() - tail..].iter().collect();
	format!("{start}…{end}")
}

/// The error notice (§10): a generic message in the shared dialog chrome, with a
/// single Back button to the form. Its detail is logged, not shown, so nothing
/// sensitive leaks to the UI (§12). The message is a selectable body so it can be
/// copied; the close (✕) does the same as Back. `body` is `App::dialog_body`, seeded
/// with the error text when the screen opens.
pub fn error_view(body: &text_editor::Content, card: dialog::Card) -> Element<'_, Message> {
	dialog::dialog(
		"Connection failed".to_owned(),
		Message::BackPressed,
		dialog::selectable_body(body),
		vec![button("Back").on_press(Message::BackPressed).into()],
		card,
	)
}

/// The first-contact host-key prompt (§8), in the shared dialog chrome: show the
/// fingerprint and make the user explicitly accept or reject. There is intentionally
/// no "always trust" shortcut — accepting pins this exact key, and any later change
/// is refused. Closing (✕) rejects, the safe default: an unverified host is not
/// trusted just because the dialog was dismissed. `body` (`App::dialog_body`) holds the
/// explanation plus the fingerprint as one selectable block, so the fingerprint can be
/// copied for out-of-band comparison.
pub fn host_key_view(body: &text_editor::Content, card: dialog::Card) -> Element<'_, Message> {
	dialog::dialog(
		"Trust this host key?".to_owned(),
		Message::RejectHostKey,
		dialog::selectable_body(body),
		vec![
			button("Reject").on_press(Message::RejectHostKey).into(),
			button("Accept").on_press(Message::AcceptHostKey).into(),
		],
		card,
	)
}

/// The CHANGED host-key prompt (§8): the loud override dialog. A key pinned for this host no
/// longer matches — key rotation OR a man-in-the-middle — so this is a security decision, never an
/// auto-accept. It shows a bright warning line plus BOTH fingerprints (stored vs presented, carried
/// in the selectable `body` for out-of-band comparison) and three deliberate choices:
///   * **Reject** — refuse; the safe default, also on ✕ / a backdrop click.
///   * **Trust once** — connect this session only, leaving `known_hosts` unchanged (warns again).
///   * **Replace key** — pin the new key, so future connections verify against it silently.
///
/// `body` (`App::dialog_body`) holds the warning copy and the two fingerprints as one selectable
/// block. There is intentionally no type-to-confirm speed bump: the warning, both fingerprints and
/// the reject-by-default dismissal are the friction (§8).
pub fn host_key_changed_view(
	body: &text_editor::Content,
	card: dialog::Card,
) -> Element<'_, Message> {
	// The warning line sits above the selectable body — the same shape as the passphrase prompt's
	// hint — so the "possible attack" message is loud without needing a colour on the shared chrome.
	let content = column![
		text("⚠ Possible man-in-the-middle — override only if you expect this change.")
			.size(14)
			.color(WARNING_FG),
		dialog::selectable_body(body),
	]
	.spacing(12);

	dialog::dialog(
		"Host key has CHANGED".to_owned(),
		Message::RejectHostKey,
		content.into(),
		vec![
			button("Reject").on_press(Message::RejectHostKey).into(),
			button("Trust once")
				.on_press(Message::TrustHostKeyOnce)
				.into(),
			button("Replace key")
				.on_press(Message::ReplaceHostKey)
				.into(),
		],
		card,
	)
}

/// The key-passphrase prompt (§7), shown only when the chosen private key turns
/// out to be encrypted. A masked field plus Unlock / Cancel; pressing Enter in
/// the field submits too. The typed value is owned by `App` and passed in for
/// display — this view stays pure. A wrong passphrase simply brings the prompt
/// back (the session re-asks), so no separate error state is needed here.
pub fn passphrase_view<'a>(
	value: &'a str,
	failed: bool,
	body: &'a text_editor::Content,
	card: dialog::Card,
) -> Element<'a, Message> {
	// Only the message (`body`) is selectable; the field and the "incorrect" hint are
	// their own widgets. The hint is added only on a re-ask (`failed`), so the first
	// prompt stays clean. Building the column with `push` inserts it conditionally
	// without duplicating the layout; the shared chrome then wraps it as a dialog.
	let mut content = column![dialog::selectable_body(body)].spacing(12);

	if failed {
		content = content.push(
			text("That passphrase was not correct. Please try again.")
				.size(14)
				.color(PASSPHRASE_ERROR),
		);
	}

	content = content.push(
		text_input("Passphrase", value)
			.id(PASSPHRASE_INPUT_ID)
			.secure(true)
			.on_input(Message::PassphraseChanged)
			.on_submit(Message::PassphraseSubmitted),
	);

	dialog::dialog(
		"Unlock encrypted key?".to_owned(),
		Message::PassphraseCancelled,
		content.into(),
		vec![
			button("Unlock")
				.on_press(Message::PassphraseSubmitted)
				.into(),
			button("Cancel")
				.on_press(Message::PassphraseCancelled)
				.into(),
		],
		card,
	)
}

/// The master-passphrase prompt for the portable secret vault (§16), in the shared dialog
/// chrome. Two modes, chosen by `creating`:
///   * CREATE (no vault file yet) — a passphrase field plus a confirm field, so the first
///     passphrase cannot be a typo the user can never reproduce; the button reads "Create".
///   * UNLOCK (a vault file exists) — a single passphrase field; the button reads "Unlock".
///
/// A wrong unlock or a mismatched create brings the prompt back with `failed` set, which shows
/// the matching hint. Only the message (`body`) is selectable; the fields are their own widgets.
/// `value` / `confirm` are owned by `App` and passed in, so this view stays pure.
pub fn vault_view<'a>(
	value: &'a str,
	confirm: &'a str,
	creating: bool,
	failed: bool,
	body: &'a text_editor::Content,
	card: dialog::Card,
) -> Element<'a, Message> {
	let mut content = column![dialog::selectable_body(body)].spacing(12);

	if failed {
		// The hint differs by mode: a mismatch on create, a wrong passphrase on unlock. Neither
		// is a credential oracle (§12) — this is a LOCAL vault passphrase, not remote auth.
		let hint = if creating {
			"The passphrases are empty or do not match. Please try again."
		} else {
			"That passphrase was not correct. Please try again."
		};
		content = content.push(text(hint).size(14).color(PASSPHRASE_ERROR));
	}

	content = content.push(
		text_input("Master passphrase", value)
			.id(VAULT_INPUT_ID)
			.secure(true)
			.on_input(Message::VaultInputChanged)
			.on_submit(Message::VaultSubmitted),
	);

	// The confirm field exists only while creating — there is nothing to confirm on unlock.
	if creating {
		content = content.push(
			text_input("Confirm passphrase", confirm)
				.id(VAULT_CONFIRM_INPUT_ID)
				.secure(true)
				.on_input(Message::VaultConfirmChanged)
				.on_submit(Message::VaultSubmitted),
		);
	}

	let (title, action) = if creating {
		("Create vault passphrase", "Create")
	} else {
		("Unlock vault", "Unlock")
	};

	dialog::dialog(
		title.to_owned(),
		Message::VaultCancelled,
		content.into(),
		vec![
			button(action).on_press(Message::VaultSubmitted).into(),
			button("Cancel").on_press(Message::VaultCancelled).into(),
		],
		card,
	)
}

/// The keyboard-interactive prompt (§7): the server's challenge shown as one masked-or-plain
/// field per prompt, in the shared dialog chrome. `body` (`App::dialog_body`) carries the intro
/// plus the server's heading/instructions as one selectable block; the fields are their own
/// widgets. `prompts` describes each field (its caption and whether to mask it), `answers` holds
/// what the user has typed so far — one entry per prompt, in order — so a submit reads them back
/// in the same order. Both are owned by `App` and passed in, so this view stays pure.
pub fn interactive_view<'a>(
	prompts: &'a [crate::bridge::InteractivePrompt],
	answers: &'a [String],
	body: &'a text_editor::Content,
	card: dialog::Card,
) -> Element<'a, Message> {
	let mut content = column![dialog::selectable_body(body)].spacing(12);

	// One field per prompt, in order. `echo` decides masking: an echoed prompt (a username)
	// shows its text, a non-echoed one (a password / OTP) is masked. The caption sits above the
	// field — server prompts can be a full sentence — and the answer is looked up by index so
	// the field shows what has been typed. Enter in any field submits the whole set.
	for (index, prompt) in prompts.iter().enumerate() {
		let value = answers.get(index).map_or("", String::as_str);
		let field = text_input("", value)
			.id(interactive_field_id(index))
			.secure(!prompt.echo)
			.on_input(move |text| Message::InteractiveAnswerChanged(index, text))
			.on_submit(Message::InteractiveSubmitted);
		content = content.push(column![text(&prompt.label).size(14), field].spacing(4));
	}

	dialog::dialog(
		"Additional authentication".to_owned(),
		Message::InteractiveCancelled,
		content.into(),
		vec![
			button("Submit")
				.on_press(Message::InteractiveSubmitted)
				.into(),
			button("Cancel")
				.on_press(Message::InteractiveCancelled)
				.into(),
		],
		card,
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_string_within_budget_is_left_untouched() {
		// Arrange: shorter than the budget, and exactly at it — both fit.
		// Act / Assert
		assert_eq!(elide_middle("notes.txt", 20), "notes.txt");
		assert_eq!(elide_middle("exactly-ten", 11), "exactly-ten");
		assert!(!elide_middle("notes.txt", 20).contains('…'));
	}

	#[test]
	fn a_long_string_keeps_both_ends_and_fits_the_budget() {
		// Arrange: distinct head and tail so the cut is checkable, well past the budget.
		let text = format!("{}{}", "a".repeat(40), "b".repeat(40));

		// Act
		let shown = elide_middle(&text, 21);

		// Assert: the middle is gone, both ends survive, and it lands on the budget.
		assert!(shown.contains('…'));
		assert!(shown.starts_with('a'));
		assert!(shown.ends_with('b'));
		assert_eq!(shown.chars().count(), 21);
	}

	#[test]
	fn the_cut_falls_on_a_character_never_inside_a_glyph() {
		// Arrange: multi-byte characters, so a byte-wise cut would split one and panic.
		let text = "日本語のとても長いファイル名前.txt";

		// Act
		let shown = elide_middle(text, 8);

		// Assert: it fits the budget and is still valid UTF-8 (it is a `String`, so a cut
		// through a glyph would have panicked building it).
		assert!(shown.contains('…'));
		assert_eq!(shown.chars().count(), 8);
	}
}

#[cfg(test)]
mod layout_boundary_tests {
	use super::{
		cell_index, cells, cells_covering, fraction, lines_scrolled, pixels, signed_pixels,
	};

	/// The two directions round-trip for every count a layout can reach (§111).
	#[test]
	fn a_count_survives_the_trip_to_pixels_and_back() {
		for count in [0usize, 1, 2, 39, 100, 4096, 65_535, 1_000_000] {
			assert_eq!(cells(pixels(count, 20.0), 20.0), count, "at {count}");
		}
	}

	/// The floor and the ceiling differ by exactly one partial row, which is the whole reason both
	/// exist: a viewport wants the partly-visible row at its bottom edge drawn.
	#[test]
	fn covering_a_partial_row_takes_one_more_than_fits_in_it() {
		assert_eq!(cells(50.0, 20.0), 2, "two whole rows fit in 50px");
		assert_eq!(
			cells_covering(50.0, 20.0),
			3,
			"three are needed to cover it"
		);
		// An exact multiple is the one case where they agree.
		assert_eq!(cells(40.0, 20.0), 2);
		assert_eq!(cells_covering(40.0, 20.0), 2);
	}

	/// Nothing measurable answers zero rather than a wrapped count. Before §111 each of these was a
	/// bare `as usize` on a float, where a negative became an enormous index.
	#[test]
	fn a_measurement_that_is_not_a_measurement_answers_zero() {
		assert_eq!(cells(-100.0, 20.0), 0, "scrolled to a negative offset");
		assert_eq!(cells(f32::NAN, 20.0), 0, "measured before the first layout");
		assert_eq!(
			cells(100.0, 0.0),
			0,
			"a pitch of nothing divides into nothing"
		);
		// Infinity is the one that actually bit: it floors to infinity rather than to NaN, so the
		// first version of the guard let it through and `as usize` saturated at `usize::MAX`.
		assert_eq!(cells(f32::INFINITY, 20.0), 0, "no finite row count");
		assert_eq!(cells_covering(f32::INFINITY, 20.0), 0);
		assert_eq!(cells_covering(-1.0, 20.0), 0);
		assert_eq!(cell_index(-1.0, 20.0), 0);
	}

	/// A negative row is a real position, not an error: a picture whose top has scrolled above the
	/// viewport is placed at a negative `y` so its visible part still lands correctly (§41).
	#[test]
	fn a_row_above_the_viewport_keeps_its_sign() {
		assert!((signed_pixels(-3, 20.0) - -60.0).abs() < f32::EPSILON);
		assert!((signed_pixels(0, 20.0)).abs() < f32::EPSILON);
		assert!((signed_pixels(3, 20.0) - 60.0).abs() < f32::EPSILON);
	}

	/// The grid's own geometry clamps rather than wrapping, since a grid past `u16` cannot be drawn.
	#[test]
	fn a_cell_index_clamps_at_the_grids_own_ceiling() {
		assert_eq!(cell_index(0.0, 10.0), 0);
		assert_eq!(cell_index(95.0, 10.0), 9);
		assert_eq!(cell_index(f32::MAX, 1.0), u16::MAX);
	}

	/// A progress fraction stays inside `0.0..=1.0` whatever it is handed, including the zero whole
	/// that would otherwise be a NaN painted as a hole in the bar.
	#[test]
	fn a_progress_fraction_is_bounded_at_both_ends() {
		assert!((fraction(0, 100) - 0.0).abs() < f32::EPSILON);
		assert!((fraction(50, 100) - 0.5).abs() < f32::EPSILON);
		assert!((fraction(100, 100) - 1.0).abs() < f32::EPSILON);
		assert!(
			(fraction(200, 100) - 1.0).abs() < f32::EPSILON,
			"more sent than expected still reads as full"
		);
		assert!((fraction(5, 0) - 0.0).abs() < f32::EPSILON, "nothing to do");
	}

	/// A wheel flick rounds to the nearest line, so the smallest real movement still scrolls.
	#[test]
	fn a_wheel_flick_rounds_rather_than_vanishing() {
		assert_eq!(lines_scrolled(0.6), 1);
		assert_eq!(lines_scrolled(-0.6), -1);
		assert_eq!(lines_scrolled(0.4), 0, "less than half a line is no line");
		assert_eq!(lines_scrolled(f32::NAN), 0);
		// Clamped, so a nonsensical delta cannot wrap into a scroll the other way.
		assert_eq!(lines_scrolled(f32::MAX), i32::from(i16::MAX));
		assert_eq!(lines_scrolled(f32::MIN), i32::from(i16::MIN));
	}
}
