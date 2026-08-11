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
use crate::preview::{Picture, Preview, Status};
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

/// The whole picture screen (§53): the toolbar, then whichever of the three states the tab is in.
pub fn view(preview: &Preview, tab_id: u64) -> Element<'_, Message> {
	let body: Element<'_, Message> = match &preview.status {
		Status::Loading => centered(text("Loading…").size(15).color(MUTED_FG).into()),
		Status::Failed(reason) => failed_body(reason, tab_id),
		// `Ready` without a picture cannot happen — `set_loaded` writes both in one move — but the
		// view refuses to assume it: a missing picture reads as still loading rather than panicking.
		Status::Ready => match &preview.picture {
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
/// `ContentFit::Contain` is the starting fit: the whole picture, scaled DOWN to the region if it is
/// too big and left at its own size if it is not. Never scaled up to fill — a 32×32 favicon blown
/// across the window would be a wall of soft squares, and the user asked to see the file, not an
/// enlargement of it. Zooming in from there is a deliberate act, and the widget's own.
fn picture_body(picture: &Picture) -> Element<'_, Message> {
	let viewer = image::viewer(picture.handle.clone())
		.content_fit(ContentFit::Contain)
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
