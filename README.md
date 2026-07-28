# cmote

[![CI](https://github.com/clemeno/cmote/actions/workflows/ci.yml/badge.svg)](https://github.com/clemeno/cmote/actions/workflows/ci.yml)

A **native, portable SSH client for Windows 11 and macOS** written in Rust. A home
screen lists your saved connection targets; pick one (or start a new connection), fill
in host / port / user, pick an auth method (password or a private key — PEM or PuTTY
`.ppk`), connect. On success the server hands us a shell and cmote renders a **full VT
terminal** inside the window — a working interactive prompt, with a browsable tree of the
remote filesystem beside it, a grid of the current directory's files under it (keyboard
navigable, with a details popup and rubber-band multi-selection), the remote working
directory in the title bar, and file transfer both ways. Full-screen programs — btop,
vim, htop, midnight commander — draw properly and take the mouse. Reconnect to a saved
target and the shell and both panels come back to the directories you left them in.

This is a **learning project**. The code is meant to be read as much as run, so it is
written didactically: it favours idiomatic Rust, explains *why* each choice was made,
and marks every deliberate shortcut with a `ponytail:` note so "simple" reads as
intent, not oversight. The full design rationale lives in [PLAN.md](PLAN.md); section
references below (§n) point into it.

## Features

- **Home screen of saved targets** — every successful connection is remembered as a
  named target and listed alphabetically. Profiles only: **no passwords or passphrases
  are ever written to disk** (only host / port / user / auth method / key path). Click a
  target to select it, then click it again (or press **Enter**) to open it and pre-fill
  the form; **rename** it in place with **F2** or right-click → **Rename** (the list
  re-sorts); right-click also offers **Open** and **Delete** (deleting asks to confirm —
  cancelling keeps the target); **New connection** opens a blank form.
- Connection form: host, port, user, and an auth method.
- **Password** auth, or **private-key** auth with a native file picker (`rfd`).
- Key formats: OpenSSH / PEM (via `russh::keys`) and PuTTY **`.ppk`** (via
  `ssh-key`'s `from_ppk`). Encrypted keys prompt for a passphrase on their own screen —
  or pre-fill an optional passphrase field on the form (leave it empty to be prompted).
- **Trust-on-first-use** host-key verification against a portable `known_hosts`:
  first contact shows the fingerprint for explicit accept/reject; a later key change
  is a hard stop, not a warning (§8).
- A full **VT terminal** — a complete VT engine (`alacritty_terminal`) whose grid cmote
  draws with iced — that reflows to the window size, forwarding the new pty size to the
  remote (§9, §23).
- **Full-screen programs draw properly** — btop, htop, vim, midnight commander. The screen
  is one widget that puts every glyph at the exact pixel its column starts at, so nothing a
  program prints can shift the line it is on; **braille** graphs and **rounded box corners**
  — glyphs no monospace font we could bundle actually carries — are drawn from their own
  geometry rather than borrowed from whatever font the system offers. The engine interprets
  the full escape-sequence set — the DEC line-drawing characters older programs box-draw
  with, custom tab stops, origin mode — so a program's screen lands where it belongs instead
  of coming out as wrapped, scrolling gibberish. It also **answers the queries a program
  blocks on or adapts to** — "where is the cursor?" (`CSI 6n`), "what terminal are you?"
  (`CSI c`), "what is your background colour?" (`OSC 11`, which lets an editor pick a light or
  dark colourscheme to match), "how big is your screen?" — that otherwise stall vim, tmux and
  less on a startup timeout or leave them guessing; cmote answers each with what it actually
  shows (§9, §23). **F1-F12** are mapped as the pty's terminfo entry describes them (§9).
- **Text styling comes through** — colour (256-colour and truecolor), bold, faint, reverse
  video, concealed text, strikethrough, and every underline style a program reaches for:
  single, double, dotted, dashed and the curly one an editor draws under a spelling mistake,
  each in its own colour when the program sets one, plus **italic** — drawn from a bundled
  IBM Plex Mono face, since Fira Mono ships no italic of its own (§23).
- **Mouse text selection** (drag to select, highlighted in place) with **Copy** and
  **Paste** — from the status-bar buttons or a right-click menu. Paste is
  **bracketed-paste** aware and strips the paste-injection terminator (§9-§10).
- **The mouse reaches the program that asked for it** — click a process in btop, a tab in
  tmux, a line in vim; the wheel scrolls what is under it. cmote forwards clicks, releases,
  drags and scrolls in the xterm protocols a program enables, and **holding Shift takes the
  pointer back** for text selection and cmote's own right-click menu (§9).
- **Remote folder tree** — a 2D explorer of the remote filesystem to the right of the
  terminal, over **SFTP** (falling back to `ls` on a server with the subsystem disabled).
  Click a folder to expand or collapse it; the tree **follows the shell**, opening the
  whole chain from `/` down to wherever you `cd`. Right-click a folder for **Open in
  terminal** (types a quoted `cd`), **Upload…** (sends local files into that folder),
  **Rename…** (inline, like F2 on the home list), **Copy name / relative path / full path**,
  **Expand** (which also refreshes) and **Collapse**. Its header names the folder on show —
  middle-ellipsised and capped at two lines, with a **copy button** beside it. Drag the
  splitter to resize the panel — the terminal reflows to match — or hide it with the status
  bar's **Folders** button; the `.*` checkbox in its header hides dot-folders (§18, §22).
- **Remote files pane** — a grid of **every entry** in one directory, full width under the
  terminal. Each cell is a wide row: a small icon in front of the name, with the **size and
  the modified date** on a second, muted line underneath (`2026-03-20 11:46 CEST`; a folder
  shows only the date, and anything the listing never learned reads as a dash). A name too
  long for its cell is **middle-ellipsised**, so the start *and* the extension survive. A big
  directory streams in batches of 1000 and the header counts as they land. Icons come from a
  bundled icon font, by category (folder, image, code, archive, document, audio, video, link,
  plain). Right-click an entry for **Open in terminal**, **Download…**, **Rename…**, **Copy
  name / relative path / full path** and **Refresh**; right-click **empty space** for
  **Upload… here** and **Refresh**. The header carries an **up** button, the directory's path
  (middle-ellipsised to one line) and a **copy button** for it. Drag the splitter to resize
  the pane, or hide it with the status bar's **Files** button; the same `.*` checkbox hides
  dot-files here too (§19).
- **Browsing never moves the console** — a click in the folder tree, a **double-click** on a
  folder in the grid, the pane's **up** button and **Enter** all point the *pane* somewhere
  else and leave the shell where it is, so you can look inside a directory without disturbing
  what is running. The shell moves only on a `cd` it can see: one you type, either panel's
  **Open in terminal**, or the status bar's **Sync**, which brings the shell (and with it the
  tree and the title) to the folder the pane is showing. Sync is disabled when the two already
  agree (§19).
- **Keyboard focus across the three regions** — the shell, the folder tree and the files
  pane each take the keyboard. A session starts at the shell; a click focuses whatever was
  clicked, **Ctrl+Tab** cycles forward and **Ctrl+Shift+Tab** back (hidden panels are
  skipped), and the focused panel wears a ring so it is never a guess. In a panel the
  **arrow keys** walk the rows (in the grid, left/right move one cell and up/down a whole
  row), **Tab / Shift+Tab** step next/previous, **Enter** opens, **F2** renames and **Esc**
  hands the keyboard back to the shell. A keyboard-moved selection scrolls itself into view,
  only at the edges (§20).
- **A details popup beside the selection** — the entry's **full name** (the grid's label is
  narrow and may clip it), where a **symlink points**, the file's **MIME type**, its
  **modification time in the server's own timezone** (`2026-03-20 11:46:40 CEST (+02:00)` —
  the zone is read off the server once per session), its **size** (human, with the exact
  byte count behind it) and its **`owner:group`** as names, not numbers. Anything the server
  would not say reads as a dash, and a button on the card copies **the whole thing** at once
  (§20, §22).
- **Selecting many entries at once** — drag a **rubber band** over the grid's empty space,
  **Ctrl+click** to add or remove one, **Shift+click** or **Shift+arrow** to take the run
  between two ends, **Ctrl+A** to take the lot; **Ctrl+drag** adds a band to what is already
  selected. The popup then summarises the set (how many, folders versus files, total size).
  A right-click *inside* the selection acts on all of it — the copy items join their results
  one per line and say how many they will take — while a right-click outside collapses onto
  that one entry first (§21).
- **File download** — right-click a file in the pane → **Download…**, pick where to save
  it in the native dialog, and it comes down over **SFTP** on its own channel with a
  progress bar in the status bar. Downloading a **multiple selection** asks for one
  destination folder instead of a dialog per file, queues the transfers (one at a time, one
  progress bar) and leaves any folders in the selection behind. If some of those names are
  already in the folder, one dialog asks about the whole batch before anything is written:
  **Skip them**, **Save alongside** (`notes-1.txt`), **Replace**, or **Cancel** (§19, §21).
- **File upload, one or many, into a folder** — pick local files with **Files…** (the
  picker is multi-select) and send them with **Upload**, over **SFTP** on its own channel so
  the shell keeps running. The confirmation lists what you picked under an **editable
  destination folder** — each file keeps its own name inside it, and an empty folder means
  the login directory. Upload starts from four places, each seeding that folder: the status
  bar (the shell's directory), the terminal's right-click **Upload…** (the shell's
  directory), the files pane's empty-space **Upload… here** (the pane's directory), and a
  folder's **Upload…** in the tree. Before a byte is sent, every destination name is checked
  on the server; if some are already there, **one dialog asks about the whole batch** —
  **Replace**, **Skip**, **Keep both** (`name-1.txt`) or **Cancel**. The files then go one at
  a time behind the status bar's progress bar, closing with `Uploaded N files`, and a failure
  names its reason and moves on to the next (§17).
- **The remote working directory in the window title** — cmote reads the `OSC 7` /
  `OSC 9;9` sequences shells emit on each prompt, so the title follows `cd` on POSIX *and*
  Windows remotes. bash and zsh are hooked up automatically when the shell opens; fish and
  Windows Terminal-style prompts already announce it themselves (§17). When a program sets its
  own title (`OSC 0` / `OSC 2` — `vim` naming the file it is editing, say), that shows in the
  title bar instead; the host is always kept alongside so the window stays identifiable (§23).
- **Consistent dialogs** — the delete-target, disconnect, upload and overwrite
  confirmations, the host-key prompt, the passphrase prompt, and the error notice share
  one chrome: a header bar (question on the left, close ✕
  on the right, wired to the safe action), an explanatory body, and evenly-spaced footer
  buttons. Each **floats over the page it belongs to** (the connect-flow dialogs over the
  connect form, the disconnect modal over the shell) behind a dim backdrop; clicking the
  card never dismisses it (only a click outside does); the body message is **selectable and
  copyable** — drag to select, `Ctrl+C` to copy (handy for the host-key fingerprint or an
  error message); and the dialog is **draggable** by its header, clamped to the window (§10).
- **Every copy says so** — any **Copy** (a menu item, a header's copy button, the details
  card's) raises a short toast at the bottom of the window that fades itself after three
  seconds, so a copy is never a silent no-op you have to test by pasting (§10).
- **Resuming where you left off** — a saved target remembers, per target, the **shell's
  directory**, the **files pane's directory**, the `.*` toggle and both **panel sizes**. On
  the next connection the pane reopens there, the tree reveals the chain down to it, and the
  shell is put back with a visible `cd`. The snapshot is written at every teardown — a clean
  Disconnect, a remote hangup, an error — and a value this session never learned never erases
  the one already saved. Profile metadata only: still **no secrets on disk** (§22).
- Session-only **secrets** — passwords and key passphrases are held in memory and
  `zeroize`d on drop, never written to disk (§12). Only non-secret connection *profiles*
  are persisted, for the home list (§14).

## Gestures and shortcuts

Everything the mouse and the keyboard do, by region. The **focused** region is the one that
gets a keystroke; a click focuses what it lands on, and the ring shows where the keyboard is.

**Anywhere in the window**

| Gesture | What it does |
|---|---|
| **Ctrl+Tab** / **Ctrl+Shift+Tab** | Move the keyboard to the next / previous region — shell, folder tree, files pane (hidden panels are skipped) |
| Click a region | Focus it |
| Drag a dialog's header | Move the dialog; **Esc** or ✕ takes the dialog's safe way out |
| Drag inside a dialog's body | Select its text; **Ctrl+C** copies it |

**Terminal (the shell)**

| Gesture | What it does |
|---|---|
| Drag across the grid | Select text (highlighted in place) |
| Right-click | Context menu: Copy / Paste / Upload… (into the shell's directory) |
| **Ctrl+C** / **Ctrl+V** via the buttons or menu | Copy the selection, paste (bracketed-paste aware) |
| Click / drag / scroll **in a program that asked for the mouse** | Goes to that program (btop, vim, tmux, mc) instead of selecting |
| **Shift** + click or drag | Takes the pointer back: select text, or right-click for cmote's own menu |
| Any other key | Goes to the remote shell — arrows (SS3 form in application-cursor mode) and **F1-F12** included |
| Drag either splitter | Resize the folder tree or the files pane; the pty is reflowed to match |
| **Sync** in the status bar | `cd` the shell to the folder the pane is showing (disabled when they already agree) |
| **Files…** / **Upload** in the status bar | Pick local files, then send them into the shell's directory |

**Folder tree** (right of the terminal — the status bar's **Folders** button hides it)

| Gesture | What it does |
|---|---|
| Click a folder | Expand or collapse it, and select it |
| Right-click a folder | Open in terminal / Upload… / Rename… / Copy name / Copy relative path / Copy full path / Expand / Collapse |
| **↑** / **↓**, **Tab** / **Shift+Tab** | Walk the visible rows |
| **→** / **←** | Open / close the selected folder |
| **Enter** | `cd` the shell into it |
| **F2** | Rename in place (**Enter** commits, **Esc** abandons) |
| **Esc** | Give the keyboard back to the shell |
| Copy button in the header | Copy the path of the folder on show |
| `.*` checkbox | Hide or show dot-entries (shared with the files pane) |

**Files pane** (under everything — the status bar's **Files** button hides it)

| Gesture | What it does |
|---|---|
| Click an entry | Select it (and show its details popup) |
| Double-click a folder | Show it in the pane; the shell stays where it is |
| Click empty space | Clear the selection |
| **Drag** from empty space | Rubber-band selection; **Ctrl+drag** adds to what is selected |
| **Ctrl+click** | Add or remove one entry |
| **Shift+click** | Select everything between the anchor and here |
| **Ctrl+A** | Select every entry on show |
| Right-click an entry | The entry's menu — on a multiple selection it acts on all of it |
| Right-click empty space | Upload… here / Refresh |
| Copy button in the details popup | Copy the whole details card |
| **←** / **→** | Move one cell; **↑** / **↓** move a whole row |
| **Shift** + those arrows | Extend the selection instead of moving it |
| **Tab** / **Shift+Tab** | Next / previous entry |
| **Enter** | Show the selected folder in the pane |
| **F2** | Rename in place |
| **Esc** | Give the keyboard back to the shell |
| ↑ button in the header | Show the parent directory |
| Copy button in the header | Copy the path of the directory on show |

**Home screen**

| Gesture | What it does |
|---|---|
| Click a target | Select it; click again (or **Enter**) to open it |
| Right-click a target | Open / Rename / Delete (deleting asks first) |
| **F2** | Rename the selected target (**Enter** commits, **Esc** abandons) |
| **Delete** | Delete the selected target, after the confirmation (**Esc** cancels it) |
| **Tab** / **Shift+Tab** on the connect form | Move focus across the fields, the auth radios and Connect; **Enter** / **Space** activates the focused radio or button |

## Requirements

- **Rust** stable (developed against 1.91.0 on Windows, 1.97.1 on macOS).
- **Windows 11** — target `x86_64-pc-windows-msvc` and the **MSVC** toolchain (Visual
  Studio Build Tools with the VC++ x64 tools and the Windows SDK — the default MSVC
  linker). No NASM or C compiler: the `ring` crypto backend ships pre-generated
  assembly for this target (§2).
- **macOS Sequoia (Intel)** — target `x86_64-apple-darwin` and the **Xcode Command
  Line Tools** (`clang`), which compile `ring`'s crypto from source. No NASM (§2).
- No external SSH library on either target — the SSH stack is pure Rust (§12).

## Build and run

```sh
# Debug build and run
cargo run

# Optimized, self-contained portable binary
cargo build --release
# Windows → target/release/cmote.exe
# macOS   → target/release/cmote
```

On **Windows** the release `cmote.exe` is portable: copy it anywhere (including a USB
stick) and run it — no installer, no registry writes, no external runtime.

On **macOS** wrap the binary in a minimal app bundle so Finder launches it as a GUI
app (double-clicking a bare Unix binary would open a Terminal window instead):

```sh
cargo build --release
./bundle-macos.sh        # → target/release/cmote.app
open target/release/cmote.app
```

`cmote.app` is self-contained and relocatable — no installer or external runtime. It
is not code-signed or notarized yet (deferred — §12), so the first launch needs a
right-click → **Open** to clear Gatekeeper's "unidentified developer" prompt.

## Data and portability

cmote writes two files — `known_hosts` (pinned host keys) and `targets.json` (saved
connection profiles plus where each session left off: the two directories, the `.*` toggle
and the panel sizes — **no secrets**). Both live in the same directory, resolved at
runtime (§11, `paths::data_dir`):

1. **Portable mode (preferred):** `cmote-data/` beside the binary, when that directory
   is writable. This keeps the data travelling with the app — on macOS the binary lives
   in `cmote.app/Contents/MacOS/`, so the store sits inside the bundle.
2. **Fallback (Windows):** `%LOCALAPPDATA%\cmote\` when the exe sits in a read-only
   location (e.g. `Program Files`); on macOS `~/Library/Application Support/cmote/`.

To reset trust for a host, delete the offending line (or the whole file) from
`known_hosts`. To drop a saved target, use right-click → Delete in the app and confirm
the prompt (or delete its entry from `targets.json`).

## Testing

Pure logic is unit-tested; anything needing a live server is manual (§13). No test
framework is pulled in — everything uses Rust's built-in `#[test]` / `#[cfg(test)]`.

```sh
cargo test          # run the unit tests
cargo fmt           # format (rustfmt, hard tabs — see rustfmt.toml)
cargo clippy --all-targets -- -D warnings
```

**CI** (`.github/workflows/ci.yml`) runs these same gates on every push and pull
request to `main`, on **both** targets — `cargo fmt --check` plus `cargo clippy -D
warnings` and `cargo test` on Windows (`x86_64-pc-windows-msvc`) and macOS
(clippy against the Intel target `x86_64-apple-darwin`, tests native on the runner).
It also audits the dependency tree: `cargo audit` for RustSec advisories and
`cargo deny` (see `deny.toml`) for the license allow-list, banned crates (no
`aws-lc-*` — keeps the NASM-free portable build, §12), and trusted sources.

Automated coverage: key parsing (encrypted/unencrypted OpenSSH, RSA and Ed25519
`.ppk`, unsupported-key error path), host-key match/unknown/mismatch decisions and
fingerprint formatting, terminal byte-stream → grid, key-event → byte-sequence
mapping (including application-cursor-mode arrow keys, CSI vs SS3, and every F1-F12
against the terminfo entry), the terminal engine's wiring end to end (an `f`-spelling move
lands in its own cell, a wide glyph reserves two columns, and the engine's query replies are
drained and sent back — device status, device attributes, a live cursor-position report, the
save/jump/report/restore size-probe reporting the clamped corner, and a query split across
two chunks answered on completion), pointer-event → mouse-report encoding (each encoding, each
mode's gating, the classic form's 223-column ceiling, the wheel, the modifier bits), the
grid's run packing and the geometry of the glyphs it draws itself (a braille
cell read back as its dot pattern, a rounded corner's arc and tails measured against a real
cell), the grid-resize
math, mouse-selection geometry and text extraction (wide
glyphs, trailing-blank trimming, multi-row joins), paste encoding (bracketed-paste
wrapping and the injection-terminator scrub), the remote-cwd scanner (OSC 7 and
OSC 9;9, split across chunks, percent-escapes, Windows paths, oversized payloads), and
the folder tree's model (row flattening and indentation, the hidden-folder filter,
subtree collapse, `cd` reveal and its no-op on a repeat, rename validation and the
post-rename refresh, relative-path arithmetic, shell quoting, and the panel's width
clamps), and the files pane's model (batch accumulation and the dropping of batches for a
directory already left, the cwd-follow rule that a repeated announcement is not a move,
the folders-first sort, icon categories from kind and extension, rename validation, and
the pane's height clamps). The keyboard and selection work adds: the arrow walk across both
panels (clamping at both ends, skipping hidden entries, and not panicking on an empty
directory), the keep-it-visible scroll rule (including an item taller than its viewport),
MIME types from extensions and their `application/octet-stream` fallback, mtime rendering in
a server timezone (the epoch, a leap day and both sides of Greenwich), the `date +'%z %Z'`
and `ls -l` `longname` parsers with their half-answer fallbacks, the link target belonging
to the selection that asked for it, the selection gestures (range from an anchor, toggle,
plain and additive band), the rubber band's hit-testing against the grid geometry (scrolled,
past the end of the listing, and in the gap between two rows), and — through the app's own
handlers rather than the model's — Shift+click and Shift+arrow, which is what proves the
modifier state reaches a mouse press. The upload, path-eliding and resume work adds: the
upload batch planner (every file queued under its own name when nothing clashes, and each
collision answer — Replace, Skip, Keep both — deciding what happens to each clashing file,
all without an App or a server), the middle-ellipsis cut (a short string left alone, a long
one keeping both ends inside its budget, and the cut never landing inside a glyph) and the
grid cell's two-line version of it, the short mtime that drops the seconds but keeps the
zone, the session snapshot's round trip through `targets.json` (including a pre-v2.2 file
with no session fields at all), and — again through the app's own handlers — a reconnect
that resumes both paths and pins the pane until the shell has caught up.

### Manual smoke test (live SSH)

There is no CI SSH server in v1, so the end-to-end path is verified by hand against a
local `sshd`. Any reachable server works; the steps below use Docker for a disposable
one.

**1. Start a throwaway server** (creates user `tester` / password `testpass` on port
`2222`):

```sh
docker run --rm -d --name cmote-sshd -p 2222:22 \
  -e USER_NAME=tester -e USER_PASSWORD=testpass -e PASSWORD_ACCESS=true \
  linuxserver/openssh-server
```

(Or use WSL / any host you control. On a native Windows OpenSSH server, connect to
`localhost:22`.)

**2. Password auth + first-contact host key.** Run `cargo run`, enter `localhost`,
port `2222`, user `tester`, choose **Password**, type `testpass`, connect. **Tab** /
**Shift+Tab** should move focus across every control — the fields, both auth radios, and
the Connect button (the active radio/button shows a highlight ring); **Enter/Space**
activates the focused radio or button. Expect:

- The **Unknown host key** dialog appears once, showing a SHA-256 fingerprint. You can
  drag it by its header, select the fingerprint and copy it (`Ctrl+C`), and closing (✕)
  rejects. Accept → the shell opens; the fingerprint is now pinned in `known_hosts`.
- Reconnecting no longer prompts (the key matches the pinned one).

**3. Terminal behaviour.** In the shell: run `ls`, `echo hi`, an interactive program
(`top`, then `q`), and **Ctrl-C** to interrupt. Print bold text
(`printf '\033[1mBOLD\033[0m normal\n'`) and confirm the bold run is visibly heavier
than the normal one (both weights are bundled — §9). Print the other styles
(`printf '\033[2mfaint\033[0m \033[3mitalic\033[0m \033[9mstruck\033[0m \033[4munder\033[0m \033[4:3mcurly\033[0m\n'`)
and confirm faint reads dimmer, italic slants (in IBM Plex Mono, §23), struck has a line
through it, and the two underlines differ — one straight, one wavy (§23). Ask the terminal its
background colour (`printf '\033]11;?\033\\'`): it replies on the input channel, so at a bash
prompt the answer `rgb:1e1e/1e1e/1e1e` appears as if typed — proof it reports what it draws (§23). Print wide glyphs over aligned
columns (e.g. `printf '12\n世b\n'`) and confirm the character after a CJK/emoji glyph
stays in its column — a wide glyph reserves two cells (§9). Resize the window and run
`tput cols; tput lines` (or `stty size`) — the reported size should track the window.
With **NumLock on**, type a command using the **numpad** digits (e.g. `echo 2` /
`pm2 ls`) and confirm the digits appear; with **NumLock off**, the numpad arrows
(2/4/6/8) should move the cursor instead of typing digits (§9). Click **Disconnect**
→ you return to the form immediately.

**4. Key auth.** Generate a test key and authorize it:

```sh
ssh-keygen -t ed25519 -f ./smoke_key -N ""                 # unencrypted
ssh-keygen -t ed25519 -f ./smoke_key_enc -N "hunter2"      # encrypted
# copy the .pub of each into the server's ~tester/.ssh/authorized_keys
```

- **Unencrypted key:** choose **Key**, browse to `smoke_key`, connect → shell opens
  with no passphrase prompt.
- **Encrypted key:** browse to `smoke_key_enc`, connect → the **Encrypted key**
  screen appears with the field already focused; type `hunter2` → shell opens. Enter
  a **wrong** passphrase first to confirm the prompt simply re-appears (bounded
  re-ask) before the correct one succeeds.
- **PuTTY `.ppk`:** convert a key with PuTTYgen and repeat — both encrypted and
  unencrypted `.ppk` should behave like the OpenSSH cases.

**5. Host-key mismatch (hard stop).** Delete the server container and start a fresh
one (new host key) on the same port, then reconnect. Expect a hard failure that names
the changed key and does **not** offer to continue — remove the stale `known_hosts`
line to proceed intentionally.

**6. Selection, copy, and paste.** In the shell, run `echo hello world`, then drag
across the output to select it — the selection should highlight and **Copy** (status
bar or right-click menu) should enable. Copy, then **Paste**: the text lands at the
shell's cursor. Paste into a bracketed-paste-aware shell (bash/zsh with readline) and
confirm a multi-line clipboard does **not** auto-run each line (bracketed paste frames
it). Right-click anywhere to confirm the context menu opens at the cursor and dismisses
on a click away. Copy is disabled with nothing selected; pasting keeps the highlight.

**7. Remote directory + upload.** On connect, one setup line is echoed into the shell
(the cwd hook, §17) and the window title should read
`cmote — tester@localhost:2222 — /config` (or wherever the shell starts). `cd /tmp` and
the title should follow within a prompt. Set a title from a program
(`printf '\033]2;my title\033\\'`) and the bar should switch to `cmote — tester@localhost:2222
— my title`; clearing it (`printf '\033]2;\033\\'`) brings the directory back (§23). Then:

- Click **Files…**, pick a local file — its name appears next to the buttons and **Upload**
  becomes enabled. Click **Upload**: the dialog lists the file under an editable destination
  folder of `/tmp`. Confirm → a progress bar with the byte count runs in the status bar, then
  `Uploaded to /tmp/<name>`, and the pick is cleared (Upload disabled again). `ls -l /tmp` on
  the remote should show it, byte-for-byte identical (`sha256sum` both ends for a binary
  file).
- Pick **several files at once** and upload them → they go one at a time, each with its own
  progress bar, and the closing notice reads `Uploaded N files`.
- Upload the **same batch again** → the collision dialog names the files that are already
  there: **Cancel** sends nothing (check the remote `mtime`s), **Skip** leaves them alone,
  **Keep both** writes `name-1.ext` beside them, **Replace** overwrites.
- Try the other three ways in — right-click the **terminal** → **Upload…** (destination is
  the shell's directory), right-click the files pane's **empty space** → **Upload… here**
  (the pane's directory, so point the pane elsewhere with the tree first and confirm the
  destination follows the *pane*, not the shell), and right-click a **folder in the tree** →
  **Upload…** (that folder).
- Edit the destination in the dialog to a directory you cannot write (`/etc/x`) → the
  status bar shows the failure and the shell stays open.
- Start a shell that does **not** announce its directory (`docker exec … sh`, or unset the
  hook with `unset PROMPT_COMMAND; unset -f cmote_cwd`) → the title drops the directory
  and the upload dialog offers an empty folder, which lands in the login directory.

**8. Remote folder tree.** The panel on the right should list `/` on connect. Then:

- Click folders to expand and collapse them; a slow directory shows `·` until its listing
  arrives. Expand a few levels, collapse the top one, re-open it — it should show exactly
  one level again, instantly (nothing is re-fetched).
- `cd /etc/ssh` in the shell → the tree opens `/` → `/etc` → `/etc/ssh` on its own and
  highlights it. `cd` back and forth: the tree only ever expands, never closes what you
  opened.
- Toggle the `.*` checkbox in the panel header → dot-folders (`.ssh`, `.config`) disappear and
  reappear with no round trip.
- Right-click a folder: **Open in terminal** should run a quoted `cd` in the shell (make a
  folder with a space and a quote in its name — `mkdir "/tmp/it's here"` — and confirm the
  `cd` still lands in it). **Copy full path** / **Copy relative path** / **Copy name**
  should put the right text on the clipboard (paste into the shell to check; the relative
  item is greyed out on a shell that never announces its directory).
- **Rename…** turns the row into a field: Esc abandons, Enter commits and the row
  reappears sorted under its new name. Rename onto an existing name → the notice line
  under the tree says it already exists and nothing changed. Rename a folder you cannot
  write → the notice shows the refusal and the shell stays open.
- Drag the splitter left and right: the grid reflows (`tput cols` should follow) and the
  panel stops at its minimum and at 60% of the window. The **Folders** button hides the
  panel and gives its columns back to the grid.
- Against a server with the sftp subsystem disabled (`Subsystem sftp` commented out in
  `sshd_config`), the tree should still list folders — the `ls` fallback (§18).

**9. Remote files pane.** The grid across the bottom should fill with `/` on connect,
then follow the shell. Then:

- `cd /etc` → the grid shows every entry in `/etc` with an icon per type; the header names
  the directory and counts the entries. Create a directory with thousands of files
  (`mkdir /tmp/many && cd /tmp/many && seq 1 5000 | xargs touch`) and re-enter it: the
  count should climb in steps of 1000 as the batches land, and the window stays responsive
  throughout.
- Each cell should read as a row: icon, name, and under it the size and the modified date
  in the server's zone (a folder shows only the date). Compare a few against `ls -l` on the
  remote. A very long name should be cut in the middle (`report-fin…-draft.pdf`), never at
  the extension.
- Double-click a folder in the grid → the grid enters it and **the shell stays put** (the
  prompt's directory does not change, and the pane must NOT snap back on the next prompt).
  Same for the header's **up** button and **Enter**.
- Click a folder in the **tree** → the grid shows that folder while the shell stays where
  it is, and it must NOT snap back on the next prompt. Click **Sync** in the status bar →
  now the shell `cd`s there, the tree reveals it and the title follows; Sync greys out once
  the two agree.
- Toggle the `.*` checkbox → dot-files disappear from the grid and the tree together.
- Right-click a file → **Download…** opens the save dialog; pick a path and the status bar
  runs a progress bar, then reports where it landed. Downloading onto an existing local
  file goes through the OS dialog's own replace prompt. **Open in terminal** is greyed out
  on a file, **Download…** on a folder.
- **Rename…** edits the label in place; Enter commits and the grid re-lists so the entry
  lands in its new sort position. **Refresh** picks up a file created from the shell.
- Drag the horizontal splitter: the grid reflows (`tput lines` should follow) and stops at
  the minimum and at 60% of the window. The **Files** button hides the pane and gives its
  rows back to the terminal.

**10. Keyboard focus and the details popup.** Press **Ctrl+Tab** repeatedly: the focus ring
should go shell → tree → files pane → shell. Hide one panel with its status-bar button and
cycle again — the hidden stop is skipped. Then, in the files pane:

- Walk with the **arrows**: left/right move one cell, up/down a whole row, and both ends
  clamp instead of wrapping. Keep going past the bottom of the pane — the grid should scroll
  only when the selection reaches an edge, never re-centre. **Tab** / **Shift+Tab** step
  next and previous. **Esc** hands the keyboard back to the shell (type at the prompt to
  confirm).
- With an entry selected, the popup beside it should name the entry in full, and show the
  MIME type (`text/x-python` on a `.py`, `application/octet-stream` on something unknown),
  the time, the size and `owner:group`. Compare the time and the owner against `ls -l` on
  the remote — they should agree, including the timezone. Select a symlink
  (`ln -s /etc /tmp/link-to-etc`) → the popup adds `→ /etc` a moment later.
- **Enter** on a folder enters it; **F2** renames in place.

**11. Selecting many entries.** In a directory with a dozen or so entries:

- Drag from **empty space** across several cells — a translucent rectangle follows the
  pointer and everything it touches highlights. Release outside the pane (over the terminal)
  and the band should end there, not keep selecting when the pointer comes back.
- **Ctrl+drag** a second band: it adds to the first selection instead of replacing it.
  **Ctrl+click** toggles one entry, **Shift+click** takes the run between two, **Shift+←/→/↑/↓**
  extends from the anchor, **Ctrl+A** takes them all. The popup should switch to
  `N items selected` with the folders/files split and the total size.
- Right-click **inside** the selection → the copy items carry the count; **Copy full path**
  should paste one path per line into the shell. **Rename…** and **Open in terminal** are
  greyed out. Right-click **outside** the selection → it collapses to that one entry first.
- Select several files (a folder among them is fine) → **Download… (N)** asks for a
  destination folder, downloads them one at a time with the progress bar, skips the folder,
  and finishes with `Saved N files`. Run it again into the same folder → the
  **Some of these files are already there** dialog lists the names: **Cancel** downloads
  nothing, **Skip them** leaves the local copies untouched, **Save alongside** writes
  `name-1.ext`, **Replace** overwrites. Check the results with `ls -l` locally.

**12. Full-screen apps.** Run `vim` (or `less` on a long file). The file should render, and
the **arrow keys** should move the cursor — this exercises application cursor mode (DECCKM):
the app enables it and cmote switches its arrow keys to the SS3 form so they register. In
`vim`, `:q!` to exit. Then the query answering and the harder cases:

- **Cursor-position probe.** At the shell, run
  `printf '\033[6n'; read -rsdR r; echo "cursor: ${r#*[}"`. It should print the cursor's
  row;col at once and return to a prompt — if cmote did not answer, `read` would hang until
  you press Enter. Then measure the screen:
  `printf '\0337\033[999;999H\033[6n\0338'; read -rsdR r; echo "size: ${r#*[}"` should report
  the terminal's actual rows;cols (resize the window and repeat — it should track). A program
  like `vim` or `tmux` should now open without the ~1s startup pause its DA probe used to
  cost.

- Run **btop** (`brew install btop` on a mac remote). Every panel should sit in its own box
  where it belongs — no line running on into the next, no frame drawn twice down the screen.
  btop positions its whole UI with cursor moves the previous engine could not follow; the
  VT engine (§23) interprets them, so the layout lands where it belongs.
- Its **graphs** should be dot patterns, evenly spaced inside their cells, and its **box
  corners** should be rounded and meet the straight lines cleanly. Both are drawn from
  geometry, not shaped from a font — no monospace font we could bundle has braille at all.
- Press **F2**: btop's options menu should open. **Esc** closes it. (F1-F12 are mapped to
  the `xterm-256color` terminfo entry.)
- **Click** a process row — btop selects it. **Scroll** over the process list. Drag one of
  its sliders. Then hold **Shift** and drag across the screen: you should get cmote's own
  text selection instead, and Shift+right-click should open cmote's menu. Release Shift and
  the pointer belongs to btop again. Quit with `q`.
- Run **htop** and **mc** (midnight commander) for a second opinion on both — mc lives on
  F1-F10 and is entirely mouse-driven.

**13. Copying, confirmed.** Click the copy button in the **files pane header**, then in the
**folder-tree header**, then the one on a selected entry's **details popup**. Each should
raise a toast at the bottom of the window that fades on its own after about three seconds,
and each should paste back what it promised — the pane's directory, the tree's folder, and
the whole details card (name, target, type, time, size, owner). The context menus' **Copy…**
items should raise the same toast.

**14. Resuming where you left off.** With a session open, `cd /etc/ssh` in the shell, point
the **pane** at a different directory (`/tmp` via the tree), toggle `.*` on, and drag both
splitters to unusual sizes. Then **Disconnect** and reconnect to the same target from the
home screen. Expect: the shell replays a visible `cd /etc/ssh`, the pane reopens on `/tmp`
(and does **not** get dragged to `/etc/ssh` by the shell's first announcement), the tree has
revealed the chain down to it, `.*` is still on, and both panels are the size you left them.
Kill the connection the hard way too (`docker stop cmote-sshd`, or `pkill sshd` on the
remote) and reconnect — the snapshot should survive a hangup, not just a clean disconnect.
Finally, connect to a target saved by an older build (or delete the session fields from
`targets.json` by hand): it should open at the login directory with default panels, no error.

**Cleanup:**

```sh
docker rm -f cmote-sshd
rm -f smoke_key smoke_key.pub smoke_key_enc smoke_key_enc.pub
```

## License

MIT — see [LICENSE](LICENSE).

Bundled fonts keep their own licenses (redistributed under them): **Fira Mono** and
**IBM Plex Mono** under the SIL Open Font License 1.1, and **Material Icons** under
Apache-2.0 — each with its license text in [assets/](assets/).
