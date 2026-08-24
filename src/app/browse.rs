// app/browse.rs — the `Tab` methods for the browser strip's two panes (PLAN §18, §19, §20, §21).
//
// The folder tree and the files pane are the two halves of the band under the grid (`CONTEXT.md`:
// **Browser strip**, **Pane**), and this file is the tab's half of them: the two dispatchers a pane
// event lands in, the keys each pane answers while it holds the focus, the scrolling that keeps the
// selection on screen, and the errands a row's action turns into — a listing to fetch, a `cd` to
// type, a folder to create, entries to delete.
//
// What a pane *means* is not here. `crate::explorer` owns the tree's nodes and path arithmetic,
// `crate::files` one directory's batched listing and its selection rules, and `crate::panes` the
// rules that span the two; all three are tested without a session. What is left here is everything
// that needs the channel, the shell, or the window's width — the same split `crate::forward` and
// `app/forwards.rs` already make.
//
// Split out of `app/mod.rs` for size (§128). `pub(super)` marks the methods the rest of `app`
// calls; a plain `fn` is used only here. Named `browse` rather than `panes` on purpose: `panes` is
// already a crate module, and `self.panes` is the field these methods work on.

use super::{
	ExplorerMessage, FilesMessage, Focus, Message, Modal, SshCommand, Tab,
	browse_upload_folder_into, browse_upload_into, explorer, files, join_lines, keep_visible,
	pick_download_folder, pick_download_target, pick_download_tree_target, term, transfer, ui,
};

impl Tab {
	/// Keys while the folder tree has the focus (§20). Up/Down walk the visible rows,
	/// Right opens a folder and Left shuts it, Tab/Shift+Tab step like the arrows, Enter
	/// sends the shell there, F2 renames, and Esc hands the keyboard back to the shell.
	pub(super) fn on_tree_key(&mut self, key: &iced::keyboard::Key) -> iced::Task<Message> {
		use iced::keyboard::key::Named;
		let iced::keyboard::Key::Named(named) = key else {
			return iced::Task::none();
		};

		let step = match named {
			Named::ArrowDown | Named::Tab => 1,
			Named::ArrowUp => -1,
			Named::ArrowRight => {
				// Open the folder — the same call the row click makes: a folder never listed is
				// fetched here too, and re-opening a closed one re-lists it (`expand`), so the
				// keyboard catches a shell-side change just as the mouse does.
				if let Some(path) = self.panes.tree.selected().map(str::to_owned)
					&& let Some(fetch) = self.panes.tree.expand(&path, false)
				{
					self.send_command(SshCommand::ListDir(fetch));
				}
				return iced::Task::none();
			}
			Named::ArrowLeft => {
				if let Some(path) = self.panes.tree.selected().map(str::to_owned) {
					self.panes.tree.collapse(&path);
				}
				return iced::Task::none();
			}
			Named::Enter => {
				let Some(path) = self.panes.tree.selected().map(str::to_owned) else {
					return iced::Task::none();
				};
				return self.on_explorer(ExplorerMessage::Cd(path));
			}
			Named::F2 => {
				let Some(path) = self.panes.tree.selected().map(str::to_owned) else {
					return iced::Task::none();
				};
				return self.on_explorer(ExplorerMessage::RenameStarted(path));
			}
			// F5 refreshes the whole visible tree, the same as the header ↻ button — the
			// familiar file-manager key for "bring what I am looking at up to date".
			Named::F5 => return self.on_explorer(ExplorerMessage::RefreshTree),
			Named::Escape => {
				self.set_focus(Focus::Terminal);
				return iced::Task::none();
			}
			_ => return iced::Task::none(),
		};

		self.panes.tree.step(step);
		self.scroll_tree_into_view()
	}

	/// Keys while the files pane has the focus (§20). Left/Right step one cell and Up/Down
	/// a whole row — the grid wraps at the window's width, so how many cells that is comes
	/// from the same arithmetic the layout uses. Tab/Shift+Tab are next/previous, Enter
	/// opens a folder, F2 renames, and Esc hands the keyboard back to the shell.
	///
	/// The movement keys fold into a `FilesNav` first, because a step is relative to the current
	/// cell while an edge is an absolute end of the grid. Home and End MUST be absolute: a relative
	/// jump reads the empty-selection default and would land on the wrong end when nothing is
	/// selected yet (see `Files::jump_to_edge`).
	pub(super) fn on_files_key(
		&mut self,
		key: &iced::keyboard::Key,
		modifiers: iced::keyboard::Modifiers,
	) -> iced::Task<Message> {
		use iced::keyboard::key::Named;

		// What one movement key asks for. Declared here rather than mid-body: an item is in scope
		// from the start of the block whatever line it is written on, so writing it where it is first
		// used only makes the reader think its scope begins there (`items_after_statements`, §111).
		enum FilesNav {
			/// Relative to the current cell, in model-space cells.
			Step(isize),
			/// An absolute end of the grid — `true` for the last cell.
			Edge(bool),
		}

		// Ctrl+A takes the whole listing (§21). Checked before the named-key gate below,
		// since it is the pane's only shortcut on a character key.
		if modifiers.control()
			&& matches!(key, iced::keyboard::Key::Character(character)
				if character.as_str().eq_ignore_ascii_case("a"))
		{
			self.panes.pane.select_all(self.panes.show_hidden());
			return iced::Task::none();
		}

		let iced::keyboard::Key::Named(named) = key else {
			return iced::Task::none();
		};

		// Signed, because these become the deltas the pane's `step` walks by and a delta goes both
		// ways. `cast_signed` says the reinterpretation out loud where `as isize` only implied it.
		let columns = ui::files::columns(self.files_width()).cast_signed();
		// A page is a screenful of rows (less one, for context), turned into a model-space delta
		// by the column count — the same units `step` moves the arrows in.
		let page = ui::files::page_rows(&self.panes.pane).cast_signed() * columns;
		// Shift held on a movement key extends the selection instead of moving it (§21). Not on
		// Tab: there, Shift already means "the other way".
		let extend = modifiers.shift();
		let (nav, extend) = match named {
			Named::ArrowRight => (FilesNav::Step(1), extend),
			Named::ArrowLeft => (FilesNav::Step(-1), extend),
			Named::ArrowDown => (FilesNav::Step(columns), extend),
			Named::ArrowUp => (FilesNav::Step(-columns), extend),
			// PageDown/PageUp are focus-gated to the pane, so they never fight the terminal's own
			// scrollback on the same keys (`scroll_motion`) — that fires only while the terminal
			// holds the keyboard.
			Named::PageDown => (FilesNav::Step(page), extend),
			Named::PageUp => (FilesNav::Step(-page), extend),
			// Home/End land on an absolute end, right even with nothing selected yet.
			Named::Home => (FilesNav::Edge(false), extend),
			Named::End => (FilesNav::Edge(true), extend),
			Named::Tab if modifiers.shift() => (FilesNav::Step(-1), false),
			Named::Tab => (FilesNav::Step(1), false),
			Named::Enter => {
				let Some(path) = self.panes.pane.cursor().map(str::to_owned) else {
					return iced::Task::none();
				};
				// Straight through the double-click's own handler, which is where "only a
				// directory can be entered" is decided.
				return self.on_files(FilesMessage::EntryOpened(path));
			}
			Named::F2 => {
				let Some(path) = self.panes.pane.cursor().map(str::to_owned) else {
					return iced::Task::none();
				};
				return self.on_files(FilesMessage::RenameStarted(path));
			}
			// F5 re-lists the directory on show, the same as the header ↻ button — the pane's
			// twin of the tree's F5, each refreshing the pane that holds the keyboard.
			Named::F5 => return self.on_files(FilesMessage::Refresh),
			Named::Escape => {
				self.set_focus(Focus::Terminal);
				return iced::Task::none();
			}
			_ => return iced::Task::none(),
		};

		let show_hidden = self.panes.show_hidden();
		match nav {
			FilesNav::Step(delta) => self.panes.pane.step(show_hidden, delta, extend),
			FilesNav::Edge(to_last) => self.panes.pane.jump_to_edge(show_hidden, to_last, extend),
		}
		self.resolve_selected_link();
		// Only the keyboard scrolls: a click is already on a cell the user can see, and
		// scrolling under their cursor would move the thing they just aimed at.
		self.scroll_files_into_view()
	}

	/// Select whatever the rubber band now covers (§21). The grid's geometry belongs to the
	/// view, so the band is turned into cell indices there and back into paths here — the
	/// same split the arrow keys already use.
	fn apply_band(&mut self) {
		let Some(rect) = self.panes.pane.band().map(files::Band::rect) else {
			return;
		};
		let Some(directory) = self.panes.pane.path().map(str::to_owned) else {
			return;
		};
		let rows = self.panes.rows();
		let paths: Vec<String> = ui::files::band_hits(
			rect,
			ui::files::columns(self.files_width()),
			rows.len(),
			self.panes.pane.scroll(),
		)
		.into_iter()
		.filter_map(|index| Some(explorer::join(&directory, &rows.get(index)?.name)))
		.collect();
		self.panes.pane.set_band_selection(paths);
	}

	/// Which entries a context-menu item acts on (§21): the whole selection when the menu
	/// was opened on part of it, that one entry otherwise. In grid order, since that is the
	/// order a list of copied names should come out in.
	fn action_targets(&self, path: &str) -> Vec<String> {
		if self.panes.pane.selected_count() > 1 && self.panes.pane.is_selected(path) {
			self.panes
				.pane
				.selected_rows(self.panes.show_hidden())
				.into_iter()
				.map(|(path, _)| path)
				.collect()
		} else {
			vec![path.to_owned()]
		}
	}

	/// Ask the server where the selected entry points, when it is a symlink (§20) — the
	/// details popup shows a link's target, and only the server can resolve it.
	///
	/// One `readlink` per *selected* link, not one per link in the listing: resolving them
	/// all is the round-trip-per-entry cost the pane is built to avoid (§19).
	fn resolve_selected_link(&mut self) {
		if let Some(path) = self.panes.pane.cursor().map(str::to_owned)
			&& self.panes.pane.kind_of(&path) == Some(files::FilesKind::Link)
			&& self.panes.pane.link_target().is_none()
		{
			self.send_command(SshCommand::ReadLink(path));
		}
	}

	/// How wide the files pane is: the window less the folder tree's column beside it (§18, §19).
	/// The tree took its width off the terminal before; it takes it off the pane now, so every
	/// piece of the pane's geometry that keys off its width — the column count, the popup, the
	/// rubber band, the menus — reads this rather than the raw window width. `Explorer::reserved`
	/// is zero when the tree is hidden, so the pane is the full window then.
	pub(super) fn files_width(&self) -> f32 {
		self.window_size.width - self.panes.tree.reserved()
	}

	/// Scroll the files pane so the selected cell is on screen (§20). The grid's geometry
	/// is the view's (`ui::files`), so the same arithmetic that lays the cells out is what
	/// works out where the selected one sits. The model is told the new offset as well as
	/// the widget, because the details popup is placed against it on this very frame.
	fn scroll_files_into_view(&mut self) -> iced::Task<Message> {
		let Some(index) = self.panes.pane.selected_index(self.panes.show_hidden()) else {
			return iced::Task::none();
		};
		let row = index / ui::files::columns(self.files_width());
		let current = self.panes.pane.scroll();
		// Already visible falls back to where it already is: the model and the widget are told the
		// offset either way, because the details popup is placed against it on this very frame.
		let offset = keep_visible(
			current,
			ui::files::grid_height(&self.panes.pane),
			ui::files::row_top(row),
			ui::files::CELL_HEIGHT,
		)
		.unwrap_or(current);
		self.panes.pane.set_scroll(offset);
		iced::widget::operation::scroll_to(
			ui::files::GRID_ID,
			iced::widget::scrollable::AbsoluteOffset { x: 0.0, y: offset },
		)
	}

	/// The same, for the folder tree — one fixed-height row rather than a wrapping grid.
	fn scroll_tree_into_view(&mut self) -> iced::Task<Message> {
		let Some(index) = self.panes.tree.selected_index() else {
			return iced::Task::none();
		};
		let current = self.panes.tree.scroll();
		let offset = keep_visible(
			current,
			ui::explorer::tree_height(
				self.panes.pane.height(),
				self.panes.pane.path(),
				self.panes.tree.width(),
			),
			ui::pixels(index, ui::explorer::ROW_HEIGHT),
			ui::explorer::ROW_HEIGHT,
		)
		.unwrap_or(current);
		self.panes.tree.set_scroll(offset);
		iced::widget::operation::scroll_to(
			ui::explorer::TREE_ID,
			iced::widget::scrollable::AbsoluteOffset { x: 0.0, y: offset },
		)
	}

	/// Handle one event from the remote folder tree (§18). The model decides what the
	/// action means; this only relays the network side of it — the listings it asks for,
	/// the `cd` it types into the shell, the clipboard writes — and refits the grid when
	/// the pane's footprint changes.
	#[expect(
		clippy::too_many_lines,
		reason = "a dispatch over ExplorerMessage: length is the number of tree actions, not depth"
	)]
	pub(super) fn on_explorer(&mut self, message: ExplorerMessage) -> iced::Task<Message> {
		match message {
			ExplorerMessage::Toggled => {
				self.panes.tree.toggle();
				// A hidden pane cannot hold the keyboard: hand it back to the shell (§20).
				if !self.panes.tree.visible() && self.focus == Focus::Tree {
					self.set_focus(Focus::Terminal);
				}
				// The pane's width just moved between it and the grid: reflow both the
				// local emulator and the remote pty to the new column count.
				self.refit_grid();
			}
			ExplorerMessage::HiddenToggled => {
				self.panes.tree.toggle_hidden();
				// Persist the flip now (§14, §22): the toggle folds into the same per-target
				// snapshot as the paths and pane sizes, so it survives even a later hard exit.
				self.persist_session();
			}
			ExplorerMessage::PanePressed => self.focus_pane(Focus::Tree),
			ExplorerMessage::Scrolled(offset) => self.panes.tree.set_scroll(offset),
			ExplorerMessage::RowClicked(path) => {
				self.focus_pane(Focus::Tree);
				if let Some(fetch) = self.panes.tree.toggle_node(&path) {
					self.send_command(SshCommand::ListDir(fetch));
				}
				// Clicking a folder in the tree also points the files pane at it, WITHOUT
				// moving the shell — that is what makes the pane usable to look inside a
				// folder you are not in (§19).
				if let Some(request) = self.panes.pane.show(&path) {
					self.list_files(request);
				}
			}
			ExplorerMessage::RowRightClicked(path) => {
				self.focus_pane(Focus::Tree);
				self.panes.tree.select(&path);
				self.panes.tree.open_menu(path);
			}
			ExplorerMessage::PointerMoved(point) => self.panes.tree.set_pointer(point),
			ExplorerMessage::MenuDismissed => self.panes.tree.close_menu(),
			ExplorerMessage::RefreshDir(path) => {
				self.panes.tree.close_menu();
				// The menu's "Refresh" answers "is this folder still here, under this name, holding
				// these children?" Its CONTENTS come from re-listing the folder itself (forced open,
				// so the result shows at once); its own NAME and EXISTENCE come from re-listing its
				// PARENT — a rename or deletion made from the shell surfaces in the parent's listing,
				// never the folder's. The root has no parent, so only its contents refresh.
				if let Some(parent) = explorer::parent(&path).map(str::to_owned)
					&& let Some(fetch) = self.panes.tree.refresh_dir(&parent)
				{
					self.send_command(SshCommand::ListDir(fetch));
				}
				if let Some(fetch) = self.panes.tree.expand(&path, true) {
					self.send_command(SshCommand::ListDir(fetch));
				}
			}
			ExplorerMessage::RefreshTree => {
				// The header ↻ button and F5: re-list every open folder, so all the expanded
				// content is current in one action — the user never has to work out which folders
				// a move touched. Each becomes its own listing request.
				self.panes.tree.close_menu();
				for fetch in self.panes.tree.refresh_open() {
					self.send_command(SshCommand::ListDir(fetch));
				}
			}
			ExplorerMessage::CollapseAll => {
				// The header's collapse-all button: close every branch back to the root's own
				// children. Local state only — nothing is re-fetched — so this needs no command.
				self.panes.tree.close_menu();
				self.panes.tree.collapse_all();
			}
			ExplorerMessage::Cd(path) => {
				// The tree's "Open in terminal" and its Enter key: a deliberate console move,
				// quoted so a folder name carrying a quote stays one argument (§18). The pane
				// then follows the `cd` it can see, the same as any other console move.
				self.panes.tree.close_menu();
				self.move_shell_to(&path);
			}
			ExplorerMessage::UploadHere(path) => {
				// The tree's "Upload…": pick local files to send into this folder (§17),
				// whichever directory the shell itself is in.
				self.panes.tree.close_menu();
				return browse_upload_into(path);
			}
			ExplorerMessage::UploadFolderHere(path) => {
				// The tree's "Upload folder…": pick a local folder to send whole into this one (§17).
				self.panes.tree.close_menu();
				return browse_upload_folder_into(path);
			}
			ExplorerMessage::NewFolderHere(path) => {
				// The tree's "New folder…": create a subfolder inside the right-clicked one (§18).
				self.panes.tree.close_menu();
				return self.begin_new_folder(path);
			}
			ExplorerMessage::DeleteStarted(path) => {
				// The tree's "Delete…": remove this folder and its whole subtree, once confirmed (§18).
				self.panes.tree.close_menu();
				self.begin_delete(vec![path]);
			}
			ExplorerMessage::RenameStarted(path) => {
				self.panes.tree.start_rename(path);
				// The root has no parent, so it declines to be renamed; only focus the
				// field when an edit actually opened.
				if self.panes.tree.editing().is_some() {
					return iced::widget::operation::focus(ui::explorer::RENAME_INPUT_ID);
				}
			}
			ExplorerMessage::RenameEdited(text) => self.panes.tree.edit_rename(text),
			ExplorerMessage::RenameCommitted => {
				if let Some((from, to)) = self.panes.tree.commit_rename() {
					self.send_command(SshCommand::RenameDir { from, to });
				}
			}
			ExplorerMessage::CopyName(path) => {
				self.panes.tree.close_menu();
				let text = explorer::name(&path).to_owned();
				return self.copy_to_clipboard(text);
			}
			ExplorerMessage::CopyRelative(path) => {
				self.panes.tree.close_menu();
				// The menu disables this item without a cwd, so this is belt and braces.
				let Some(cwd) = self.terminal.as_ref().and_then(term::Terminal::cwd) else {
					return iced::Task::none();
				};
				let text = explorer::relative(cwd, &path);
				return self.copy_to_clipboard(text);
			}
			ExplorerMessage::CopyPath(path) => {
				self.panes.tree.close_menu();
				return self.copy_to_clipboard(path);
			}
			ExplorerMessage::CopyCurrentPath => {
				// The header path, not a tree selection: copy the one directory the header
				// names — the files view's — verbatim, the twin of the pane's own button.
				if let Some(path) = self.panes.pane.path() {
					let text = path.to_owned();
					return self.copy_to_clipboard(text);
				}
			}
			ExplorerMessage::SplitterGrabbed => self.panes.tree.set_dragging(true),
			ExplorerMessage::SplitterDragged(pointer) => {
				if self.panes.tree.dragging() {
					// The splitter sits at the pane's left edge and the pane runs to the
					// window's right edge, so the pointer's distance from that edge IS the
					// width — no drag anchor to track. The clamp and the arithmetic are the
					// pair's, so this arm and the pane's twin below no longer restate them
					// with `width`/`x` swapped for `height`/`y`.
					self.panes.drag_tree_splitter(pointer.x, self.window_size);
					self.refit_grid();
				}
			}
			ExplorerMessage::SplitterReleased => self.panes.tree.set_dragging(false),
			// Hover only lights the bar (§18); no relayout, so no grid refit.
			ExplorerMessage::SplitterEntered => self.panes.tree.set_splitter_hovered(true),
			ExplorerMessage::SplitterExited => self.panes.tree.set_splitter_hovered(false),
		}
		iced::Task::none()
	}

	/// Type a quoted `cd` into the shell so the console moves to `path` (§19). The single
	/// way cmote moves the console on the user's behalf: the Sync button, the tree's and the
	/// pane's "Open in terminal" items, and the tree's Enter key all land here. Browsing —
	/// a pane double-click, the "up" button, a tree row click — no longer drags the console;
	/// it only ever follows a `cd` it can see (its own, or one of these), which is what keeps
	/// "who moved the console" answerable. An explicit move also ends any reconnect resume
	/// (§22): the pin that held the pane against the shell's login announcements has done its
	/// job, so the pane is free to follow this move and later ones again.
	///
	/// `ponytail:` a POSIX shell is assumed and the line is typed blind — if a full-screen
	/// program (vim, less) is running these bytes go to it instead, since cmote cannot tell a
	/// prompt from an editor. Upgrade path: only offer it between prompts, which the OSC
	/// announcements could mark.
	/// On a LOCAL session (§103) neither half of that line holds: the pane path is not a path on this
	/// platform, and the four shells the Local bar offers disagree about both the spelling of a path and
	/// the name of the command. So the shell composes its own (`local::shells::ShellKind::cd`), and a path
	/// that will not translate types nothing at all rather than a `cd` to somewhere invented.
	fn move_shell_to(&mut self, path: &str) {
		self.resume_cwd = None;
		let command = match self.local {
			Some(kind) => kind.cd(path),
			None => Some(format!("cd {}", explorer::shell_quote(path))),
		};
		let Some(command) = command else {
			return;
		};
		self.send_command(SshCommand::Input(format!("{command}\r").into_bytes()));
	}

	/// The status bar's "Sync" button (§19): move the console into the directory the files
	/// pane is showing. Browsing the pane or the tree leaves the console where it is, so the
	/// two drift apart on purpose; this is the deliberate, manual way to bring the console
	/// (and with it the tree and the title, which follow it) to the folder on show. A no-op
	/// with no shell or no directory on show; the button dims in those cases and when the two
	/// already agree, so pressing it always has something to do.
	pub(super) fn on_sync(&mut self) {
		let Some(path) = self.panes.pane.path().map(str::to_owned) else {
			return;
		};
		self.move_shell_to(&path);
	}

	/// The status bar's "Reveal" button (§19): Sync read backwards — bring the PANES to the shell.
	///
	/// The two drift apart in both directions, and until now only one of them could be closed from
	/// the bar. Browsing moves the pane and leaves the console alone (§19), and the shell's own
	/// re-announcement cannot bring the pane back: `Files::follow` acts on a *move*, and a shell
	/// standing still at the same prompt announces the same directory every time. So a browse three
	/// folders away was undone only by `cd`-ing the shell somewhere — moving the thing that was
	/// already where you wanted it — or by walking the tree back by hand.
	///
	/// It moves nothing on the remote. No `cd` is typed, no bytes reach the shell: this is the
	/// local view catching up with a shell that stays exactly where it is, which is why it is safe
	/// while a full-screen program is running and `move_shell_to` is not.
	///
	/// Four things happen, and all four are the point:
	///
	/// * the tree opens the chain down to the cwd and selects it — through `Explorer::reveal`, the
	///   **unguarded** one, since the whole reason to press this is that the tree has been walked away
	///   from a cwd that never changed;
	/// * the pane shows that directory (`Files::show`, the deliberate move, not `follow`); and
	/// * both follow-guards are seeded with the same path, so the next prompt's announcement is
	///   correctly read as "still there, nothing to do" rather than as a move — and a real `cd`
	///   after it still carries both panes along; and
	/// * any reconnect resume still settling is ended, the same rule an explicit `move_shell_to`
	///   follows and for the same reason (§22). The pin exists to hold the panes against the
	///   shell's login-then-`cd` announcements until it settles; the user saying out loud where
	///   the panes go outranks that, and leaving it armed would let it swallow the settle as
	///   "already there" and strand them at the login directory — the exact drift this button is
	///   for, caused by pressing it.
	///
	/// A no-op when the shell has never announced a cwd (§17: it needs OSC 7, or a shell configured
	/// to send it) — the button dims then, and whenever the panes are already there. Nothing is
	/// spent in that case either, the pin included: there is no ask to outrank.
	pub(super) fn on_reveal(&mut self) {
		let Some(cwd) = self
			.terminal
			.as_ref()
			.and_then(term::Terminal::cwd)
			.map(str::to_owned)
		else {
			return;
		};
		self.resume_cwd = None;
		let fetches = self.panes.reveal(&cwd);
		self.send_fetches(fetches);
	}

	/// Browse the files pane into a directory (§19): a double-clicked folder, the toolbar's
	/// "up" button, or Enter on the keyboard. This points the PANE only — the console stays
	/// put, so you can look inside a folder you are not in without disturbing the shell. The
	/// console is moved separately and on purpose, by Sync or "Open in terminal"
	/// (`move_shell_to`); a real `cd` there is what brings the pane back into step, via the
	/// shell-follow (§19 "last one wins").
	fn browse_to(&mut self, path: &str) {
		let fetches = self.panes.browse(path);
		self.send_fetches(fetches);
	}

	/// Handle one event from the files pane (§19). Same division of labour as the tree's
	/// handler: the model decides what an action means, this relays the network side of
	/// it — the listings, the `cd`, the clipboard writes, the download — and refits the
	/// grid when the pane's footprint changes.
	#[expect(
		clippy::too_many_lines,
		reason = "a dispatch over FilesMessage: length is the number of pane actions, not depth"
	)]
	pub(super) fn on_files(&mut self, message: FilesMessage) -> iced::Task<Message> {
		match message {
			FilesMessage::Toggled => {
				self.panes.pane.toggle();
				// A hidden pane cannot hold the keyboard: hand it back to the shell (§20).
				if !self.panes.pane.visible() && self.focus == Focus::Files {
					self.set_focus(Focus::Terminal);
				}
				// The pane's height just moved between it and the grid: reflow both the
				// local emulator and the remote pty to the new row count.
				self.refit_grid();
			}
			FilesMessage::PanePressed => {
				self.focus_pane(Focus::Files);
				// A cell's own `mouse_area` swallows the press that lands on it, so one that
				// reaches the pane missed them all. On the grid that starts a rubber band
				// (§21) — which also clears the selection, as every file manager's empty
				// space does; on the header or the notice line it only clears it.
				let pointer = self.panes.pane.pointer();
				let grid = pointer.y >= ui::files::HEADER_HEIGHT
					&& pointer.y
						<= ui::files::HEADER_HEIGHT + ui::files::grid_height(&self.panes.pane);
				if grid {
					self.panes
						.pane
						.begin_band(pointer, self.modifiers.control());
				} else if !self.modifiers.control() {
					self.panes.pane.deselect();
				}
			}
			FilesMessage::PaneReleased => self.panes.pane.end_band(),
			FilesMessage::PaneRightPressed => {
				// A right-press that reached the pane missed every cell, so it landed on the
				// empty grid: open the pane's own menu there (§17). The keyboard follows too,
				// as a left-press would.
				self.focus_pane(Focus::Files);
				self.panes.pane.open_pane_menu();
			}
			FilesMessage::PaneUploadHere => {
				// "Upload… here": send local files into the directory the pane is showing.
				self.panes.pane.close_menu();
				let dir = self.panes.pane.path().unwrap_or("").to_owned();
				return browse_upload_into(dir);
			}
			FilesMessage::PaneUploadFolderHere => {
				// "Upload folder… here": send a whole local folder into the directory on show (§17).
				self.panes.pane.close_menu();
				let dir = self.panes.pane.path().unwrap_or("").to_owned();
				return browse_upload_folder_into(dir);
			}
			FilesMessage::NewFolderHere => {
				// "New folder…": create a folder in the directory the pane is showing (§18).
				self.panes.pane.close_menu();
				let dir = self.panes.pane.path().unwrap_or("").to_owned();
				return self.begin_new_folder(dir);
			}
			FilesMessage::DeleteStarted(path) => {
				// "Delete…": remove the whole selection once confirmed (§18). A right-click inside
				// the selection kept it; one outside has already collapsed onto the clicked entry.
				self.panes.pane.close_menu();
				let targets = self.action_targets(&path);
				self.begin_delete(targets);
			}
			FilesMessage::DownloadFolder(path) => {
				// "Download folder…": recreate this remote directory's tree locally (§19). One
				// transfer at a time, like every other, so a running one blocks it.
				self.panes.pane.close_menu();
				if self.transfers.busy() {
					self.panes.pane.set_notice(transfer::BUSY_NOTICE.to_owned());
					return iced::Task::none();
				}
				return pick_download_tree_target(path);
			}
			FilesMessage::BandMoved(point) => {
				// Window coordinates from the capture layer: the pane's left edge is the window's
				// and it runs to the bottom, so only the vertical origin — the strip's top — comes off.
				let local = iced::Point::new(
					point.x,
					point.y - (self.window_size.height - self.panes.pane.height()),
				);
				self.panes.pane.set_pointer(local);
				if self.panes.pane.drag_band(local) {
					self.apply_band();
				}
			}
			FilesMessage::Scrolled(offset) => self.panes.pane.set_scroll(offset),
			FilesMessage::EntryClicked(path) => {
				self.focus_pane(Focus::Files);
				self.panes.pane.close_menu();
				let show_hidden = self.panes.show_hidden();
				// Shift runs a range from the anchor, Ctrl adds or removes this one, a plain
				// click takes it alone (§21).
				if self.modifiers.shift() {
					self.panes.pane.extend_selection(show_hidden, &path);
				} else if self.modifiers.control() {
					self.panes.pane.toggle_selection(&path);
				} else {
					self.panes.pane.select(&path);
				}
				// A clicked link is resolved the same way a walked-to one is (§20).
				self.resolve_selected_link();
			}
			FilesMessage::EntryOpened(path) => {
				self.panes.pane.close_menu();
				// A directory is entered — browsing the PANE there, the console left where it is
				// (§19). A FILE opens in a new editor tab (§32). The console is moved on purpose, by
				// Sync or "Open in terminal", never as a side effect of either.
				match self.panes.pane.kind_of(&path) {
					Some(files::FilesKind::Dir) => self.browse_to(&path),
					Some(_) => return self.request_open(path),
					None => {}
				}
			}
			FilesMessage::OpenStarted(path) => {
				// The menu's "Edit…" — the deliberate twin of a file double-click (§32).
				self.panes.pane.close_menu();
				return self.request_open(path);
			}
			FilesMessage::OpenInTerminal(path) => {
				// The pane's own "Open in terminal": the deliberate console move that a
				// double-click no longer is (§19). Same landing as the tree's item.
				self.panes.pane.close_menu();
				self.move_shell_to(&path);
			}
			FilesMessage::ParentOpened => {
				self.panes.pane.close_menu();
				// The toolbar disables the button at the root and before the first listing,
				// so this is belt and braces — and the parent is read HERE, from the
				// directory actually on show, rather than carried in the message. Browses the
				// PANE up; the console is left where it is (§19).
				let Some(parent) = self.panes.pane.path().and_then(explorer::parent) else {
					return iced::Task::none();
				};
				let parent = parent.to_owned();
				self.browse_to(&parent);
			}
			FilesMessage::EntryRightClicked(path) => {
				self.focus_pane(Focus::Files);
				// A right-click INSIDE the selection keeps it — that is how a menu comes to
				// act on all of it (§21); one outside collapses onto the entry clicked, so
				// the menu never acts on entries the user has looked away from.
				if !self.panes.pane.is_selected(&path) {
					self.panes.pane.select(&path);
				}
				self.panes.pane.open_menu(path);
				self.resolve_selected_link();
			}
			FilesMessage::PointerMoved(point) => {
				self.panes.pane.set_pointer(point);
				// A move with the button down is a band being stretched (§21).
				if self.panes.pane.drag_band(point) {
					self.apply_band();
				}
			}
			FilesMessage::MenuDismissed => self.panes.pane.close_menu(),
			// The sort menu is a plain view preference: none of these re-list or re-fetch, they
			// only re-order what `rows` already holds, so each just mutates and falls through to
			// the shared `Task::none()` below (§19).
			FilesMessage::SortMenuOpened => self.panes.pane.toggle_sort_menu(),
			FilesMessage::SortMenuDismissed => self.panes.pane.close_sort_menu(),
			// Picking a key or a direction leaves the menu open, so both halves of a sort can be
			// set in one visit; a click-away (or the button) closes it. Each pick persists the sort
			// into the connected target (§22), the same way the `.*` toggle folds into the snapshot,
			// so the chosen order survives a disconnect and even a later hard exit.
			FilesMessage::SortKeyPicked(key) => {
				self.panes.pane.pick_sort_key(key);
				self.persist_session();
			}
			FilesMessage::SortDirPicked(dir) => {
				self.panes.pane.pick_sort_dir(dir);
				self.persist_session();
			}
			FilesMessage::Refresh => {
				self.panes.pane.close_menu();
				if let Some(request) = self.panes.pane.refresh() {
					self.list_files(request);
				}
			}
			FilesMessage::CopyName(path) => {
				self.panes.pane.close_menu();
				let names = self.action_targets(&path);
				let text = join_lines(names.iter().map(|path| explorer::name(path).to_owned()));
				return self.copy_to_clipboard(text);
			}
			FilesMessage::CopyRelative(path) => {
				self.panes.pane.close_menu();
				// The menu disables this item without a cwd, so this is belt and braces.
				let Some(cwd) = self.terminal.as_ref().and_then(term::Terminal::cwd) else {
					return iced::Task::none();
				};
				let cwd = cwd.to_owned();
				let targets = self.action_targets(&path);
				let text = join_lines(targets.iter().map(|path| explorer::relative(&cwd, path)));
				return self.copy_to_clipboard(text);
			}
			FilesMessage::CopyPath(path) => {
				self.panes.pane.close_menu();
				let text = join_lines(self.action_targets(&path));
				return self.copy_to_clipboard(text);
			}
			FilesMessage::CopyCurrentPath => {
				// The header path, not a selection: copy the one directory verbatim, with no
				// `action_targets` detour and no line-joining — there is only ever the one.
				if let Some(path) = self.panes.pane.path() {
					let text = path.to_owned();
					return self.copy_to_clipboard(text);
				}
			}
			FilesMessage::CopyDetails(text) => {
				// Already joined in the view (§20): the popup owns the exact lines shown, so
				// this just writes them and raises the shared confirmation toast.
				return self.copy_to_clipboard(text);
			}
			FilesMessage::RenameStarted(path) => {
				self.panes.pane.start_rename(path);
				return iced::widget::operation::focus(ui::files::RENAME_INPUT_ID);
			}
			FilesMessage::RenameEdited(text) => self.panes.pane.edit_rename(text),
			FilesMessage::RenameCommitted => {
				if let Some((from, to)) = self.panes.pane.commit_rename() {
					self.send_command(SshCommand::RenameDir { from, to });
				}
			}
			FilesMessage::Download(path) => {
				self.panes.pane.close_menu();
				// One transfer at a time — the status bar has one progress bar, and two
				// concurrent transfers would fight over it (§17). A batch respects that by
				// queueing; a batch started while something else runs still has to wait.
				if self.transfers.busy() {
					self.panes.pane.set_notice(transfer::BUSY_NOTICE.to_owned());
					return iced::Task::none();
				}
				// Folders are dropped rather than refused: a band that swept up a directory
				// alongside nine files should still fetch the nine (§21).
				let mut targets = self.action_targets(&path);
				targets.retain(|path| self.panes.pane.kind_of(path) != Some(files::FilesKind::Dir));
				return match targets.len() {
					0 => iced::Task::none(),
					// One file keeps the save dialog, which asks its own overwrite question.
					1 => pick_download_target(targets.remove(0)),
					_ => pick_download_folder(targets),
				};
			}
			FilesMessage::SplitterGrabbed => self.panes.pane.set_dragging(true),
			FilesMessage::SplitterDragged(pointer) => {
				if self.panes.pane.dragging() {
					// The splitter sits at the pane's top edge and the pane runs to the
					// window's bottom edge, so the pointer's distance from that edge IS the
					// height — no drag anchor to track. The tree's twin, on the other axis.
					self.panes.drag_pane_splitter(pointer.y, self.window_size);
					self.refit_grid();
				}
			}
			FilesMessage::SplitterReleased => self.panes.pane.set_dragging(false),
			// Hover only lights the bar (§19); no relayout, so no grid refit.
			FilesMessage::SplitterEntered => self.panes.pane.set_splitter_hovered(true),
			FilesMessage::SplitterExited => self.panes.pane.set_splitter_hovered(false),
		}
		iced::Task::none()
	}

	/// Open the "new folder" dialog for a folder to be created inside `parent` (§18): the tree
	/// folder that was right-clicked, or the directory the files pane is showing. Seeds the body
	/// with what it does and where, then focuses the name field so the user types straight away.
	/// An empty parent (the pane has shown nothing yet) asks nothing.
	pub(super) fn begin_new_folder(&mut self, parent: String) -> iced::Task<Message> {
		if parent.is_empty() {
			return iced::Task::none();
		}
		let body = format!("{}\n\n{parent}", ui::terminal::NEW_FOLDER_DIALOG_BODY);
		self.open_modal(
			Modal::NewFolder {
				parent,
				name: String::new(),
			},
			&body,
		);
		iced::widget::operation::focus(ui::terminal::NEW_FOLDER_INPUT_ID)
	}

	/// Ask the server to create the folder the dialog is holding (§18). A blank name, or one
	/// carrying a path separator (which would put the folder somewhere other than asked), is not
	/// submittable — the dialog stays open rather than closing on nothing, the same rule the
	/// inline rename follows. A good name closes the dialog and sends the request.
	pub(super) fn confirm_new_folder(&mut self) {
		let Some(Modal::NewFolder { parent, name }) = &self.modal else {
			return;
		};
		if !explorer::is_plain_name(name) {
			return;
		}
		let path = explorer::join(parent, name.trim());
		self.modal = None;
		self.send_command(SshCommand::MakeDir(path));
	}

	/// Open the delete confirmation for `paths` (§18): name each target, warn that a folder goes
	/// with everything inside it, and hold the paths until the user confirms. Nothing to delete is
	/// a no-op. Deleting is not undoable, so this only ever raises the question — the removal
	/// happens on an explicit confirm, the same discipline as Disconnect and the home list (§14).
	pub(super) fn begin_delete(&mut self, paths: Vec<String>) {
		if paths.is_empty() {
			return;
		}
		let names = join_lines(paths.iter().map(|path| explorer::name(path).to_owned()));
		let body = format!("{}\n\n{names}", ui::terminal::DELETE_DIALOG_BODY);
		self.open_modal(Modal::Delete(paths), &body);
	}

	/// Delete the held entries (§18) — only reached from a confirmed prompt. The panes re-list
	/// when the server reports it done (`on_deleted`), so nothing is dropped from the view on a
	/// hopeful guess.
	pub(super) fn confirm_remote_delete(&mut self) {
		let paths = match self.modal.take() {
			Some(Modal::Delete(paths)) => paths,
			// Some other dialog, or none: put back what was open and send nothing. Taking the
			// paths is what closes the confirmation, so nothing can be deleted twice.
			other => {
				self.modal = other;
				return;
			}
		};
		self.send_command(SshCommand::Delete(paths));
	}

	/// Re-list a remote directory in whichever pane is showing it (§18): the tree, if it knows
	/// the folder, and the files pane, if that is the directory on show. The refresh a create or a
	/// delete triggers, so a new row appears — or a gone one vanishes — in place.
	pub(super) fn refresh_remote_dir(&mut self, dir: &str) {
		let fetches = self.panes.refresh_dir(dir);
		self.send_fetches(fetches);
	}

	/// Entries were deleted (§18): step the files pane out of any folder that is now gone, drop
	/// the deleted subtrees from the tree, and re-list each parent they vanished from so the rows
	/// update in place. Done here rather than in a model because it spans both panes and the
	/// pane's own idea of where it is.
	pub(super) fn on_deleted(&mut self, paths: &[String]) {
		let fetches = self.panes.deleted(paths);
		self.send_fetches(fetches);
	}

	/// Ask the SSH task for each folder listing the tree still needs (§18). Stops at the
	/// first send failure, which has already surfaced its own error.
	pub(super) fn list_dirs(&mut self, paths: Vec<String>) {
		for path in paths {
			if !self.send_command(SshCommand::ListDir(path)) {
				return;
			}
		}
	}

	/// Ask the SSH task for the directory the files pane wants (§19). One command per
	/// listing; the batches come back tagged with this same request number.
	pub(super) fn list_files(&mut self, request: u64) {
		let Some(path) = self.panes.pane.path().map(str::to_owned) else {
			return;
		};
		self.send_command(SshCommand::ListFiles { path, request });
	}
}

/// The wiring these methods add on top of the models next door (§18, §19, §21): what a pane event
/// turns into on the connection, and what Reveal does to the resume pin. The models' own rules are
/// tested in `crate::explorer`, `crate::files` and `crate::panes`, without a session.
#[cfg(test)]
mod tests {
	use super::super::fixtures::*;
	use super::super::*;

	/// Reveal is an explicit ask, so it ends the resume pin (§19, §22) — the same rule
	/// `move_shell_to` already follows, for the same reason: once the user has said where the
	/// panes go, the pin protecting the restored view has nothing left to protect.
	///
	/// Without that, pressing Reveal in the window between the login prompt and the replayed `cd`
	/// landing left the panes stranded. They went to the login directory, the still-armed pin
	/// swallowed the settle as "already there", and the shell then sat at a directory the panes
	/// had been explicitly asked to come to and had not — with no further announcement coming to
	/// put it right, since a shell standing still announces no move.
	#[test]
	fn reveal_during_a_resume_ends_the_pin_rather_than_stranding_the_panes() {
		use crate::ui::connect::AuthKind;

		let (tx, _rx) = mpsc::channel(64);
		let mut app = Tab {
			command_tx: Some(tx),
			..Tab::default()
		};

		app.targets
			.borrow_mut()
			.upsert_on_connect("h", 22, "u", AuthKind::Password, None, None);
		app.targets.borrow_mut().set_session(
			"u@h:22",
			crate::targets::SessionState {
				terminal_path: Some("/var/log".to_owned()),
				files_path: Some("/etc".to_owned()),
				..crate::targets::SessionState::default()
			},
		);
		app.connection = Some("u@h:22".to_owned());
		app.pending_target = Some(app.targets.borrow().find("u@h:22").unwrap().clone());

		let announce = |dir: &str| shell_output(format!("\x1b]7;file://host{dir}\x07").as_bytes());

		let _ = app.on_ssh_event(SshEvent::Connected);
		let _ = app.on_ssh_event(announce("/home/u"));
		assert_eq!(
			app.resume_cwd.as_deref(),
			Some("/var/log"),
			"still settling"
		);

		// The user asks for the panes to come to the shell, mid-resume.
		let _task = app.update(Message::RevealPressed);
		assert_eq!(app.panes.pane.path(), Some("/home/u"), "the panes came");
		assert_eq!(app.resume_cwd, None, "and the pin is spent");

		// The replayed `cd` lands. It is a real move now, so both panes follow it — where
		// before, the leftover pin read it as "already there" and left them behind.
		let _ = app.on_ssh_event(announce("/var/log"));
		assert_eq!(app.panes.pane.path(), Some("/var/log"), "the pane kept up");
		assert_eq!(
			app.panes.tree.selected(),
			Some("/var/log"),
			"and the tree with it"
		);
	}

	/// The status bar's Reveal button (§19): the panes come to the shell, and nothing is typed at
	/// it. The case that matters is the one the shell cannot fix by itself — a browse away from a
	/// shell that has not moved since. Its next prompt announces the same directory, which is not a
	/// move, so the pane rightly stays put and only an explicit ask brings it back.
	#[test]
	fn reveal_brings_the_panes_to_the_shell_without_typing_anything() {
		let (mut app, mut rx) = app_with_terminal(32);
		let announce = |dir: &str| shell_output(format!("\x1b]7;file://host{dir}\x07").as_bytes());

		// The shell says where it is, and both panes follow it there as usual.
		let _ = app.on_ssh_event(announce("/var/log"));
		assert_eq!(app.panes.pane.path(), Some("/var/log"));
		assert_eq!(app.panes.tree.selected(), Some("/var/log"));

		// A look somewhere else, with the tree walked off the shell's folder too.
		app.browse_to("/etc");
		app.panes.tree.select("/etc");
		let _ = app.on_ssh_event(announce("/var/log"));
		assert_eq!(
			app.panes.pane.path(),
			Some("/etc"),
			"a re-announcement is not a move, so the browse stands (§19)"
		);

		let _ = drain(&mut rx);
		let _task = app.update(Message::RevealPressed);
		assert_eq!(
			app.panes.pane.path(),
			Some("/var/log"),
			"the pane came back"
		);
		assert_eq!(
			app.panes.tree.selected(),
			Some("/var/log"),
			"and the tree with it"
		);
		assert!(
			!drain(&mut rx)
				.iter()
				.any(|command| matches!(command, SshCommand::Input(_))),
			"the shell was never typed at — this moves the local view alone"
		);
	}

	/// With no cwd announcement (§17: it takes OSC 7, which not every shell sends) Reveal has
	/// nowhere to go, so it leaves both panes where they are rather than guessing at the root.
	/// The button dims in that case; this is what sits behind the dimming.
	#[test]
	fn reveal_does_nothing_when_the_shell_never_said_where_it_is() {
		let (mut app, mut rx) = app_with_terminal(32);
		app.browse_to("/etc");
		let _ = drain(&mut rx);

		let _task = app.update(Message::RevealPressed);
		assert_eq!(app.panes.pane.path(), Some("/etc"), "left where it was");
		assert!(drain(&mut rx).is_empty(), "and nothing asked of the server");
	}

	/// Shift+click and Shift+arrow through the app's own handlers (§21) — the model's rules
	/// are tested next door in `files`, but only this path proves the wiring: the modifier
	/// state comes off the keyboard subscription, and a mouse press carries none of its own.
	#[test]
	fn shift_click_and_shift_arrow_reach_the_selection() {
		use iced::keyboard::{Event, Modifiers};

		let mut app = Tab::default();
		let request = app
			.panes
			.pane
			.show("/home")
			.expect("a new directory needs listing");
		app.panes.pane.chunk(
			request,
			["a", "b", "c", "d"]
				.into_iter()
				.map(|name| files::Entry {
					name: name.to_owned(),
					kind: files::FilesKind::File,
					meta: files::Meta::default(),
				})
				.collect(),
			true,
		);
		let chosen = |app: &Tab| {
			app.panes
				.pane
				.selected_rows(app.panes.show_hidden())
				.into_iter()
				.map(|(path, _)| path)
				.collect::<Vec<_>>()
		};

		let _ = app.on_files(FilesMessage::EntryClicked("/home/a".to_owned()));
		assert_eq!(chosen(&app), ["/home/a"]);

		// Shift goes down, then the second click lands: everything between comes with it.
		let _ = app.on_key(Event::ModifiersChanged(Modifiers::SHIFT));
		let _ = app.on_files(FilesMessage::EntryClicked("/home/c".to_owned()));
		assert_eq!(chosen(&app), ["/home/a", "/home/b", "/home/c"]);

		// Still held: the arrow key extends rather than moving.
		let _ = app.on_key(Event::KeyPressed {
			key: iced::keyboard::Key::Named(iced::keyboard::key::Named::ArrowRight),
			modified_key: iced::keyboard::Key::Named(iced::keyboard::key::Named::ArrowRight),
			physical_key: iced::keyboard::key::Physical::Code(
				iced::keyboard::key::Code::ArrowRight,
			),
			location: iced::keyboard::Location::Standard,
			modifiers: Modifiers::SHIFT,
			text: None,
			repeat: false,
		});
		assert_eq!(chosen(&app), ["/home/a", "/home/b", "/home/c", "/home/d"]);

		// Shift released, plain click: back to one.
		let _ = app.on_key(Event::ModifiersChanged(Modifiers::empty()));
		let _ = app.on_files(FilesMessage::EntryClicked("/home/b".to_owned()));
		assert_eq!(chosen(&app), ["/home/b"]);
	}
}
