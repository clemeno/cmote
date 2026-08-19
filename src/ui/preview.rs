// ui/preview.rs — the picture tab's view (PLAN §53).
//
// A pure view over `preview::Preview`: a toolbar naming the file and what it turned out to be, then
// the picture itself — or, while it is on its way, a word; or, if it could not be drawn, the reason
// and a Close. The model owns the decoded handle and which of those three states it is in; this file
// only draws them. The same three-way split the editor uses (§32), and it keeps the whole decode —
// caps, sniffing, refusals — testable with no window.
//
// Zoom and pan are `iced::widget::image::viewer`'s, not cmote's: the widget already scroll-zooms
// about the pointer and drag-pans, and it keeps that scale in its OWN state. So the model carries no
// zoom level and nothing here has to be reset when a picture is replaced — there is exactly one
// picture per tab for the life of the tab, which is what makes that free.

use iced::alignment::{Horizontal, Vertical};
use iced::widget::{button, column, container, image, row, text};
use iced::{Border, Color, ContentFit, Element, Length, Padding};

use crate::app::Message;
use crate::preview::{Picture, Preview, PreviewStatus};
use crate::ui::explorer::{FG, HEADER_BG, MUTED_FG, NOTICE_FG, PANEL_BG};

/// The ground a picture is drawn on. Deliberately a MID grey rather than the panels' near-black:
/// a transparent PNG shows this colour through its holes, and near-black would swallow the dark
/// artwork that transparency is most often used for. `ponytail:` a checkerboard is the convention
/// and would be unambiguous — it needs a tiled custom widget, and one flat tone that loses to
/// neither end of the range buys most of the same clarity for none of it.
const IMAGE_GROUND: Color = Color::from_rgb8(0x4a, 0x4a, 0x4a);

/// How far in and out the picture may be zoomed (§53). Out to a third so a big scan fits in a small
/// region; in to 10×, which is far enough to read a screenshot's smallest text — the reason someone
/// zooms a screenshot at all.
const MIN_SCALE: f32 = 0.33;
const MAX_SCALE: f32 = 10.0;

/// How the picture is laid into the body BEFORE anyone has zoomed anything (§53) — see the tests at
/// the foot of this file, which pin both halves of it.
///
/// `ScaleDown` and NOT `Contain`, which is what this was and was wrong. The two agree on the case
/// everyone pictures — a photograph larger than the window, shrunk until all of it is on screen at
/// once, so there is nothing to scroll to — and differ on the case nobody pictures: `Contain` fits
/// SMALL pictures too, upward, so a 32×32 favicon opened as a 600-pixel wall of soft squares.
/// Upscaling invents detail the file does not contain, and the request was to see the file. So:
/// smaller than the body opens at 1:1, bigger than the body opens contained, and both open centred.
/// Zooming past either is a deliberate act, and the widget's own.
const INITIAL_FIT: ContentFit = ContentFit::ScaleDown;

/// The whole picture screen (§53): the toolbar, then whichever of the three states the tab is in.
pub fn view(preview: &Preview, tab_id: u64) -> Element<'_, Message> {
	let body: Element<'_, Message> = match &preview.status {
		PreviewStatus::Loading => centered(text("Loading…").size(15).color(MUTED_FG).into()),
		PreviewStatus::Failed(reason) => failed_body(reason, tab_id),
		// `Ready` without a picture cannot happen — `set_loaded` writes both in one move — but the
		// view refuses to assume it: a missing picture reads as still loading rather than panicking.
		PreviewStatus::Ready => match &preview.picture {
			Some(picture) => picture_body(picture),
			None => centered(text("Loading…").size(15).color(MUTED_FG).into()),
		},
	};

	column![toolbar(preview, tab_id), body]
		.width(Length::Fill)
		.height(Length::Fill)
		.into()
}

/// The toolbar: the path, then what the file turned out to BE — format, pixel size, byte size — and
/// a Close (§53). No dirty dot and no Save: there is nothing here that can be changed.
fn toolbar(preview: &Preview, tab_id: u64) -> Element<'_, Message> {
	let title = text(preview.path.clone()).size(13).color(FG);

	let mut info = row![].spacing(10).align_y(Vertical::Center);
	if let Some(picture) = &preview.picture {
		// The format is what the BYTES said, not what the name promised (§53) — so a `.jpg` that is
		// really a PNG says PNG here, which is the one place a user would ever find that out.
		info = info.push(badge(picture.format));
		info = info.push(badge(&format!("{}×{}", picture.width, picture.height)));
		info = info.push(badge(&crate::ui::terminal::human_bytes(picture.bytes)));
	}

	let bar = row![
		title,
		// A greedy gap, so the title sits left and the facts and the Close sit right whatever the
		// path's length.
		iced::widget::space::horizontal(),
		info,
		tool_button("Close", Message::TabCloseRequested(tab_id)),
	]
	.spacing(12)
	.align_y(Vertical::Center);

	container(bar)
		.width(Length::Fill)
		.padding(Padding::from([6.0, 10.0]))
		.style(|_theme| container::Style {
			background: Some(HEADER_BG.into()),
			..container::Style::default()
		})
		.into()
}

/// The picture, on its ground, zoomable and pannable (§53).
///
/// It opens at `INITIAL_FIT` — 1:1 if it fits, shrunk whole if it does not — and CENTRED, which is
/// the widget's own doing and not this container's: `image::viewer` splits whatever room is left
/// over evenly on both sides of the image, so a small picture sits in the middle of its ground with
/// no alignment asked for here. That also means panning is dead until someone zooms past the body,
/// because at the opening fit there is nothing hidden to pan to.
fn picture_body(picture: &Picture) -> Element<'_, Message> {
	let viewer = image::viewer(picture.handle.clone())
		.content_fit(INITIAL_FIT)
		.min_scale(MIN_SCALE)
		.max_scale(MAX_SCALE)
		.width(Length::Fill)
		.height(Length::Fill);

	container(viewer)
		.width(Length::Fill)
		.height(Length::Fill)
		.style(|_theme| container::Style {
			background: Some(IMAGE_GROUND.into()),
			..container::Style::default()
		})
		.into()
}

/// The reason a picture could not be shown, in place of it, with only a Close (§53) — the same
/// shape the editor's refusal takes, because it is the same situation: a tab that opened on a file
/// it cannot render, and the honest thing is the sentence plus the way out.
fn failed_body(reason: &str, tab_id: u64) -> Element<'static, Message> {
	centered(
		column![
			text("This file cannot be previewed.")
				.size(15)
				.color(NOTICE_FG),
			text(reason.to_owned()).size(13).color(FG),
			tool_button("Close", Message::TabCloseRequested(tab_id)),
		]
		.spacing(14)
		.align_x(Horizontal::Center)
		.into(),
	)
}

/// One dimmed fact in the toolbar's right-hand cluster.
fn badge(label: &str) -> Element<'static, Message> {
	text(label.to_owned()).size(11).color(MUTED_FG).into()
}

/// One toolbar button. Always enabled — a preview has a single action, and Close can never be
/// inapplicable — so unlike the editor's there is no greyed form of this.
fn tool_button(label: &str, message: Message) -> Element<'static, Message> {
	button(text(label.to_owned()).size(12).color(FG))
		.padding(Padding::from([4.0, 10.0]))
		.style(|_theme, _status| button::Style {
			background: Some(PANEL_BG.into()),
			text_color: FG,
			border: Border {
				radius: 4.0.into(),
				..Border::default()
			},
			..button::Style::default()
		})
		.on_press(message)
		.into()
}

/// Centre one element in the whole remaining area — the loading word and the refusal card.
fn centered(inner: Element<'_, Message>) -> Element<'_, Message> {
	container(inner)
		.width(Length::Fill)
		.height(Length::Fill)
		.align_x(Horizontal::Center)
		.align_y(Vertical::Center)
		.into()
}

// The opening fit, and only the opening fit. There is no window here and nothing is drawn: these ask
// `INITIAL_FIT` the same question `image::viewer` asks it on the first frame — given a picture this
// big and a body this big, what size does it come out — and check the answer against the two rules
// the tab is supposed to obey. They are worth their length because the wrong variant LOOKS right in
// the common case: a photograph is bigger than the body, and every fit that scales at all handles
// that identically. It is the SMALL picture that tells them apart, and that is the case a screenshot
// of a photograph will never show you.
#[cfg(test)]
mod tests {
	use super::*;
	use iced::Size;

	/// The size the picture opens at, in the body it opens into.
	fn opened(picture: (f32, f32), body: (f32, f32)) -> Size {
		INITIAL_FIT.fit(Size::new(picture.0, picture.1), Size::new(body.0, body.1))
	}

	#[test]
	fn a_picture_that_already_fits_opens_at_its_own_size() {
		// A 32×32 favicon in a big window is thirty-two pixels across, not a wall of soft squares.
		// Upscaling invents detail that is not in the file, and the user asked to see the file.
		assert_eq!(opened((32.0, 32.0), (800.0, 600.0)), Size::new(32.0, 32.0));
		// Exactly-fits counts as fits: no scaling at the boundary either.
		assert_eq!(
			opened((800.0, 600.0), (800.0, 600.0)),
			Size::new(800.0, 600.0)
		);
	}

	#[test]
	fn a_picture_too_big_for_the_body_opens_whole_rather_than_cropped() {
		// A 4000×2000 photograph in an 800×600 body: bounded by the WIDTH, because that is the side
		// that runs out first, and the height follows to keep the shape. The whole picture is on
		// screen at once — nothing to scroll to, which is the point.
		let fitted = opened((4000.0, 2000.0), (800.0, 600.0));
		assert_eq!(fitted, Size::new(800.0, 400.0));
		assert!(fitted.width <= 800.0 && fitted.height <= 600.0);
	}

	#[test]
	fn a_tall_picture_is_bounded_by_the_height_instead() {
		// The other axis, so the rule is "whichever side runs out first" and not "always the width".
		let fitted = opened((1000.0, 4000.0), (800.0, 600.0));
		assert_eq!(fitted, Size::new(150.0, 600.0));
		assert!(fitted.width <= 800.0 && fitted.height <= 600.0);
	}

	#[test]
	fn the_shape_of_the_picture_survives_the_fit() {
		// Never `Fill`: a 2:1 photograph squeezed into a 4:3 body would be a 4:3 photograph of
		// something that does not look like that.
		let fitted = opened((4000.0, 2000.0), (600.0, 600.0));
		assert!((fitted.width / fitted.height - 2.0).abs() < 0.001);
	}
}
