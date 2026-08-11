# cmote — Design Plan

A **native, portable SSH client** written in Rust for **Windows 11 and macOS Sequoia
(Intel, `x86_64`)**. A single window: fill
in host / port / user, choose an auth method (password and/or a private key — PEM or
PuTTY `.ppk`), connect. On success the SSH server gives us a shell and we render a
**full VT terminal** inside the window — a working interactive prompt.

This is a **learning project**: the code is meant to be read as much as run, so this
plan is didactic. It explains *why* each choice was made (idiomatic Rust, async,
security) and marks every deliberate shortcut with a `ponytail:` note so "simple"
reads as intent, not ignorance.

Status: **shipping — v4.0.0** (v1 feature set complete; v1.3 adds saved connection
targets on a home screen — profiles only, no secrets — plus an optional
key-passphrase field, §14; v1.3.1 fixes numpad number keys sending navigation
instead of their digits, §9; v1.3.2 makes the home screen follow the system
light/dark theme so the target list stays readable, §14; v1.4.0 tracks the remote
working directory and uploads a local file into it over SFTP, §17; v2.0.0 puts a
2D folder tree of the remote filesystem in the browser strip beside the files pane — browse,
jump, rename, copy paths, §18; **v2.1.0** adds the icon grid of files under the terminal — every entry
in one directory, streamed in batches, with rename and download, §19 — makes it and the
tree keyboard-navigable with a details popup beside the selection, §20, and lets a rubber
band, Ctrl/Shift click or Ctrl+A select many entries at once for a batch copy or
download, §21; **v2.2.0** turns upload into a multi-file, folder-destination batch reachable
from four surfaces with one collision question, §17, lays the grid's cells out as rows
carrying each entry's size and date, §19, confirms every copy with a self-dismissing toast,
§10, and remembers per target where the shell and both panels were so a reconnect resumes
there, §22; **v2.3.0** makes full-screen programs work — the cursor-move spellings the parser
lacked are rewritten on the way in, §9, the grid became one widget that draws every cell at
an exact pixel and every glyph the bundled font lacks itself, §11, the mouse is forwarded to
programs that ask for it, §9, and F1-F12 are mapped, §9; **v2.4.0** answers the DSR/DA status
and identity queries the parser never replied to, so a program that probes the cursor
position or the terminal type no longer stalls waiting on a timeout, §9; **v3.0.0** swaps the
terminal engine from `vt100` to `alacritty_terminal` — a full VT implementation — which
unblocks the DEC line-drawing charset, the rich SGR set (dim / italic / strikethrough /
conceal, every underline style + underline colour), origin-mode-correct cursor reports, the
OSC colour and pixel-size query replies, the remote-set window title, the DECSCUSR cursor
shape, focus reporting, and 10 000 lines of scrollback with a scroll indicator, §23; it also
follows OSC 8 hyperlinks — a Ctrl-hover underline reveals one, then Ctrl+click or a right-click
Open link / Copy link opens it, the scheme gated to http/https/mailto, §24; and speaks the kitty
keyboard protocol — the `CSI u` encoding
for disambiguate, press / repeat / release events, report-all and associated text, superseding
modifyOtherKeys when an editor turns it on, §25). Everything since v2.3.0 lands in the one
**v3.0.0** major release — §23, §24 and §25 are all part of it, with no point increments above
3.0.0; and everything since *that* lands the same way in the one **v4.0.0** major release —
§32 through §50, with no point increments in between (the 3.1.0 the manifest carried for a
while was work in progress, never tagged, and is folded in here). What earns the major number
is that two of them change what a cmote window *is*: it can be **split** in two, each half with
its own tab strip and its own session on screen at once (§48), and one connection can hold
**more than one account**, with the file panes following the account you switched to (§45, §46).
The rest of the release: a **remote text editor** in its own tab — line-numbered, encoding- and
line-ending-aware (BOM-detected, saved as opened), with changed-line marks and save / save-as /
close over SFTP (§32); **inline sixel images** in the scrollback, decoded in-house and anchored to
the document so they scroll with it (§41); **finding text** in the whole scrollback, every match
on screen washed and the list kept up to date under live output (§35, §39, §44); **shell-integration
prompt marks** driving a per-tab command-status dot, jump-to-prompt and select-command-output
(§34); a **selection that speaks document lines**, so text stays selected as it scrolls (§40),
with **double- and triple-click** to take a word or a line (§42); the **identity and input queries**
the engine drops, answered beside it (§33, §36); a **tab strip the user orders**, with files beside
their session and drag to rearrange (§38), and a close that **returns you where you were** (§37);
and a **filter box over the saved-target list** — a fragment while you type, a whole-row glob the
moment a `*` or `?` appears (§49); a keyboard that **follows what you act on**, so typing while
a side panel holds it, or choosing an item off the grid's menu, hands it back to the shell (§50);
and the **open and closed hand** over everything you can pick up — a tab chip, a dialog header —
drawn by cmote because Windows has neither cursor (§51); and a **chip's own right-click menu** that
sends a tab to another area of the window — moving it there, or opening a second copy of the session
where the first one is standing (§52); and a **picture that opens as a picture**, so double-clicking
a `.png` gives its own zoomable tab instead of a text editor that can only refuse it (§53).
Both targets are supported first-class, and each has a verified toolchain on its host:

- **macOS Sequoia (Intel)** — this machine (15.7.7): `rustc`/`cargo` 1.97.1 stable,
  `x86_64-apple-darwin`, Xcode Command Line Tools `clang` 17.
- **Windows 11** — a Windows host: `rustc`/`cargo` 1.91.0, `x86_64-pc-windows-msvc`,
  VS 2019 BuildTools VC x64 tools.

This document is the reference to build against.

---

## 1. Locked decisions

| Area | Decision |
|---|---|
| Language | Rust, stable channel (verified: 1.97.1 on `x86_64-apple-darwin`, 1.91.0 on `x86_64-pc-windows-msvc`) |
| Target | `x86_64-pc-windows-msvc` (native Win11, MSVC linker) **and** `x86_64-apple-darwin` (macOS Sequoia, Intel; `clang` linker via Xcode CLT) |
| Distribution | **Portable**: one self-contained binary (`.exe` on Windows, a bare Mach-O on macOS — optionally wrapped in a `.app`), no installer, no registry/`plist` writes, no external runtime |
| GUI | **iced 0.14** — pure-Rust, Elm architecture (state / `Message` / `update` / `view`) |
| SSH | **russh 0.62** — pure-Rust async SSH client (no C deps → clean static build) |
| Async runtime | **tokio** (multi-thread) on a background thread; bridged to the GUI by channels |
| Terminal | **Full VT emulator** — `alacritty_terminal` maintains the screen grid and answers the host's status/identity queries itself (§9, §23); the grid is drawn by one custom iced widget, cell-exact (§9). *(v3.0 replaced the original `vt100`, whose small subset was the compatibility ceiling — §23.)* |
| Key formats | OpenSSH / PEM native via `russh::keys`; **PuTTY `.ppk` via `ssh-key`'s `from_ppk`** (already in the russh tree, `ppk` feature) |
| Host key | **TOFU** (trust-on-first-use) against a portable `known_hosts`; explicit user accept; a mismatch opens a loud override dialog — reject / trust once / replace — never auto-trusted (§8, §28) |
| Credentials | Secrets **session-only** — held in memory, `zeroize`d on drop, never written to disk (§12). Connection *profiles* (no secret) are saved so the home screen can list targets (§14) |
| Auth order | The chosen method first (`publickey` / `password` / `keyboard-interactive` / `agent`), then chain into `keyboard-interactive` while the server still offers it — 2FA / OTP and challenge-response (§7); driven by what the server accepts |
| File picker | `rfd` — native open-file dialog for the key file (Win32 on Windows, `NSOpenPanel` on macOS) |
| Errors | `anyhow` at the app boundary; typed `thiserror` enums deferred until a real API needs them |
| Config location | `known_hosts` **and** `targets.json` in `./cmote-data/` beside the exe, falling back to `%LOCALAPPDATA%\cmote` (Windows) or `~/Library/Application Support/cmote` (macOS) if that dir is read-only |

---

## 2. Why these choices (didactic)

Each decision below is a thing to learn from, not just a dependency.

- **iced over egui/Tauri** — iced uses the **Elm architecture**: your whole UI is a
  pure function `view(state) -> Element<Message>`, and all change flows through one
  `update(&mut state, Message)`. There is no hidden mutable widget tree. This teaches
  Rust's ownership model *by making it visible*: state is owned in one place, events
  are plain `enum` values (a `Message`), and the compiler forces every case to be
  handled. Tauri would have meant writing the UI in JavaScript — the opposite of a
  Rust learning project.
- **russh over ssh2 (libssh2)** — `ssh2` binds a C library: simpler at first, but you
  learn C-wrapper Rust and add build/portability friction (linking a C lib on
  Windows). `russh` is **Rust and async** with no external C library to link — it
  forces the real lessons: `async`/`.await`, `Send`/`Sync` across threads, trait
  objects (`Handler`), and ownership of a connection handle. Harder, and that's the
  point. *(Reality check found at scaffold time: russh's crypto is not literally
  C-free. Its default `aws-lc-rs` backend needs a C toolchain **and NASM** to build,
  which breaks the portable build. We select the `ring` backend instead — it ships
  pre-generated assembly for `x86_64-pc-windows-msvc`, so it builds with no NASM and
  no external SSH library. On `x86_64-apple-darwin`, `ring` builds the same assembly
  with the Xcode Command Line Tools' `clang` — no NASM, still no SSH library (§12 records
  this target difference). See §3 / Cargo.toml.)*
- **tokio on a background thread** — iced's event loop is synchronous; SSH I/O is
  async and must never block the UI. The idiomatic bridge is a dedicated tokio runtime
  on its own thread, talking to the GUI over channels (§4). This is *the* pattern for
  "GUI + network" in Rust; learning it here transfers everywhere.
- **vt100 first, then alacritty_terminal (DECISION REVERSED in v3.0)** — a real terminal
  must interpret ANSI escape sequences (colors, cursor moves, clears). `vt100` parsed a byte
  stream into a simple `Screen` grid of cells we could render directly in iced — small,
  readable, enough for v1 — and starting there was the right call: it kept the early terminal
  code legible while the async/GUI bridge and the security path were the real lessons. But
  its subset became the **compatibility ceiling** (whole classes of documented behaviour it
  cannot represent, §16), so **v3.0 swapped to `alacritty_terminal`** — a full VT
  implementation, heavier and with an API that tracks Alacritty's needs, but the only way to
  render arbitrary programs and answer their queries. The staged swap and its reasoning are
  §23. The lesson stands both ways: start simple, and replace the simple thing when its
  limits — not a guess about them — are what block you.
- **Session-only *secrets*** — the safest secret is the one never persisted. Passwords
  and decrypted keys live only for the session and are wiped with `zeroize`. As of v1.3
  connection *profiles* (host / port / user / auth kind / key path — no secret) ARE
  saved so the home screen can list targets (§14); persisting the secrets themselves,
  encrypted at rest, stays a deliberate later feature (§16), not a v1 gap.

---

## 3. Tech stack + versions (mid-2026)

| Crate | Version | Purpose | Notes |
|---|---|---|---|
| `iced` | 0.14.0 | GUI (Elm architecture, `Task`, `Subscription`) | pure Rust; wgpu/tiny-skia renderer, no web runtime. **`features = ["advanced"]`** since v2.3 — it unlocks the `Widget` trait, which the terminal grid is one of (§9) — **plus `"image-without-codecs"`** since v4.0.0, which turns on the renderer's raster pipeline so that same widget can composite inline sixel images (§41). The `-without-codecs` spelling adds the `image` crate with *no* format decoders: cmote decodes sixel itself, so a PNG/JPEG parser would be attack surface for a format we never hand the renderer |
| `russh` | 0.62.4 | async SSH client | tokio-based; `client::Handler` trait. **`default-features = false` + `ring`** backend (not the default `aws-lc-rs`, which needs NASM; `ring` builds on both targets — prebuilt asm on Windows, via Xcode CLT `clang` on macOS) |
| `russh::keys` | (with russh) | key loading + `known_hosts` | `load_secret_key`, `decode_secret_key`, `check_known_hosts_path` |
| `russh-sftp` | 2.3.0 | the sftp subsystem, for file upload (§17) | rides russh's `ChannelStream` — a protocol on the existing SSH stack, not a second one. Pure Rust, no C |
| `tokio` | 1.53 | async runtime | features: `rt-multi-thread`, `net`, `io-util`, `fs` (streaming an upload off disk, §17), `sync`, `macros`, `time` |
| `alacritty_terminal` | 0.26.0 | VT/ANSI terminal engine | full VT implementation behind Alacritty; feeds bytes via its `vte` ANSI `Processor`, exposes a grid of cells, and answers host queries through an `EventListener` (§9, §23). Pure Rust, Apache-2.0. *(v3.0 replaced `vt100` 0.16.2 — §23; `vte` is pulled in transitively by it.)* |
| `.ppk` support | (in `ssh-key`) | read PuTTY `.ppk` → `PrivateKey` | **No separate crate.** `ssh-key 0.7.0-rc.11` (pinned by russh, `ppk` feature on) provides `PrivateKey::from_ppk` — see §7 |
| `zeroize` | 1.9 | wipe secrets from memory on drop | `Zeroizing<String>` for passwords/passphrases |
| `rfd` | 0.17.2 | native file-open dialog | portable; used to pick the key file (0.17, not 0.15) |
| `serde` / `serde_json` | 1.0 | serialize `targets.json` — saved profiles + the per-target session snapshot (§14, §22) | `derive` on the profile structs; a corrupt store is logged and treated as empty, never a crash |
| `open` | 5 | launch an OSC 8 hyperlink in the OS browser (§24) | pure Rust, no C toolchain; hands the URI to PowerShell `Start-Process` as data (an env var), never a shell command line — the `cmd /C start` inject path is behind an off-by-default `insecure` feature we do not enable. cmote still gates the scheme to http/https/mailto first (`link`) |
| `anyhow` | 1.0 | app-level error handling (`Result<_, anyhow::Error>`) | context-rich errors, `?` everywhere |
| `thiserror` | 1.x | *(deferred)* typed error enums for module boundaries | add when a module becomes a real API |
| `tempfile` | 3 | *(dev-dependency)* temp dirs for tests writing `known_hosts` fixtures (§13) | test-only; not linked into the shipped binary |

Versions above are the ones actually resolved by `cargo add` at scaffold time and
recorded in `Cargo.lock`. We keep **caret (`^`) requirements** in `Cargo.toml` and
rely on the **committed `Cargo.lock`** for reproducible, auditable builds (§12) —
that is the idiomatic reproducibility guarantee for a binary crate, so hard `=`
pins are unnecessary.

---

## 4. Architecture — the async ↔ GUI bridge (core pattern)

The single most important pattern in this app. Two worlds that must not block each
other, joined by two channels.

```
        GUI thread (iced event loop, synchronous)                 background thread
   ┌─────────────────────────────────────────────┐          ┌───────────────────────┐
   │  App state ── update(Message) ── view(state) │          │  tokio runtime         │
   │        ▲                    │                │          │   russh client          │
   │        │ Message            │ user input     │          │   (Handler, channel)    │
   │  Subscription               ▼                │          │                        │
   │   (reads rx) ◄── SshEvent ── tx ─────────────┼──mpsc────┼─► terminal output,      │
   │                                              │  (out)   │    status, errors       │
   │  Command sender ── SshCommand ── tx ─────────┼──mpsc────┼─► keystrokes, resize,    │
   │                                              │  (in)    │    disconnect            │
   └─────────────────────────────────────────────┘          └───────────────────────┘
```

- **`Message`** — the app's event `enum`: UI events (`HostChanged(String)`,
  `ConnectPressed`, `KeyPressed(...)`) *and* SSH events surfaced from the background
  (`Ssh(SshEvent)`). One type, exhaustively matched in `update`.
- **Outbound channel (SSH → GUI)** — the tokio task sends `SshEvent`
  (`Output(Vec<u8>)`, `NeedPassphrase`, `HostKey(fingerprint)`, `Connected`,
  `Disconnected`, `Error(String)`). An iced **`Subscription`** owns the receiver and
  turns each item into a `Message::Ssh(..)`. (iced 0.14 exposes this via
  `iced::stream` + `Subscription::run`; confirm the exact constructor at impl time.)
- **Inbound channel (GUI → SSH)** — `update` sends `SshCommand`
  (`Connect(ConnectParams)`, `Input(Vec<u8>)`, `Resize{cols,rows}`, `Disconnect`) into
  an `mpsc::Sender` the tokio task drains.
- **Why channels, not shared mutexes** — message-passing keeps ownership clear and
  sidesteps `Send`/`Sync` fights over the russh handle. The GUI never touches the
  socket; the network task never touches the widget tree. This is the Rust-idiomatic
  "share memory by communicating" model.
- **Backpressure** — bounded channels: a flood of terminal output can't grow memory
  without limit; the reader task awaits when the GUI is behind. `ponytail:` a
  generous fixed bound is fine for v1; tune only if a profiler complains.

---

## 5. Repo layout (single crate, many small files)

Not a workspace — one binary crate. Small, cohesive modules (per the <800-line rule),
organized by responsibility:

```
cmote/
├── Cargo.toml
├── Cargo.lock            (committed — reproducible, auditable builds)
├── PLAN.md
├── README.md
├── assets/
│   ├── FiraMono-Regular.ttf  monospace font (normal weight, 400) embedded in the exe (§9, §11)
│   ├── FiraMono-Medium.ttf   its medium weight (500), bundled for family completeness (§9)
│   ├── FiraMono-Bold.ttf     its bold weight (700), for bold cells (§11)
│   ├── FiraMono-LICENSE.txt  the family's OFL 1.1 license (required for redistribution)
│   ├── IBMPlexMono-Italic.ttf      the italic face Fira Mono lacks, for italic cells (§9, §23)
│   ├── IBMPlexMono-BoldItalic.ttf  its bold-italic, for bold+italic cells (§23)
│   ├── IBMPlexMono-LICENSE.txt     IBM Plex's OFL 1.1 license (required for redistribution)
│   ├── MaterialIcons-Regular.ttf  the file-type icons the files pane draws with (§19)
│   └── MaterialIcons-LICENSE.txt  its Apache-2.0 license (required for redistribution)
└── src/
    ├── main.rs           entry; #![windows_subsystem = "windows"] (inert on macOS); spawns runtime + iced::run
    ├── app.rs            iced App: a strip of independent `Tab`s + the shared target list / vault; `Tab` = one session's State/Message/update/view; App delegates + routes SSH events per tab + draws the strip (§26)
    ├── cursor.rs         the open / closed hand over every grab handle (tab chip, dialog header): the art, the `HCURSOR`s built from it, and the `WM_SETCURSOR` subclass that paints them — Windows has neither cursor (§51)
    ├── explorer.rs       the remote folder tree's model: nodes, expansion, path arithmetic (§18)
    ├── files.rs          the files pane's model: one directory, batched listings, icon categories (§19)
    ├── forward.rs        the pure port-forward spec: kind (L/R/D) + bind/target, parse / validate / label / serialise (§27)
    ├── glob.rs           the home filter's text rule: a fragment until `*` or `?` is typed, then a whole-text glob; case-insensitive (§49)
    ├── integration.rs    the OSC 7 / OSC 133 block a remote's rc file can be given, its markers, and the install / remove edits (§17, §34)
    ├── link.rs           opening an OSC 8 hyperlink safely: the scheme allow-list + the OS browser launch (§24)
    ├── mru.rs            the tabs' activation order (ids, most recent last): a close falls back to the previous visit (§37)
    ├── palette.rs        the terminal colour scheme (default fg/bg + xterm-256), shared by the renderer and the colour-query answerer (§9, §23)
    ├── paths.rs          data-dir resolution: `cmote-data/` beside the exe if writable, else `%LOCALAPPDATA%\cmote` / `~/Library/Application Support/cmote` (§11)
    ├── preview.rs        the picture tab's model: which files open as a picture, and the fenced decode — sniff by magic bytes, cap the dimensions and the allocation, name the format in every refusal (§53)
    ├── profiles.rs       load/save `targets.json`: saved connection profiles + the per-target session snapshot; corrupt file → treated as empty (§14, §22)
    ├── secret.rs         the session-secret wrapper (`Secret` over `zeroize`): passwords / passphrases held in memory, wiped on drop, never logged (§12)
    ├── ui/
    │   ├── mod.rs         view helpers, incl. the shared `elide_middle` path/name cut (§22); host-key / passphrase / error dialogs (§8, §7, §6)
    │   ├── connect.rs     the connection form (host/port/user/auth/key)
    │   ├── dialog.rs      shared modal-dialog chrome: header (title + ✕, the drag handle and its hand cursor) / body / footer, and `Card` — where a floating dialog sits and how a header drag moves it, once for every dialog in the app (§10, §26, §51)
    │   ├── explorer.rs    the folder-tree panel, its splitter and its context menu (§18)
    │   ├── files.rs       the file icon grid, its splitter and its context menu (§19)
    │   ├── forward.rs     the port-forwards manager dialog: active-tunnel rows + the add form (§27)
    │   ├── grid.rs        the terminal screen as ONE custom widget: cell-exact quads + text, drawn braille and box corners, mouse reports, search-match washes (§11, §39)
    │   ├── home.rs        the home screen: the filter box, the saved-target list, select / open / rename / delete, theme-following colours (§14, §49)
    │   ├── menu.rs        shared right-click menu chrome: panel / items / separator / dismiss layer (§10, §19, §52)
    │   ├── preview.rs     the picture tab: a toolbar naming what the bytes turned out to be, the image on its ground (iced's own zoom/pan viewer), the refusal card (§53)
    │   ├── selection.rs   stream text selection over the grid, in absolute document lines; word / line expansion for a double or triple click; text extraction, unwrapping across a wrap (§10, §40, §42)
    │   ├── snackbar.rs    the copy-confirmation toast, bottom-centre, self-dismissing (§10)
    │   ├── tabs.rs        the tab strip across the top: one chip per session + "+"; mouse-only select / open / close (§26), drag a chip to move it (§38), the hand cursor over one (§51), the right-click menu that sends it to another area (§52)
    │   └── terminal.rs    the terminal screen's layout and chrome; the cell metrics; pixel→cell resize math (§9)
    ├── ssh/
    │   ├── mod.rs         module tree + `open_sftp`, shared by upload, download and browse (§17-§19)
    │   ├── client.rs      russh Handler impl; connect → auth → shell; the tokio task loop
    │   ├── agent.rs       publickey auth via an SSH agent / Pageant (OpenSSH pipe, Pageant, `SSH_AUTH_SOCK`); no key material seen (§7)
    │   ├── auth.rs        method selection + attempts (publickey, password, keyboard-interactive, agent) + 2FA chaining (§7)
    │   ├── browse.rs      list + rename + create + delete remote entries over sftp, falling back to `ls`/`mv`/`mkdir`/`rm -rf` (§18, §19)
    │   ├── download.rs    file + recursive-folder download over an sftp channel: stream, progress, per-file collisions (§19)
    │   ├── forward.rs     run port forwards: local/dynamic listeners → direct-tcpip, remote via tcpip_forward + Handler, SOCKS5 (§27)
    │   ├── hostkey.rs     TOFU: check_known_hosts_path, fingerprint, accept/learn; a changed key's stored fingerprint + replace, for the override dialog (§8, §28)
    │   ├── integration.rs find the login shell + its rc file and write the cwd announcer into it, atomically (§17)
    │   ├── keyfile.rs     load PEM/OpenSSH + PuTTY .ppk (via ssh-key from_ppk); passphrases; zeroize (§7)
    │   ├── transfer.rs    the recursive transfer's shared spine: the tree plan + the per-file collision protocol (§17, §19)
    │   ├── upload.rs      file + recursive-folder upload over an sftp channel: batch pre-scan, stream, progress, per-file collisions (§17)
    │   └── fixtures/      real .ppk test vectors (Ed25519, plain + encrypted)
    ├── term/
    │   ├── mod.rs         terminal emulator wrapper: drive the engine, expose the screen view, resize, answer the host's colour/size queries, reserve the cells an inline image covers (§9, §16, §23, §41)
    │   ├── cwd.rs         scan OSC 7 / OSC 9;9 out of the output stream: the remote cwd (§17)
    │   ├── graphics.rs    scan the sixel images out of the stream and anchor each to a document line, capped and evicted oldest-first (§41)
    │   ├── sixel.rs       decode a sixel payload into RGBA pixels — in-house, no image-format dependency (§41)
    │   ├── keymap.rs      GUI key events → the bytes a terminal sends; legacy or kitty per the active mode (§9, §25)
    │   ├── kitty.rs       encode a key event in the kitty keyboard protocol's CSI u form (§25)
    │   ├── mouse.rs       pointer events → the xterm mouse reports a program that asked for them expects (§9)
    │   ├── modkeys.rs     scan `CSI > 4 ; p m` out of the stream: the remote's modifyOtherKeys level (§9)
    │   ├── osc133.rs      scan the OSC 133 shell-integration marks out of the stream: prompt lines, command state, output ranges (§34)
    │   ├── query.rs       answer the identity queries the engine drops — XTVERSION, DECRQSS, XTGETTCAP, DA3, XTSMGRAPHICS — and amend its DA1 to advertise sixel (§33, §36, §41)
    │   ├── screen.rs      the engine-agnostic Screen/Cell/Color view the app reads through — incl. a cell's OSC 8 link, the kitty flags, the viewport↔document line mapping and whether a line wraps into the next (§9, §16, §23, §24, §25, §40, §42)
    │   └── search.rs      find text anywhere in the scrollback: a row flattened for searching, the match list, which is current, which are on screen (§35, §39)
    ├── transfer.rs       the ONE transfer slot and everything queued behind it: the batch being set up, the file / folder / download queues, the collision questions, resume — including the one a dropped session hands to the next — and an OS drop settling into all of it (§16, §17, §19, §21, §29)
    └── bridge.rs          SshCommand / SshEvent enums + channel wiring (§4)
```

---

## 6. Connection + authentication flow

Ordered so cheap validation and security gates come first.

0. **Validate input** (GUI, before anything): host non-empty; port a valid `u16`
   (default 22); user non-empty; if a key path is given, the file exists. Fail fast
   with a clear message — never send garbage to the network layer.
1. **Resolve + TCP connect** (tokio task): `tokio::net::TcpStream` to `host:port`,
   with a connect timeout. Report `Connecting` → GUI.
2. **SSH handshake**: hand the stream to `russh::client::connect`/`Connection` with our
   `Handler`.
3. **Host-key check (security gate, §8)** — russh calls our `Handler::check_server_key`
   *before* auth. We compare against the portable `known_hosts`:
   - known + matches → proceed silently.
   - unknown → emit `SshEvent::HostKey(fingerprint)`; the GUI shows it and asks the
     user to accept. On accept we append to `known_hosts` and continue. **Never
     auto-accept.**
   - known + **mismatch** → emit `SshEvent::HostKeyChanged { stored, presented }` and block on
     the user's explicit choice: **reject** (the default), **trust once** (this session, no
     write) or **replace** (pin the new key). A loud dialog with both fingerprints; never
     auto-trusted (§8, §28).
4. **Authenticate (§7)** — the chosen method first, then chain into keyboard-interactive
   as the server directs:
   - the form's choice runs first: `authenticate_publickey` (key), `authenticate_password`
     (password), the keyboard-interactive loop (interactive), or `authenticate_publickey_with`
     driven by a live SSH agent / Pageant (agent — `ssh/agent.rs`, which offers each agent key
     in turn and lets the agent sign, so no key material is ever seen).
   - then, while the server still lists `keyboard-interactive` and attempts remain, run its
     prompt loop — the same code path covers a **fallback** (our method was not offered but
     the server does challenge-response) and a **second factor** after a partial success
     (a key/password **plus** an OTP). Bounded like OpenSSH's `MaxAuthTries` so a re-offering
     server cannot loop the prompt forever; the user can cancel any prompt to abort.
   - each keyboard-interactive request emits `SshEvent::Interactive { name, instructions,
     prompts }`; the GUI shows a field per prompt (masked when `echo` is false — an OTP /
     password) and sends the answers back as `SshCommand::Interactive`. A message-only request
     (no prompts) is answered with an empty response set without troubling the user.
   - respect the server's advertised methods; report `Authenticating`, then either
     `Connected` or a **single generic** `Error` (no oracle about which factor was wrong).
5. **Shell**: `channel_open_session()` → `request_pty(term = "xterm-256color", cols,
   rows, …)` → `request_shell()`. The pty size comes from the current terminal-view
   dimensions.
6. **Stream**: loop — server data arrives on the channel → `SshEvent::Output(bytes)` →
   GUI feeds it to the terminal engine (§9); user keystrokes arrive as
   `SshCommand::Input(bytes)` → `channel.data(&bytes)`. Window resize →
   `SshCommand::Resize` → `channel.window_change(...)`.

---

## 7. Key handling (PEM / OpenSSH / PPK)

Two format families; only one is native to the SSH ecosystem.

- **OpenSSH / PEM (native)** — `russh::keys::load_secret_key(path, passphrase)` (or
  `decode_secret_key` for in-memory bytes). If the key is encrypted and no passphrase
  was given, russh errors → we emit `SshEvent::NeedPassphrase`, the GUI prompts, we
  retry. The passphrase lives in a `Zeroizing<String>` and is wiped after use.
- **PuTTY `.ppk` (via `ssh-key`'s parser — DECISION REVISED)** — the original plan
  was to hand-roll a `.ppk` parser because "no usable crate exists". **That premise
  was false.** The exact `ssh-key` version russh 0.62.4 pins (`=0.7.0-rc.11`) ships a
  complete PuTTY parser, and **russh enables its `ppk` feature unconditionally** — so
  `russh::keys::PrivateKey::from_ppk(text, passphrase)` is already compiled into our
  binary, with **no new dependency**. It reads PPK **v2 and v3**, verifies the MAC in
  constant time before trusting any bytes (HMAC-SHA-256 for v3, HMAC-SHA-1 for v2),
  derives the key (Argon2id/i/d for v3, a SHA-1 construction for v2) and AES-256-CBC
  decrypts the private blob — RSA, Ed25519, ECDSA **and** DSA inner keys. We reuse it.
  Flow (`ssh/keyfile.rs`):
  1. Read the file once; sniff the format by *content* (the `PuTTY-User-Key-File-`
     header line), not the extension, which a user can rename freely.
  2. `.ppk` → `PrivateKey::from_ppk`; OpenSSH/PEM → `decode_secret_key`. Both yield
     the same `russh::keys::PrivateKey` the auth step consumes.
  - **Why reuse, not hand-roll** — MAC verification and key decryption are a
    security-sensitive path, and PLAN §12 puts *security over purity*. An audited
    RustCrypto implementation already in the tree beats our own crypto glue; the
    didactic loss (binary-format parsing) is real but outweighed here. A standalone
    "parse a binary format by hand" exercise can live outside the security path if
    wanted.
  - Passphrases stay in `Secret`/`Zeroizing`. `ponytail:` `from_ppk` takes an owned
    `String` by value, so the copy handed to it is a plain, non-zeroized `String`
    dropped inside the crate — a small, API-imposed secret-hygiene gap, noted in
    `keyfile.rs`.

`ponytail:` `from_ppk` covers the current PPK v2/v3 containers and RSA/Ed25519/
ECDSA/DSA inner keys; a genuinely exotic container surfaces a clear error, not a
silent failure.

**OpenSSH certificates (add-on to key auth).** A user certificate is a public key signed by
a trusted CA, so a server trusts the one CA instead of every individual key in an
`authorized_keys` file. cmote treats it exactly the way OpenSSH does — **an add-on to key
auth, not a method of its own**: the private key still signs the challenge; the certificate
is the extra CA-signed blob presented with the offer. So there is no fifth radio — under key
auth an optional **Certificate** file sits beside the key file (a `Certificate: Option<PathBuf>`
on `AuthMethod::Key`). When it is set, `ssh/auth.rs` loads it (`keyfile::load_certificate` →
`russh::keys::load_openssh_certificate`, which parses the one-line `ssh-…-cert-v01@openssh.com`
file) and authenticates with `session.authenticate_openssh_cert(user, key, cert)`; when it is
absent the path is unchanged plain public-key auth. russh derives the signature algorithm from
the certificate itself, so there is no separate RSA-hash negotiation on the certificate path,
and a certificate that will not load is a hard error surfaced to the user rather than a silent
fall-back to bare-key auth.

- **Auto-detect** — picking a key auto-fills the OpenSSH `<key>-cert.pub` sibling when that
  file exists and no certificate is already chosen (`keyfile::cert_sibling`), matching what the
  command-line client does; a **Clear** button drops it back to plain key auth. Non-destructive:
  a certificate the user chose, or a key with no sibling, is left untouched.
- The certificate is **public data**, like the key *path* — so, unlike the passphrase, it is
  remembered with the saved target (§14), never a secret in the vault (§12).
- `ponytail:` **agent-held certificates** (a cert the SSH agent holds and signs for) are still
  deferred (§16) — this is the file-based certificate only.

---

## 8. Host-key verification (security)

The one control that stops a man-in-the-middle. Implemented in `Handler::check_server_key`.

- **Store**: a portable OpenSSH-format `known_hosts` file (§11). Checked with
  `russh::keys::check_known_hosts_path(host, port, key, path)`.
- **First contact (TOFU)**: unknown host → present the key's **fingerprint**
  (SHA-256, the format users recognize) to the user and require an explicit accept
  before appending it. This is trust-on-first-use: we can't verify a key we've never
  seen, but we pin it and detect any change afterward.
- **Mismatch (override UI — REVISED in v3.0.0, §28)**: a stored key that no longer matches →
  treat as hostile (key rotation *or* MITM). v1 refused outright and told the user to edit
  `known_hosts` by hand. v3.0 keeps that suspicion but **surfaces the decision** instead of
  dead-ending it, because the by-hand path is opaque and the common real cause (a server that
  rotated its key) is legitimate. `check_server_key` no longer returns `Ok(false)` on a change:
  it emits `SshEvent::HostKeyChanged { stored, presented }` and **blocks** the handshake on the
  user's explicit choice, exactly like first contact. The dialog is deliberately loud — a red
  "possible man-in-the-middle" line and **both** SHA-256 fingerprints (the one pinned vs the one
  presented, each selectable for out-of-band comparison) — and offers three choices:
  - **Reject** — refuse. The safe default: the ✕ and a backdrop click both pick it, and a GUI
    that went away counts as reject.
  - **Trust once** — connect this session only, leaving `known_hosts` untouched, so the same key
    warns again next time. The safer override when the change might be transient or unverified.
  - **Replace key** — drop the stale line and pin the presented key, so future connections verify
    against it silently. The path for a confirmed rotation.

  The friction is the warning, the two fingerprints and reject-by-default — **not** a
  type-to-confirm speed bump, which was considered and left out as disproportionate. Crucially a
  changed key is still **never auto-trusted**: every override is an explicit, informed click. The
  three-way choice rides one `HostKeyChoice` enum on the existing decision one-shot; `Pin` learns
  a first-contact key or **replaces** a changed one (`hostkey::replace` = drop the offending line
  + `learn`), `TrustOnce` connects without writing, `Reject` refuses.
- **Why an override at all** — refusing outright did not make anyone safer; it made the legitimate
  case (rotation) a dead end that pushed users to disable checking entirely elsewhere. A loud,
  fingerprint-comparing, reject-by-default dialog keeps the MITM signal while giving the honest
  case a visible, auditable path. The line held is *no silent* override — never *no* override.
- **Why not skip it** — accepting any host key unconditionally (the "just make it work" shortcut)
  turns every connection into a spoofing target. Non-negotiable; never simplified away.

---

## 9. Terminal emulator

Turning a raw byte stream into a screen.

- **Parser**: every `SshEvent::Output` chunk is fed to `alacritty_terminal` through its
  `vte` ANSI `Processor` (`Processor::advance`), which interprets each ANSI escape and glyph
  and maintains the grid — cells with a glyph, fg/bg colour and attributes, plus the cursor
  and the terminal modes. It is a full VT implementation (§23), so the two gaps cmote used to
  paper over from outside — the cursor-move spellings the old `vt100` lacked (btop's `CSI
  y;x f` among them) and the status/identity queries it never answered — are handled by the
  engine itself. The hand-rolled `term/compat` (a rewriter for five alias sequences) and
  `term/answer` (a DSR/DA answerer) were **retired** with the swap.
- **The engine answers the host's queries itself.** Some sequences are not commands but
  **questions** the program writes downstream and then **blocks reading its stdin** for —
  cursor-position reports (`CSI 6 n`), device status (`CSI 5 n`), device attributes (`CSI c`
  / `CSI > c`), mode requests (DECRQM) — so leaving them silent stalls vim, tmux, less and
  shell size-probes on a timeout. The engine writes each reply as an `Event::PtyWrite`;
  cmote's listener collects those and `Terminal::process` returns them, and app.rs sends them
  **back on the input channel**, the same path a keystroke takes. A cursor-position report is
  emitted at the instant the query is parsed, so it reflects the cursor **where the query
  sat** — the size-probe idiom (`ESC 7` save, jump to a far corner clamped to the real size,
  `CSI 6 n`, `ESC 8` restore) reports the corner and not the restored position; a test locks
  it. cmote also answers the terminal's **colour** queries (OSC 10 / 11 / 12 foreground /
  background / cursor, OSC 4;n a palette slot) and its **text-area pixel size** (CSI 14t),
  resolving them against cmote's own colour scheme (`palette`, the same table the grid paints
  from) and the cell metrics — so a program that probes the background to pick a light/dark
  theme is told exactly what the screen shows (§23). **Security**: every reply cmote sends is a
  report the engine formatted — a `PtyWrite` numeric report, an OSC-colour string, or a
  pixel-size report — and none carries `CR`/`LF`, so a remote cannot use a query reply to submit
  a command at a prompt. The OSC 52 clipboard events (`ClipboardLoad` / `ClipboardStore`) are
  **dropped**: a remote must not read or poison the local clipboard (§12).
  `ponytail:` the ancient X10 mouse protocol (`?9`, press-only) is not implemented by the
  engine, so it is no longer supported — an accepted loss, since no current program asks for
  it (§23).
- **Render** (`ui/grid.rs`, rewritten v2.3): the `Screen` is drawn by **one custom
  widget** (`iced`'s `advanced` feature), not by a widget per cell. It paints quads and
  text at absolute pixel positions, using a **bundled** monospace font (**Fira Mono**,
  embedded in the exe — OFL 1.1). Bundling it (rather than `Font::MONOSPACE`) makes the
  grid look identical on every machine and gives an **exact** cell advance (600/1000 em =
  0.6), which the resize math depends on. The **Regular (400)**, **Medium (500)** and
  **Bold (700)** weights are all embedded (same release, same OFL licence): normal cells draw
  in Regular, bold cells in Bold, and every weight shares the 0.6 advance so the choice never
  disturbs the metric. Fira Mono ships **no italic**, so italic cells draw from **IBM Plex
  Mono** (OFL 1.1) instead — the closest humanist monospace whose advance is the same 0.6, so
  an italic run stays on the grid (§23). The engine picks the exact bundled face by
  family + weight + style, because `cosmic-text` does not nearest-match within a family: an
  unbundled combination would fall back to a proportional system font and break the grid.
  Two rules make the drawing what it is:
  - **Every glyph starts at the exact pixel its column starts at.** A run of consecutive
    same-styled **ASCII** cells is still drawn as one cached string — that is the common
    case and the cheap one — but anything non-ASCII is sealed into a run of its own (a
    *wide* CJK cell claims two columns, anything else one). A glyph the bundled font lacks
    falls back to a system font whose advance we do not control; laid out as flowing text
    it drags the rest of the line sideways, and the row stops lining up with the screen.
    Sealed and positioned, a fallback glyph can only be the wrong *shape*, never in the
    wrong *place*. Text is clipped to its **row**, not its cell, so a glyph a shade too
    wide leans on its neighbour instead of losing a slice of itself.
  - **What no bundled font has, we draw.** **Braille** (U+2800-U+28FF — btop's default
    graph symbol, and absent from every monospace font we looked at, including DejaVu Sans
    Mono, Consolas and Liberation Mono) *is* a 2x4 dot bitmap in the low byte of its code
    point, so eight rounded quads at exact sub-cell positions render it better than any
    font could and cost no asset. The four **rounded box corners** (U+256D-U+2570, which
    Fira Mono lacks while having all 148 other box-drawing glyphs) are a quarter arc: a
    circular quad whose *border* is the stroke and whose fill is transparent, clipped with
    `with_layer` to the one quadrant the arc lives in, plus straight tails to the cell
    edges. The radius is half the cell's short side and the centre sits that far toward
    the corner the two lines leave by, which puts both arc ends exactly on the cell's
    centre lines — where the `─` and `│` in the neighbouring cells run. The join is
    seamless by construction, not by tuning.

  The widget also earns its keep on cost: a truecolor full-screen program gives nearly
  every cell its own color, so nothing coalesces and the old renderer built tens of
  thousands of layout nodes per frame. Backgrounds are now one backdrop quad plus one per
  non-default run, every rule the SGR set asks for — underline (single / double / dotted /
  dashed / curly), strikeout — is a quad, and ASCII runs skip shaping and font fallback
  entirely (`Shaping::Basic`). It owns the pointer too (§9, the mouse): it
  encodes and **captures** the clicks a mouse-aware program asked for, and leaves
  everything else — including every bare move — to the selection layer above it.
- **The mouse, for programs that ask for it** (`term/mouse.rs`, v2.3): a full-screen
  program turns a mouse protocol on (`ESC[?1000h` and friends) and then expects every
  click, release and — in the motion modes — every move between cells to come back as a
  short report on the input channel. The engine tracks which mode and encoding was asked for
  (`Screen::mouse_protocol_mode` / `mouse_protocol_encoding`); `term::mouse::encode` turns
  one pointer event plus that state into the bytes. Both encodings are covered: **SGR**
  (`ESC[<b;col;row M|m` — what everything modern asks for, no coordinate ceiling) and the
  classic single-byte form (`ESC[M` + three bytes biased by 32, so it cannot name a cell
  past 223 — clamped rather than wrapped), plus the UTF-8 variant in between. X10 (`?9`)
  hears presses only and predates modifier reporting, so its reports carry no modifier
  bits. The wheel is a button in this protocol: a scroll is a press of button 64/65 and is
  never released. **Holding Shift takes the pointer back** — the xterm convention — so
  text selection and cmote's own context menu are always one modifier away; a button
  already down is the exception, since its press went to the program and its release must
  too. The grid widget decides all this and **captures** a click it forwarded, which is
  what stops the selection layer above from also acting on it (§11).
- **Input**: iced keyboard events → the bytes a terminal sends (printable chars
  direct; Enter → `\r`; Ctrl-C → `0x03`; arrows/Home/End/F-keys → their escape
  sequences). Sent as `SshCommand::Input`. **F1-F12** (v2.3) follow the terminfo entry for
  the pty we request (`xterm-256color`), which is where a remote program looks them up:
  F1-F4 keep the VT100 keypad's **SS3** form (`ESC O P/Q/R/S`), F5 onward the CSI `~` form
  with its historical gaps — 15, 17, 18, 19, 20, 21, 23, 24, never 16 or 22. One wrong
  byte is a key that does nothing in every full-screen program (btop's options menu is F2,
  midnight commander lives on F1-F10). The cursor and Home/End keys honour
  **application cursor mode** (DECCKM — read from `Screen::application_cursor()`):
  when a full-screen app such as vim/less/nano sets it (`ESC[?1h`), `term::keymap`
  emits the **SS3** form (`ESC O A`) instead of the default **CSI** form (`ESC [ A`),
  which is what those apps bind their arrow keys to — without it the arrows are
  ignored and the cursor cannot move (fixed in v1.1.1). PageUp/Down/Insert/Delete
  are `~` sequences DECCKM does not alter, so they are the same in both modes. The
  **numpad number keys** (0-9 and the decimal) get special handling (fixed in
  v1.3.1): they mean a digit with NumLock on but navigation with it off, and winit
  reports the *navigation* logical key either way while filling `text` only when a
  digit was typed. iced does not surface NumLock, so `term::keymap::encode` keys off
  the **physical** code plus the presence of `text` — a numpad number key that
  produced text sends that character; otherwise it falls through to the navigation
  mapping. Without this, typing e.g. `pm2` on the numpad emitted arrow keys.
  **Modified named keys**: a Ctrl/Shift/Alt held with an arrow, Home/End, a `~`
  navigation key, or an F-key is now encoded the xterm way, where before the modifier
  was dropped on named keys. The parameter is `1 + Shift(1) + Alt(2) + Ctrl(4)` (so
  Ctrl = 5, Ctrl+Shift = 6, all three = 8), exactly what terminfo's `kRIT5`, `kf13`, …
  spell. The letter-final keys (arrows, Home `H`, End `F`, F1-F4 `P`-`S`) take the CSI
  `ESC [ 1 ; <mod> <final>` form, which **overrides DECCKM** — matching xterm, so
  Ctrl+Right is `ESC [ 1 ; 5 C` in both cursor modes — while the `~` keys insert the
  same parameter (`ESC [ <n> ; <mod> ~`, e.g. Ctrl+Delete `ESC [ 3 ; 5 ~`). Word-motion
  (Ctrl+arrow) and select-by-line (Shift+arrow) now reach a remote editor. **F13-F24**
  are mapped too, to the terminfo `kf13`…`kf24` forms — which xterm defines as the
  Shift-modified F1-F12 sequences, so they are fixed. One data-driven encoder builds it
  all: `modifier_param` computes the parameter, and `letter_key` / `tilde_key` shape the
  two key families. Caveat: the four scrollback keys — **Shift** + PageUp/PageDown/Home/End —
  are claimed by the app layer for cmote's own history (§23) before `encode`, so their
  Ctrl/Alt variants reach the shell but the Shift form is intentionally cmote's.
  **modifyOtherKeys** (xterm XTMODKEYS resource 4): the *main-keyboard* keys — letters,
  digits, punctuation — cannot carry a modifier in the classic input alphabet. Ctrl+letter
  collapses onto a C0 byte (Ctrl+I is indistinguishable from Tab), and Ctrl+digit / most
  Ctrl+symbol combos have no byte at all and are simply lost. When a remote editor turns the
  mode on it wants those combos back as the unambiguous `CSI 27 ; <mod> ; <code> ~` form
  (`<code>` the base character's codepoint, `<mod>` the same summed parameter as above). The
  engine does **not** interpret this private-CSI — it is an input-encoding hint, not a screen
  op — so cmote scans the output stream for `CSI > 4 ; p m` in a small state machine
  (`term::modkeys`, mirroring the cwd scanner), exposes the level through
  `Terminal::modify_other_keys`, and `encode` reads it: **level 2** wraps every Ctrl/Alt
  character combo (so Ctrl+C becomes the event, not the interrupt — which is what the editor
  that asked for the mode wants), **level 1** fills only the gaps (a Ctrl combo with no C0,
  leaving Ctrl+letter as its byte), and **off** (the default) changes nothing. Shift-only and
  unmodified keys, and every named/navigation/function key, keep their ordinary encoding —
  the mode governs the "other" main-keyboard keys only.
- **Paste** (done, v1.1): `term::keymap::encode_paste` turns clipboard text into input
  bytes. When the remote enabled **bracketed paste** (DECSET 2004 — read from
  `Screen::bracketed_paste()`) the text is framed by `ESC[200~`…`ESC[201~` so the shell
  inserts it literally instead of running embedded newlines. **Security**: a hostile
  clipboard could embed the `ESC[201~` terminator to close the bracket early and inject a
  command, so every occurrence is stripped from the payload before wrapping (xterm does
  the same). Without bracketing the bytes go raw — the classic terminal behaviour, where
  embedded newlines execute; bracketed paste, which modern shells enable, is the fix.
- **Resize** (done): a `window::resize_events()` subscription (Terminal screen only)
  gives the window's logical size; `ui::terminal::grid_size` converts it to `(rows,
  cols)` using the known cell metrics (minus padding, rounded down so nothing clips,
  clamped ≥ 1×1). On a *change*, `App` resizes the terminal engine **and** sends
  `SshCommand::Resize{cols,rows}` so the server reflows (`window_change`). A fresh shell
  fits immediately by fetching the current size once (`window::latest` → `window::size`)
  instead of waiting for the first resize event.
- **Scrollback** (done, §23 Stage 8): `term::SCROLLBACK` is **10 000** lines, so the engine keeps a
  bounded history above the live screen. `term::screen` offsets every cell read by the engine's
  display offset, and `Terminal::scroll` (a cmote-owned `ScrollMotion`) moves the viewport. The
  wheel scrolls it whenever no mouse-aware program wants the wheel, Shift+PageUp/PageDown page and
  Shift+Home/End jump to the ends, and every keystroke snaps back to the live bottom; new output
  leaves a scrolled-back view stationary. A thin, read-only **scroll indicator** rides the grid's
  right padding gutter while the view is scrolled up and vanishes at the live bottom; its thumb
  reports position and history depth (`screen::history_size`), sized as the viewport's share of
  the whole document with a floor so a deep history still shows a mark. Selecting across the
  scrolled view already works (extract reads the same offset the grid draws). §23 is complete.
- **Security note**: rendering untrusted server bytes is safe here — the engine
  *interprets* escapes into grid state; it never executes anything. We deliberately do
  **not** honor dangerous sequences (e.g. clipboard-write OSC 52) in v1.

---

## 10. UI (iced)

A small state machine drives the single window.

```
enum Screen  { Home, Connect, Connecting { status }, Terminal, Editor }
enum Prompt  { HostKey, HostKeyChanged, Passphrase, Interactive, Vault, Failed }   // over Connect
enum Modal   { Disconnect, NewFolder, Delete, Forwards }                           // over Terminal
```

**Five screens, not eleven (v4.0.0).** `ConfirmHostKey`, `HostKeyChanged`, `NeedPassphrase`,
`Interactive`, `VaultUnlock` and `Error` were `Screen` variants, but none of them was a screen: every
one renders `form_with_dialog(…)` — the connect FORM with a dialog over it. Calling them screens cost
a real thing. `Screen::Connect` is where the form's own keyboard ring lives (Tab / Shift+Tab / Enter,
below), and the ring was off during those six only because each of them happened to have no keyboard
subscription of its own: six places that had to remember, and no one line saying why. As
`Option<Prompt>` it is said once — `Screen::Connect if prompt.is_none()` — and `on_form_key` refuses
the same way, because iced rebuilds the subscription list only AFTER the update that opened the
prompt returns, so a key pressed in that frame still arrives (Enter would have pressed the Connect
button under a host-key dialog).

Each `Prompt` variant carries what answering it needs — the passphrase being typed, the interactive
challenge and its answers, the vault's two fields and what its unlock resumes — so the answer is read
off the thing that asked. That is also where §12 lands: the secret buffers live in the prompt, so
dismissing it drops them, and there is no buffer on the tab for a later prompt to inherit. The two
host-key variants carry nothing, because their message is already in the selectable dialog body and
their answer goes straight back down the wire (§8).

`Screen::Terminal` has the same shape one layer down — see `Modal` under "One dialog, one field"
below. Two owners, because a terminal dialog and a connect prompt can never be up together: the
screen is one or the other.

- **Connect form** (`Screen::Connect`): text inputs for host, port, user; a radio for
  the auth method (Password, Key, Interactive **or** Agent — a sum type, never more than one,
  §7), the four laid out two-by-two beside the label so they fit the column;
  a "Browse…" button (`rfd`) for the key file; a password field for password auth.
  **Interactive** (keyboard-interactive / 2FA / OTP) and **Agent** (SSH agent / Pageant) are
  both *promptless*: they show no credential field at all — the server drives every prompt, or
  the agent holds and signs with the key — so they also hide the passphrase and "Remember"
  controls, and Tab skips them. There is **no**
  passphrase field: a key's passphrase is asked for on its own screen, and only if the
  key turns out to be encrypted (see below). A Connect button; validation fails fast to
  the Error screen (§6.0). **Full keyboard navigation** (§10): iced can only focus text
  inputs, so a bespoke focus ring (`ui::connect::FormStop`) also covers the radios and
  the Connect button. `App::form_focus` tracks the current stop; a Connect-screen
  `keyboard::listen` subscription feeds `FormKey`, where **Tab / Shift+Tab** move the
  stop (`next`/`previous`), **Enter / Space** activate a radio/button stop
  (`activation`), and a text stop takes native focus (`focus(id)`) while a
  radio/button stop unfocuses all (`focus(NO_FOCUS_ID)`) and gets a highlight ring in
  the view. **Enter on a text stop submits the form** (v2.2) — iced's `text_input` has a
  submit callback only if one is wired, and none was, so Enter in a field did nothing at all
  where every other form on every platform connects. Space on a text stop is still a space:
  it is a character, and a host or user name can contain one.
- **Connecting** (`Screen::Connecting`): a status line reflecting the flow steps —
  *connecting → verifying host key → authenticating*.
- **Confirm host key** (`Prompt::HostKey`): first-contact fingerprint with
  Accept / Reject (§8), in the shared dialog chrome floating over the dimmed connect form
  (below). Closing (✕) or a backdrop click rejects — the safe default, so dismissing never
  trusts an unverified host.
- **Host key changed** (`Prompt::HostKeyChanged`, §8, §28): the mismatch override dialog, over
  the same dimmed form. Loud by design — a red "possible man-in-the-middle" line and **both**
  SHA-256 fingerprints (stored vs presented, selectable for out-of-band comparison, seeded into
  `App::dialog_body`) — with a three-button footer: **Reject** / **Trust once** / **Replace key**.
  Closing (✕), a backdrop click or Esc all reject, so dismissing never trusts a changed key. A
  changed key is never auto-trusted; each override is one explicit click (§8).
- **Need passphrase** (`Prompt::Passphrase`): shown only when the chosen private
  key is encrypted (§7). A masked field with Unlock / Cancel; the field is auto-focused
  when the screen opens (a `text_input::focus` task keyed to a shared id, refocused on
  every re-ask) so the user can type at once. A wrong passphrase re-shows the prompt
  (the session re-asks, bounded) with an "incorrect" hint — the app tracks whether an
  attempt was already made this connection, since the bridge emits the same
  `NeedPassphrase` for a first ask and a re-ask. The typed text is moved into a `Secret`
  and cleared on submit. This is a local key-file passphrase, not remote auth, so the
  hint is not a credential oracle (§12). The prompt uses the shared dialog chrome (below),
  floating over the dimmed connect form.
- **Interactive prompt** (`Prompt::Interactive`, §7): the server's keyboard-interactive
  challenge — 2FA / OTP and challenge-response. One field per prompt in the server's request,
  each masked when its `echo` flag is false (a password / OTP) and plain when true (a
  username), captioned with the server's own prompt text. The dialog's selectable body carries
  a fixed intro plus the server's optional heading/instructions, so the whole message can be
  copied. The first field is auto-focused; Enter in any field submits the whole set, which
  rides back as `SshCommand::Interactive(Vec<Secret>)`. The server can send several requests in
  a row (password, then a one-time code), so the dialog reappears until auth resolves. Answers
  are moved into `Secret`s and cleared on submit; Cancel tears the connection down. Shares the
  dialog chrome, floating over the dimmed connect form.
- **Context menus** (`ui::menu`, done — v2.0): the four right-click menus — the grid's
  Copy/Paste (§10), the home list's Open/Rename/Delete (§14), the folder tree's seven
  items (§18) and the files pane's (§19) — share one chrome, the way the dialogs share `ui::dialog`. They had drifted
  into three looks (raised buttons, flat themed buttons, transparent ones; three paddings,
  three widths, three copies of the click-away layer), so the definition now lives in one
  place: a dark rounded panel of a fixed width (set by the longest item any of them
  carries), full-width items that highlight on hover, a **dimmed** label for a disabled
  item (a transparent button gives no other signal — the folder tree's "Copy relative
  path" is disabled without a cwd), and one `dismiss_layer` taking the caller's cancel
  message. Positioning stays per-screen, because the three anchor differently: the
  pointer, a row index, the panel's right edge.
  - The home screen's menu is the one place this **deliberately overrides** that screen's
    "take every colour from the theme" rule (§14). The rule exists to stop a surface that
    themes its background but not its foreground; this panel sets *both*, so it stays
    readable in light and dark alike — and one menu that looks the same everywhere beats
    one that changes identity per screen.
- **Dialogs** (`ui::dialog`, done): the disconnect confirmation, the host-key prompt, the
  passphrase prompt, and the error notice all wear one chrome — a **header bar** with the
  question as a title on the left and a transparent **close ✕** on the right (wired to the
  safe action: cancel / reject / cancel / back, so dismissing is never the destructive
  choice), a **body** explaining what confirming will do, and a **footer** of evenly-spaced
  buttons. A single builder — `dialog(title, on_close, body, footer)` — centres the card in
  the window, so the frame changes in one place and every prompt stays consistent. The card
  has a rounded border and the header bar rounds its own top corners to match (the card's
  clip is rectangular, so a square header would otherwise poke past the radius).
  - **Placement over a backdrop**: every dialog floats over the page it belongs to, dimmed
    by the shared `dialog::backdrop` — the connect-flow dialogs (host-key, passphrase,
    error) over the connect form (`App::form_with_dialog` stacks form + backdrop + card),
    the disconnect modal over the live shell. A click on the backdrop dismisses with the
    dialog's safe action (reject / cancel / back / cancel), the same as its ✕.
  - **Overlays are always a `stack`, never a conditional root** (fixed in v2.0): a screen
    that can show overlays builds a `Vec` of layers with the page at index 0 and *always*
    wraps it in a `stack`, even when nothing is over it. iced keys each widget's internal
    state to its position in the widget tree, and `Tree::diff` discards the whole subtree
    when the root's type changes — so returning the bare page when there is no overlay and
    a `stack` when there is one reset every stateful widget underneath. It showed up as
    the folder tree (§18) and the target list (§14) scrolling back to the top whenever a
    menu or a dialog opened. Appending layers keeps the page at index 0, where
    `diff_children` preserves its state.
  - **Card swallows its own clicks**: the card is wrapped in a `mouse_area` that captures
    presses (a no-op `Message::Ignored`), so clicking the dialog does not fall through to
    the dimming backdrop and dismiss it; only a click *outside* the card reaches the
    backdrop and cancels.
  - **Selectable, copyable body**: the body message is a **read-only** `text_editor` bound
    to `App::dialog_body`, seeded when the dialog opens (the host-key body includes the
    fingerprint on its own line). The user can drag to select and copy the selection
    (Ctrl+C); `update` applies every `text_editor::Action` except an edit (`!is_edit()`), so
    the text is selectable yet never mutable. While a modal is open, `on_key`
    stops forwarding keys to the shell so Ctrl+C copies rather than sending ETX to the
    remote. `Prompt::HostKey` / `Prompt::Failed` carry no text of their own —
    the message lives in `dialog_body`, so they are bare markers.
  - **One dialog, one field** (`Tab::modal`, v4.0.0). The terminal screen can put four
    questions to the user — Disconnect, New folder, Delete, the tunnels manager — and only
    ONE at a time: they share the body buffer above and the card below, so two were never
    drawable. That was four independent fields (two bools, two `Option`s) and a convention;
    it is now `Option<Modal>`, and the convention's three holes closed with it:
    - opening one now closes whatever was up (they each used to write only their own field,
      so both cards drew, one over the other);
    - all four now take the keyboard. Three of them did not, so typing a folder name, or a
      forward's port, ALSO typed at the remote prompt — the very thing the inline rename
      fields (§18, §19) already guard against. Esc closes whichever is open, which is safe
      because none of them acts on being dismissed;
    - all four now close with the session. `pending_delete` and `new_folder` did not, so a
      delete confirmation could outlive the server whose paths it was holding and, on the
      next connect, delete them on a different machine.

    Each variant carries what answering it needs — the folder's parent and typed name, the
    paths to delete, the tunnels dialog's add form — so the answer is read off the thing
    that asked rather than off a field that might have been left over from something else.
  - **Draggable by the header** (§10): pressing the header background starts a drag
    (`DialogGrabbed`), and while dragging a transparent full-window capture layer reports
    every pointer move (`DialogDragged`) and the release (`DialogReleased`) — so tracking
    survives the pointer leaving the card.

    **The card is a module, not a field triple** (`ui::dialog::Card`). It owns the whole
    gesture — centred on open (`Card::opened`), `grab` / `drag_to` / `release` for the drag
    itself, `reflow` to pull it back when the box under it shrinks — and the box it is
    measured against is an ARGUMENT. That last point is why it exists: the arithmetic was
    written twice, once for a tab's own dialogs (`dialog_pos` / `dialog_dragging` /
    `dialog_drag_last`) and once for the App-level overlay cards (§26, §30), differing in
    nothing but whether the box was the OS window or a region. The copies were line for
    line the same, down to the comments, and each correction had to be made in both.

    The arithmetic itself is unchanged, and now stated once: the first move of a drag only
    records an anchor (a press reports where the *pointer* is, not where inside the header
    it landed — applying it as a delta would snap the card's corner to the pointer), later
    moves apply the delta, and the result is clamped — horizontally exact via the fixed
    width, vertically only far enough to keep the header on screen
    (`DIALOG_DRAG_MIN_VISIBLE`), since iced does not expose the card's real height. That
    keeps the dialog draggable to the bottom edge (and grabbable back) rather than stopping
    short of it. A release forgets the anchor too, so the next drag re-anchors instead of
    flinging the card by the distance between two gestures.

    The fields are private, so a caller holds a `Card`, hands it the pointer, and passes it
    to `dialog` — it never learns that a drag needs an anchor. The ✕ button captures its own
    press, so closing never starts a drag.
- **The copy toast** (`ui::snackbar`, v2.2): `iced::clipboard::write` is silent, and by v2.2
  a dozen surfaces write to the clipboard — both panel headers, the details card, four
  context menus. A copy that quietly did nothing looked exactly like a copy that worked, so
  every one of them now goes through `App::copy_to_clipboard`, which writes the text *and*
  raises one small card at the bottom-centre of the window: "Copied to clipboard." One
  funnel, so a new copy item cannot forget the confirmation.
  - **It dismisses itself after `SNACKBAR_DWELL` (3 s).** The state is the message plus the
    `Instant` it appeared; a `window::frames` subscription — added *only* while a toast is up
    — ticks `SnackbarTick`, and `update` clears it once the age passes the dwell. No timer
    task to cancel, no reset bug on a second copy: writing a new `Snackbar` restarts the
    clock by construction.
  - **It never takes a click.** The card is a plain `container` in a `stack` layer over the
    page, bottom-aligned with a margin, with no `mouse_area` under it — so it floats over the
    panels without swallowing a press aimed at what it covers, and it needs no dismiss button
    to get out of the way.
- **Terminal** (`Screen::Terminal`, done): a fixed-height status bar in three
  equal-width zones — **Copy / Paste** on the left, the live session's `user@host:port`
  centered, and on the right the panel toggles, a **Tunnels** button (§27, its label
  carrying the live forward count) and **Disconnect**; the terminal grid fills the rest, and
  keyboard focus goes there. Tunnels opens the port-forwards manager (`ui::forward`, a modal in
  the shared chrome, §27). Disconnect opens a
  **confirmation modal** (the shared dialog chrome — Cancel / Disconnect footer — over a
  dimming, click-away scrim) so an accidental click cannot drop a live session; confirming sends
  `SshCommand::Disconnect` and returns to the form immediately (the `Disconnected` event
  that follows just confirms it). The bar's fixed height is subtracted in
  `ui::terminal::grid_size`, so the reflow math (§9) still fits the grid exactly.
  - **Text selection + clipboard** (done, v1.1): a `mouse_area` over the grid turns
    press-drag-release into a *stream* selection (`ui::selection`), highlighted in place;
    `on_move` reports a grid-local point that `ui::terminal::cell_at` maps to a cell.
    **Copy** (button, right-click item, enabled only with a selection) extracts the
    selected cells — wide glyphs once, trailing blanks trimmed, rows joined by `\n` — and
    writes them via `iced::clipboard::write`. **Paste** (button, right-click item) reads
    `iced::clipboard::read` and sends the text to the shell. A **right-click** opens a
    small context menu (an iced `stack` overlay with a click-away dismiss layer) at the
    pointer. The selection is a *local* view over rendered cells and drives copy only:
    paste always goes to the remote's stdin at its own cursor — a terminal cannot
    "replace" a selection the way an editor can — and the highlight is kept after a paste.
    Paste wrapping/injection safety lives in `term::keymap::encode_paste` (§9). Since v2.3
    the `mouse_area` shares the pointer with the grid widget underneath it: when a remote
    program has asked for the mouse, that widget encodes the click and **captures** the
    event, and `mouse_area` — which skips a captured event — never sees it, so no selection
    starts and no menu opens. **Shift** hands the pointer back to this layer. A click that
    went to the program still moves the keyboard focus to the shell (§20) and closes any
    open menu, exactly as a click on the grid does.
  - **Copy / paste keyboard shortcuts + styled copy** (done, v3.0): copy and paste are also
    on the keyboard, taken in `on_key` *before* the key is encoded for the remote so a
    terminal binding wins over the program — the way xterm and kitty reserve theirs. Matched
    on the **physical** key, so they hold on any layout, not just QWERTY. **Ctrl+C** copies —
    but only when a selection exists; with none it falls straight through to the shell as the
    interrupt (ETX / SIGINT), and a copy then **clears the selection** so an immediate second
    Ctrl+C interrupts rather than re-copying (a stale highlight can never swallow an intended
    interrupt). Ctrl+C is a **rich** copy: `ui::richcopy` serialises the selected cells to
    HTML carrying each cell's resolved colour (through the shared `palette`, so it matches the
    grid), reverse video, faint, conceal, bold, italic, underline and strike-through, wrapped
    in one `<pre>` whose defaults are the terminal's own; `arboard` writes that HTML **and** a
    plain-text alternate together, so a paste into a rich editor keeps the look while a
    plain-text reader (and the shell) still gets the characters. iced's own `clipboard::write`
    is plain-text only, hence the dedicated backend; a failed rich write falls back to it so a
    copy is never lost. **Ctrl+Shift+C** copies the same selection as **plain text only**.
    **Ctrl+V** and **Ctrl+Shift+V** both paste plain text: a terminal takes bytes for the
    remote's stdin, so there is no styled paste to distinguish — pasting escape codes would be
    the very paste-injection the bracketed-paste strip guards against (§9). The context menu's
    and status bar's **Copy** now route through the rich path too, for one copy behaviour
    whatever the trigger.
  - **Folder tree beside the files pane** (done, v2.0; moved v4.0.0): the right end of the
    bottom browser strip holds the remote folder explorer, with a draggable splitter between
    it and the files pane and a status-bar button that hides it. Its width comes out of the
    *pane's* now, not the grid's — the terminal keeps the full width above the strip — so a
    tree resize only reshapes the pane, and only the strip's height reflows the pty (§18, §19).
- **Error** (`Prompt::Failed`): a generic, non-leaking message (selectable/copyable) plus
  a "Back" button, in the shared dialog chrome floating over the dimmed connect form.
  Closing (✕) or a backdrop click goes Back. Detail is logged, not shown (§12). It is a prompt
  over the form rather than a screen of its own because that is what it always rendered as — and
  because a failure raised from the TERMINAL screen (a dead worker channel, a session that
  dropped) has to leave the user somewhere they can retry from, which is the form.

All state is owned in the iced `State` struct; every transition is a `Message` handled
in `update`. No mutable global state, no `unsafe`.

---

## 11. Portability / config / build

"Portable" is a hard requirement: copy one `.exe`, run it anywhere, leave no trace in
the registry.

- **Initial window size**: `run` opens the window sized for a **180×40** terminal via
  `ui::terminal::window_size(cols, rows)` — the inverse of `grid_size`, built from the
  same cell metrics + padding + status-bar height so the two never drift (a round-trip
  test locks it). The user can still resize freely afterwards (§9).
- **No stray console window**: `#![windows_subsystem = "windows"]` in `main.rs` so
  launching the exe doesn't pop a black cmd window (we render our own terminal). The
  attribute is inert on macOS, where a GUI binary spawns no console. A bare binary
  double-clicked in Finder opens through Terminal; wrap it in a minimal `.app` bundle
  (`Contents/MacOS/` + `Info.plist`) for a proper Finder/Dock launch. `ponytail:` the
  bundle is a packaging step, not code — add it only when a double-clickable app is
  actually wanted; `cargo run` and terminal launch need nothing.
- **Config path resolution** (in this order):
  1. `./cmote-data/` next to the executable (`std::env::current_exe()`), if writable —
     true portable mode (USB stick, any folder).
  2. else the per-user data dir — `%LOCALAPPDATA%\cmote\` on Windows,
     `~/Library/Application Support/cmote/` on macOS — a fallback when the exe sits in a
     read-only location (`Program Files`, `/Applications`, inside a `.app`).
  `ponytail:` plain `std` (`current_exe` + a write-probe + `%LOCALAPPDATA%`/`$HOME`) for
  these paths; no `directories` crate needed.
- **Only file written**: `known_hosts`. No secrets on disk in v1 (§1, §12).
- **Release profile** (`Cargo.toml`): `opt-level = "z"` or `3`, `lto = true`,
  `codegen-units = 1`, `strip = true`, `panic = "abort"` — smaller, faster, single
  self-contained binary (the MSVC CRT links statically enough for portability on Win11;
  on macOS the binary links only `libSystem`, present on every Sequoia install, so it
  stays self-contained without bundling).
- **Build/run**: `cargo run` (dev), `cargo build --release` → `target/release/cmote.exe`
  on Windows, `target/release/cmote` on macOS. **On an Apple Silicon Mac**, add
  `--target x86_64-apple-darwin` to get the *shipped* Intel binary (it lands under
  `target/x86_64-apple-darwin/release/`): the Xcode CLT SDK carries both arch slices, so no
  extra tooling is needed to build it — only **Rosetta 2** to run it locally, since a native
  `cargo build` already gives you a runnable aarch64 binary for day-to-day work. Same split
  CI uses (§13).

---

## 12. Security

Threat model: a desktop SSH client handling the user's credentials and talking to
possibly-hostile networks. Rust removes whole bug classes (memory safety, data races)
for free; the rest is deliberate.

- **Memory safety** — no `unsafe` in our code; buffer overruns / use-after-free are
  compiler-prevented. Any future `unsafe` block must carry a `// SAFETY:` justification.
- **MITM defense (host keys)** — TOFU pinning, explicit accept, hard stop on mismatch,
  no "connect anyway" (§8). The single most important control.
- **Secrets in memory only** — passwords, passphrases, and decrypted key material are
  `Zeroizing<…>` so they're wiped on drop; nothing is persisted; nothing is logged.
  Error messages and the terminal never echo secrets.
- **No credential oracle** — auth failure returns a generic message; we don't reveal
  whether the user, password, or key was the wrong one.
- **Input validation at the boundary** — host/port/user/key-path validated before use
  (§6.0); the port is parsed as `u16`, not trusted as a string.
- **Key conversion safety** — `.ppk` conversion output is secret and treated as such;
  unsupported key types fail with a clear message, not a crash (§7).
- **Modern crypto by default** — rely on russh's default algorithm negotiation
  (current ciphers/KEX/MACs); do not hand-enable legacy/weak algorithms.
- **Supply chain** — keep the dependency tree small, **commit `Cargo.lock`** (caret
  requirements + a committed lockfile give reproducible, auditable builds; §3), and
  audit that tree in CI (done, v1.2 — `.github/workflows/ci.yml`): `cargo audit`
  (RustSec advisory DB) scans for known vulnerabilities, and `cargo deny` (config in
  `deny.toml`) enforces the license allow-list, the banned-crate list, and trusted
  sources (scoped by `deny.toml`'s `[graph] targets` to the two platforms we ship, so
  crates for targets we don't build are not judged). The two tools split the concerns so
  the advisory database is not scanned twice. This is where a Rust app's real risk lives
  — the dependency tree.
  - **Accepted advisories** (audit trail — CI ignores exactly these, so a *new* one still
    fails): **RUSTSEC-2023-0071** — the `rsa` "Marvin" timing side-channel, pulled by
    russh's `rsa` feature for RSA key auth (§7). No fixed version exists upstream; the
    attack needs precise timing of our RSA private-key operations, and there is no patch
    to take, so the risk is accepted until `rsa` ships a fix (or RSA key support is
    dropped). **RUSTSEC-2024-0436** (`paste`) and **RUSTSEC-2026-0192** (`ttf-parser`) —
    both *unmaintained* warnings on transitive iced dependencies we don't control; neither
    is a vulnerability.
- **Dependency purity vs. security (decided)** — the project is **not 100% Rust
  source**, and that is an accepted, deliberate trade: **security outranks purity**.
  Audited findings for `x86_64-pc-windows-msvc`:
  - **On `x86_64-pc-windows-msvc`, no C/C++ is compiled during our build** — `cc`,
    `cmake`, `bindgen`, `nasm`, `pkg-config` are all absent from the invoked build; only
    `cargo` + `rustc` run. **On `x86_64-apple-darwin` this differs:** `ring` compiles its
    C + assembly with `clang` from the **Xcode Command Line Tools**, so a C toolchain
    *is* invoked at build time on macOS. That is an accepted, target-specific cost of the
    same audited `ring` crypto core — not a new dependency we own — and the CLT is the
    standard prerequisite for building any Rust binary on macOS.
  - **Exactly one non-Rust-source dependency: `ring`** (crypto), pulled in by russh.
    Its source is C + assembly but ships **pre-built** for this target (hence no C
    compiler / NASM at build). We keep it on purpose: ring is BoringSSL-derived and
    is the same, heavily-audited crypto core `rustls` uses — safer than swapping in
    less-reviewed pure-Rust crypto. russh's only backends are `ring` and `aws-lc-rs`
    (also non-Rust), so a 100%-Rust SSH stack is not available today (§2).
  - `*-sys` crates in the tree (`windows-sys`, `renderdoc-sys`) are pure-Rust FFI
    *bindings* to OS libraries — no bundled C. Every native app calls the OS; that is
    the platform, not a C dependency we own.
  - **Policy going forward:** prefer pure-Rust crates; do **not** add a new C/`-sys`
    dependency (or anything that compiles C at build) without a security-grade
    justification recorded here. `deny.toml` bans re-introducing `aws-lc-rs` /
    `aws-lc-sys`, so a stray `default-features = true` on russh fails CI (§13).
- **No telemetry / no network beyond the SSH target.**
- **Least authority on disk** — the only writable artifact is `known_hosts`; portable
  mode keeps even that beside the exe.

---

## 13. Testing (AAA pattern, 80% target on logic)

Pure logic is unit-tested; anything needing a live server is integration/manual.

- **Key handling** (`ssh/keyfile.rs`): fixtures for an unencrypted OpenSSH key, an
  encrypted one (correct + wrong passphrase → error), an RSA `.ppk`, an Ed25519
  `.ppk`, and an unsupported (ECDSA) `.ppk` → clear-error path.
- **Host key** (`ssh/hostkey.rs`): known-match → accept; unknown → prompt path;
  known-mismatch → refuse. Fingerprint formatting is stable.
- **Terminal** (`term/`): feed byte fixtures through the engine and read the result back
  through the `screen::Screen` view. Deterministic, no network. `term/mod.rs` covers the
  wiring the engine swap put in cmote's hands (§23): a full-screen program's `f`-spelling
  moves land in their own cells (the engine parses HVP natively — the reason the rewriter is
  gone), a wide glyph reserves two columns, and the engine's query replies are drained and
  returned — DSR → `CSI 0 n`, DA → a `CSI ? … c`, a cursor report at the live position, the
  save/jump/report/restore size-probe reporting the clamped corner, and a query split across
  two chunks answered on completion.
- **Input mapping**: key events → correct byte sequences (Enter, Ctrl-C, arrows, and every
  F1-F12 in both cursor modes), plus the **modified named keys** — the summed modifier
  parameter, a modified arrow overriding DECCKM to the CSI form, the `~`-key parameter
  insertion, F1-F4 switching SS3→CSI when modified, F13-F24 at their terminfo forms, and a
  bare key left unchanged — and **modifyOtherKeys**: level 2 wrapping Ctrl+C / Ctrl+digit /
  Ctrl+Alt into the `CSI 27` form, level 1 encoding only the gap combos while leaving Ctrl+C
  its C0, and the mode leaving plain typing, Shift-only keys, and named keys untouched. The
  mode scanner (`term/modkeys.rs`) is tested on its own: the two set levels, both off
  spellings, another XTMODKEYS resource ignored, a split-across-chunks sequence, and an
  ordinary SGR not tripping it. Pointer events likewise (`term/mouse.rs`): each encoding,
  each mode's gating, the classic form's 223-column ceiling, the wheel, and the modifier
  bits.
- **Kitty keyboard** (§25): the encoder (`term/kitty.rs`) is tested per flag — disambiguate
  turning Esc into `CSI 27 u` and Ctrl/Alt letters into unambiguous codes while plain and
  Shift-only typing stays text; Enter/Tab/Backspace holding their legacy bytes until modified;
  the functional keys keeping their legacy final byte (Ctrl+Left `CSI 1;5D`, F1 SS3, F5 `~`);
  event types adding `:3` on a release and `:2` on a repeat, and a text key having no release
  until report-all promotes it to a code; report-all making a plain letter `CSI 97 u`;
  associated text riding along as `CSI 97;1;97u`; and alternate keys adding the shifted glyph
  `CSI 97:65;2u`. The seam (`term/screen.rs`) reads the active flags back off the engine as a
  program pushes / pops them, and `term/mod.rs` shows the engine answering the `CSI ? u` query
  now that the config flag is on. `keymap` is checked at the boundary: an active flag routes to
  the kitty encoder (superseding modifyOtherKeys), and the legacy path stays silent on a release
  and treats a repeat as a press.
- **Hyperlinks** (§24): the seam reads an **OSC 8** link back on its cells — a
  `ESC ] 8 ; ; URI BEL` opening covers the text after it and the cell past the close carries
  none (`term/screen.rs`) — and the **scheme allow-list** is tested on its own (`link.rs`):
  http/https/mailto pass (case-insensitively), `file:`/`vscode:`/`javascript:` and a
  scheme-less URI are refused, and a later colon in the path cannot smuggle an allowed scheme
  past the check. The launch itself is a side effect, so only the pure policy is unit-tested.
- **Grid geometry** (`ui/grid.rs`): the run packing (a wide glyph sealed into two columns,
  a non-ASCII one into one, runs covering every column exactly once and each starting
  where the last ended) and the drawn glyphs' maths — a braille cell read back as its dot
  pattern, a rounded corner's arc and tails measured against a real cell. No renderer
  needed for either; the part that can be wrong is the arithmetic.
- **Deferred / manual**: end-to-end connect against a local `sshd` (or a container).
  `ponytail:` no CI SSH server in v1; the manual smoke test is documented in the
  README (password + key + `.ppk` auth, TOFU first-contact, terminal I/O and resize,
  disconnect, and the host-key-mismatch hard stop).

Tests use Rust's built-in `#[test]` / `#[cfg(test)]` — no framework dependency.

**CI (done, v1.2 — `.github/workflows/ci.yml`).** Every push to `main`, and every pull
request whose *base* is `main`, runs the same gates the README asks for locally, so `main`
stays green on both targets: `cargo fmt --check` (once, platform-independent),
`cargo clippy -D warnings` + `cargo test` on **Windows** (native
`x86_64-pc-windows-msvc`) and on **macOS** (the arch split below), plus the supply-chain
audit (§12). Those two events are the only triggers — no `workflow_dispatch`, no schedule,
no tags — so a push to any other branch runs nothing until a PR targets `main`. The four
jobs are independent (no `needs:`), so a formatting failure never masks a test failure;
`concurrency` keyed on `github.ref` with `cancel-in-progress` kills a superseded run
instead of burning minutes on it; and the token is `contents: read`, the least authority
that works.

**The macOS arch split — why an aarch64 runner validates an Intel target.** GitHub's
`macos-latest` is Apple Silicon, but cmote ships `x86_64-apple-darwin` (§1). That job
therefore does two deliberately different things on two architectures:

- **Cross-compiling to Intel from an ARM Mac is a first-class Apple case, not a
  workaround** — it is how universal binaries have always been made. The Xcode Command
  Line Tools ship a macOS SDK carrying **both** the `arm64` and `x86_64` slices, `clang`
  and `ld` accept `-arch x86_64` on an M-series host, and `x86_64-apple-darwin` is a Rust
  **tier 1** target, so `rustup target add` fetches a prebuilt `std` for it. Same OS, same
  SDK, a different arch slice: no sysroot hunting, no cross-linker, no container.
- **Compiling for an architecture and running it are different asks.** Emitting x86_64
  code on an arm64 host needs nothing extra; *executing* an x86_64 binary needs **Rosetta
  2**. That is the constraint behind the asymmetry — clippy carries
  `--target x86_64-apple-darwin` because nothing is executed, while `cargo test` omits it
  and builds/runs native aarch64 binaries, sidestepping Rosetta entirely. Valid because
  the logic under test is architecture-agnostic (§16) — and that native run doubles as
  standing evidence the stack works on Apple Silicon.
- **Build scripts and proc macros always compile for the *host*.** Even under `--target`,
  cargo builds `ring`'s `build.rs` as arm64 and then has it emit x86_64 assembly via `cc`.
  That is the normal split, and it is why the cross job genuinely exercises ring's
  target-specific assembly path (§12) rather than skipping it.
- **What the cross job does *not* prove: linking.** `cargo clippy` has `cargo check`
  semantics — `--emit=metadata`, no link step — so it shows the Intel target type-checks,
  lints clean, and gets its assembly generated, but never that an Intel *binary links*. A
  symbol-level break would slip through. `ponytail:` accepted ceiling — a real link check
  costs a full codegen pass on every PR. The upgrade path is one added step,
  `cargo build --target x86_64-apple-darwin`; take it when CI starts producing Intel
  artifacts.

Only the live-SSH end-to-end path stays manual — there is still no CI SSH server. A tag
push does build and attach the portable binaries (`release.yml`, §16); the manual step left
is **publishing the draft**, which is a human review on purpose, not a gap. The artifacts are
unsigned by decision (§16), so nothing here is waiting on a signing step either.

---

## 14. Saved connection targets (v1.3)

The home screen (`ui/home.rs`) is the landing screen: a list of previously used
connection **targets**, so reconnecting is a click instead of re-typing the form.

- **What persists — profiles only, never secrets in this file (§12).** A target records
  `name`, `host`, `port`, `user`, `auth_kind`, (for key auth) `key_path` and — when the target
  presents one — the OpenSSH `cert_path` (§7), the panels' `show_hidden` preference, and a
  `remember_secret` flag. A certificate is public data like the key *path*, so it rides here;
  no password and no key passphrase is ever written to `targets.json`. This keeps the §12 "the safest secret is the one never
  persisted" guarantee for this file **and** keeps it fully portable — a `targets.json` copied
  to another machine leaks nothing. The user enters the secret on the form each time, unless it
  was remembered. *(Opt-in, PORTABLE encrypted-at-rest secret persistence now exists — a
  master-passphrase `age` vault, `secrets.age`, separate from this file; see §16. The
  `remember_secret` flag here is only the hint that such a secret can be pre-filled.)*
- **Store** (`profiles.rs`): `targets.json` in the shared data directory
  (`paths::data_dir`, the same portable-or-fallback resolution `known_hosts` uses, §11),
  serialized with `serde` / `serde_json`. A missing file means "no targets yet"; a
  corrupt file is logged and treated as empty — a broken store never blocks connecting.
- **Identity + ordering.** A target's identity is its endpoint `user@host:port`; the
  store keeps at most one target per endpoint. The list is sorted by `name`
  (case-insensitively, endpoint as the tie-breaker) and re-sorted whenever a name changes.
- **Save-on-connect.** A target is written only once a session actually opens
  (`SshEvent::Connected`), never on a mere attempt. `upsert_on_connect` adds a new target
  (named after the endpoint) or refreshes an existing endpoint's auth/key/certificate while
  keeping its custom name — so reconnecting never spawns a duplicate and never clobbers a rename.
- **Per-target display preferences.** The `.*` toggle shared by the folder tree and the
  files pane (§18, §19) is remembered with the target: whether a server's dotfiles are
  the point or the noise is a property of that server, not of the app. It is applied on
  `Connected` — before the first listing, so nothing flashes — and written back only when
  the toggle actually moves. A `targets.json` written before the field existed defaults
  to *shown*, which is what those installs already did. The **files pane's sort** (§19)
  rides the same rail: both halves — key and direction, each a tri-state — fold into the
  session snapshot, are applied on `Connected` before the first listing, and are written
  back whenever a pick moves them. Each is omitted from the JSON when unset, so an older
  file loads unsorted and behaves exactly as before. The snapshot carries the sort as an
  `Option<Option<_>>`: the outer says "the session determined it", the inner is the value,
  so a session that *cleared* its sort is remembered as cleared, distinct from "leave it".
- **Interactions** (`app.rs` + `ui/home.rs`): pick a row to **pre-fill the form**
  (host / port / user / auth / key; the secret fields start empty); **New connection**
  opens a blank form; **rename** in place via **F2** or the right-click menu (Enter
  commits and re-sorts, Esc cancels); the right-click menu also offers **Open** and
  **Delete**; **filter** the list from the box above it (§49). `Esc` on the form returns to
  the list.
- **Delete asks first.** Removing a target is not undoable, so the menu item and the
  **Delete** key only open a confirmation in the shared dialog chrome (§10) — the same
  treatment Disconnect gets. Its body names the target being removed, since the list is
  one click away from the wrong row. Cancel, the header's ✕, a click on the backdrop and
  `Esc` all emit the same cancel message, so *every* dismissal keeps the target; only the
  Delete button removes it. While the prompt is up the list's own shortcuts are inert, so
  a stray Enter cannot open a connection behind the modal.
- **Colours come from the theme, not constants (fixed in v1.3.2).** The app pins no
  theme, so iced resolves `Theme::default` from the **system** light/dark preference.
  This screen originally hard-coded a light palette, so under Windows dark mode the
  theme's near-white default text landed on a fixed light-blue selected row and the list
  was unreadable. It now takes every colour from the active theme: `text::secondary` for
  muted text, `container::bordered_box` / `button::text` for the right-click menu, and
  the extended palette's `primary.weak` **pair** for the selected row — a pair carries
  both the background and a `text` colour guaranteed readable on it, in either mode. The
  muted endpoint label is the one exception: `text::secondary` pins an absolute grey that
  ignores the row tint, so on the *selected* row the text style is left at its default
  (`color: None`) and inherits the pair's `text` from the row container instead. The
  other screens keep hard-coded colours because they always set background *and*
  foreground together, so their contrast does not depend on the system theme.
- **Optional key-passphrase pre-seed (§7).** The form gained an optional passphrase
  field under key auth. Left empty it keeps the original behavior (an encrypted key
  prompts interactively); filled, it is tried first so a known passphrase unlocks the key
  without a prompt. It is session-only — a `Secret`, never saved with the target.

---

## 15. Coding conventions — DECIDED: idiomatic Rust

**Decision (locked):** this project uses **idiomatic Rust** — `snake_case` items,
`SCREAMING_SNAKE_CASE` constants, no Hungarian prefixes, `rustfmt` defaults, and a
`clippy`-clean build. The org's C-family naming rules are treated as scoped to their
Java/C++ projects and do **not** apply here. Rationale below.

The active organization coding rules specify Hungarian/C-family naming:
`k`-prefixed **camelCase** constants (`kDefaultPort`), `v`-prefixed locals (`vScreen`),
`in`-prefixed parameters (`inHost`), `f`-prefixed struct fields, Whitesmith brace
formatting.

**These conflict with idiomatic Rust and with the compiler itself:**
- `const kDefaultPort` triggers the `non_upper_case_globals` lint (Rust wants
  `DEFAULT_PORT`).
- `fn connect(inHost: &str)` / `let vScreen` trigger `non_snake_case`.
- The rules read as authored for Java/C++ (the ruleset even has a Java-only brace
  section); they don't map onto Rust, whose `rustfmt` + `clippy` enforce the opposite.

Since this is a *learn-Rust-properly* project, forcing non-idiomatic names would teach
the wrong habits **and** produce constant compiler warnings (or require blanket
`#![allow(...)]`, which hides real lints).

**Confirmed:** idiomatic Rust wins (`snake_case` items, `SCREAMING_SNAKE` consts, no
Hungarian prefixes, `rustfmt` defaults, `clippy` clean); the org rules are scoped to
their C-family languages. `rustfmt.toml` + a `clippy` gate in CI enforce it.

---

## 16. Deferred (with upgrade paths)

- **Credential persistence (secrets at rest)** — *done (v3.0), as a PORTABLE opt-in.* Saved
  profiles carried metadata only (§14); a password / key passphrase is now optionally kept too,
  in a separate encrypted vault. The obvious store — Windows DPAPI / macOS Keychain, or an OS
  keyring — is machine-bound and would NOT travel with `cmote-data/`, against the portable-USB
  identity (§11). So instead the vault is one file, `secrets.age`, encrypted with the **`age`**
  format (scrypt KDF + XChaCha20-Poly1305) under a **master passphrase** the user chooses, which
  unlocks on any OS or machine. Off by default (the §12 "never persisted" default holds): a
  "Remember" tick on the connect form stores the secret only on a SUCCESSFUL connect (a wrong
  password is never saved), and opening a saved-secret target pre-fills the masked field after a
  one-time master-passphrase unlock. `targets.json` keeps only a `remember_secret` flag, never
  ciphertext; the decrypted secrets are `Secret` (zeroized) in memory (`vault.rs`). The
  inescapable trade: a portable key must live outside the machine — here, in the user's head —
  so a forgotten master passphrase means the secrets are gone, by design. Still deferred: the
  other `age` unlock paths (encrypt to the user's own SSH key, or a dedicated generated identity
  file), which drop into the same code path when wanted.
  - **The capture and the store are two ends of ONE attempt (v4.0.0).** The secret is captured
    when Connect is pressed with Remember ticked, and written only when `Connected` arrives — so
    everything that ends the attempt in between has to drop it. It did not. A failed
    authentication left the capture in place, and because the capture site only *wrote* the field
    when Remember was ticked, a later connect with Remember OFF inherited it: that connect then
    stored the EARLIER host's password, under the EARLIER endpoint, with nothing ticked and no
    connection to it. Fixed at both ends and locked by tests — the capture site now writes the
    field unconditionally (`None` included), and one `abandon_attempt` is called by every path
    that ends an attempt without opening a session: a dial that never left, `SshEvent::Error`,
    `Disconnected`, a cancelled credential prompt, a cancelled vault prompt, and going Home.
- **Multiple sessions / tabs** — *done (v3.0)*. Fully independent tabs (§26): each tab is a whole
  session state machine — its own screen (home / connect / a live shell), terminal, panels and
  dialogs — so one can browse the home list while another runs a shell. `App` owns a `Vec<Tab>`
  and the active index; the ONE target list and secret vault are shared (`Rc<RefCell<…>>`) so a
  rename or an unlock in any tab is seen by all. Per-tab SSH workers via
  `bridge::session_subscription(id)` (keyed `Subscription::run_with`) route each session's events
  back home. Mouse-only strip: click to switch, "+" to open, "×" to close (a live tab confirms
  first). See §26 and `ui/tabs.rs`.
- **`keyboard-interactive` auth (2FA / OTP)** — *done (v3.0)*. An explicit "Interactive"
  method on the form, plus automatic chaining into keyboard-interactive after a password/key
  attempt while the server still offers it (a fallback, or a second factor after a partial
  success — key/password **plus** an OTP). The server's prompts are shown one masked-or-plain
  field each and answered live; bounded like `MaxAuthTries`. See §7 and `ssh/auth.rs`.
- **SSH agent / Pageant auth** — *done (v3.0)*. An explicit "Agent" method on the form: a
  running agent holds the keys and signs the challenge, so cmote never sees the private key and
  there is no file to pick or passphrase to type. On Windows it tries the OpenSSH agent (named
  pipe `\\.\pipe\openssh-ssh-agent`, also via `SSH_AUTH_SOCK` when that points at a pipe) then
  Pageant; on macOS it uses `ssh-agent` via `SSH_AUTH_SOCK`. Each agent's public keys are offered
  in turn until the server accepts one, then it chains into keyboard-interactive like any other
  primary method (§7). Agent-held *certificate* identities are still deferred (the file-based
  certificate path is done — §7, §16); an agent's plain public keys are what is offered here.
  See §7 and `ssh/agent.rs`.
- **Certificate auth** — *done (v4.0.0), file-based*. An OpenSSH user certificate is presented
  alongside a key as an add-on to key auth (§7): under key auth an optional **Certificate** file
  sits beside the key file, auto-filled from the `<key>-cert.pub` sibling and clearable, loaded at
  connect time and sent via russh's `authenticate_openssh_cert`. It is remembered with the target
  as public metadata (§14), never in the secret vault. Still deferred: **agent-held certificates**
  (a certificate the SSH agent holds and signs for — russh exposes `authenticate_certificate_with`
  for the agent signer, which drops into the same auth path when wanted).
- **More key types for `.ppk`** — *done, and by a different route than first planned*:
  the original plan was a hand-rolled parser covering RSA + Ed25519 with ECDSA deferred, but
  the swap to `ssh-key`'s `from_ppk` (§7 — already in the russh tree, no new dependency) reads
  PPK v2/v3 with RSA, Ed25519, **ECDSA and DSA** inner keys, so the "ECDSA is a follow-up" gap
  no longer exists. A genuinely exotic container surfaces a clear error, not a crash.
- **SFTP / file transfer** — *partly done (v1.4, v2.0, v2.1, v2.2, v3.0)*: **upload** of one or
  many local files into a chosen remote folder, from four surfaces, with the collisions
  settled up front (§17), a **folder tree** of the remote filesystem that browses, renames,
  **creates and deletes** (§18), and a **files pane** that lists one whole directory and
  **downloads** files from it, one or a whole selection at a time (§19, §21). v3.0 filled the
  three biggest gaps: **creating** a folder (a "New folder" dialog on the tree and pane menus),
  **deleting** any entry — a folder goes with its whole subtree, behind a confirmation naming
  the targets (§18) — and **recursive directory transfers** in both directions: a local folder
  uploaded tree-and-all, a remote folder downloaded the same, each merging into an existing
  destination and asking about every colliding *file* one at a time (overwrite / keep both / skip
  this one, overwrite-all / skip-all, or cancel the lot), the per-file mirror of the flat batch's
  up-front question (§17, §19). All of it is SFTP-first with an `mkdir`/`rm -rf`/exec fallback,
  like the listings. v4.0.0 then added **cancel and resume** (§16 below, wired in §17): the status
  bar's ✕ stops the running transfer — the worker deletes the partial it was writing and drops the
  rest of the batch, since a deliberate cancel is final — and a mid-flight *failure* instead keeps
  its partial and offers a **Resume** that re-sends only the bytes still missing (a byte-offset
  append for a single file; a whole tree re-walked and size-compared so only the gaps and the
  interrupted file's tail cross again). v4.0.0 also began **preserving file metadata** across a copy
  (§17, §19): every finished file is stamped, best-effort, with the source's **modification time**
  — the one attribute meaningful in the everyday Windows case, where the client neither has a Unix
  mode to send nor can apply one — and, when both ends are Unix, its **permission bits** too (so a
  script keeps its `+x`). It never fails a transfer: a server that refuses `setstat` or a filesystem
  that will not take the timestamp is logged and the bytes stand. v4.0.0 then carried resume **across
  a dropped connection** (§16 below): a session that dies under a transfer hands the resume point to
  the tab rather than losing it with the queue, and the next session **to that same endpoint** offers
  to finish it — which matters because the link itself is the commonest reason a big transfer stops,
  and it was the one reason cmote could not offer to pick up from. Still deferred: aiming a drop at a
  PARTICULAR folder — the
  gesture itself now takes any number of files or a whole folder (§29, v4.0.0), but iced's drop
  events carry no pointer position, so every drop lands in the pane's own directory — two transfers
  at once (a batch queues instead, §17, §21), and preserving the *access* time as its own attribute
  (SFTP couples it with mtime, so an upload sends the pair but does not treat atime as a goal).
  v4.0.0 also began **following symlinks** inside a recursive walk (§17): a link is copied as what
  it points at, and only a link that leads back up its own tree — or nowhere at all — is counted and
  left, so a cycle still cannot loop the transfer.
- **Port forwarding (local/remote/dynamic)** — *done (v3.0.0)*. All three — `-L` local, `-R`
  remote, `-D` dynamic (a SOCKS5 proxy) — run over the live connection, managed from a **Tunnels**
  dialog on the status bar and remembered per target so a reconnect re-establishes them (§27). v4.0.0
  then added the **server-assigned remote port** (`-R 0`): a remote forward may bind port 0, the
  server picks a free port, and the row shows the port it chose while the saved spec keeps 0 so a
  reconnect asks afresh (§27). v4.0.0 also made the dialog a **live monitor** — each active row shows
  `N open · M total`, the connections crossing the tunnel now and in all, driven from the byte pumps
  themselves (§27). v4.0.0 then closed the last gap by accepting **bracketed-IPv6 addresses** on either
  side — `[::1]:8080`, exactly as a URL or OpenSSH writes them — with an unbracketed IPv6 refused with
  a message pointing at the bracket form rather than mis-split on its last colon (§27). That was the
  last port-forward follow-up, so the chapter is complete.
- **Richer terminal** — *the engine swap (§23) raised the ceiling* (v3.0): `vt100` was
  replaced by `alacritty_terminal`, so the DEC line-drawing charset, origin-mode-correct
  cursor reports, custom tab stops, the autowrap toggle and the host's status/identity
  queries (DSR/DA/DECRQM) are handled by the engine, not papered over — a full-screen program
  renders and no longer stalls on a probe. All of the rich SGR attributes the engine tracks
  are **rendered** as of v3.0 (§23 Stage 3): dim, italic, strikethrough, conceal, the underline
  *styles* (double / dotted / dashed / curly) and underline colour — italic drawn from a
  bundled IBM Plex Mono face, since Fira Mono ships none (Stage 3b). The terminal also now
  **answers the host's colour and pixel-size queries** (OSC 10/11/12/4, CSI 14t) from its own
  colour scheme and cell metrics (§23), so a program that probes the background to pick a theme
  is answered rather than left guessing, and the **window title** a program sets (OSC 0/2) is
  shown in the title bar (§23). The **cursor shape** a program picks with DECSCUSR
  (`CSI Ps SP q`) is now drawn too (§23 Stage 6): block (the default, an inverted cell),
  underline, bar, or the hollow-block outline — steady, since cmote runs no animation timer, so
  blink is dropped. Focus reporting (DECSET `?1004`) is answered too (§23 Stage 7): a program that
  turns it on hears `CSI I` / `CSI O` when the shell gains or loses focus. And **scrollback** is now
  on (§23 Stage 8): `SCROLLBACK = 10 000`, the wheel and Shift+PageUp/PageDown/Home/End scroll the
  history, typing snaps back to the live bottom, and a thin, read-only **scroll indicator** in the
  grid's right gutter shows position and depth while the view is scrolled up. That was the last §23
  follow-up, so §23 (the engine swap and everything it unblocked) is complete. Two further
terminal features then shipped on top of the swap, each a small addition beside the engine
rather than a change to it: **OSC 8 hyperlinks** (§24) — the engine records the
per-cell URI and cmote follows it on Ctrl+click (revealing it first with a **Ctrl-hover underline**
over the link's run) or a right-click **Open link / Copy link**,
the scheme gated to http/https/mailto — and the **kitty keyboard protocol** (§25) —
the engine tracks the push/pop/query flag stack and answers `CSI ? u` itself, cmote flips the
engine flag on and encodes the `CSI u` key reports (disambiguate, press / repeat / release,
report-all, associated text), superseding modifyOtherKeys when an editor enables it. The full audited
  inventory of what the terminal still lacks to drive *any* documented app UX — rewritten
  against the `alacritty_terminal` baseline now that the swap is done, with each remaining gap
  grouped by *where the work lives* (`[keymap]`, `[reply]`, `[seam+grid]`, or the short
  `[engine-limit]` ceiling), grounded in ECMA-48 / the DEC VT manuals / xterm `ctlseqs`, with a
  `file:line` evidence appendix — lives in
  [`TERMINAL_COMPATIBILITY_PLAN.md`](TERMINAL_COMPATIBILITY_PLAN.md).
- **Clipboard: mouse selection + copy + bracketed paste** — *done (v1.1), extended (v3.0)*:
  stream selection with copy, and bracketed paste with the injection-terminator scrub (§9-§10).
  v3.0 added **copy/paste keyboard shortcuts** (Ctrl+C / Ctrl+Shift+C / Ctrl+V / Ctrl+Shift+V,
  physical-key matched) and made **Ctrl+C a styled (HTML) copy** via `ui::richcopy` + `arboard`,
  with a plain-text alternate alongside (§10). Still deferred: honoring remote **OSC 52**
  clipboard-write requests (kept out on purpose — we only touch the clipboard on explicit local
  action) and rectangular/block selection.
- **Host-key mismatch override UI** — *done (v3.0.0)*. A guarded "the key changed, here's the old
  vs new fingerprint" flow: §8's TOFU refused a changed key outright; §28 now blocks the handshake
  on a loud, reject-by-default dialog showing both SHA-256 fingerprints, with **Reject** /
  **Trust once** / **Replace key**. Never auto-trusted — every override is one explicit, informed
  click (§28).
- **Release pipeline** — *done (v4.0.0)*. A tag-triggered `release.yml` sits beside `ci.yml`
  in `.github/workflows/`: a bare `MAJOR.MINOR.PATCH` tag (the repo's convention — `2.3.0`,
  not `v2.3.0`) builds the optimized binary on both targets, packages each the platform way
  — a portable `cmote.exe`, a zipped Finder-launchable `cmote.app` (`bundle-macos.sh`, whose
  `BIN` the workflow overrides at the cross-compiled Intel binary since the runner is Apple
  Silicon) — checksums them into `SHA256SUMS`, and attaches the lot to a **draft** GitHub
  Release for a human to review and publish. A manual `workflow_dispatch` run builds both
  targets *without* publishing, to exercise the pipeline before cutting a tag. The publish
  job is the only one granted `contents: write`; the builds stay read-only.
- **Code signing + auto-update** — **decided: NO, not deferred.** This used to read "still
  deferred", which kept it on every remaining-work list as a thing about to happen. It is not:
  cmote will not be Authenticode-signed, `codesign`ed or notarized, and there will be no update
  channel, until that decision is explicitly reversed. Three reasons, worth writing down so the
  question stops being reopened:
  - **A certificate would not buy what it looks like it buys.** Win11 SmartScreen warns on
    *reputation*, not on the presence of a signature, so an ordinary OV certificate leaves the
    first-download prompt exactly where it is — only an EV certificate skips it, and only until
    each new binary has been downloaded enough times. The prompt one pays to remove would still
    be there.
  - **The signing key is a new high-value secret, and it would live in CI.** The release builds
    from a committed lockfile on a runner nobody logs into; adding a key that can vouch for "this
    is cmote" turns that pipeline into something worth attacking, and gives it an authority the
    project does not otherwise have. A yearly-renewed identity-bound certificate is also the one
    part of the build that cannot be reproduced from this repository.
  - **Auto-update rides on signing, and cmote does not want the update channel either.** An
    updater is a program that downloads and runs code on the user's machine on its own initiative
    — for a portable exe carried on a USB stick (§11) that is the wrong shape entirely. The update
    path is: download the next release yourself.

  What stands in its place: the release attaches **`SHA256SUMS`**, and that is the sanctioned
  integrity check — verify the download against it (`sha256sum -c SHA256SUMS`, or `shasum -a 256
  -c SHA256SUMS`). The cost is honest and stays: a fresh download trips SmartScreen ("More info"
  → "Run anyway") and macOS Gatekeeper (right-click → **Open** the first time). Reversing this
  means saying so explicitly; nothing else here is waiting on it.
- **GNU toolchain build** — only if a fully MSVC-CRT-free static exe is ever required.
- **Apple Silicon (`aarch64-apple-darwin`) build** — the whole stack is
  architecture-agnostic; add the target (and a universal binary via `lipo`) when an ARM
  Mac needs it. v1 targets Intel Sequoia as asked. Note that CI already runs the full test
  suite *natively on an aarch64 runner* (§13), so ARM portability is continuously proven —
  what is missing is a shipped artifact, not the port.

---

## 17. Remote working directory + file upload (v1.4)

Two features that only make sense together: the status bar gained a **file picker** and
an **Upload** button, and the upload's default destination is *the directory the shell is
currently in*. SSH does not offer that directory, so cmote has to learn it. (v2.2 turns the
upload itself into a multi-file batch into a chosen folder, startable from four surfaces —
see "One file or many" below; the cwd tracking here is unchanged.)

### Tracking the remote cwd (`term/cwd.rs`)

The cwd belongs to a process on the far side; the protocol carries bytes, not state. Every
terminal that displays the remote directory (VS Code, iTerm2, WezTerm, Windows Terminal)
solves it the same way, and so does cmote: **the shell announces its directory in an OSC
escape sequence on each prompt, and the terminal reads it out of the output stream.**

- **Two conventions, both read.** `OSC 7` — `ESC ] 7 ; file://host/path ST` — is the POSIX
  one (fish emits it out of the box; zsh/bash do with shell integration). `OSC 9;9` —
  `ESC ] 9 ; 9 ; C:\path ST` — is the Windows one. Reading both is what makes the tracker
  OS-agnostic. The OSC 7 URI is percent-decoded, its authority dropped, and a Windows
  `/C:/…` loses its URI slash.
- **A scanner, not a buffer + regex.** Output arrives in arbitrary chunks and a sequence
  can be split anywhere — including between the `ESC` and the `]`. `Cwd` is therefore a
  four-state machine fed every byte, carrying its state across chunks. A payload longer
  than 4 KiB is abandoned rather than buffered, so a hostile stream cannot grow our memory
  (§12). Non-cwd OSC sequences (titles, clipboard writes) pass through and leave the last
  known path alone.
- **Where it runs.** `Terminal::process` feeds every raw byte to the tracker and to the engine
  alike. The engine ignores OSC codes it does not know, so the cwd sequence passes through it
  untouched and the two never disagree. The path is exposed as `Terminal::cwd`.
- **No shell hook — passive reading only.** cmote types **nothing** into the remote shell. A
  shell that announces its own directory is followed for free (fish emits OSC 7 out of the box, a
  Windows shell emits OSC 9;9); a plain bash/zsh with no shell integration stays silent, so its
  cwd is simply unknown and the upload dialog asks for a path. An earlier build installed an
  announcer by typing one line into the shell — but that line is unavoidably recorded in the
  remote's command history (readline/zle log every submitted line; hiding its on-screen echo does
  not stop the record), and it is bash syntax a shell like fish would choke on. Rather than
  pollute the user's shell, cmote leaves it untouched and accepts an unknown cwd on a silent
  bash/zsh. The way OUT of that on a bash/zsh remote is the dialog below, which writes the hook
  into the shell's own config file instead of typing it at a prompt (v4.0.0).
- **Shown in the window title.** `App::title` is a function of the state:
  `cmote — user@host:port — /current/dir` while connected, dropping the third part when the
  shell never announces one. When a program sets its own window title (OSC 0/2, §23) that takes
  the third slot instead of the cwd — the endpoint always stays, so the window is still
  identifiable by host even while a program owns the title. The title costs no grid space,
  which the status bar would.

### Shell integration, written to the config rather than typed (`integration.rs`, v4.0.0)

Passive reading is the right rule and it has one consequence nobody enjoys: on a plain bash — a
Rocky/CentOS/Amazon box, which is most fleets — the cwd is never known, so the title has no
directory, Sync and Reveal are permanently dimmed, the upload dialog asks for a path, and §22's
reconnect resume has no `terminal_path` to remember. Nothing announces that this is a *missing
shell hook* rather than a broken feature. That is what turned up in use: `targets.json` had
`files_path`, the panel sizes and the sort for every saved target, and `terminal_path` for exactly
one — an old entry from before the typed hook was removed.

The fix does not reverse the rule. cmote still types nothing. It offers to write the announcer into
the **shell's own config file**, over SFTP, once, from a dialog — the same thing every terminal with
"shell integration" offers, and the same thing the user would otherwise do by hand.

- **Terminal right-click → "Shell integration…".** A once-per-server act belongs in a menu, not on
  the status bar beside the per-moment buttons. The dimmed Sync/Reveal pair is the tell that sends
  people looking.
- **It reads before it writes, and shows what it would write.** The probe resolves the login
  account's home directory over SFTP, reads `/etc/passwd` for its login shell (the authoritative
  answer, and one obtainable without typing at a prompt), falls back to whichever of `.zshrc` /
  `.bashrc` exists for an LDAP/SSSD account that is not in the file, and reports whether cmote's
  block is already there. The dialog then shows **the exact block** in its selectable body. This is
  a change to a file every future login of theirs reads, on a machine cmote does not own; the honest
  way to ask for that is to put the text in front of them — and it doubles as the answer for anyone
  who would rather paste it in themselves.
- **The LOGIN account, never the elevated one.** The config being written is the one the shell a
  reconnect opens will read. Installing into root's `.bashrc` because the panes happen to be
  elevated would do nothing for the session that asked.
- **Marker-bounded, so it can be removed.** `# >>> cmote shell integration >>>` … `# <<<`. The
  marker is the whole bookkeeping: its presence is "already installed" (installing twice would
  announce the directory twice per prompt), and removal cuts exactly what installation added,
  blank line included, leaving anything written after it alone. A block whose closing marker
  someone deleted by hand is left ALONE rather than truncated to the end of the file — the user's
  own lines may be under it.
- **`.bashrc`, not `.bash_profile`.** cmote's shell is a login shell, which reads only the profile
  — but every mainstream distribution's default profile sources `.bashrc`, and `.bashrc` is also
  what a non-login interactive shell reads, so it is the file that covers both. On an account whose
  profile does not source it, nothing happens; the dialog names the file it wrote, so that reads as
  "installed, still silent" rather than as a mystery.
- **Written atomically** through `edit::write_atomic` — temp sibling, then rename. A config file is
  the one file on a server where a half-written copy costs the user their way back in.
- **What goes in.** OSC 7 on every prompt, plus OSC 133;D (with `$?`) and 133;A, which light up
  §34's prompt ticks, jump-to-prompt and the per-tab exit-code glyph for free. zsh also gets 133;C
  from `preexec`. bash does **not**: 133;C needs a global `DEBUG` trap, a single slot every preexec
  framework wants, and cmote will not take it silently. The cost is bounded and known — no
  "running" dot, and Ctrl+Shift+O finds no output span, because a command with no output start is
  filed as an empty range.
- **fish is recognised and left alone.** It announces its own directory; offering to fix a shell
  that is not broken would be a lie the user pays for with a change to their config. An
  unrecognised login shell (`ksh`, a bare `sh`) is reported as "could not tell" and offered
  nothing — writing bash syntax into a ksh rc file is how an account loses its login.
- **BEL, not ST.** The sequences end `\007`. Not taste: writing ST means `\\` in the `printf`
  format, and `\\` immediately followed by `\033` does not come back out of bash's `printf` as
  backslash-then-escape — the backslashes are eaten together and the next sequence is emitted as
  the literal text `033]7;…`. Found by *running* the block, not by reading it. A test asserts no ST
  survives in either block.
- **Nothing changes in the session that asked.** A shell reads its config when it starts, and this
  one already has. The dialog says so rather than leaving the user to wonder why the title is still
  bare.
- **`ponytail:`** the path in the OSC 7 URI is not percent-encoded — encoding it in portable shell
  is a per-character loop on every prompt. cmote's own reader takes a raw path fine; the case it
  gets wrong is a directory name containing a literal `%` followed by two hex digits.
- **`ponytail:`** bash's `PROMPT_COMMAND` is prepended as a STRING. On bash 5.1+ it can be an
  array, and an account that has deliberately made it one loses the other entries. The guard
  (`case`) makes re-sourcing safe, which is the common hazard; the array case is rare enough to
  disclose rather than branch on.

### Uploading (`ssh/upload.rs`)

- **Its own channel, not the pty.** The shell channel is for keystrokes: everything sent
  is echoed, binary needs encoding, and the terminal would have to render the transfer.
  The upload therefore opens a **second channel running the sftp subsystem** (`russh-sftp`
  over russh's `ChannelStream`), which is real file transfer — the shell keeps running
  untouched beside it. Opening happens inline in the session loop (a borrow of the session
  handle); the transfer itself is **spawned**, so a large file never stalls the shell pump.
- **One file or many, into a folder.** The picker is multi-select (`pick_files`), and the
  confirmation shows an **editable destination FOLDER** with the picked files listed under
  it — each keeps its own name inside that folder. An empty folder normalises to `.`, which
  the server resolves against the login directory, so a shell that never announced its cwd
  still has somewhere to send to. One dialog, one shape, whether one file or twenty.
- **Four ways in, one flow (v2.2).** Upload starts from the status bar (File… picks, Upload
  confirms into the shell's cwd) or from a right-click **Upload…** on any of three surfaces,
  each seeding the destination folder before the picker even opens: the **terminal grid**
  (the shell's cwd), the **files pane's empty space** (the directory it is showing), and a
  **folder in the tree** (that folder itself). All converge on the same confirm-then-send
  flow — the surface only decides the starting folder.
- **Pre-scan, ask once (v2.2).** Before a byte is sent the batch is checked against the
  destination with `CheckUploads` → `UploadPrescan`: the server stats every name and, for
  each that already exists, proposes a free `name-1` path. If any clash, one dialog asks the
  whole-batch question — **Replace / Skip / Keep both / Cancel** — the mirror of the
  multi-file download's collision model (§21), not a prompt per file. `Replace` overwrites in
  place, `Skip` drops the clashing files, `Keep both` writes them to the proposed `-1` path,
  `Cancel` sends nothing. Checking before creating matters because SFTP's create truncates
  (§21): a late failure would already have destroyed the old contents.
- **A queue, one transfer at a time.** The confirmed batch becomes a `VecDeque` (`plan_uploads`
  builds it — pure, so the collision-answer logic is unit-tested without a server), and each
  landing pumps the next, exactly as the download queue does (§21). The status bar's
  centre zone is a progress bar per file; the closing notice is `Uploaded N files` for a
  batch, `Uploaded to <path>` for a lone one. A race — a file that appears on the server
  after the pre-scan — is skipped rather than reopening the question mid-batch.
- **All of it is one module (v4.0.0): `transfer::Queue`.** Uploads, downloads, whole folders and
  drops are ONE feature with many entrances, and the rule they share — a single transfer at a time,
  because there is one progress bar — used to be spelled out at each entrance, slightly differently
  every time. `Queue` states it once, as `busy()`, and owns everything behind it: the batch being
  set up, the three queues (files, folders, downloads), the slot, whichever question is open, the
  resume point and the last notice. Every field is private, so a caller says what the USER DID and
  never has to know that a folder queue drains after a file queue, or that a resume point goes stale
  the moment a fresh transfer starts.
  - It reaches for nothing — no SSH channel, no dialog buffer, no panels. Each call returns
    `transfer::Effects` (commands to send, a dialog body to seed, a folder to re-list, whether the
    destination field takes the keyboard) and `Tab::apply` carries it out. That is what makes every
    rule in here testable with no session, no window and no server.
  - The six SSH events that can end a transfer collapse into one `ended(Ended)` — `Done`, `Failed`,
    `Interrupted`, `Skipped` — because which DIRECTION the thing in the slot was going is already
    remembered (`in_flight`), so it need not be asked. Same for the two collision dialogs: only one
    can be open, so one `answer_clash` serves both.
  - A session ending is `reset()`, one call. The dozen hand-written clears it replaced missed six
    fields — including the resume point, which meant a Resume button could survive a disconnect and
    relaunch a transfer against whatever server the tab connected to next (§16).
- **Progress.** The copy loop streams 32 KiB chunks and emits `TransferProgress` every
  256 KiB — enough for a smooth bar, far below the flood that per-chunk events would be.
- **Failures stay in the bar.** A failure shows its reason in the status bar, never the error
  *screen* — that would tear down a healthy shell over a file that never left. Unlike an auth
  failure (§12), the detail here is the user's own path, so showing it is what makes the error
  actionable. A failure *before any bytes move* (the sftp channel would not open, the source would
  not read, **the destination refused to be created** — a folder this account cannot write to) has
  nothing to resume, so the queue moves to the next file as it always did; a failure *mid-copy*
  keeps its partial and pauses the batch for a **Resume** instead (§16 cancel/resume, below).
- **Success clears the batch.** Once the queue drains, the picked files are cleared, which
  disables the Upload button, so a stray click cannot re-send what just landed. The reported
  destination is the server's `canonicalize` of the path, so the user sees where the bytes
  actually went rather than what they typed.
- **Keyboard.** While a confirmation or the collision question is open the terminal's key
  listener swallows keys (as it does for the Disconnect modal), so typing goes to the folder
  field and not the remote shell; `Esc` cancels the *confirmation*. A running transfer still
  swallows `Esc` — stopping one is the status bar's ✕, a deliberate press rather than a stray key
  (§16 cancel/resume, below).

### Recursive folder transfer (v3.0)

*Upload folder…* and *Download folder…* move a whole directory **tree** — the local one recreated
on the server, or the remote one recreated on this machine. Both directions share one spine
(`ssh/transfer.rs`) and differ only in which filesystem they read and which they write:

- **Walk, then copy.** The source tree is walked into a plan — the directories (parents before
  children) and the files with their sizes, so the one progress bar has a real total — held in
  memory before a byte moves (`ponytail:` fine for an ordinary folder, felt for one of millions;
  the upgrade path is to stream the walk and the copy together, the way the pane's listing already
  batches, §19). The walk is **iterative, not recursive**, so a deep tree costs heap not stack.
  Missing destination directories are created; existing ones are merged into.
- **Symlinks are followed** *(v4.0.0)*. "Send this folder" means send what is in it, so a link to a
  file copies the file's bytes and a link to a folder copies that folder's contents: the far side
  gets real files and real directories, which is what `cp -L` / `rsync -L` produce. Nothing writes a
  *link* on the destination — a link's target is a path on the SOURCE machine and would point at
  nothing over there. What that costs is one extra call per symlink and none at all for a tree
  without any: a `stat` (the listing's own attributes come from `lstat`, so they describe the link,
  not what it points at), plus a `realpath` for a link to a directory.
  - **The cycle is the only real danger**, and one rule settles it (`transfer::loops_back`, unit
    tested): a link is followed unless its **canonical target is the directory holding it or one
    above it**, because walking in there would come straight back out through the same link,
    forever. Each frontier item therefore carries the canonical path of the directory it is — free
    for a real subdirectory (its parent's canonical path plus its name), asked of the filesystem
    only when a link was followed to get there. The test compares by **path component**, not text,
    so `/home/ab` does not read as sitting inside `/home/a`.
  - **A link sideways is followed** even though the folder it reaches may already be in the tree,
    so its content is copied twice. That is what `cp -L` does, it ends, and a user can see it and
    delete one — a walk that never returns is none of those things.
  - **A link that leads nowhere is counted, not fatal**: a dangling target, a target this account
    cannot stat, or the cycle above. One bad link in a corner of a tree must not lose the other ten
    thousand files, so the transfer finishes and reports how many it could not follow.
  - **The shell fallback delegates it** (§46): `find -L` for all three of its runs. `-L` also
    changes what `-type l` still matches — a link that resolves has become its target, so only the
    dangling ones are left, which is exactly the count wanted. `ponytail:` the cycle is then GNU
    find's check (it prints "File system loop detected" and refuses to descend, which is why stderr
    is dropped from those runs), not ours; a `find` without it would spin. The SFTP path, which is
    what runs unless the server has no sftp subsystem, decides for itself.
- **Per-file collisions, mid-transfer.** A tree cannot be pre-scanned into one list a user would
  read, so each file whose destination is already taken **parks the transfer and asks** — the
  spawned transfer sends `SshEvent::TransferConflict` and awaits a `ResolveConflict` reply on a
  channel `client::run` forwards to it, the same request/await shape auth uses for a passphrase
  (§7); the shell keeps flowing behind the prompt. The six answers are a file manager's: *Overwrite*
  / *Keep both* (a `-1` copy) / *Skip* settle just this file, *Overwrite all* / *Skip all* set a
  sticky policy that settles every later collision without asking again, and *Cancel* stops the
  whole transfer — files already copied stay. This is the per-file mirror of the flat batch's
  up-front question above; the flat paths are untouched.
- **One "keep both" rule, in one place** (`explorer::free_candidate` + `explorer::FREE_NAME_TRIES`).
  `notes.txt` becomes `notes-1.txt` — the number before the extension, so the copy still opens in
  the same program; `.bashrc` becomes `.bashrc-1`, because a dot-file is all name and has no
  extension to keep; `archive.tar.gz` becomes `archive.tar-1.gz`, because only the last dot is an
  extension. A hundred tries, then the last candidate is returned unprobed — the create that follows
  re-checks, so the worst a hundred-deep collision costs is one skipped file, never an overwrite.
  Worth stating because it was written **five** times before it was written once: the SFTP upload,
  the SFTP download, both shell-backend halves, and the queue's own local "save alongside". Three
  spellings, two caps (one of them a bare `1000` on the backend whose probes are the DEAREST), and
  three different answers when the tries ran out — one of which handed back the path it had just
  been told was occupied. It lives in `explorer` beside `join` and `name` rather than in either
  transfer module because both of those callers must reach it and neither may depend on the other:
  the queue deliberately knows nothing of `ssh::`.
- **The progress bar's arithmetic is one type** (`ssh::transfer::Ticker`), not six copies of a
  five-line gate and a pair of `&mut u64` counters threaded through every copy loop. It answers one
  question — has a whole `PROGRESS_STEP` gone by since anyone was last told — and it makes the two
  cases that move no bytes explicit rather than incidental: a file **skipped** whole and a resume's
  **carry-in** are both counted AND announced at once, so neither stalls the bar nor leaves a report
  owing. That last part is the bug the tests now pin: the counters it replaced started *both* at the
  resume offset, and a ticker that carried the bytes without also marking them reported would make
  the first chunk of every resumed file fire an event.

### Cancel and resume (v4.0.0)

A running transfer can be **stopped** (the status bar's ✕) and, after a mid-flight *failure*, can be
**resumed** (a Resume beside the failure notice). The two are deliberately different endings:

- **Cancel is final.** The ✕ sends `SshCommand::CancelTransfer`; `client::run` sets a per-transfer
  `Arc<AtomicBool>` it holds beside the collision channel — one transfer at a time, so each start
  makes a fresh flag and keeps a clone. Every copy loop (single file and each file of a tree) polls
  it between 32 KiB chunks; on seeing it set the loop **deletes the partial it was writing** and
  returns a neutral *cancelled* outcome. The GUI empties both queues on the click, so cancelling
  the running file takes the rest of the batch with it. A cancel is not a failure — it reports
  through the usual `*Failed` event with a "cancelled" message, so the bar stays calm and offers no
  resume: the partial is gone.
- **Resume continues a failure.** A copy that *errors* mid-flight (not a cancel) keeps its partial
  and reports `SshEvent::TransferInterrupted`; the queue remembers what it launched (`in_flight` →
  `resumable`, both private to `transfer::Queue`) and shows Resume. It is also the only thing that
  survives a stop, which is why a session ending has to clear it — see `reset()` under §17.
  Resume re-issues the exact command with `resume` set. The task then
  **sizes the destination and sends only the bytes still missing**: `resume_start` (pure, unit-tested
  in `ssh/transfer.rs`) compares the destination's current size to the source's — equal or larger is
  *skip* (already there), smaller is *append from there*, absent is a fresh send. A single file opens
  its destination without truncating and seeks both ends to the offset; a tree re-walks and
  size-compares every file, so only the gaps and the interrupted file's tail cross again — and in
  resume mode an existing destination is the transfer's own earlier work, never a collision, so it
  never re-prompts. This is size-based, trusting the existing prefix to be the file's own (`ponytail:`
  no checksum — the same assumption `curl -C -` and rsync's naive mode make). A batch resumes its
  failed file, then drains the rest as usual.
- **A refusal is not an interruption.** Uploading into a folder the account cannot write to fails at
  the `create`, before a byte exists on the far side — so there is no partial, and a Resume would run
  that very same refused create again. The few places that make or open a destination (both
  backends, both directions: the file `create`/open, the tree's `mkdir`) mark their failure with
  `transfer::refused`, an error wrapper the reporting end reads back with `transfer::was_refused`.
  A marked failure reports as a plain `*Failed` — the server's own reason in the status bar, the
  queue behind it moving on, no Resume — and the mark rides under any context added on the way up,
  so it cannot be lost between the refusal and the report. Everything past the line that opened the
  destination stays resumable, because past that line there is somewhere for bytes to survive.

### Resuming across a dropped connection (v4.0.0)

Cancel and resume above both lived inside ONE session: a connection that died took the tab down to
the error screen, and `reset()` cleared the resume point along with everything else. Which left the
commonest reason a big transfer stops — the link — as the one reason cmote could not offer to
continue from. The bytes were still on the far side; only the memory of them was gone.

- **The mechanism never cared which connection carried it.** A resume re-issues an absolute command
  with `resume` set, and the task sizes the destination before it sends a byte (`resume_start`,
  above). Nothing in it refers to the session that started it. So this is a memory feature, not a
  transfer one: `transfer::Unfinished` is what a dying session hands over, and the two calls that
  move it — `Queue::abandon` on the way out, `Queue::adopt` on the way in — are the whole of it.
- **A dropped connection is not a cancel, and that is the whole justification.** A cancel deletes
  the partial it was writing, deliberately, so there is nothing to continue; a connection dying
  deletes nothing at either end (the copy loops only `remove_file` on the cancel flag), so the
  partial is exactly as good as the one a mid-flight failure leaves. A cancelled transfer therefore
  hands over nothing, because `cancel` has already cleared both slots — unit-tested, since "the
  cancel I asked for came back on the next connection" would be the worst version of this feature.
- **What was on the wire outranks an older parked offer.** Either can be what a session dies
  holding: a transfer still running, or one that failed and was waiting on a Resume the user had
  not pressed yet. The running one wins — its partial was growing a moment ago.
- **The endpoint travels with it, and is matched on adoption.** Both paths belong to one machine and
  one account, and the partial to append to is on it. `Unfinished` therefore names the endpoint it
  was made on, and a session that opens somewhere else is offered nothing, silently. The offer is
  **spent by the first session either way** — taken and then matched, exactly as a duplicate's
  carried directory is (§52) — because a resume point that waited through a session on another
  machine is one nobody remembers making.
- **It lives on the `Tab`, outside the queue**, and is the ONE thing `clear_grid_interaction` does
  not clear — that function runs on the way INTO a session as well as out of one, and this is meant
  to survive exactly that crossing. Every teardown sets it through `Tab::abandon_transfers` (remote
  hangup, session failure, confirmed Disconnect, dead worker channel), which must run *before*
  `connection` is cleared, since that endpoint is the whole guard.
- **A deliberate Disconnect keeps the offer too.** The ✕ beside the progress bar is how a transfer
  is *cancelled*; leaving the server is not that, and it leaves the partial there either way. An
  offer to finish it beats a half file on a server with nothing said about it.
- **The notice comes with the offer.** A Resume button appearing on a freshly opened session would
  otherwise offer to finish something without saying what — so the bar reads
  `<name> stopped when the connection dropped`, the source's own name in both directions (a
  destination may have been renamed to a `-1` copy on the way, §17, §21).
- **It is deliberately NOT persisted with the target** (§22). A partial, and the source beside it,
  are facts about this machine's disk and that server's *now*; an offer to append to one after a
  restart hours later would trust far more than the size comparison behind it (`ponytail:` no
  checksum, above) can carry. In-memory, per tab, for the reconnect that follows the drop — which
  is the case the user is actually in.
- `ponytail:` **the account is not carried.** A transfer runs as the identity selected at the time
  (§46), and an elevated one does not exist after a reconnect — so a resume issued after one runs as
  the login account. If that account cannot write the destination, the resume fails with the
  server's own reason in the status bar and nothing is lost; it just cannot be finished from there.

### Preserving file metadata (v4.0.0)

Every finished file is **stamped to match its source**, best-effort, so a copy is not silently
re-dated to "now". What is meaningful depends on the two ends, and cmote's everyday case is a
Windows client:

- **Modification time**, both directions. On upload the worker calls SFTP `setstat`; on download it
  sets the local file's time with std's `File::set_modified`. The wrinkle is on the SFTP side:
  the protocol carries access and modification time as **one** attribute, so an mtime cannot be sent
  without an atime, and an omitted-but-flagged field goes out as zero — which would reset the file's
  access time to 1970. So `transfer::upload_stamp` (pure, unit-tested) sends the source's real atime
  alongside the mtime, or the mtime itself when the source has no readable atime, never a bare zero.
  Setting the local time has no such coupling.
- **Unix permission bits**, only where both ends have them. A Windows source exposes no Unix mode to
  read and a Windows destination cannot apply one, so the permission half is compiled in only under
  `#[cfg(unix)]`; on Windows the timestamp travels alone. Only the low bits (`& 0o7777`) are carried
  — the file-type bits are the far side's business.

It is **never** allowed to fail a transfer: a server that refuses `setstat` (read-only exports,
chrooted SFTP) or a filesystem that will not take a timestamp is logged and the bytes stand. A tree
captures each file's metadata during the walk (`transfer::PlannedFile`), so stamping costs no extra
round trip per file; a single file reads it off the one stat it already does. The *access* time is
not itself a goal (SFTP just forces it along for the ride), and there is no mode/timestamp UI — it is
always on, the friendlier default for a GUI than scp/rsync's `-p` flag.

---

## 18. Remote folder explorer (v2.0)

The headline of v2: a **2D tree of the remote filesystem** in the browser strip along the
bottom, to the right of the files pane (§19) — the terminal keeps the whole width above it —
so the far side can be navigated with the mouse instead of `cd` and `ls`. It is split
three ways — a pure model (`explorer.rs`), a pure view (`ui/explorer.rs`), and the
network calls (`ssh/browse.rs`) — which is what keeps the interesting rules (relative
paths, what collapsing does, which folders a `cd` reveals) unit-testable with no server.

### The model (`explorer.rs`)

- **Folders only, POSIX paths.** The tree lists directories, not files: files are the files
  pane's job (§19), so listing them here too would only duplicate it, and leaving them out
  keeps both the row count and the traffic down. Paths are `/`-separated because that is
  what SFTP puts on the wire whatever the server runs on — one dialect, no guessing.
- **Lazy, because a filesystem is not a list.** A folder's children are unknown until
  something asks. `expand` and `reveal_if_new` therefore *return the paths that still need
  fetching* rather than fetching anything, and `app` turns each into one
  `SshCommand::ListDir`. Walking eagerly would be unbounded work against someone else's
  disk.
- **`children: Option<Vec<String>>`.** `None` means never listed, `Some(vec![])` means
  listed and empty. Collapsing that distinction is what would make a refused directory
  re-request itself on every redraw.
- **Collapse takes the subtree with it.** Closing a folder closes everything under it, so
  re-opening shows one clean level again. Nothing is discarded, so the cached rows draw at
  once with no empty flash.
- **Opening a folder re-lists it.** `expand` re-fetches whenever the call is what *opens* a
  folder — a genuine closed→open transition — not only the first time. A user who collapses
  a folder, moves a child out of it from the shell, then clicks it open again must see the
  new contents, not the stale cache; opening is deliberate, so it asks the server every time.
  The cached children stay on screen under a spinner until the fresh listing lands, so the
  row never flashes empty. `force` (the menu's Refresh, a completed rename) re-lists a folder
  that is *already* open; `reveal_if_new`'s ancestors that are already open are left untouched,
  so following the shell does not re-list the whole chain on every `cd`.
- **Hidden folders are a filter, not a fetch.** Listings always include dot-prefixed
  entries; the panel's `.*` checkbox only decides whether the rows are drawn, so
  flipping it is free. They are shown by default — on a server, `.ssh` / `.config` are
  usually the reason the tree was opened.

### Following the shell

The cwd tracker from v1.4 (§17) already reports the remote directory on every prompt, so
the tree follows it for free: `reveal_if_new` opens the whole chain from `/` down to the
directory, opens the directory itself (so its contents are visible, not just its row),
selects it, and returns the ancestors still needing a listing. It compares against the
last revealed path first, because the shell re-announces the same directory at *every*
prompt and this runs on every chunk of output. It only ever expands — a folder the user
opened is never closed behind their back.

`ponytail:` POSIX paths only. A remote that announces a native Windows directory
(`C:\Users\…`, OSC 9;9) does not sit anywhere on this `/`-rooted tree, so it is left alone
rather than revealed at a made-up place. Upgrade path: root the tree at the drive when the
announced path carries one.

### Reading the server (`ssh/browse.rs`)

- **SFTP first.** `read_dir` returns typed entries, so a directory is a directory because
  the server said so — no text to parse, and names with spaces, quotes or newlines survive
  intact. The channel is opened on the first listing and **kept for the session**: a tree
  asks many small questions, and paying channel setup for each would be felt. The upload
  path keeps its own short-lived channel on purpose — it closes the session when the
  transfer ends, which would take a shared one down with it. Only the *opening* is shared
  (`ssh::open_sftp`), so the two cannot disagree about how the subsystem is requested.
- **A symlink is stat'ed, not assumed.** An entry's own type says nothing about what a
  symlink points at, so a symlink is followed with one `metadata` call and kept only if
  the target is a directory. `||` short-circuits, so a real directory pays nothing.
- **`ls` as the fallback.** A server with the sftp subsystem switched off still gets a
  tree: an exec channel runs `ls -1Ap -- '<path>'` and the lines ending in `/` are the
  folders; rename becomes `if [ -e to ]; then exit 1; fi; mv -- from to`, one command so
  nothing can slip into the destination between the test and the move. `ponytail:` this is
  text, and text lies — a folder whose name contains a newline reads as two entries, and a
  symlink to a directory is missed. Both are correct on the SFTP path, which is what runs
  unless the server refuses. Upgrade path: `find -maxdepth 1 -type d -print0`.
- **Off the shell pump.** Only the channel opening happens inline (it borrows the session
  handle); the listing itself is spawned, so a slow directory never stalls terminal
  output. Output is capped at 1 MiB — an enormous or non-listing answer fails cleanly
  instead of growing our memory (§12).
- **Renaming checks first.** SFTP's rename refuses an occupied destination on most servers
  but not all, and a folder quietly replaced cannot be undone, so `try_exists` gates it and
  a server that will not answer is treated as a failure rather than as "the path is free".

### The panel (`ui/explorer.rs`)

- **Layout.** A fixed-width column at the right end of the bottom browser strip — to the
  right of the files pane (§19), not beside the terminal any more — with a draggable
  splitter between the pane and the tree, and a status-bar button that hides the whole
  thing. The tree only ever shows beside a visible files pane: the strip is one region, so
  hiding the pane takes the tree with it. The column's width is taken out of the *pane's*
  now, not the terminal's — `files_width` subtracts `Explorer::reserved` — so a splitter
  drag only reshapes the pane's grid, never the pty; the terminal reflows for the strip's
  *height* alone (`grid_size` subtracts `Files::reserved`), and a round-trip test locks
  `window_size`/`grid_size` together with that height reserve. The drag is clamped to 60% of
  the window, because a splitter with no ceiling can leave the pane a sliver wide and the
  user dragging their way back out.
- **Right-click menu**, on the folder under the pointer: *Open in terminal*, *New folder…*,
  *Upload…*, *Upload folder…*, *Rename…*, *Delete…*, *Copy name*, *Copy relative path*,
  *Copy full path*, *Refresh*. "Copy relative path" is disabled when the shell has never announced
  a cwd — there is nothing to be relative to. *Refresh* answers "is this folder still here, under
  this name, holding these children?" — it re-lists the folder's **contents** (forced open, so the
  result shows) *and* its **parent**, because a rename or deletion done from the shell surfaces in
  the parent's listing, never the folder's. It is named "Refresh", not "Expand", deliberately —
  that is the word a user hunts for when the tree has gone stale, and the earlier "Expand
  (refresh)" label was missed for exactly that reason. There is **no menu *Collapse*** (nor
  *Expand*): a single folder still opens/closes by clicking its row or pressing → / ←, and the
  whole tree collapses from the header button below — a menu item for each was redundant.
- **Refresh and collapse-all, in the header (v3.0).** A shell command that moves or makes a folder
  leaves the GUI with no way to know, so refreshing must be both effective and obvious. Three
  refresh affordances, all labelled/keyed the way the file world expects: the per-folder menu
  *Refresh* above; a header **↻ button** (the shared `ui::files::refresh_button`, twin of the copy
  button) that re-lists **every open folder** in one press via `Explorer::refresh_open`, so all
  the expanded content comes current without the user working out which folders a move touched;
  and **F5** while the tree holds the keyboard, mapped to that same whole-tree refresh. The pane
  below wears the twin button and its own F5 (§19), so each panel refreshes the one that has
  focus. A closed or already-loading branch is skipped — nothing changes under rows you cannot
  see, and a fetch in flight will bring the fresh listing itself. Beside ↻ sits a **collapse-all
  button** (`unfold_less`, `Explorer::collapse_all`): it closes every branch back to the root's own
  children — the clean top-level view after a deep dive — while leaving the cached listings in
  place, so a re-opened branch draws instantly (though opening re-lists it in the background, as
  above). The root itself stays open; closing it would shrink the panel to a single `/` row.
- **Relative paths walk both ways.** `relative` emits `..` for every level the two paths do
  not share, so the result is usable from the shell's current directory even when the
  folder sits on another branch (`/home/user` → `/var/log` gives `../../var/log`).
- **"Open in terminal" types `cd`.** The path is single-quote escaped, so a folder called
  `'; rm -rf ~` reaches `cd` as a name and not as commands — this string goes straight into
  a live shell, which is exactly why it is quoted. `ponytail:` a POSIX shell is assumed,
  and if a full-screen program (vim, less) is running, those characters go to *it* — cmote
  cannot tell a prompt from an editor. Upgrade path: only offer the item between prompts,
  which the OSC announcements could mark.
- **Rename is inline**, the same interaction as the home screen's F2 rename (§14): the row
  becomes a field, Enter commits, Esc abandons. A blank name, a name containing `/` (that
  would *move* the folder, not rename it) and an unchanged name all just close the edit.
  While the field is open the terminal's key listener swallows keys, so renaming a folder
  is not also typing at the remote prompt. On success the tree forgets the old subtree and
  re-lists the parent, so the row reappears under its new name in sort order. **The shell's
  own cwd is not updated** — a shell sitting inside a renamed folder is left on a stale
  path, which is the server's semantics, not something cmote can fix from outside.
- **New folder and delete (v3.0).** *New folder…* opens a small name dialog (the shared modal
  chrome, §10) whose one field is auto-focused; the same blank / `/` guard the rename uses keeps
  a bad name unsubmittable. *Delete…* opens a confirmation naming every target and warning that a
  folder goes with everything inside it — not undoable, so, like Disconnect and the home list
  (§14), it only ever raises the question; the removal waits for an explicit confirm. Both are
  driven from `ssh/browse.rs` on the shared listing session: an SFTP `mkdir`, and a delete that
  **walks a folder's subtree** (files unlinked, then directories removed deepest-first, symlinks
  unlinked never followed), with an `mkdir` / `rm -rf` exec fallback for a server with no sftp
  subsystem. On success both panels re-list the affected parent, and a files pane sitting inside a
  deleted folder steps up to the nearest surviving one.
- **Recursive transfers (v3.0).** *Upload folder…* sends a picked local folder tree-and-all into
  the menu's folder; the files pane's own *Download folder…* is the mirror. See §17 for the
  merge-and-per-file-collision protocol they share.
- **Failures are a notice line**, under the tree, not the error screen: a directory the
  user may not read must not tear down a working shell (the same call as an upload
  failure, §17). The path is the user's own, so naming it is what makes it actionable.
- **Fixed colours, like the grid.** Every surface in the panel sets background *and*
  foreground together, so contrast does not depend on the system light/dark preference —
  the trap §14 documents.
- **The menu opens under the cursor.** A right-press carries no coordinates, so the panel
  is wrapped in a `mouse_area` that tracks the pointer — the same trick the terminal grid
  uses (§10). A child `mouse_area` only captures *presses*, so the rows' own click
  handlers do not swallow the moves. The anchor is **frozen into the open menu** rather
  than read live: the panel keeps reporting moves while the menu is up (the dismiss layer
  above it handles no moves, so they fall straight through), and a menu that tracked them
  would slide out from under the cursor before an item could be reached. The menu is laid
  out right-aligned with a
  padding of `panel width − pointer.x − menu width`: since the panel's right edge is the
  window's right edge, that puts the menu's left edge under the cursor, and clamping the
  padding at a minimum slides a menu opened near the edge back inside the window instead
  of letting it hang off. Placing from the pointer rather than from a row index (what the
  home screen does, §14) is also what makes it correct on a scrolled tree — iced does not
  expose the scrollable's offset, but the pointer needs no such correction.

---

## 19. Remote files pane (v2.1)

An **icon grid of every entry in one directory**, in the browser strip under the terminal.
The strip runs the window's full width; the pane fills it, save for the folder tree's column
on the right when that is shown (§18). The tree answers "where am I in the filesystem"; this
answers "what is actually in here". Same three-way split — a pure model
(`files.rs`), a pure view (`ui/files.rs`), and the network calls (`ssh/browse.rs`,
`ssh/download.rs`) — so the rules that matter are unit-testable with no server.

The layout is now two rows under the status bar: `terminal | tree` on top, the files pane
across the bottom, with a draggable splitter on each seam. `grid_size` subtracts the
tree's width AND the pane's height, so the pty reflows for either drag exactly as it does
for a window resize.

### One directory at a time, in batches

- **Flat, not recursive.** One listing per directory shown. A crowded folder therefore
  costs exactly one request and can never fan out into thousands, which is the failure
  mode a recursive view invites.
- **Batches of 1000.** The server task sends the listing as `FilesChunk` messages of
  `files::BATCH` entries, the last one flagged `done`; the grid grows as they land and the
  header counts "N so far…". An empty directory still sends one empty final batch — that
  is what tells the pane to stop waiting. `ponytail:` the batching bounds the *message*
  size and the relayout, not the fetch: `read_names` runs the whole readdir loop before
  returning. It costs no extra round trips (SFTP sends a name's attributes with the name,
  so there is no per-file stat either way), but it does hold the listing in memory once.
  Upgrade path: emit a batch per `readdir` packet rather than per 1000 collected names.
- **Every batch carries its request number.** Leaving a directory bumps the number, so
  batches still in flight for the folder just left are dropped instead of being mixed into
  the new one — the bug this design exists to prevent. Failures carry it too.
- **Sorted by the server task, appended by the model.** Directories first, then everything
  else, each case-insensitively. Sorting once, before the cut into batches, is what lets
  the model simply append and still hold a stable order across batches; sorting per batch
  in the model would either break the order or re-sort the whole listing on every one. This
  is the DEFAULT order — what the pane shows until the user picks a sort of their own (below).
- **A user-chosen sort, on top of that default.** The header's **sort** button drops a small
  menu: one group of four keys — **Name**, **Last modified**, **Extension** (the text after a
  name's last dot, so all `.rs` sit together — labelled "Extension", not "Type", to say it is
  not the SFTP kind) and **Size** — and a second group of two directions, **Ascending** and
  **Descending**. **Both halves are a tri-state and independently unset-able.** The choice lives
  on the model (`sort: Option<SortKey>`, `sort_dir: Option<SortDir>`), not the view, so it survives
  every relayout and outlives a change of directory — a sort is a view preference, not a property
  of one folder. `rows` applies it only when a key is set, so the common no-sort case still pays
  nothing beyond the dot-file filter; with a key it `sort_by`s the (already small, one-directory)
  row slice, and an **unset direction sorts ascending** — so a key on its own already reorders the
  grid. **Folders stay first** whatever the key or direction — that grouping is the one thing the
  direction never flips; it reorders only *within* each group (`compare_entries` settles the
  folder/file split before it ever reverses). Every key falls back to the name, so the order is
  total and stable across re-listings, exactly as the default is. The menu carries no "None" row
  for either group: picking the **lit** key clears the sort back to the default order, and picking
  the **lit** direction unsets the order back to ascending — so at the default NEITHER direction is
  ticked. The header's sort button is lit (foreground vs. muted) whenever a **key** reorders the
  grid; a direction with no key leaves the default order, so the button stays muted. Picking a key
  or a direction leaves the menu open, so both halves of a sort can be set in one visit; a
  click-away (`sort_dismiss_layer`) or the button itself closes it. Unlike the pane's
  context menus, which anchor to the window's bottom and grow up, the sort menu hangs from the
  header at the pane's TOP and grows down — there is room below the toolbar, none above it (the
  overlay is full-window, so it adds the pane's own top offset, `window height − pane height`, to
  land beside the button rather than up near the window's top). The ticked-row and separator chrome
  is shared: `menu::check_item` and `menu::separator` join `menu::item` so the sort menu reads as
  one of the family. **The sort is remembered per target** (§22): both halves fold into the same
  session snapshot as the `.*` filter and the panel sizes, so the grid reopens in the order a
  target was last left in.
- **Symlinks keep their own kind.** Resolving one costs a round trip *per link*, and a
  crowded directory is exactly where that adds up — so a link gets the link icon and is
  not followed. (The tree does resolve them, §18: it sees far fewer entries and needs to
  know whether the link is expandable. The pane resolves the *selected* one, §20 — one
  link, when the user asks by looking at it.)
- **Sizes, times and owners ride along.** SFTP sends a name's attributes *with* the name,
  so collecting them costs nothing extra and they travel on the entry (§20). The grid shows
  the size and the date (below); the owner, the exact byte count and the seconds are the
  details popup's job.

### What a cell looks like (v2.2)

- **A row, not a portrait box.** The first cut was a big icon centred above a centred name,
  which reads as a photo album; a file manager reads as a list. A cell is now a wide, short
  row — a small icon in front of a left-aligned name — and the grid still wraps into columns,
  so a wide window shows several of them per line rather than one long column.
- **Each cell wears a one-pixel border** (`CELL_BORDER`, the splitter bar's grey), so the grid
  reads as a field of distinct tiles rather than one run of names. It sits *under* the selection
  fill, so a chosen cell keeps the same footprint as its neighbours — just filled blue.
- **A name too long for its two lines is middle-ellipsised** (`crate::ui::elide_middle`,
  §22): the start *and* the extension survive, which is what tells two similar names apart —
  a tail-clipped `report-2026-q1-fin…` and `report-2026-q1-dra…` do not. The full name is
  always one selection away in the popup (§20).
- **A second, muted line carries the size, the modified date, then the permission word and the
  `owner:group`** (`4.0 KB · 2026-03-20 11:46 · -rw-r--r-- cme:staff`), reading left to right like
  a terse `ls -l` line. A directory shows no size — a directory entry's own size is not the size of
  its contents, and printing 4096 for every folder would be noise that reads as data — but keeps the
  date, the mode and the owner. The mode (`drwxr-xr-x`, `-rw-r--r--`) is built from the numeric mode
  SFTP carries with every entry, by `format_mode` — not from the `longname` text, which can be
  absent or carry a trailing ACL marker the ten-column form has no room for — and it sits just ahead
  of the owner it governs (`access_line`). Any fact the `ls` fallback never learned shows as a dash,
  the same convention the popup uses; the `owner:group` is the popup's own `owner_group` helper (§20),
  so the two agree.
- **One date computation, two forms — but only the popup wears the zone tag.** The cell's
  compact `format_mtime_short` (day and minute) and the popup's full form share `local_parts`,
  so both read the *same* instant on the *same* server wall clock (§20) — they can never
  disagree about the time. Where they part is the tag: the popup appends `CEST (+02:00)` via
  `with_zone`, the cell does not. A cell is narrow and the same zone on every row is noise; the
  cell still keeps the *shift* (that is what makes its clock match `ls -l`), it just drops the
  label and offset text. Naming the zone is the popup's job.
- **Every cell keeps a uniform height.** Band hit-testing, row-wise arrow navigation and
  popup placement are all arithmetic over `CELL_HEIGHT` (§20, §21) — a cell that grew with
  its content would break all three at once.
- **The pane opens at 330 px** (`DEFAULT_HEIGHT`), tall enough for several rows of the new
  cells. `window_size` folds the pane's reserved height into the initial window, so the
  window opens taller with it and the terminal keeps its 40 rows rather than shrinking.

### Two sources for "which directory", last one wins

- The shell's working directory (OSC 7, §17) drives it: `cd` in the terminal and the pane
  follows, exactly as the tree's auto-reveal does.
- **Browsing never moves the console.** A tree row click, a pane double-click, the "up"
  button and Enter all point the *pane* at a folder (`Files::show`) and leave the shell
  where it is — so you can look inside a folder you are not in without disturbing what is
  running. This is the change from the first cut, where a double-click also `cd`'d the
  shell: the two directions were coupled and the console kept getting dragged around.
- **The console moves only on a `cd` it can see** (`App::move_shell_to`): one you type, the
  **Sync** button, either panel's **"Open in terminal"**, the tree's Enter, or the replay a
  reconnect does (§22). Every one of those is a deliberate act, never a side effect of
  browsing.
- The catch the two sources create: the shell re-announces its directory at *every prompt*,
  so a naive "follow the cwd" would drag the pane back from a browse on the next keystroke.
  `Files::follow` therefore acts only when the announced directory differs from the last one
  followed — a repeat is not a move — while `Files::show` (a browse) is unconditional. Last
  one wins: browse and the pane moves; move the console and the pane follows the `cd`.
- **The tree carries the same guard, and the two must agree** (`Explorer::reveal_if_new`
  against its own `revealed`, `Files::follow` against `followed`). Two panels, one question —
  "has the shell actually moved?" — answered in two places, which is two chances to disagree.
  They did, on the reconnect path: see §22's pin, which held the pane and not the tree.

### No `remote::Location` module — and why (v4.0.0)

An architecture review proposed lifting the shell/tree/pane coordination — `on_sync`,
`on_reveal`, `browse_to`, `refresh_remote_dir`, the resume pin and the shell-follow — into one
`remote::Location` owning "where the panels point relative to the shell". It was explored and
rejected. Recorded so a later review does not re-suggest it.

- **It would have to own `Explorer` and `Files`, which are used everywhere for reasons that
  have nothing to do with location.** Scroll offset, panel width and reserved space, visibility,
  hidden-file mode, the context menus, the inline rename, the selection and the rubber band —
  around a hundred call sites in `app.rs`, against the eight that are about *where the panels
  point*. Owning them means an `explorer()` / `explorer_mut()` pair carrying ninety per cent of
  the traffic straight through: a module whose interface is as wide as what it hides, which is
  the definition of shallow.
- **Not owning them is worse.** The alternative is free functions taking `&mut Explorer`,
  `&mut Files` and a path — three or four arguments to move two lines of state, with the
  invariants still living in the caller.
- **The peers are already the deep modules.** `explorer.rs` and `files.rs` each own a panel and
  hand back listing requests; `transfer.rs` works because the state it owns is *only* used by
  transfers. A third layer mediating two widely-used peers is not a deepening, it is a wrapper.
- **What is genuinely shared is one field**, `App::resume_cwd`, and merging the two panels'
  follow-guards behind it would move state *out* of the panel modules and *into* `app.rs` — the
  wrong direction, since `app.rs` is the file the review flagged for being 11k lines.

The exploration was not wasted: it found the pin covering only half of what it was for, and
Reveal stranding the panels when pressed against it. Both are fixed below.
- **The "Sync" button brings the console to the pane.** Since browsing no longer moves the
  shell, the pane and the console drift apart on purpose; Sync is the manual way to close
  that gap, typing a quoted `cd` (via `move_shell_to`) so the shell — and with it the tree
  and the title — comes to the folder on show. It carries no path: `app` reads `Files::path`
  when the press lands, so it can never move the shell somewhere the pane has since left. It
  sits in the left button group after Upload, and is **disabled** whenever there is nothing
  to do — no directory on show, or the pane and the shell's announced cwd already agree
  (an exact string compare, so an un-announced cwd leaves it live and the `cd` is a harmless
  no-op). Dimmed, it doubles as a tell that the two are already in step.
- **The "Reveal" button brings the panes to the console** — Sync read backwards, and the half
  that was missing. The drift goes both ways, but only one way could be closed from the bar: a
  browse three folders deep left the panes somewhere the shell was not, and the shell's own
  announcements could not undo it, because `Files::follow` acts on a *move* and a shell standing
  at the same prompt re-announces the same directory. The choices were to `cd` the console —
  moving the side that was already right — or to walk the tree back by hand. Reveal opens the
  chain down to the announced cwd, selects it and points the pane there in one press.
  - **It sends nothing.** No `cd` is typed and no bytes reach the remote: this is the local view
    catching up with a shell that stays where it is, which is why it is safe with a full-screen
    program running and `move_shell_to` (Sync, "Open in terminal") is not.
  - **It uses the UNguarded reveal** (`Explorer::reveal`, split out of `reveal_if_new`). The
    guard's job is to keep the per-chunk call cheap and to stop a re-announcement undoing a
    browse; both are wrong here, since the whole reason to press it is that the tree was walked
    away from a cwd that never changed — the guarded call would decline exactly when asked.
  - **It seeds both follow-guards** with the same path — the pane's through `Files::set_followed`,
    the tree's inside `Explorer::reveal` itself — so the next prompt reads as "still there" rather
    than as a move, and a real `cd` after it still carries both panels.
  - **It ends a reconnect resume still settling** (`resume_cwd = None`, §22), the rule
    `move_shell_to` already follows and for the same reason: the pin holds the panels against the
    shell's login announcements, and the user saying out loud where the panels go outranks that.
    Left armed, the pin swallowed the settle as "already there" and stranded the panels at the
    login directory with no further announcement coming to put it right — the exact drift this
    button exists to close, caused by pressing it. Nothing is spent when there is no announced cwd,
    since there is then no ask to outrank.
  - **Disabled when there is nothing to do:** no announced cwd (§17 — it takes OSC 7), the strip
    hidden (the tree goes with the pane, so a press would change nothing anyone can see), or both
    panels already there. "Both" is three terms rather than Sync's one: the pane can be on the cwd
    while the tree is not, which is what a collapsed branch under an unmoved selection leaves —
    `selected_index` is `None` for a row inside a collapsed branch, and that is what says the
    folder is on screen rather than merely remembered.
  - It sits **beside Sync at the head of the left group**, the pair reading as one question in two
    directions: whichever way the two have drifted, the lit button says which way it will move.

### Icons

- **A bundled icon font** (Material Icons, Apache-2.0, ~349 KB, in `assets/`). The
  monospace face has no folder glyph and emoji are neither guaranteed nor monochrome; a
  font gives glyphs that scale and colour like text. Drawing them on a `canvas` instead
  would mean one canvas widget per cell — hundreds in a crowded directory.
- **Nine categories, not per-extension.** folder / link / image / code / archive /
  document / audio / video / plain, from one small extension table. An unknown type gets
  the neutral file icon rather than a wrong one, and the table is a one-line change to
  extend. A leading dot is not an extension: `.bashrc` is a name.
- Colours are per category and fixed, like everything else in these panels (§18).

### Actions

- The right-click menu uses the shared chrome (§10): **Open in terminal** (directories
  only), **Download…** (files, or a selection's files), **Download folder…** (a lone
  directory, §17), **Rename…**, **Delete…**, **Copy name / relative path / full path**,
  **Refresh**. Each inapplicable item is *disabled*, not hidden, so the menu keeps one shape.
  Opened on a multiple selection it acts on all of it, which is what disables Rename and Open in
  terminal there and puts the count on Delete and the rest (§21). The empty-grid menu adds
  **New folder…** and **Upload folder… here** beside the existing Upload / Refresh (§17, §18).
- **Rename** reuses the tree's rules and the same `RenameDir` command — SFTP's rename does
  not care whether it is moving a directory — with the same guards: no blank name, no `/`
  (that would be a move, not a rename), and a destination that already exists is refused
  rather than replaced. Both panels react to `RenameDone`.
- **Download** is the mirror of the upload (§17): its own sftp channel, its own spawned
  task, progress in the status bar through the shared `TransferProgress`. The destination
  comes from the native **save dialog**, which is also what asks about replacing a local
  file — a second prompt in our own chrome would only be a second chance to answer it
  wrong. One transfer at a time: starting a download while one runs is refused with a
  notice rather than fighting over the one progress bar — which is why a batch of them
  queues instead (§21).
- **The menu opens upwards.** Same frozen-anchor construction as the tree's (§18), but
  bottom-aligned: this pane is at the bottom of the window, so a menu dropping downwards
  would fall off it. `pane height − pointer.y` puts the menu's bottom under the cursor, and
  the left edge is **clamped against the pane's width** (v2.2) — the panel is a fixed
  `menu::WIDTH`, so once the anchor would push its right edge past the pane, it is pinned
  `MENU_INSET` in from that edge instead of spilling off. The pane's width is the window less
  the tree's column when one is shown (§18); the tree's menu already did the same trick, and
  `place_menu` now does it for both of this pane's menus — the entry's and the empty-space one.
- **Empty space has its own menu (v2.2).** A right-click that lands on no cell opens a short
  menu of the things that are about the *directory* rather than an entry: **Upload… here**
  (§17) and **Refresh**. It shares the chrome, the frozen anchor and the placement above.
- **"Up" is the header's first item.** A button at the left of the toolbar browses to the
  directory above the one on show, where every file manager puts it. It goes through the
  same pane-only `browse_to` as a double-clicked folder — the console stays put — and it is
  *disabled* — not hidden — at the root and before the first listing, the two cases with
  no parent. The message carries no path: the pane's own is read when the press lands.
- **A header ↻ button, matching the tree's (v3.0).** The same shared `refresh_button` sits in
  this header too, beside the `.*` toggle, and re-lists the directory on show (`FilesMessage::Refresh`
  — the menu item's twin). **F5** does the same while the pane holds the keyboard. So refresh is
  reachable the same way in both panels — menu, header button, F5 — and each key/button acts on
  the panel that has focus.
- **The `.*` toggle is the tree's.** One flag (`Explorer::show_hidden`) filters both
  panels, and each header carries a checkbox that shows and flips it — so hiding dot-files
  hides them everywhere, and the pane still has the control when the tree is collapsed.
  Toggled on, it hides *nothing*: every name the server reported is shown, dot-prefixed or
  not, whatever attribute the far side considers hidden or system. The two exceptions are
  `.` and `..`, dropped at ingest (`explorer::is_dot_link`) because they are this folder
  and the one above it rather than entries in it — a tree row for `..` would walk back up
  itself. SFTP omits them and `ls -A` leaves them out; the guard makes it true regardless.

---

## 20. Keyboard focus and entry details (v2.1)

Two panels now sit beside the shell, and both want the arrow keys. This section is the
answer to "who gets the keystroke", plus what the files pane shows about the entry the
keyboard just landed on.

### One focus for the window

- **Three stops: shell, tree, files pane** (`app::Focus`). A session opens with the
  **shell** focused — that is what a terminal is for — and `clear_grid_interaction` puts it
  back there whenever a session starts or ends.
- **A click focuses what was clicked.** Each panel's own `mouse_area` reports a
  `PanelPressed`, so an empty patch of panel focuses it just as a row or a cell does, and a
  press on the grid hands the keyboard back to the shell. In the files pane that press also
  **clears the selection** — a cell's own `mouse_area` swallows the press that lands on it,
  so one that reaches the pane missed every cell, which is the click-away every file
  manager deselects on.
- **Ctrl+Tab cycles**, Ctrl+Shift+Tab the other way, skipping panels that are hidden — a
  stop you cannot see is a dead press. It is read *before* anything else on the terminal
  screen, because it is the way out of a panel that is swallowing keys.
- **A focused panel keeps every key it could mean**, not just the ones it uses. A panel that
  swallowed only the arrows would leave Tab completing paths at a prompt the user is not
  looking at. **Esc** hands the keyboard back to the shell from either panel — and so does
  **plain typing**, which no panel answers to and the shell always does (§50).
- The focused panel wears a one-pixel ring (`ui::explorer::focus_border`, shared by both),
  which is the only thing that tells the two panels apart at a glance.

### Walking the panels

- **Files pane:** Left/Right step one cell, Up/Down a whole row, **PageUp/PageDown** a
  screenful of rows (a page is a viewport's worth less one, so a row of context carries
  across the jump — `ui::files::page_rows`), **Home/End** the first and last entry,
  Tab/Shift+Tab next/previous, **Enter** enters a folder (through the double-click's own
  handler, so "only a directory can be entered" is decided in one place), **F2** renames.
  Both ends clamp instead of wrapping. The moving keys share one exit — a `Nav` value the
  key match yields — that is either a relative `step` (arrows, Tab, the Page keys) or an
  absolute `jump_to_edge` (Home/End). Home and End must be absolute, not a big step: a
  relative jump reads the empty-selection default (forward starts at the top, backward at
  the bottom), so from *nothing* selected a huge delta would land on the opposite end.
  **Shift** on any moving key extends the selection from the anchor rather than moving it,
  exactly as Shift+arrow already does (§21). The Page keys are focus-gated to the pane, so
  they never fight the terminal's own PageUp/PageDown scrollback (§23) — that answers the
  keys only while the shell holds the keyboard.
- A row is `ui::files::columns(window width)` cells, computed with the same arithmetic
  `Row::wrap` breaks lines with — iced never reports where a laid-out cell ended up, so the
  view and the app both derive it rather than either one guessing.
- **Tree:** Up/Down walk the visible rows, Right expands (fetching the folder if it has
  never been listed), Left collapses, Tab/Shift+Tab step, **Enter** sends the shell there,
  **F2** renames.
- **The selection is scrolled back into view**, and only by the keyboard: a click is
  already on something visible, and scrolling under the cursor would move what was just
  aimed at. Both panels report their scroll offset (`Scrolled`) and share one rule,
  `app::keep_visible` — already visible means *do not move*, so a walk across a screenful
  scrolls at the edges rather than re-centring on every press.

### The details popup

- Shown **beside the selected cell** for every kind of entry — folder, file or symlink —
  because the "type" line only earns its place if the type can vary. It leads with the
  entry's **full name** (the grid cell middle-ellipsises a name too long for its two lines) and, for a
  symlink, **where it points**; then the type, the modification time, the size (human, with
  the exact byte count once the two differ), the **permission word** (`drwxr-xr-x`, on its own line
  just ahead of the owner it governs) and `owner:group`.
- **The type of a file is its MIME type**, from `files::mime` — an extension table, because
  asking the server would be a round trip per selection and the extension is already in
  hand; unlisted extensions read `application/octet-stream`, the same answer
  `file --mime-type` gives when it recognises nothing. Folders and symlinks keep their
  plain names: there is no MIME type worth showing for either.
- **Names and targets wrap**, and the card grows by whole lines to fit them, because its
  height is what keeps it inside the pane. `ponytail:` the row count is estimated from an
  average glyph advance (`ui::files::wrapped_rows`), not measured — iced shapes text only
  during layout, too late to place the card.
- **Placed by arithmetic**, from the selection's index, the column count and the scroll
  offset, and flipped to the cell's left when the card would hang off the right edge —
  the same "iced does not expose layout positions" constraint the context menus work
  around (§18, §19). It floats in a `stack` over the grid: a card in the flow would
  reshuffle the cells every time the selection moved.
- **Absent facts show as a dash** rather than vanishing. The `ls` fallback (§19) reports
  none of them, and a card that changed shape per entry would be harder to read.
- **A copy button takes the whole card (v2.2).** The facts on it — a full name, a link
  target, an exact byte count, an owner — are exactly what gets pasted into a shell, a ticket
  or a message, and copying them one line at a time is six gestures for one thought. The
  button carries the already-rendered lines (`FilesMessage::CopyDetails(String)`), joined one
  per line: the view builds the text once, for the eye and the clipboard both, so the two can
  never drift. It confirms with the shared toast (§10). It sits on **its own bar at the top,
  pinned right**, rather than floating over the card — an overlay would paint across the
  first line, and showing the full name is the whole reason the card exists; the card grows
  by `POPUP_BUTTON_ROW` to pay for the bar. The summary card of a multiple selection (§21)
  wears the same button, so a "12 items selected, 3 folders…" reading copies too.

### Where the details come from

- **Owner and group as names**, not numbers: SFTP v3 carries only numeric ids in a
  listing's attributes, but it also carries a `longname` — the `ls -l` line the *server*
  built, having resolved the names itself. `SftpSession::read_dir` throws that away and
  keeps its raw session private, so the browse channel is opened as a `RawSftpSession`
  (`ssh::open_raw_sftp`) and drives `opendir`/`readdir`/`close` itself. Same channel, same
  handshake, same round trips — only the parsing layer differs. `ponytail:`
  `files::parse_longname` is a column split, guarded by requiring the mode column to be
  mode-shaped and the size column to be a number; numeric ids are the fallback.
- **The permission word from the numeric mode**, not the `longname` text: SFTP sends a
  numeric mode with every entry, and `files::format_mode` renders it the way `ls -l` prints —
  the type letter (`d`/`l`/`-`/`b`/`c`/`p`/`s`), the three rwx triads, and the setuid/setgid/sticky
  bits folded into their execute columns (`s`/`S`, `t`/`T`). The number is chosen over the
  `longname`'s own mode field because it is always present and always ten columns wide, where the
  text can be absent or carry a trailing ACL marker (`+`, `.`, `@`) the form has no room for.
  The `ls` fallback (§19) sends no mode, so it reads as a dash.
- **Times in the server's own timezone.** An mtime is an instant; reading it as a wall
  clock needs a zone, and the honest one is the machine the files live on — `ls -l` there
  says the same thing. One `date +'%z %Z'` per session on an exec channel
  (`browse::probe_zone`) answers it; until it comes back, and on a server with no `date`,
  times render as UTC, which is at least never wrong about the instant.
- **A fixed `YYYY-MM-DD HH:MM:SS ZONE` format**, not the user's locale. Rust's std has no
  locale lookup, so localising means a dependency (`chrono` + `sys-locale`) or an OS call
  (`GetDateFormatEx`, and the first `unsafe` in this codebase) — and an ordering that is
  unambiguous everywhere costs neither. The calendar arithmetic is Hinnant's
  `civil_from_days`: closed-form, no table, no loop, and unit-tested against the epoch, a
  leap day and both sides of Greenwich.
- **One `readlink` per selected link.** Resolving every link in a listing is the
  round-trip-per-entry cost the pane exists to avoid (§19), so the target is fetched when a
  link is selected and keyed by the link's own path — an answer that arrives after the
  selection moved on is recognisable as stale and is not shown.

---

## 21. Selecting many entries at once (v2.1)

§20 gave the files pane one selected entry. This section makes it a *set*: a rubber band
pulled over the grid, Ctrl+click, Shift+click, Shift+arrow and Ctrl+A, and what the menu
does when an action has nine targets instead of one.

### The selection is a set with two ends

- **`Files` keeps a `HashSet` of paths, a cursor and an anchor.** The cursor is the entry
  the keyboard is on — what the arrows step from, what the popup describes, what Enter, F2
  and a single-target menu item act on; the anchor is the fixed end a Shift-extended range
  runs from. `selected_rows` hands them back in GRID order, because that is the order a
  list of copied names has to come out in.
- **The gestures**: a plain click takes one; Ctrl+click adds or removes one; Shift+click
  and Shift+arrow run a range from the anchor; Ctrl+A takes the whole listing; a press on
  the grid's empty space clears it and starts a band; Ctrl+drag makes the band additive,
  keeping what was already selected as its floor.
- **A press carries no modifiers of its own**, so `App::modifiers` is tracked from the
  keyboard subscription (`Event::ModifiersChanged`) and read by every mouse handler. Bare
  modifier changes are never captured by a widget, so `keyboard::listen` always sees them.

### The band

- **Only the grid's empty space starts one.** A cell's own `mouse_area` swallows the press
  that lands on it, so a press reaching the pane missed every cell — and a band from a cell
  would take press-drag away from a future drag-and-drop.
- **Hit-testing is the grid's own arithmetic** (`ui::files::band_hits`), the same geometry
  the popup and the arrow keys use: the header height and the scroll offset come off the
  pane-local rectangle, and only the rows the band actually spans are walked, so a band in a
  directory of thousands costs what one in a directory of ten costs. Touching counts — a
  cell is in as soon as the rectangle overlaps it at all.
- **A full-window capture layer** (`band_drag_layer`) carries the moves and the release,
  the same trick the splitters use: `mouse_area` reports a release only while the pointer is
  over it, so a band dragged out of the pane and let go over the terminal would otherwise
  never end. Its points are window coordinates; the pane's left edge is the window's and it
  runs to the bottom, so only the vertical origin — the strip's top — has to come off.
- `ponytail:` **no auto-scroll** at the pane's edges — a band cannot reach past what is on
  screen. Add a scroll-on-edge timer if selecting more than a screenful becomes routine.

### What a batch action does

- **The menu acts on the whole selection** when it was opened on part of it; a right-click
  outside the selection collapses onto that one entry first, so an action never reaches
  entries the user has looked away from. The copy items carry the count in their label,
  Rename and Open in terminal are disabled on a multi-selection, and the three copy items
  join their results one per line.
- **Download takes the files and leaves the folders**, then asks for ONE destination folder
  rather than a save dialog per file. The transfers queue (`App::downloads`) and run one at
  a time, because the status bar has one progress bar (§17); a failure notes itself and the
  queue carries on.
- **Local names already taken are one question, not one per file**: before anything is
  written, the batch stops on a dialog offering Skip / Save alongside (`name-1.ext`) /
  Replace / Cancel. Nothing has been downloaded when it is asked, so every answer is safe —
  including cancelling the batch outright.
- **The popup summarises a multi-selection**: how many entries, how they split between
  folders and files, and what the files come to. A folder's size is the size of its
  directory entry rather than of its contents, so it is left out of the total instead of
  making it wrong.

## 22. Resuming where you left off (v2.2)

A reconnect to a saved target used to drop you at the shell's login directory with both
panels at the root. This section remembers where the last session was — the shell and the
files pane, each on its own — and puts you back there, and gives the folder tree a path
header so both panels name the same place.

### One snapshot, remembered per target

- **`SessionState` is the one place that names what persists per target** (§14): the two
  paths (`terminal_path`, `files_path`), the `.*` filter, and the two panel sizes
  (`explorer_width`, `files_height`). It is a transfer struct; `Target` keeps the fields flat
  (so the JSON stays flat and a pre-v2.2 `targets.json` loads unchanged), all optional and
  omitted when absent. Profile metadata, never a secret — §12 is untouched. Adding another
  remembered value is one field on `SessionState` and `Target`, one line each in capture /
  restore / `set_session`.
- **The panel sizes stay per target; the WINDOW size does not.** The tree width and pane
  height belong to a connection — one server's files want a tall pane, another a wide tree —
  so they ride here. The OS window is a different thing: there is one of it, shown on the home
  screen before any target exists, so its size is an app-wide preference kept in `settings.json`
  (§31), not in any target.
- **`App::capture_session` reads the snapshot, `persist_session` folds it in and saves.** It
  runs at every teardown of a *live* session — clean Disconnect, remote hangup, error — and
  again the moment a value changes mid-session (the `.*` toggle), so a later hard exit still
  keeps what was set. Guarded on a live terminal, so a connect that fails before a shell
  opens writes nothing.
- **A `None` never wipes a good value.** `Targets::set_session` treats each `None` field as
  "leave it": a shell that announced no cwd this session (§17) must not erase a resume point
  an earlier session recorded.
- **Not the shell's `Cwd` scanner.** The terminal path is *sourced* from `term::cwd::Cwd`
  (the OSC scanner, §17), but the pane path and panel sizes are GUI state the app owns — they
  never appear in the byte stream, so the scanner stays a scanner and the snapshot lives with
  the target.
- **So the shell resume only exists on a shell that announces.** Everything else here works on
  any remote — the pane path, the panel sizes, the `.*` filter and the sort are the GUI's own
  state — but `terminal_path` can only ever be what the shell said. On a plain bash it is
  therefore always `None`, forever, and the rule above quietly keeps whatever was there. That
  reads as a broken feature rather than a missing shell hook, which is what §17's shell-integration
  dialog exists to close: install it once per server and the resume starts working from the next
  connection.

### Putting you back

- **`App::restore_session` applies the snapshot before the first listing**: the `.*` filter
  and the two panel sizes go straight onto the panels (a size clamped to the same window
  fraction a splitter drag is, and only once the window size is known), and the two resume
  paths come back for the caller to drive the rest.
- **The pane reopens at `files_path`** (root as the fallback) and the tree reveals the
  chain down to it, so both panels start on the resume point.
- **The shell is resumed with a `cd`** typed in exactly as the tree's "Open in terminal"
  does (§18) — quoted, POSIX-assumed, visible in the scrollback. Nothing to replay leaves
  the shell at its login directory, the previous behaviour.
- **Both panels are pinned while the shell settles.** The shell announces its login directory
  *before* the replayed `cd` runs, so without a guard that announcement would drag them off a
  divergent `files_path`. `App::resume_cwd` holds the cwd we are waiting for: until the shell
  reaches it, `SshEvent::Output` moves neither panel; once it does, both follow-guards are seeded
  (so they stay put now but follow the next real `cd`) and the pin lifts. An explicit move by the
  user — Sync, "Open in terminal", Reveal — lifts it early.
- **The tree used to sit outside the pin, and that was the bug.** It followed every announcement
  while the pane was held, so a resume walked it to the login directory and then on to the
  replayed one, opening each chain in turn and asking the server for a listing of every folder
  along both — to land somewhere the pane had deliberately not gone. The two panels are meant to
  open a session agreeing on the resume point, and one of them was leaving before the user saw it
  there. `Explorer::set_revealed` is the tree's half of the seed, the exact mirror of
  `Files::set_followed`, and the reveal now happens *inside* the not-pinned arm rather than in
  front of the whole match.

### The folder tree shows the path too

- **The tree panel's header now shows the current directory**, the same `Files::path` the
  files pane shows — the two views are synchronised, even though the tree's selection can
  sit elsewhere.
- **It wraps across up to two lines**, because the panel is narrow and a deep path would
  otherwise overflow. A path too long for those two lines is middle-ellipsised (`…`) to fit,
  the same cut the file grid's names use (`crate::ui::elide_middle`) — so both the start of
  the path and its leaf folder survive, and the header can no longer grow without bound and
  crowd the tree beneath it. The header is `Shrink`, so a short path still shrinks it back to
  one line; `ponytail:` the keyboard scroll-into-view math subtracts an *estimated* header
  height (`header_height`, capped at those two lines), a rough proportional-font guess, so a
  path may scroll the tree a line more than strictly needed — the same tolerance the notice
  line already carries.
- **The files pane's own header trims its path the same way, but to one line** — that header
  spans the pane's width (the window less the tree's column, §18) and is a busy toolbar row
  (up · path · copy · item count · `.*`), so the path stays on one line (a line that wide
  holds a long path) and is middle-ellipsised
  to fit rather than wrapping and shoving those controls around. The connect form's chosen
  key-file path is trimmed the same way, to two lines. One `elide_middle` rule keeps every
  name and path in the app cut alike; each caller only owns its own "how many fit" estimate.
- **A copy button sits right after the path**, the twin of the files pane's own (§20): it
  copies the same directory verbatim and raises the shared "Copied to clipboard" toast.
  Both headers wear one drawn by a single message-agnostic `ui::files::copy_button` — the
  caller passes the message, so each header copies its own path with no duplicated chrome.
  It reads `Files::path` live (`ExplorerMessage::CopyCurrentPath` carries no path), so the
  button and the header can never name different directories, and it dims before the first
  listing when there is nothing to copy.

## 23. Terminal engine swap: vt100 → alacritty_terminal (v3.0)

The engine under the terminal is being replaced. This section records **why**, **which**,
and **how** — the decision so it is not re-litigated, and the staging so the work stays
shippable at every commit.

### Why replace it at all

`vt100` 0.16 is a deliberately small VT subset built on `vte`: it drops the control
functions it has no arm for and, decisively, **never stores a whole class of state**, so a
number of documented features cannot be rendered or reported no matter what is bolted on
beside it. cmote already papers over two of those gaps from outside the crate — `term/compat`
rewrites the cursor-move spellings it lacks (§9), `term/answer` replies to the DSR/DA
queries it never answers (§9) — but blink / strikethrough / conceal / undercurl /
underline-colour, the DEC line-drawing charset, DCS (sixel), custom tab stops, the autowrap
toggle, origin-mode-correct cursor reports and double-width lines are **unrepresentable** in
its data model. The full audited inventory, each gap tagged *bolt-on* (addable beside the
engine) or *engine* (needs replacing it), is in
[`TERMINAL_COMPATIBILITY_PLAN.md`](TERMINAL_COMPATIBILITY_PLAN.md). Everything tagged
*engine* there is what this swap buys at once.

### Which engine — decided: `alacritty_terminal` 0.26

A full VT implementation (the engine behind Alacritty): DEC charsets, scrollback, origin
mode, custom tab stops, the autowrap toggle, and the rich SGR set — dim, italic,
strikethrough, conceal, and single / double / curly / dotted / dashed underline plus
underline colour. Crucially it **answers host queries itself**: writing a DSR/DA/DECRQM or an
OSC colour query into it produces the reply through its `EventListener`
(`Event::PtyWrite`, `Event::ColorRequest`, `Event::TextAreaSizeRequest`), which **subsumes
`term/answer` and extends it** to the OSC-colour and pixel-size probes the old engine never
handled — all now wired (Stage 4). It
is **pure Rust and Apache-2.0** (already on the `deny.toml` allow-list via Material Icons),
so it keeps the no-C-toolchain portable build (§12) — verified: `cargo deny` clean, no C
compiler pulled, and it adds no new advisory over the three already accepted.

Alternatives weighed (grounded, not from memory):
- **`wezterm-term`** is strictly richer — it adds text-**blink** and inline **images**
  (sixel / kitty / iTerm2) — **but it is not published to crates.io** (git dependency or a
  pre-1.0 single-maintainer fork only), which fails the reliable-and-publishable bar for a
  portable app; and its image capability was unusable here without an image compositor we did
  not have. Revisit only if images become a hard requirement *and* it gets published
  (wezterm#6663, open, no ETA). **§41 settled that question the other way**: sixel arrives as a
  DCS this engine cleanly ignores, so cmote scans it out beside the engine and composites it
  itself — the engine choice never had to be revisited at all.
- **`termwiz`** alone is a parser + screen buffer, **not** a full emulator — using it would
  mean re-implementing the very state machine the swap exists to drop.

**Trade accepted:** no inline images and no text-blink attribute — both marginal or unusable
for us today. The major version bump to **v3.0** marks this core change. *(The images half of
that trade was bought back in §41, without touching the engine; blink stands, because the engine
drops the attribute before it can reach a cell — see the compatibility plan's §5.)*

### How — staged behind one seam, green at every commit

`term/mod.rs` was always meant to be the seam the emulator hides behind (§9), but the grid,
the selection extractor and the mouse encoder read `vt100::Screen`/`Cell` **directly** — the
leak that would spread the swap across the GUI. So the work is staged:

- **Stage 1 — seal the seam (done).** `term/screen` is a cmote-owned view — `Screen`, `Cell`,
  `Color`, `MouseMode`, `MouseEncoding` — with a method surface mirroring vt100's, so
  `ui/grid`, `ui/selection` and `term/mouse` change in *type* only. `Terminal::screen()` hands
  back this view; `vt100` is now named **nowhere outside `term/`**. A pure refactor, no
  behaviour change, so `app.rs` and `ui/terminal.rs` did not move at all.
- **Stage 2 — swap the engine behind the seam (done).** `term/mod.rs` and `term/screen.rs`
  now drive `alacritty_terminal`: bytes go through `vte::ansi::Processor::advance` with the
  `Term` as the handler; an `EventListener` collects the engine's `PtyWrite` replies into the
  bytes `process()` returns; `term/compat` and `term/answer` were **deleted** (the engine
  parses every cursor-move spelling and answers DSR / DA / DECRQM / cursor-position reports
  natively). The callers did not move. Gains: the DEC line-drawing charset, origin-correct
  cursor reports, custom tab stops, and the query replies — at once. **Accepted loss:** the
  ancient X10 mouse protocol (`?9`, press-only), which the engine does not implement, so
  `screen::MouseMode` lost its `Press` variant and `term/mouse` its X10 path — no current
  program asks for it. The `ColorRequest` (OSC colour) and `TextAreaSizeRequest` (pixel-size)
  replies were left collected-but-unwired at this stage — answering them needs cmote's colour
  scheme and cell metrics — and are wired in Stage 4.
- **Stage 3a — enrich the view, no new font (done).** `term::screen::Cell` now carries
  `dim`, `hidden` (conceal), `strikeout`, an `UnderlineStyle` (none / single / double / dotted
  / dashed / curly, read from the engine's distinct flags) and `underline_color` (SGR 58); the
  grid renders each — dim fades the foreground toward its background, conceal paints it *in*
  the background (the glyph stays for copy), strikeout and every underline style are quads we
  place ourselves (a font gives us none of them). All of these were attributes vt100 could not
  represent. No font asset needed, so no metric risk to the pixel↔cell grid.
- **Stage 3b — italic, from a bundled face (done).** iced draws glyphs from a named face and
  Fira Mono ships **no italic**, so italic needed one. The official Mozilla Fira release
  confirms it: Fira Mono is Regular / Medium / Bold only (Fira Sans has italics but is
  proportional). So italic and bold-italic cells draw from **IBM Plex Mono** (OFL 1.1), the
  closest humanist monospace whose advance is **verified identical** — 600/1000 em = 0.6, unit
  for unit with Fira Mono — so an italic run coalesces and lands on the same pixel grid as the
  upright text, no per-cell sealing needed. `term::screen::Cell` gained `italic`; `ui/grid`
  swaps the family (Fira Mono → IBM Plex Mono) and sets `Style::Italic` for italic runs, which
  are their own runs because the family is part of the run-grouping key. Bundled the real Fira
  Mono **Regular (400)** at the same time and made it the body weight (the terminal previously
  used Medium (500) as a stand-in only because Regular was not bundled). Each face is picked by
  exact family + weight + style, since `cosmic-text` does not nearest-match within a family.
- **Stage 4 — answer the colour and pixel-size queries (done).** The replies Stage 2 left
  unwired. A program asks the terminal its foreground / background / cursor colour (OSC 10 / 11
  / 12), a palette slot (OSC 4;n) or its text area in pixels (CSI 14t); the engine hands the
  listener a slot plus a formatter, and cmote resolves it against its own colour scheme and cell
  metrics, so the answer is exactly what the grid paints. The colour scheme moved to a new
  shared `palette` module — one source of truth for both the renderer and the answerer, so the
  two can never disagree (a terminal that misreports its background would defeat the theme
  detection the query exists for). The character-size query (CSI 18t) the engine already
  answered itself as a plain report; that still works. The GUI passes the cell pixel size down
  to the emulator once (`set_cell_pixels`), keeping the render metrics out of `term/`.
- **Stage 5 — show the window title the remote sets (done).** A program sets its title with
  OSC 0/2 (`vim README.md`, an ssh prompt's `user@host:cwd`); the engine parses it but has no
  public getter, so the reply listener captures `Event::Title` / `ResetTitle` into the same
  shared buffer, stripped of control characters (the title bar is chrome cmote owns — a remote
  must not smuggle newlines or escapes into it). `Terminal::title` hands it to `App::title`,
  which shows it in the third slot of the bar in place of the cwd, keeping the endpoint (§17).
- **Stage 6 — draw the cursor shape the remote picks (done).** DECSCUSR (`CSI Ps SP q`) lets a
  program choose a block, underline, or bar cursor (blinking or steady); the engine tracks the
  choice, `term::screen` exposes it as a `CursorShape` (the engine's "beam" renamed `Bar`, plus
  the unfocused `HollowBlock` and an explicit `Hidden`), and the grid draws it. The block shape
  keeps its old path — inverting its cell in the run planner, so a glyph under it stays legible;
  the other three are overlays drawn on top of an untouched cell after its row. Blink is dropped
  (cmote runs no animation timer), so every shape is steady, and DECTCEM hiding still wins.
- **Stage 7 — report focus to the remote (done).** Focus reporting (DECSET `?1004`): a program
  asks to be told when the terminal gains or loses focus, and the terminal answers `CSI I` /
  `CSI O` so it can undim, pause a spinner, or repaint. The engine tracks the mode but leaves the
  sending to the host, so `term::screen` exposes `focus_reporting()` and `app` watches iced's
  window `Focused` / `Unfocused` events. What counts as focus is cmote's call: the shell is
  focused only while the OS window is **and** the keyboard ring is on the terminal, so a switch to
  a side panel reads as a focus-out too — the remote is blind to cmote's panels, so it should hear
  about either. Every internal focus move funnels through one `set_focus`, and only a real change
  from the last reported state reaches the wire (a steady state is never re-sent); the state is
  reconciled after each output chunk, so a program toggling `?1004` mid-session is never stranded.
- **Stage 8 — scroll back over what left the screen (done).** `SCROLLBACK` is now
  **10 000** lines, so the engine keeps a bounded history. Because the viewport can sit above the
  live screen, `term::screen` exposes `display_offset()` and offsets every `cell` read by it (a
  viewport row maps to grid line `row − offset`, walking into the negative lines history lives on);
  the renderer adds the offset to the cursor's active-screen row and leaves the cursor undrawn once
  it drops below the viewport. `Terminal::scroll` takes a cmote-owned `ScrollMotion`
  (`Lines`/`PageUp`/`PageDown`/`Top`/`Bottom`) so the engine's `Scroll` type stays behind `term/`.
  Input: the wheel scrolls history whenever no mouse-aware program wants it (grid publishes
  `TerminalScroll`), **Shift+PageUp/PageDown** page and **Shift+Home/End** jump to the ends
  (Shift-guarded so the bare keys still reach the shell), and every keystroke (and paste) snaps the
  view back to the live bottom so what is typed lands where it echoes. New output leaves a
  scrolled-back viewport stationary in content — the engine grows the offset underneath, so reading
  is not yanked to the bottom by activity. The alternate screen keeps no history (vim/tmux/less
  manage their own pages), so scrolling is inert there by construction. The **scroll indicator**
  (chunk 2) closes the stage: `screen::history_size()` reports the retained depth, and the grid
  draws a thin, read-only thumb in its right padding gutter while the view is scrolled up (gone at
  the live bottom — "auto-hiding" without an animation timer, which cmote does not run). The thumb
  is the viewport's share of the whole document (`rows / (history + rows)`) with a floor so a deep
  history still shows a mark, and it slides from the track's top at the oldest line down toward the
  live tail. Scrolling stays on the wheel and keys — the thumb reports, it does not control. The
  geometry is a pure `scrollbar_thumb` (testable without a renderer, as with `corner_parts`).
- **§23 is complete:** the engine swap and every capability it unblocked (Stages 1–8) have landed.

### Security stays put

The engine surfaces OSC 52 clipboard requests as `Event::ClipboardLoad` / `ClipboardStore`;
those are **deliberately dropped** — the same policy as §9/§12, a remote must not read or
poison the local clipboard, and cmote only touches it on an explicit local action. The listener
answers only the events that expect a report — `PtyWrite`, and the colour and pixel-size queries
(Stage 4), each resolved to a fixed report with no `CR`/`LF`, so none can submit a command at a
prompt. Every other event — the clipboard pair, the bell, the title, a colour *set* — is
ignored, so nothing a remote sends can reach the clipboard or echo attacker-controlled text back
as input.

---

## 24. OSC 8 hyperlinks (v3.0.0)

A modern program can mark a run of text as a **clickable link** with the OSC 8 escape —
`ESC ] 8 ; params ; URI ST`, the text, then `ESC ] 8 ; ; ST` to close — so `ls --hyperlink`,
`gcc`'s diagnostics, and many TUIs attach a real URL to a file name or an error code. cmote
now follows those links.

### The engine already parses it; cmote surfaces and follows it

`alacritty_terminal` interprets OSC 8 itself: it records the link's URI on **every cell** the
run covers (an `Arc<Hyperlink>` shared across them). So there is nothing to scan out of the
stream — unlike modifyOtherKeys (§9) or the cwd (§17), this is screen state the engine holds.
cmote's job is only to **surface** it and **act** on a click:

- **Seam** (`term/screen.rs`): `Cell` gains a `hyperlink: Option<String>`, read back through
  `Cell::hyperlink()`. It is copied out per read like the cell's `text` — links are rare, so a
  blank cell still allocates nothing — which keeps the engine's own type off the seam (§16).
- **Follow** (`app.rs`): **Ctrl+click** on a link cell opens it instead of starting a
  selection — the modifier most terminals use, so a plain click still selects the link's text.
  A **right-click** on a link cell adds **Open link** and **Copy link** to the terminal's
  context menu (`ui/terminal.rs`), both carrying the URI — the one place the whole address is
  offered, handy when the visible text hides it.
- **Hover affordance** (`ui/grid.rs`, v4.0.0): while **Ctrl is held** and the pointer is over a link,
  the grid underlines the link's whole run — so the link reveals itself as one *before* the
  Ctrl+click that opens it, the reveal and the action sharing the one modifier. The grid finds the
  run by walking outward from the hovered cell while the OSC 8 URI stays the same (`link_run_at`);
  because a link's cells are laid out contiguously (one shared `Arc<Hyperlink>`) that run is a single
  reading-order span, which underlines correctly even across a wrap. A link cell that already carries
  an underline of its own keeps it, so the hover never downgrades a program's styling. It needs no
  new state or redraw plumbing: the app already repaints on every hover move and every modifier
  change, so the grid recomputes the run each frame from the pointer it is handed (`draw`'s cursor)
  and its own tracked modifiers (the widget `State`). `ponytail:` the affordance shows whenever Ctrl
  is over a link, even where a full-screen mouse program would eat the click — a harmless underline,
  not a promise the click is free.

### Opening is a security boundary

The URI is **remote-controlled**, and Windows opens a URI with whatever program is registered
for its **scheme** — so an arbitrary scheme (`file:`, a custom `vscode:`-style handler, …) is a
way for the remote to start a local program from a link. Two controls (`link.rs`):

- **Scheme allow-list** — cmote opens only **http / https / mailto** (the schemes a terminal
  link realistically needs) and refuses everything else, with a toast so a blocked click is
  never silent. `is_allowed` is a pure function, unit-tested apart from the launch (§13).
- **No shell** — the URI is handed to the `open` crate's default launcher, which passes it to
  PowerShell `Start-Process` as an **environment variable** (data, never a command line),
  falling back to `explorer.exe` with the URI as a single argument. The `cmd /C start` path —
  where a crafted query string (`https://x/?a=1&calc`) would let `&` inject a command — is
  behind an off-by-default `insecure` feature cmote does not enable. So even an allowed scheme
  cannot smuggle a command through the opener. The launch is fire-and-forget
  (`that_detached`), so the UI thread never stalls waiting on the browser.

This is the mirror of the §9/§12 clipboard stance: a remote may *describe* an action (a link,
a clipboard write) but cmote performs it only on an explicit local gesture and only within a
safe envelope.

---

## 25. Kitty keyboard protocol (v3.0.0)

The classic terminal input alphabet loses information the moment a key is anything but a plain
letter. `Ctrl+I` collapses onto Tab (both `0x09`), `Ctrl+M` onto Enter (`0x0d`); `Ctrl+digit`
and most `Ctrl+symbol` combos have no byte at all; `Esc` is indistinguishable from the start of
an escape sequence; and a key *release* is invisible. modifyOtherKeys (§9) patched the worst of
the Ctrl/Alt gaps; kitty's protocol replaces the lot with an unambiguous, opt-in encoding that
neovim, kakoune, helix, fish and a growing list of TUIs now speak.

### The engine tracks the state; cmote only encodes

This is the inverse of the modifyOtherKeys split. `alacritty_terminal` **fully implements** the
protocol's control plane: it parses the push (`CSI > flags u`), pop (`CSI < n u`) and set
(`CSI = flags ; mode u`) sequences, keeps the flag stack (bounded, and swapped across the
alternate screen so a full-screen program's mode is saved and restored), and **answers the
`CSI ? u` query itself**. All of it is gated behind one config flag, `kitty_keyboard`, off in
`Config::default()`. cmote flips it on in `Terminal::new` — so there is **no scanner** to write
(unlike modkeys) and **no reply path** to add (the query answer comes back as an
`Event::PtyWrite`, which the `Replies` listener already drains). The active flag set folds into
the engine's mode bits, and cmote reads it back off the seam (`Screen::kitty_flags` →
`keymap::Modes.kitty`). cmote's whole job is the other half: turning a key event into the
matching report, in `term/kitty.rs`.

The five progressive-enhancement flags, and how far cmote honours each:

- **disambiguate (0b1)** — the base every real client pushes. `Esc` becomes `CSI 27 u`, a
  `Ctrl`/`Alt` combo an unambiguous `CSI code ; mod u`. Fully encoded.
- **report events (0b10)** — press / repeat / release told apart with an `:event` sub-parameter.
  Fully wired: iced delivers `KeyReleased` and flags auto-repeat on `KeyPressed`, and `app`
  forwards both to the shell — the key-up a legacy terminal could never send.
- **report all keys (0b1000)** — even a plain letter becomes a code, not text. Encoded.
- **report associated text (0b10000)** — the produced glyph rides along as trailing code points.
  Encoded.
- **report alternate keys (0b100)** — the keycode gains the shifted glyph. Best-effort: cmote
  fills the shifted sub-field from the OS-produced text; the base-layout sub-field it cannot
  compute portably, so it is omitted.

### Encoding rules that matter (one wrong byte is a dead key)

- Keys that already had a **legacy escape code keep their final byte** — the arrows/Home/End/F1-F4
  stay letter-final with a fixed keycode of `1` (`Ctrl+Left` = `CSI 1;5D`), the navigation and
  F5-F12 keys stay `~`-final with their historic number — and only gain the modifier/event
  parameters. So a program that also knows the old sequences is never surprised, and a bare
  cursor key still honours DECCKM (SS3 under application-cursor mode).
- **Enter / Tab / Backspace** keep their C0 bytes (`\r` / `\t` / `0x7f`) until a modifier is held,
  then switch to the `CSI code ; mod u` form — the shell-compatibility carve-out the spec makes.
- A **release or repeat is reported only when the program asked for event types**; otherwise a
  release yields nothing and a repeat is a plain press. A **text key** (a bare or Shift-only
  letter) has no escape code to hang a release on, so its key-up stays silent until report-all
  promotes it to a code.
- The **modifier field is written even when empty** (as the bare `1`) whenever an event or text
  field follows it, so the later fields keep their place: a plain `a` under report-all +
  report-text is `CSI 97;1;97u`.

cmote leans on one simplification true of every real client: it treats the protocol as active
whenever *any* flag is pushed and always applies the disambiguating encoding — a program that
pushed, say, report-events without disambiguate (none do) would get disambiguated keys too,
which is harmless. The **numpad** stays on its NumLock text heuristic (§9, the `pm2 ls` fix)
ahead of the kitty branch, so a NumLock digit types its digit whatever the remote enabled;
`ponytail:` its kitty private-use keypad codes are not emitted, and **F13-F24** keep their fixed
legacy sequences under kitty rather than kitty's private-use code points — both rare, both
documented rather than chased.

### Why this over modifyOtherKeys

They answer the same need, and an editor speaks one or the other, so `keymap::encode` checks the
kitty flags first: an active set supersedes the modifyOtherKeys branch entirely. Kitty is the
stricter and more complete of the pair — it disambiguates *every* key, not just the Ctrl/Alt
main-keyboard combos, and it is the one modern editors reach for. modifyOtherKeys stays for the
programs that still emit only `CSI > 4 ; p m`.

## 26. Multiple sessions in tabs (v3.0.0)

The window held one session. Now it holds a **strip of tabs**, each a **fully independent**
session: a tab can sit on the home list while another runs a shell, dial a second connection while
the first stays live, and each keeps its own terminal, folder tree, files pane, selection and
dialogs. Two open connections no longer mean two windows.

### The split: `Tab` and `App`

The whole single-session state — everything `App` used to be — moved wholesale onto a **`Tab`**
struct, keeping its `update` / `view` / `title` and every helper unchanged. A new, thin **`App`**
owns a `Vec<Tab>`, the active index, and the two things that must be **shared**, not duplicated:

- the saved-target list (`profiles::Targets`) — one file on disk; a rename or delete in one tab's
  home screen must show in every other;
- the unlocked secret vault (`vault::Vault`) — one master passphrase; unlocking it in any tab
  unlocks it for all.

Both are held as `Rc<RefCell<…>>` and cloned into each tab, so a tab's home-screen and connect-flow
code reaches them through short borrows (`self.targets.borrow()`) with no parameter threading. The
home-screen list view is read through a borrow that outlives nothing — `ui::home::view` clones
every name it draws, so its `targets` argument has a lifetime independent of the returned element.
`window_size` / `window_focused` / `modifiers` stay per-tab (only the active tab receives those
events); the active tab's copies are carried onto a tab when it is opened or activated.

`App::update` intercepts tab-strip management and each session's SSH events, and delegates
everything else to the active tab. `App::view` draws the strip, then the active tab's own view
beneath it. `App::subscription` batches ONE worker per tab plus the active tab's keyboard listener.

### One worker per tab

The two-thread bridge (§4) was one worker for the whole app; now each tab gets its own.
`bridge::session_subscription(id)` is `Subscription::run_with(id, worker)` — iced keys a
subscription by its `(data, builder)` pair, so each tab id starts a DISTINCT worker (its own
network thread and `ssh::client::run` loop) and tears it down when the tab leaves the batch. The
protocol did not change: `SshCommand` / `SshEvent` / `client.rs` are untouched. Each tab's stream
is tagged with its id (`.with(id).map(…)`, since `map` demands a non-capturing closure) into
`Message::Ssh(id, event)`, which `App` routes to the tab that owns the session — so a **background**
tab's shell keeps drawing and its listings keep arriving while another tab is on screen. Closing a
tab drops it from the `Vec`; that drops its command sender, which ends its `run` loop, and removes
its subscription from the batch — a clean teardown from one `Vec::remove`.

### The strip, and the geometry it costs

`ui/tabs.rs` draws the strip: one chip per tab (its endpoint once connected, else the screen it is
on), tinted when active, each with a "×"; a trailing "+". **Mouse-only** (the chosen scope): a left
click on a chip selects (and grabs it, so the same press can drag the tab to another slot, §38), "×"
closes, "+" opens a fresh home tab. Closing a **live** tab first
raises a confirmation (like Disconnect, closing is not undoable); an idle tab closes at once.
Closing the last tab never leaves an empty window — it is a request to quit cmote, and takes the
quit confirmation (§30). Which tab comes forward when the active one closes is §37's rule, not the
strip's order.

The strip has a fixed height (`STRIP_HEIGHT`), so the terminal below it lives in a window that much
shorter. iced's `mouse_area` reports widget-local coordinates, so pointer math inside a tab is
unaffected; the one thing that reads the raw window size — the grid-fit and dialog-centre math — is
fed a height already reduced by `STRIP_HEIGHT` (`App` trims it off every `WindowResized` before
delegating), so the grid fits the space it has rather than overrunning it by a row. A tab switch
refits the newly shown terminal, in case the window resized while it was backgrounded.

## 27. Port forwarding (v3.0.0)

The live SSH connection can carry more than a shell. cmote now runs **port forwards** — tunnels —
over it, in all three shapes OpenSSH offers, managed from a **Tunnels** dialog on the terminal
status bar. A tunnel rides the same authenticated connection as the shell, so there is no second
login; the whole set for a target is **remembered and re-established on reconnect** (§22), because a
tunnel set (a database behind a bastion, a SOCKS proxy) is part of how a server is used, not a
secret.

### The three kinds

Exactly OpenSSH's `-L` / `-R` / `-D`:

- **Local (`-L`)** — cmote binds a local port; each connection to it is carried through the SSH
  connection and dialed **from the server** to a fixed target. `localhost:8080 → db.internal:5432`.
- **Remote (`-R`)** — the **server** binds a port; each connection there is carried back and dialed
  **from this machine**. `server:9090 → localhost:3000`.
- **Dynamic (`-D`)** — cmote runs a local **SOCKS5** proxy; every connection names its own target
  in the SOCKS handshake, each carried from the server. One tunnel, any destination.

The spec is pure data (`forward.rs`): the kind, the bind `host:port` (loopback by default — a
forward opened without thinking never exposes the tunnel to the whole network), and, for
Local/Remote, the target `host:port`. It parses from the dialog's two fields, validates, labels
itself for a row, and serialises for `targets.json`. A bare port is allowed on the bind side
(loopback assumed), and an **IPv6 literal is bracketed** on either side — `[::1]:8080`, as a URL or
OpenSSH writes it — so the address/port split is unambiguous; the host is stored without its brackets
and re-bracketed only when the pair is joined for a bind string or a row label (`join_host_port`).
`ponytail:` an *unbracketed* IPv6 that still carries a colon is refused with a message naming the
bracket form rather than mis-split on its last colon — the one place the split cannot guess.

### The one constraint that shapes the network layer

russh's session `Handle` is **not `Clone` and not `Sync`** — it owns a reply receiver — so **only
the one task that owns it** (`client::stream`) may open channels on it. A local listener therefore
cannot open its own SSH channel. The split (`ssh/forward.rs`):

- **Local / Dynamic (outbound).** A spawned listener binds the port and accepts. For Local the
  target is fixed; for Dynamic the listener runs the SOCKS5 handshake itself (no-auth, CONNECT only)
  to learn the target. It then hands the raw socket back to `stream` over a channel (`Accepted`).
  `stream` opens the `direct-tcpip` channel — the one place allowed to — and spawns a **detached**
  pump (`copy_bidirectional`), which owns the socket and the channel stream and needs nothing
  shared, so a long-lived tunnel never holds up the shell. `ponytail:` the SOCKS success reply is
  written optimistically, before the channel opens (the listener cannot open it); a channel that
  then fails shows the client a closed connection rather than a SOCKS error, one step later.
- **Remote (inbound).** `stream` asks the server to listen (`tcpip_forward`) and records the local
  target in a table **shared with the `Handler`** (an `Arc<Mutex>`). When a connection arrives the
  server opens a `forwarded-tcpip` channel back; russh delivers it to `Handler::
  server_channel_open_forwarded_tcpip`, which looks the bound port up in the table, accepts, dials
  the local target and pumps. Removal cancels the server listen (`cancel_tcpip_forward`) and prunes
  the table. `ponytail:` the table is keyed by port only.

### Letting the server choose the port (`-R 0`)

A remote forward may bind port **0** — `ssh -R 0:host:port`, "let the server pick a free port and
tell me which." The parser allows a 0 on the bind side for a Remote forward only (a local or
dynamic listener must name a real port; a target port never may). russh's `tcpip_forward` returns
the port the server chose for a 0 request (and 0 for a concrete one, per RFC 4254), so the worker
learns the bound port from the reply. Two things follow from not knowing the port until then: the
table is keyed by the **assigned** port (inserted after the reply, not before — a concrete request
still pre-inserts to close the "connection the instant it listens" gap, but a 0 request cannot, so
it accepts the same sub-millisecond window OpenSSH has), and the removal/cancel remembers the
assigned port too. The port is carried home on `ForwardReady { assigned_port: Option<u16> }` —
`Some` only for a 0-request — and the row shows it in place of the authored 0, while the persisted
spec keeps 0 so a reconnect asks for a fresh port rather than pinning an ephemeral one. Two `-R 0`
forwards no longer count as the same bind (both read as port 0, but the server assigns each a
distinct port), so the second is not refused as a duplicate. `ponytail:` a server that accepts
`-R 0` without naming a port is treated as a refusal rather than left dangling; and the cancel is
sent with the assigned port (best effort — a forward also dies with the session on teardown).

The `Forwards` manager `stream` owns holds the local listener tasks and the remote entries; dropping
it at session end aborts every local listener, and the remote listeners die with the connection —
a clean teardown from one scope exit.

### Watching the traffic (a live gauge)

The Tunnels dialog is a live monitor, not just a list: each active row shows `N open · M total` — the
connections crossing the tunnel **right now** and the number it has carried in all. The count is
driven from the same detached pumps that move the bytes. Each pump now knows the forward it belongs
to (`Accepted` carries the id for a local/dynamic tunnel; the shared table's value gained the id for
a remote one), so it can bracket its byte copy with a `ForwardConnectionOpened { id }` before and a
`ForwardConnectionClosed { id }` after. The app raises the open and total counts on an open and
lowers the open count on a close (saturating, so a stale close can never underflow); the total only
ever grows, which is what tells an idle-but-used tunnel (`0 open · 5 total`) from a never-used one. A
remote connection counts only once its **dial to the local target succeeds** — a connection that
reached nothing is not traffic. The gauge lives on `ForwardEntry` (pure data, unit-tested), so it is
display-only: nothing about it is persisted, and a reconnect starts a fresh count.

### The bridge, the state, and the dialog

Two commands (`AddForward { id, spec }` / `RemoveForward(id)`) and four events (`ForwardReady
{ id, assigned_port }` / `ForwardFailed { id, reason }` and the gauge's `ForwardConnectionOpened
{ id }` / `ForwardConnectionClosed { id }`) extend the protocol (§4); the id is app-assigned, keying a
forward to its outcome, its removal, and its activity. Each tab keeps a `Vec<ForwardEntry>` (id +
spec + status: Starting → Active / Failed, plus the assigned port and the live/total counts). The
add form (`ui::forward::ForwardForm`) is NOT beside it: it lives inside the open modal (§10), because
it exists only while the dialog does — closing the dialog throws a half-typed forward away, which is
what dismissing a form means, and the entry list is the thing that has to outlive it. The
**Tunnels** button (status bar, with the live
count) opens the manager — the shared dialog chrome (§10) with a row per forward (label, status dot,
remove ✕) above the add form (a kind selector, a listen field, a target field hidden for Dynamic).
Adding parses, refuses a duplicate bind before it is sent, queues the entry and asks the worker;
removing drops the row and tears the tunnel down. Both persist the set to the target. A forward's
**failure never tears the shell down** — unlike a session error, it just shows the row as failed. On
connect the target's saved forwards are re-established automatically; on any teardown the list
(and the worker's listeners) go with the session.

---

## 28. Host-key mismatch override (v3.0.0)

The one host-key case v1 dead-ended is the one that matters most in practice: a key that was
pinned but no longer matches. §8's TOFU refused it outright — `check_server_key` returned
`Ok(false)` and told the user to edit `known_hosts` by hand. That was safe but opaque, and the
common real cause (a server that legitimately rotated its key) had no path but a text editor. The
danger of a "connect anyway" button is real — it is how people get MITM'd — so the fix is not to
remove the friction but to make the decision **loud, informed and reject-by-default**.

### The flow

`check_server_key` no longer refuses a changed key on the spot. On `HostKeyVerdict::Changed { line }`
it reads the fingerprint currently pinned (`hostkey::stored_fingerprint`, parsing the offending
`known_hosts` line back to a key so the fingerprint is computed exactly as the presented key's is),
emits `SshEvent::HostKeyChanged { stored, presented }`, and **blocks the handshake** on the user's
choice — the same one-shot the first-contact gate uses. The GUI shows a loud dialog
(`Prompt::HostKeyChanged`, `ui::host_key_changed_view`): a red "possible man-in-the-middle" line
above **both** SHA-256 fingerprints — what was trusted vs what the server now sends, seeded into the
selectable `dialog_body` so either can be copied for out-of-band comparison. Three buttons:

- **Reject** — refuse. The safe default: the ✕, a backdrop click, Esc, and a GUI that went away all
  pick it. Locked by a test (§8), which is what stops a later refactor of the dialog wiring from
  quietly making some dismissal route trust the key instead.
- **Trust once** — connect this session only, leaving `known_hosts` untouched (it warns again next
  time). The safer override when the change is unverified or might be transient.
- **Replace key** — drop the stale line and pin the presented key, so future connections verify
  silently. The path for a confirmed rotation.

### The plumbing

One enum carries all of it: `bridge::HostKeyChoice { Reject, TrustOnce, Pin }`, sent as
`SshCommand::HostKeyResponse(HostKeyChoice)` (it replaced the old `bool`, so first contact and the
mismatch share one command). The Handler reads the choice against the verdict it is blocked on:
`Pin` **learns** a first-contact key or **replaces** a changed one (`hostkey::replace` = drop the
flagged line + `learn`), `TrustOnce` returns `Ok(true)` without writing, `Reject` returns
`Ok(false)`. A shared `Handler::await_decision` consumes the one-shot and treats a dropped sender as
`Reject`. `stored_fingerprint`/`replace` are line-indexed by the same 1-based number russh reports,
locked by tests (two hosts, so the line is not trivially 1).

### What is deliberately NOT here

- **No type-to-confirm.** A "type the host name to proceed" speed bump was considered and left out
  as disproportionate — the warning, the two fingerprints and reject-by-default are the friction.
- **No auto-trust, ever.** Every override is one explicit, informed click; nothing about a changed
  key is accepted silently. The line held is *no silent* override — never *no* override (§8).
- `ponytail:` `stored_fingerprint` reads the *first* key blob on the flagged line (the standard
  `host keytype base64` shape); an exotic hand-written `known_hosts` line surfaces "(could not read
  the stored key)" rather than a wrong fingerprint, and the dialog still opens with the presented
  key so the decision is never blocked on it.

### No `auth::Handshake` module — and why (v4.0.0)

An architecture review proposed lifting the whole connect flow into an `auth::Handshake` module
(`begin` / `answer` / `on_event`, returning a `Step`), on the reading that §7's prompts, §8's
host-key gate, §12's secrets and §16's vault were one feature scattered across `Tab`. It was
explored and **rejected**; recorded here so it is not re-suggested.

Most of the state it would have owned had already been collapsed by the change that landed just
before it: the six prompt-backing field groups became one `Option<Prompt>` (§10), which was the
bulk of it. What remained was four scalars — `pending_target`, `pending_remember`,
`pending_connect`, `passphrase_failed` — and a set of three-line message handlers. Against that,
the interface would have had to be nearly as wide as the body, which is what a shallow module is:

- **`Connected` is not an auth event.** Of its ~130 lines, about 25 are the handshake's (upsert the
  target, settle the remembered secret); the rest is §22's session restore, §27's saved forwards,
  §45's first identity, the emulator, the cwd replay. A `Step::Open(…)` would have to hand every
  auth-side capture back for the caller to finish the job, so the seam leaks at exactly the point
  it exists to close.
- **The vault prompt serves two flows, and only one of them is a handshake.**
  `VaultPending::Connect` resumes a deferred dial; `VaultPending::Prefill` fills a form field for a
  target the user merely *opened* (§14, §16). A `Handshake` owning that prompt would have to reach
  into the connect form for a case with no handshake in it at all.
- **Everything it would act on is owned elsewhere.** The vault (`Rc<RefCell<…>>`, app-wide), the
  target list (likewise), the form, the screen — each call would take three or four of them as
  arguments.

What the exploration DID find is in §16 above: the remembered-secret capture outliving the attempt
it belonged to. That is fixed. The friction was never the shape of the flow — it was one rule
stated at two ends with nothing joining them, which is now `abandon_attempt`.

## 29. Drag-and-drop upload (v3.0.0)

Uploading already had four doors (§17) — all of them a picker or a menu. This adds the obvious
fifth: drag files off the desktop and drop them onto the window, and they upload into the files
pane's current directory. As first shipped it took **one file** per drop; v4.0.0 lifted that to
**any number of files and folders at once**, each folder going tree-and-all — see "A drop is a set,
not a file" below.

### One direction only, on purpose

The gesture is **host → pane** only. iced (via winit) can *receive* an OS file drop — the
`window::Event::FileHovered` / `FileDropped` / `FilesHoveredLeft` events — but it has **no API to
originate** one, so a remote file cannot be dragged *out* onto the desktop. That half stays the
right-click **Download…** it already was (§19). Building a drag-*source* would mean native Win32
OLE (`IDataObject` + `DoDragDrop` with delayed rendering, WinSCP-style): Windows-only, unsafe, and
against the portable + learn-in-iced grain. Not worth it for one gesture with a working
menu equivalent.

### The flow

`file_drop_events()` is a second `event::listen_with` subscription beside `focus_events`, filtering
the window-event stream down to the three drag events. It is global (like focus and resize), so
`App` routes the resulting messages to the active tab:

- `FileHovered` → `Message::FileHovered` → light the pane's **drop ring** (a green border, distinct
  from the blue focus ring), but only with a live session — a hover over a home tab lights nothing.
- `FilesHoveredLeft` → `Message::FileDropLeft` → put the ring out.
- `FileDropped(path)` → `Message::FileDropped` → the path is **gathered**, not acted on.
- the next frame → `Message::FileDropSettled` → `transfer::Queue::settle` reads the whole set.

The drop events carry **no pointer position** (iced does not report one), so a drop cannot be aimed
at a widget — but it does not need to be: every drop targets the pane's own directory by definition,
which is the whole contract. So position is irrelevant and a drop anywhere on the window uploads
into the pane's folder.

`settle` reuses the entire §17 upload pipeline. It seeds the batch (the picked files, and the
destination = the pane's directory) and calls `send_batch` — the same entry point the
destination-confirm dialog calls — so the destination is **pre-scanned** and, on a name already
taken, the **same Overwrite / Keep both / Skip / Cancel dialog** opens (reused, not a new one).
There is no destination-confirm step: the drop already said where. A dropped **folder** goes the
other way, into the tree queue — travelling by the same command the menu's "Upload folder…" makes.

**Every upload now re-lists its destination.** When a batch (or a folder-tree upload) lands, the
queue asks for the destination to be re-listed (`Effects::refresh` → `refresh_remote_dir`, the same
helper a create or delete uses) — so if the files pane (or the tree) is showing the folder the file
went into, it re-lists in place and the new file appears without a manual Refresh. It is a no-op
when the pane is elsewhere, so an upload to the shell's cwd while the pane is pointed at another
folder costs no round trip. This fixes the drag-drop's most confusing gap: a file you drop onto the
pane you are looking at now shows up in it. The tree-upload flow keeps no queue, so it stashes its
destination in the batch's `dest` on start for the same completion to read.

### A drop is a set, not a file

The OS reports a drop of five files as **five events**, one per path, and says nothing about which
is the last. Acting on each as it arrives is what made the first cut single-file: the first path
would start a batch, and every sibling behind it would then be declined as "a transfer is already
running" — a drop of five files uploading exactly one, with a misleading notice for the rest.

So a `FileDropped` now only **gathers** its path, and a **frame clock** settles the drop: while the
queue is `settling()` the tab asks for `window::frames()`, and the tick reads the whole set at
once. It is the same shape as the toast's dwell and the find bar's re-scan (§10, §44) — a clock
that exists only while there is work for it — and it costs one frame, which is invisible against a
gesture that ends with a mouse button coming up. What it buys is that the *set* is what gets
decided about, which is the only way "these five files" can be one batch.

With the whole set in hand, each path joins the queue for its own kind: **files** seed the ordinary
batch (pre-scan, one collision question, queue), and **folders** go into the tree queue, each to
travel tree-and-all as its own recursive transfer. Both pipelines were already there; the drop
reaches them both, and **a drop may carry any mixture of the two**.

**Two queues, one slot.** A file and a folder are different transfers — a queued `(local, remote)`
pair against a whole recursive walk that asks its own collision questions as it goes — and there is
one progress bar, one cancel and one resume between them (§16, §17). So `Queue::pump` is the
one place that decides what runs next: it drains the file batch first, then starts the folders one
at a time, each on the landing of the last, and the downloads behind those. Files first because the
batch's collision question is answered **up front**, before a byte moves, so putting it first gets
the whole of the user's input out of the way; a tree asks as it walks and can be left to run.

Nothing else had to learn about the second queue, because a tree reports the same landing a file
does — so the existing completion path already walks the queue. The three things that did
change are the ones that count or clear: the closing notice (`upload_summary`, which names both
kinds rather than adding them into a meaningless total), the batch close (which must not run while
folders are still waiting, since it clears the destination they are going to), and the two cancels
(a deliberate cancel takes the whole drop, not just the item on the wire).

### The decision is pure

The guard logic is pulled into `drop_outcome(connected, busy, items, pane_dir) -> DropOutcome`, free
of `self` so it is unit-tested like `plan_uploads` and `band_hits`. It takes a **count**, not paths:
whether there is anything to send at all is the whole of what it decides, and sorting files from
folders is the caller's business. The order is deliberate:

- **no session** outranks everything — a drop onto a home tab is a silent `Ignore`, not a notice
  about something that could never have uploaded;
- a **busy** transfer (or a batch mid-setup — `Queue::busy` counts a picked-but-unconfirmed batch,
  which catches a menu upload waiting on its confirmation) is declined, one flow at a time behind
  the single progress bar;
- an **empty** set is silent: every dropped path would have had to vanish between the drop and the
  frame that reads it, which is not a mistake the user made;
- with a **real pane directory** it is an `Upload`; without one (nothing listed yet) it is `NoDir`,
  and the user is told to open a folder rather than the drop landing on a guess.

### What is deliberately NOT here

- **No drag-out** (pane → desktop): iced cannot originate an OS drag; see above.
- ~~**No folders, no multi-file** drop yet — single files only this iteration.~~ **Shipped in
  v4.0.0** — any number of files and folders, in any mixture; see "A drop is a set" above.
- **Still one transfer at a time.** The drop's own items queue, but a drop arriving while another
  transfer runs is declined as it always was (§16, §17): two progress bars, two cancels and two
  resumes are a different feature, not a bigger queue.
- **No positional targeting**: iced's drop events carry no coordinates, and the pane's folder is the
  only meaningful destination anyway, so a drop lands there wherever on the window it is released.
- `ponytail:` the drop ring lights on *any* hover over the window while connected, since the event
  has no position to test against the pane's bounds. It reads as "a file here will go to the pane",
  which is exactly true — but a positional highlight would need iced to report the pointer during a
  drag, which it does not.

## 30. Confirmed, clean quit (v3.0.0)

Two gaps closed at once: cmote used to *never* exit by closing tabs (the last tab silently reopened a
fresh home tab, §26), and clicking the OS window's title-bar **×** tore the process down on the spot —
whatever sessions were live got their sockets yanked, not a clean SSH disconnect. This section makes
leaving deliberate and tidy: closing the last tab, or the window ×, asks first, and the app exits only
once **every remote connection has closed cleanly**.

### Leaving is one path

Both exit routes funnel into one `QuitPhase` state machine on `App`:

- **The last tab's close** (its chip ×, or Ctrl+D on a home tab — see below). `request_close` spots
  `tabs.len() == 1` and, rather than reopening a home tab, calls `request_quit`.
- **The OS window ×.** `exit_on_close_request(false)` on the builder stops winit from closing the
  window itself; the request instead arrives as a `window::close_requests()` event, mapped to
  `Message::QuitRequested`. Per the chosen UX it **always** confirms — even with nothing live — so a
  stray × on the title bar never drops sessions by surprise.

`request_quit` puts up the **Quit cmote?** dialog (`QuitPhase::Confirming`), floated over the whole
window, strip and all, like the per-tab close dialog it outranks. It reports how many live sessions the
quit will disconnect. Esc / Cancel backs out; Enter / **Quit** accepts. While it is up it is modal
app-wide: `App::update` intercepts every keystroke (`quit_key_intercept`) so none reaches the shell
underneath — Esc and Enter drive the dialog, everything else is swallowed.

Like every other dialog (§10), both overlay cards — this quit card and the per-tab close confirmation
(§26) — are **draggable by their header**. But because they float over the *whole* window rather than
one tab, their position cannot live on any single `Tab`; it lives on `App` as one `ui::dialog::Card`
(`overlay`), seeded by `Card::opened(self.window)` each time an overlay goes up. While an overlay is up
(`overlay_open`), `App::update` steers the shared `DialogGrabbed` / `DialogDragged` / `DialogReleased`
to itself instead of delegating them to the active tab, which otherwise drives its own dialogs' card.
The only difference between the two paths is the box handed to `drag_to` — the full window here, strip
included; the tab's own region there (§48) — which is exactly the difference that used to justify two
copies of the arithmetic, and now does not (§10).
Until this, the two overlay cards were the only dialogs pinned centred every frame — a stray exception
to §10 that a drag now closes.

### Draining, so nothing is cut mid-flight

The SSH workers each run on their own tokio runtime (§4), off the GUI thread. `iced::exit()` ends the
*process*, killing those runtimes — so exiting the instant Quit is pressed could sever a session before
its clean teardown (`channel.eof()` → break → `SshEvent::Disconnected`, §6) has flushed. So `quit`
does not exit immediately:

- `quit_confirmed` snapshots the ids of the live tabs, persists each session (§22), sends every one a
  `SshCommand::Disconnect`, and enters `QuitPhase::Draining { pending, since }`. With **nothing live**
  there is nothing to wait for, so it exits at once.
- Each session's clean shutdown ends with `SshEvent::Disconnected` (or an `Error`). `App::update`
  already routes those to the owning tab; it now also notes them (`note_drained`), striking the id off
  `pending`. When `pending` empties — every connection down — it returns `iced::exit()`. So the process
  leaves only *after* the last socket has closed politely.
- **Liveness safety net.** A wedged transport must not hold quit open for ever. While draining, the
  frame clock (`window::frames()`, the same one the toast uses) ticks `Message::QuitTick`, which
  compares `since.elapsed()` against `QUIT_DRAIN_TIMEOUT` (2 s) and forces the exit if a session never
  acknowledged. It is a backstop only: a local channel EOF finishes in milliseconds, so it is never hit
  in practice.

While draining, the dialog swaps to a button-less **Quitting cmote…** card; `quit_cancelled` is inert
once past `Confirming`, so a stray backdrop click cannot abort a teardown already under way.

### Ctrl+D — only once logged off

Ctrl+D closes the current tab, but **only from the home screen** (`on_home_key`) — that is, once you
are logged off from any remote. On a live shell Ctrl+D is EOF to the remote (the way you *log out*), so
binding it to close-tab there would steal a core terminal key; it is left to the encoder. The two read
as one gesture: Ctrl+D at the shell logs out → lands back on the home screen → a second Ctrl+D closes
the tab — exactly a terminal's own "Ctrl+D twice" to close a window. It routes through the same
`TabCloseRequested`, so Ctrl+D on the *last* tab still asks to quit cmote. (**Ctrl+W** was the obvious
alternative — the universal close-tab key, no EOF conflict — but the shell-EOF-then-close pairing was
the chosen feel.)

### What is deliberately NOT here

- **No quit on a non-last tab close.** Closing a tab when others remain is unchanged (§26): an idle tab
  goes at once, a live one asks the per-tab Disconnect confirmation. Only the *last* close is a quit.
- **No forced kill.** Quit waits for a clean disconnect (bounded by the timeout); it never SIGKILLs a
  session to leave faster.
- `ponytail:` the drain waits on the **live (Terminal-screen) tabs only**. A tab still handshaking has
  no shell to disconnect and its worker unwinds when its link drops; the timeout covers any straggler.

## 31. App-wide window size, and pane-handle feedback (v4.0.0)

Two small layout niceties. cmote already remembered the panel sizes per target (§22); it did
not remember the WINDOW, so every launch opened at the built-in default however the user last
sized it. And the two resize handles were bare bars — no cursor, no answer to the pointer — so
that they were grabbable at all was something you learned by trying. This section fixes both.

### The one file that is not per-target

- **`settings.rs` owns `settings.json`**, beside `targets.json` in the shared data directory
  (§11). It holds exactly one thing today: the OS window's size as `(width, height)`. Almost
  everything cmote persists is per-target (§14, §22) — but the window is app-wide (there is one,
  and it shows before any target is chosen), so it cannot live on a `Target`.
- **A settings file must never stop the app from starting.** Absent, truncated, wrong-typed or
  hand-edited nonsense each reads as "no preference" plus a line on stderr — so neither `load`
  nor `save` returns a `Result`. `sanitized` is a trust boundary: a stored size that is
  non-finite or outside `[480, 4096]` is dropped whole, back to the first-run default.
- **The 4096 ceiling is a renderer limit, not cosmetics.** wgpu guarantees a maximum texture
  dimension of only 8192 *physical* pixels, and a surface is measured in physical pixels, so a
  2× display doubles what is asked for. 4096 logical points leaves the margin, so a stored size
  can never crash `Surface::configure` at launch — the one place a "harmless" settings file
  could otherwise kill the app before its first frame.
- **The position is deliberately NOT kept.** A window restored onto a monitor since unplugged is
  worse than a centred one.

### Where it plugs in

- **`run` opens at `Settings::load().window_size()`**, falling back to the metric-derived default
  (`ui::terminal::window_size`, a 180-column grid plus the browser strip) on a first run — so the
  default still tracks the grid metrics rather than being a frozen literal.
- **`App` holds a `Settings` and updates it on every resize.** `WindowResized` carries the whole
  OS window, before the tab strip is subtracted (§26), so `set_window` records the true outer
  size; a degenerate size (a minimize can report 0 × 0) is ignored so the last good one survives.
- **`exit_app` is the single way out.** Every quit path (§30) — the confirm with nothing live, the
  drain finishing, the drain timing out — funnels through it, so the size is written exactly once
  however the app comes down. `save` is a no-op on a default-valued `Settings`, so a run that
  never resized (and every unit test) leaves the file untouched rather than clobbering a good one.

### The handles answer the pointer

- **A resize cursor over each bar.** The tree's vertical splitter wears `ResizingHorizontally`
  (↔, it sets a width); the pane's horizontal splitter wears `ResizingVertically` (↕, it sets a
  height). The transparent full-window capture layer added during a drag (§18, §19) wears the
  same cursor, so the arrow does not flicker back to the default when the pointer leaves the thin
  bar mid-drag.
- **The bar lights while it is the active handle** — hovered *or* being dragged
  (`splitter_active` = `dragging || splitter_hovered`, on both panel models). Resting it is the
  panel grey `SPLITTER_BG`; active it is the brighter `SPLITTER_HOVER`, shared by both handles so
  they feel identical. Hover is fed back by the bar's own `mouse_area` `on_enter`/`on_exit`
  (`SplitterEntered`/`SplitterExited`), which touch only the highlight — no relayout, so no grid
  refit. This is the hand-rolled equivalent of what a `pane_grid` splitter gives for free; cmote's
  splitters are custom `mouse_area` bars (they drive the pty reflow), so the feedback is explicit.

---

## 32. Remote text editor in a tab (v4.0.0)

A **basic text editor** for a remote file, opened in **its own tab** in the strip (§26). Until now
a tab was always a session (a home list or a live shell); now a tab can also be an editor, so the
one strip manages both. The editor opens a file over SFTP, shows it with **line numbers** and a
**changed-line** gutter, and can **save**, **save as** a new remote file, and **close** — asking
about unsaved changes first. It is deliberately small: no syntax highlighting, no find/replace, no
split panes. It is for the "just fix this line in the config" job that otherwise means launching
`vi` in the shell.

Same three-way split as the panels (§18, §19): a pure model (`editor.rs`), a pure view
(`ui/editor.rs`), and the network calls (`ssh/edit.rs`) — so the rules that carry the weight
(encoding, line-ending, the changed-line diff) are unit-testable with no server.

### The tab that is not a session

- **A `Tab` now carries an `editor: Option<Editor>`.** When it is `Some`, the tab renders the
  editor and is not a session: it has **no connection and no SSH worker of its own**. `App`'s
  per-tab worker subscription (§26) skips an editor tab, so opening ten editors starts no network
  threads.
- **An editor tab is parented to the session it was opened from** (`Editor::session`, the parent
  tab's id). It has no socket, so every SFTP load and save is sent on the **parent session's**
  command channel — `App` looks the parent up by id and sends through its `command_tx`. Close the
  parent session and its editors go **read-only-save**: the buffer stays open to read and copy, but
  Save is disabled with a note, because the pipe it would write through is gone.
- **Edit events are correlated by an `editor_id`, not the session id.** The worker tags every
  event with the *session* tab's id (§26), but a loaded/saved file belongs to the *editor* tab that
  asked. So `EditLoad` / `EditSave` carry the editor tab's id and the matching
  `EditLoaded` / `EditSaved` / `*Failed` echo it back; `App` routes those to the tab whose id
  equals the `editor_id`, whichever session produced them. Two editors loading at once cannot cross
  their bytes.

### Opening a file

- **Two ways in, both from the files pane (§19).** A new **Edit…** item on the entry context menu
  (files only, disabled on a directory or a multiple selection), and a **double-click on a file** —
  which until now did nothing (double-click only browsed *into* a directory). Both emit
  `FilesMessage::EditStarted(path)`, which the tab turns into an App-level `EditorOpen` carrying the
  parent session id and the path; `App` creates the editor tab **right beside the session it came
  from** (§38), makes it active, and sends `EditLoad` on the parent's channel.
- **A size ceiling.** The whole file is held in memory as one editable buffer, so a file over
  `edit::MAX_SIZE` (**8 MiB**) is refused before it is pulled — a text editor is not a way to open a
  disk image. The refusal is a message, not a crash.
- **Undecodable is refused, not mangled.** If the bytes are not text in a supported encoding (a
  binary, or a legacy charset cmote does not decode), or the file is over the ceiling, the editor
  tab shows the reason in place of the buffer with only a **Close** — never mojibake that a save
  would then persist.

### Encoding — detect, preserve, never assume on save

The rule the user set: *keep a BOM if the file has one; a BOM decides the UTF; with no BOM assume
UTF-8 without one; refuse what cannot be opened; on save, persist exactly as opened.*

- **Detection is BOM-only, then UTF-8.** A leading byte-order mark picks the encoding —
  UTF-8 (`EF BB BF`), UTF-16 LE (`FF FE`), UTF-16 BE (`FE FF`). No BOM ⇒ **UTF-8, no BOM** (the
  default, and the common case). A UTF-32 BOM, or bytes that do not decode cleanly under the chosen
  encoding, are the "unsupported" case above. There is **no** statistical charset guessing and **no**
  Windows-1252 fallback — a deliberately narrow, predictable set, decoded in-house (`char::
  decode_utf16` and friends), so no `encoding_rs` dependency.
- **`Encoding` is remembered on the model** as `(charset, had_bom)`. Save re-encodes the current
  text under that exact charset and re-prepends the BOM **iff** the file had one — so an edit never
  silently converts UTF-16-with-BOM into UTF-8, nor strips a BOM the file arrived with. cmote's own
  default *for a brand-new Save As target* is UTF-8 without a BOM, but an opened file keeps whatever
  it was.
- **Line endings ride along, and iced already detects them.** `text_editor::Content::line_ending()`
  reports the buffer's ending (`Lf` / `CrLf` / `Cr` / …); cmote reads it once at load to show it and
  writes it back through `Content::text()`, which reassembles the buffer with that ending. A file
  with no newline at all defaults to **LF**. The editor does not normalise endings behind the user's
  back — the same "persist as opened" rule.

### Line numbers, and marking what changed

- **A gutter column beside the editor, both inside one `scrollable`.** iced's `text_editor` scrolls
  internally and does not expose its scroll offset, so a naive gutter placed alongside would desync
  the moment the text scrolled. cmote sidesteps it: the editor is laid out at `Wrapping::None` with
  its **height shrunk to the whole content** (`Content::line_count()` × the line height), so it never
  scrolls *itself* — a single outer `scrollable` moves both the gutter and the text together, and the
  numbers stay pixel-aligned with their lines **by construction**, no offset tracking. The wheel and
  the scrollbar move the view; a cursor move scrolls the outer scrollable to follow (see "Moving
  through the file" below). **The gutter is virtualised**: iced rebuilds and lays out the whole view tree
  every frame, so one number-widget per line made the gutter the dominant per-frame cost on a big
  file — a 50k-line file meant ~50k rows of nested containers built and laid out every frame, while
  the buffer beside it stays a *single* `text_editor` whose off-screen glyphs the renderer already
  clips. So the gutter now materialises only the rows the outer scrollable currently shows (plus a
  small overscan), collapsing the lines above and below into one spacer each — the same three-piece
  trick the find-line band uses — so its total height is still `count × LINE_HEIGHT` and it stays
  aligned with the text **by construction**. The visible window is read from the offset and height the
  scrollable already reports; until the first frame measures the viewport every row is drawn, that
  pre-virtualisation cost paid once. The window arithmetic (`visible_lines`) is a pure function with
  its own unit tests. (`ponytail:` one v1 limit left: a line longer than the pane is clipped at the
  right edge with no horizontal scrollbar — the cursor still reaches into it, but a fixed bar would
  need a second offset-synced scrollable. The *vertical* cursor-follow, once missing, is done.)
- **Changed lines are marked from a diff against what was loaded.** The model keeps the `original`
  lines from the moment of load; on every edit it recomputes which current lines differ and the
  gutter draws a bar on each changed or added line. The diff is a **common prefix/suffix trim**
  (which localises the change to a band) followed by a **bounded LCS within that band** — so a single
  inserted or edited line marks only *itself*, not everything below it, while an edit band larger than
  `LCS_BAND_CAP` lines falls back to marking the whole band rather than paying a quadratic diff.
  Recomputed on each edit (not each frame), so it is off the render path. Saving makes the current
  text the new `original`, so the marks clear. This is what "highlight unsaved modifications" asks for
  — per-line, not just a global flag.
- **A dirty dot rides the tab chip and the toolbar** whenever the buffer differs from `original`, so
  an editor with unsaved work is obvious in the strip without opening it. `strip_label` shows
  `• name.conf`.

### Save, Save As, Close

- **Save writes atomically.** `edit::save` streams the bytes to a temp sibling (`name~cmote.tmp`) on
  the remote and then **renames it over** the target, so a connection dropped mid-write can never
  leave the user's file half-written — the rename is the commit point. On success the editor's
  `original` is reset and the marks clear; a failure keeps the buffer dirty and shows why. SFTP v3's
  rename will not overwrite an existing name (OpenSSH's included), so an overwrite-save falls back to
  removing the target then renaming — and if THAT rename fails after the target is already gone, the
  temp is the file's only remaining copy, so it is **kept and named in the error** for a manual rescue
  rather than deleted (a stray `.tmp` beats losing the content).
- **Save As names a new remote file.** There is no native "save to remote" dialog (rfd is local
  only, §19), so Save As opens a small in-editor prompt for the destination path, pre-filled with the
  current directory and name. Confirming saves there and **re-points** the editor at the new path (it
  becomes the file being edited), the same as every editor's Save As. It is written in the **same
  encoding the buffer was opened as** (BOM and all) — "persist as opened" holds for a copy too; Save
  As is a new *name*, not a new *format*.
- **Closing asks about unsaved work.** Closing a clean editor tab drops it at once. Closing a
  **dirty** one raises a three-way confirmation over the whole window (like the live-session close,
  §26, but with the extra choice): **Save** (write, then close), **Discard** (close, lose the edits),
  **Cancel**. A dirty editor is thus as protected as a live shell — the strip's "×", not just an
  explicit menu, routes through it.

### Theme, per file type

- **Two schemes, chosen per tab.** The toolbar carries a small two-option select: **Default** —
  cmote's own dark panels, the same family as the files pane and dialogs — and **CME**, the colours
  of the user's VS Code theme (*Themer My Color Set Dark*), ported from that theme's own `editor.*` /
  `editorGutter.*` values (dark-teal ground `#1a2a30`, white text, a light-blue change marker
  `#aaddff`, a faint cyan selection, an orange warning tint). Only the buffer, gutter and toolbar are
  themed; the shared close ✕ (§10) stays a neutral glyph.
- **The choice lives in the model, the colours in the view.** `Editor::theme` holds only the
  `EditorTheme` enum; `ui/editor.rs` resolves it to a `Palette` and threads a `&Palette` into every
  drawing helper — so the model/view split (§18) holds, and two editor tabs can wear different schemes
  at once.
- **Remembered per extension, App-wide, across restarts.** The pick is recorded in `Settings` as a
  `HashMap<extension, EditorTheme>` keyed by `editor::extension_key` (lower-cased, no dot; `""` for a
  file with no extension). Opening a file seeds its editor from that map, so a `.json` reopens in the
  scheme JSON was last edited in, independent of what a `.ts` or `.php` tab is set to. The message is
  App-level (`EditorThemeSelected`), not tab-local, precisely because the memory is App-wide — and it
  now **persists**: the map rides `settings.json` beside the window size (§31), written on the single
  `exit_app` funnel and read back at launch, so the scheme a file type wears survives a restart. It
  lives in `settings.json` (app-wide), not `targets.json` (per-target), because "CME for `.rs`" is a
  preference about a file *type*, the same whatever server the file is on. `EditorTheme` serializes
  lower-cased (`"default"` / `"cme"`) for a legible hand-editable file, and the map is skipped from the
  JSON while empty so a first run stays `{}`; an unrecognised scheme name there fails the parse back to
  defaults, the same "a bad file never stops the app" rule the rest of the settings schema follows.
- **CME turns on syntax highlighting; Default stays plain.** Under CME the buffer is highlighted so a
  file reads like it does in the user's VS Code — colours and all. It is an iced `Highlighter`
  (`ui/syntax.rs`) driven by `text_editor::highlight_with`, backed by **syntect** with the big
  **two-face** grammar pack (TypeScript, PHP, TOML, … well past syntect's own defaults). The engine is
  iced's own `iced_highlighter` ported almost verbatim — including the snapshot cache that re-parses
  from the nearest `LINES_PER_SNAPSHOT` boundary rather than the top — with **one** deliberate change:
  the theme. iced's built-in highlighter can only pick a bundled `.tmTheme`; we build a `syntect::Theme`
  from the CME theme's own `tokenColors`, so the scope colours are exactly the user's (comment
  `#aaaaaa`, string `#ffffbb`, keyword `#00ddff`, …). A scope the CME theme leaves alone yields no
  modifier, so that token keeps the flat `value` colour — the highlight sits over the scheme, not
  instead of it. **syntect uses its `fancy-regex` backend (pure Rust), not Oniguruma**, so the no-C
  portable build holds (§11).
- **The grammar is resolved by more than the extension.** `resolve_syntax` widens the match, most
  specific first: the whole file NAME as a token (Sublime grammars register bare names like `Makefile`,
  `Dockerfile`, `.gitignore`, `.bashrc`, `CMakeLists.txt` among their "extensions"), then the extension
  alone (`main.rs` → Rust), then the buffer's first line as a shebang / mode-line (`#!/bin/sh` → bash,
  `#!/usr/bin/env python3` → Python), and only then plain text — so a name-only file, a dot-file, or an
  extensionless script highlights instead of dropping to plain. The highlighter's iced identity is the
  RESOLVED grammar's name, not the raw inputs, so a file that resolves by name or extension keeps a
  stable identity when its first line is edited (the shebang never enters it) — no needless re-parse;
  only a truly extensionless script re-resolves, correctly, when its own shebang changes.

### Moving through the file — find, and following the cursor

- **The cursor stays on screen, on both axes.** The vertical scroll lives on the buffer's scrollable
  (the gutter trick defeats the widget's own vertical scroll) — so a plain arrow-down past the foot of
  the view used to wait on a wheel nudge. Now the buffer's scrollable reports its offset and visible
  size (`on_scroll`, first frame included), and after any cursor move `App` runs the panels' own
  `keep_visible` over the cursor line — `cursor().position.line` × the fixed `LINE_HEIGHT` — and issues
  a `scroll_to` on the buffer's id. The same follow serves a Find jump, so a match off-screen is
  scrolled onto it.
- **Horizontal scrolling — the last long-line gap closed (v4.0.0).** The buffer now has a real
  **horizontal scrollbar** and wheel, so a line wider than the pane is reachable without arrowing the
  cursor into it. It could not be a bar synced to the widget's *own* horizontal scroll — iced hides that
  offset exactly as it hides the vertical one — so the same trick the height uses is applied to the
  width: the `text_editor` is laid out at an explicit fixed **content width** (its widest line from
  `Editor::content_columns` × the fixed `CHAR_ADVANCE`, never below the viewport), so it never scrolls
  itself, and a `scrollable::Direction::Both` supplies the visible bar and the wheel. That forced the
  **gutter out of the shared scrollable** — a `Both` bar at the viewport foot cannot coexist with a
  vertical scroll shared with a pinned gutter — so the gutter is now a `pin` translated up by the
  reported offset (`pin` clips its child to its bounds), a *pure function* of that offset with **zero
  sync lag**, preserving the old pixel-perfect lockstep without sharing a scrollable. A fixed-width
  editor no longer follows the cursor's column itself, so `App` gained a **horizontal cursor-follow**
  mirroring the vertical one (`col_x(cursor_display_column)` through the same `keep_visible`), and every
  `scroll_to` now carries *both* offsets so following one axis never zeroes the other.
  (`ponytail:` the content width is a display-column estimate — tabs expanded to 8, every other glyph
  one column — so a double-width CJK line is under-measured by a hair, its extent a touch short, never
  long; ASCII source, the common case, is exact.)
- **Find / replace, in a bar above the buffer (Ctrl+F).** A small bar rides over the top of the buffer
  (pushing the text down, so a top match is never hidden behind it): a query field with a live
  `n / total` count and prev / next steppers, a toggle for a replace row, and the shared close ✕ (§10).
  Enter in the query steps to the next match, Esc closes, Ctrl+H or Ctrl+R opens straight onto the
  replace row.
  The search is **ASCII case-insensitive** (both sides `to_ascii_lowercase`, which preserves every byte
  offset, so a hit found in the lowered copy is valid in the original — a non-ASCII case pair like
  `é`/`É` stays distinct, the same narrow-and-predictable spirit as the encoding set). Matches are
  `(line, byte range)` because iced addresses the cursor and selection by **byte** index within a line;
  stepping selects the span and the cursor-follow scrolls it in.
- **The current match's line is washed, and its number lit.** iced paints a `text_editor`'s own
  selection **only while that editor is focused** — but during a search the find *field* holds focus,
  so the selected match would be invisible. So the current match's **line** carries a translucent band
  (drawn BEHIND the text in a `stack`, the buffer laid transparent over it; three fixed spacers, not a
  widget per line) and its **gutter number** is lit on the same wash — both visible whatever holds
  focus, and both immune to tab-column geometry since they key off the line, not the byte column. (A
  glyph-exact always-on highlight would mean recolouring the match bytes through the highlighter, which
  is CME-only and would have to splice against the syntect spans — the line wash is the simpler, tab-
  correct choice.)
- **The model owns the search; the pure parts are tested with no widget.** `Editor::find:
  Option<Find>` holds the query, every match and which is current; it is recomputed on every edit (for
  the count) but only *re-selected* on an explicit step, so typing never yanks the cursor onto a hit.
  `find_matches` (all occurrences, in document order) and `apply_replacements` (splice each line's
  matches right-to-left so earlier offsets stay valid) are plain functions with unit tests. **Replace**
  pastes over the current selection — keeping the widget's undo — then re-searches; **Replace All**
  rebuilds the buffer from the matches already found (so what changes is exactly what was highlighted)
  and re-seats it as a fresh `Content`, which resets undo, the accepted cost of a bulk edit. The
  rebuild reassembles the buffer with each line's OWN ending (the way iced's `Content::text` does, via
  `join_with_endings`), so a mixed-ending file's untouched lines are not normalised to one ending.
  (`ponytail:` Replace All swaps each *original* match once, so a replacement that itself contains the
  query does not cascade; a single Replace, being manual, can re-hit a replacement that still matches.)

### Where it plugs in

- **`bridge.rs`** gains `SshCommand::EditLoad { editor_id, path }` /
  `EditSave { editor_id, path, bytes }` and `SshEvent::EditLoaded { editor_id, path, bytes }` /
  `EditLoadFailed { editor_id, reason }` / `EditSaved { editor_id, path }` /
  `EditSaveFailed { editor_id, reason }`. The bytes cross the channel raw; decoding is the model's
  job on the GUI side, so the network layer stays encoding-agnostic.
- **`ssh/edit.rs`** is the twin of `download`/`upload` (§17, §19) but buffer-shaped rather than
  file-shaped: `load` reads the whole remote file into a `Vec<u8>` (bounded by `MAX_SIZE`), `save`
  writes a `Vec<u8>` atomically. It reuses `open_sftp` (§17).
- **`ssh/client.rs`** dispatches the two new commands; **`app.rs`** owns the tab wiring (open, route
  by `editor_id`, save/save-as/close, the editor keyboard shortcuts — Ctrl+S save, Ctrl+Shift+S save
  as, Ctrl+W close, **Ctrl+F find, Ctrl+H / Ctrl+R replace, Esc close-find** — the per-extension theme
  memory,
  and the cursor-follow scroll over the buffer's scrollable id); **`ui/editor.rs`** is the
  toolbar-plus-gutter-plus-editor view, the find/replace bar, and the two-scheme `Palette`;
  **`ui/syntax.rs`** is the
  syntect-backed `Highlighter` and the CME `syntect::Theme` (behind the `two-face` dependency, pure-Rust
  `fancy-regex`); **`ui/files.rs`** adds the Edit… item and the double-click-a-file path.

## 33. Answering the identity queries the engine drops (v4.0.0)

The terminal engine (`alacritty_terminal`, §23) answers the queries that touch the grid — DSR,
DA, DECRQM, cursor-position and text-area reports — and cmote drains those replies straight through
(§9). Three it does **not** answer, because its VT parser treats every DCS string as a no-op (its
`hook`/`put`/`unhook` only log) and it has no CSI arm for the version request:

    CSI > q            XTVERSION  — "what terminal are you, and which version?"
    DCS $ q <sel> ST   DECRQSS    — "what is setting <sel> right now?" (Request Status String)
    DCS + q <hex> ST   XTGETTCAP  — "what is your value for terminfo capability <hex>?"

A program that sends one (tmux, neovim, notcurses, kitty-aware TUIs) waits for a reply; unanswered,
it stalls until a timeout, and some paste the query back as literal garbage. So cmote sniffs these
out of the stream itself — the same tactic `cwd` and `modkeys` use for the sequences the engine
ignores — and formats a reply.

- **A separate out-of-band scanner (`term/query.rs`).** A byte-at-a-time state machine, chunk-safe
  like `modkeys`, run in `process` **before** the engine advances. It parses only — it holds no
  engine state — and returns the queries that completed in the chunk; `term/mod.rs` turns each into
  a reply and appends it to the engine's own reply bytes. Unlike `modkeys`/`cwd`, which observe, this
  scanner **emits**. An unrecognised DCS (a sixel image, a reply) is followed to its terminator all
  the same, so its arbitrary data — the one place a stream legitimately carries raw bytes — cannot
  masquerade as a fresh query. It shares the `CSI >` prefix with `modkeys` (which ends in `m`) and the
  kitty query (`u`) and DA2 (`c`); the scanners are independent and key off the distinct final byte.
- **XTVERSION → cmote's identity.** `DCS > | cmote(<version>) ST`, the version stamped from
  `CARGO_PKG_VERSION` at build time so the reply never drifts from the binary. Static, no state.
- **XTGETTCAP → only the facts cmote can state truthfully.** The terminal name (`TN` →
  `xterm-256color`, the name cmote requested for the pty, §6) and the colour count (`Co`/`colors` →
  256). Every other capability is answered **unknown** (`DCS 0 + r <name> ST`) — the honest answer a
  well-behaved querier expects for a capability a terminal does not advertise. Names cross the wire
  hex-encoded both ways; the reply echoes the canonical upper-case hex.
- **DECRQSS → the one setting cmote renders faithfully.** Only **SGR** (`m`) is reported from real
  state: the current pen — `grid().cursor.template`, exactly what the grid paints — rebuilt as an SGR
  string (`0` for a reset pen, `0;1;31` for bold red), framed `DCS 1 $ r <params> m ST`. The pen is
  read **after** the chunk is advanced, so a program that sets attributes and then queries in the same
  write sees the attributes it set. Every other setting — cursor shape (cmote draws a fixed block by
  inverting the cell), scroll margins (the engine does not expose them), conformance level — is
  answered **unsupported** (`DCS 0 $ r ST`): an honest "I do not report that" that stops the program
  waiting far more cheaply than a lie about state would cost. (`ponytail:` truecolor is *not* claimed
  through XTGETTCAP `RGB` — its wire value is ambiguous — since 24-bit SGR works whether or not a
  capability query confirms it; and a DECRQSS SGR pen change that trails the query **within one chunk**
  is reflected too, because the pen is read once per chunk, not at the query's exact offset — the
  common case is the query trailing its own state, which reads correctly.)

- **`term/mod.rs`** owns the wiring: the `queries` scanner field, the `process` reply loop, and the
  `pen_sgr`/`sgr_color` helpers that read the alacritty pen (the only engine-coupled part). **`term/
  query.rs`** is the scanner, the reply formatters, the small capability map and the hex codec — free
  of any engine type, so every parse and every reply shape is unit-tested with no terminal.

## 34. Shell-integration prompt marks — OSC 133 (v4.0.0)

A shell with "shell integration" configured brackets every command it runs with OSC 133 escape
sequences — the FinalTerm/iTerm2 convention every modern terminal now reads:

    OSC 133 ; A        — a fresh prompt is about to be drawn
    OSC 133 ; B        — the prompt is written; the user's input begins
    OSC 133 ; C        — input is done; the command's output begins
    OSC 133 ; D [; N]  — the command finished, with exit code N

From those four marks a terminal knows where every prompt sits, whether a command is running, and
how the last one ended. cmote turns that into three things: a **per-tab status dot** (is a command
running, did the last one succeed?), **jump-to-prompt** (Ctrl+Shift+Up/Down walk the scrollback
between prompts), and **select-command-output** (grab everything one command printed). Like the cwd
(§17), modifyOtherKeys (§9) and the identity queries (§33), the engine treats OSC 133 as an unknown
OSC and ignores it, so cmote scans the same bytes itself.

- **A byte scanner that hands back marks with offsets (`term/osc133.rs`).** The same chunk-safe
  four-state machine as `cwd`, but where `cwd` keeps one latest value this one returns *a list* —
  each completed mark tagged with the byte offset just past its terminator. Kept a pure `bytes ->
  marks` function so it unit-tests with no engine at all. `133;A;aid=7` trailing fields are ignored;
  `D`'s exit code is the field after the letter, `None` when the shell emits a bare `133;D`.
- **Positions are captured by splitting the engine advance (`term/mod.rs::process`).** A prompt
  mark anchors to a grid line, and that line is only known once the engine has been advanced up to
  the mark — so `process`, uniquely, splits the advance at each mark's offset, reads the cursor
  there, and applies the mark. The common case (a chunk with no marks) stays a single advance; only
  a chunk that actually carries a prompt boundary pays for the split.
- **Marks are stored as ABSOLUTE line indices, so they survive scrolling.** Line 0 is the first line
  the session ever showed; a line's absolute index is `history_size + row` at the moment it is
  recorded, because the active screen's top line always sits at absolute `history_size` (that many
  lines have scrolled off above it). To place a mark on screen later, the reverse: viewport row =
  `absolute - history_size + display_offset`. A jump lands the target prompt on the top row, clamped
  to the retained history. (`ponytail:` this identity is EXACT only until the scrollback fills its
  cap (§23) — past that the engine evicts an old line per new one, so `history_size` stops growing
  while lines keep scrolling, and marks recorded across that point no longer share an origin. The
  recent prompts a jump actually reaches stay exact; only history deeper than the cap drifts, landing
  a jump *near* an old prompt rather than *on* it.)
- **The dot is per tab, in the strip (`ui/tabs.rs`).** Amber while a command runs, green when the
  last exited 0, red when it failed, and nothing at all on a tab with no shell or a shell that
  announced no integration — so the strip stays quiet until a command actually runs. Read from each
  tab's own `Terminal` (`command_state`/`last_exit`), so a background tab's dot is as live as the
  active one's — the point of showing it per tab. (`ponytail:` the exit *code* itself is not shown,
  only success/failure by colour; a chip is too small for `✗130`.)
- **The tick is per prompt, in the grid (`ui/grid.rs`).** A small cyan mark in the left padding
  gutter beside every prompt on screen — mirroring the scroll indicator on the right, a read-only
  mark that lives in the padding and never over a cell. `Terminal::prompt_rows` maps the stored marks
  to the visible rows each frame.
- **The jump is a keybind (`app.rs`).** Ctrl+Shift+Up / Ctrl+Shift+Down, reached only with the shell
  focused, guarded on Ctrl+Shift together so a bare or singly-modified arrow still reaches the shell
  — the same discipline as the Shift+Page scrollback keys (§23). It moves cmote's own view; nothing
  is sent to the remote.
- **Select-command-output has two triggers, one selection (`osc133.rs` + `term/mod.rs` + `app.rs`).**
  Alongside the prompt lines, `Prompts` now files each finished command's output as an absolute
  half-open line range `[output, end)` — the C mark's line to the D mark's — keyed by its prompt line
  (the A mark). **Ctrl+Shift+O** selects a finished command's output — the latest, then one further
  back on each repeat (below); **clicking a prompt
  tick** in the gutter (a press with the pointer inside `GRID_PADDING`) selects that command's. Both
  resolve to a range, `Terminal::select_output_*` reveals it (scrolling it into view only if it had
  left the live screen) and returns the lines it fills as a plain `OutputSpan` — so `term/` never
  touches the UI's selection type — and `app` turns that into an ordinary stream `Selection`. Reusing
  the mouse selection means the existing Copy / Ctrl+C / rich-HTML copy all work unchanged. (As
  shipped here the span was viewport *rows*, so an output taller than the screen selected only the
  first screenful from its top. **§40 made both the span and the selection absolute lines**, and the
  copy now reads the document, so a long output is grabbed whole.)
- **A resize drops the marks.** A resize reflows the grid, re-wrapping lines at the new width, so the
  line count of the history changes and the recorded absolute positions no longer line up. Rather
  than point a jump at the wrong reflowed line, `resize` clears the marks — the scrollback is kept,
  only the prompt ticks are relearned from the next prompt on. (`ponytail:` cleared on any resize,
  including a height-only one that would not actually reflow the columns.)

### Pressing again reads further back

Ctrl+Shift+O took the latest command's output and only ever that, so a session's history was reachable
only by clicking the right tick — which means finding it on screen first. Now **each press steps one
command further back**: newest, the one before it, the one before that. The key becomes a way of
reading *back* through what has been run rather than a way of grabbing the last thing.

The walk is a cursor into the filed commands (`Prompts::walk`), and everything interesting about it is
in when it goes back to the start:

- **A command finishing restarts it.** Running something new is the clearest statement there is that
  the user has stopped reading old output, so the next press takes what was just run — even if the
  command finished on its own while the user was several steps back.
- **A press on the grid restarts it.** The walk is one gesture; a click, a drag or a selection is the
  start of another. Anything else would leave a stale place kept across unrelated work.
- **A click on a prompt tick PARKS it there.** The two ways of reaching a command's output are then
  one gesture: point at a command, then keep stepping back from it. Jumping to the newest after a
  click would read as the key having lost its place.
- **The oldest is the end of the road, not a wrap.** Past it the answer is nothing at all and the
  selection stays on the oldest, because wrapping round to the newest looks like the key did something
  else entirely.
- **Commands that printed nothing are stepped over.** A `cd`, a bare Enter or a failed `cd` files a
  span with nothing in it; stopping on those would make the key look broken exactly in a session that
  has a run of them.

The cursor is an INDEX into the command ring rather than a count back from its end, and that is safe
for one reason worth writing down: the only thing that drops a command from the front is filing a new
one, and filing a new one restarts the walk on the same call. An index is therefore never held across
a shift. (The first cut of this carried an index-fixup for the drop, which was unreachable code for
exactly that reason.)

### What is deliberately NOT here

- ~~**Full-scrollback capture of an over-long output.**~~ **Shipped in §40.** As built here
  select-command-output was viewport-bound — a command whose output was taller than the screen selected
  and copied only the first screenful — because the selection itself addressed viewport rows. §40 moved
  the selection to absolute document lines and gave the copy path a history read, so the whole range
  comes back whatever the scroll position.
- ~~**Walking older commands on a repeated Ctrl+Shift+O.**~~ **Shipped** — see below. As built here the
  key always took the latest; now each press steps one command further back.
- **No injection of the marks.** cmote reads whatever the shell offers and adds nothing: a shell
  without integration configured shows no dots, no ticks, and jump-to-prompt finds nothing. cmote
  never rewrites the remote's shell init to turn it on — that is the user's to configure, exactly as
  the cwd (§17) is.

---

## 35. Finding text in the scrollback (v4.0.0)

The terminal retains 10 000 lines of history (§23) and the wheel, Shift+Page and jump-to-prompt
(§34) all move through it — but until now the only way to *find* something up there was to scroll
and read it. Every real terminal has a find bar; this is cmote's. **Ctrl+Shift+F** opens a small bar
over the grid, typing searches the whole document, and ↑ / ↓ walk the hits — each one revealed and
**selected**, so the existing Copy takes it.

It is deliberately built out of what §34 already established: absolute line coordinates, a
reveal-scroll, and "a found thing becomes an ordinary selection".

- **The pure core is a flattened row (`term/search.rs`).** A `Row` is one grid line's glyphs plus,
  for every *byte* of that text, the *column* it came from — the two only ever grown together by
  `push`, so they cannot drift. That map is the whole trick: it lets the search run over a plain
  `str` (so it is `str::find`, ASCII-lowered on both sides exactly as the editor's find is, §32)
  while reporting **columns**, which is what a selection addresses. It is also how a double-width
  glyph's trailing cell — which holds no glyph of its own — is skipped without every column after it
  sliding one to the left. `trim_end` drops the row's width-padding first, so a query of one space
  does not "match" thousands of blank cells (the same reason a copy trims them, §10).
- **`Terminal::find` walks history AND the live screen.** The engine stores scrolled-off lines on the
  *negative* grid lines below the active screen's line 0, so the whole document is `-history_size ..=
  the last screen row`, and a hit's line is recorded as `history_size + line` — the same
  scrollback-stable **absolute** index the OSC 133 marks use (§34), so a match keeps pointing at its
  own text as new output pushes the viewport down. The scan is a full grid walk (`history + rows` ×
  `columns` cell reads — a few million at the cap), which is cheap enough to redo on every keystroke
  and far simpler than an index that every scroll and reflow could invalidate. So there is nothing to
  invalidate: the list is rebuilt on each query change **and before each step**, which is also how
  output that arrived mid-search joins the results.
- **The match list is a tiny state machine (`search::Search`).** The query, every match in document
  order, and which is current. A **new query lands on the NEWEST match** — a terminal search almost
  always means "where did that last happen", and the newest hit is nearest the live prompt the user
  is already looking at. A **re-scan keeps the current match by identity** (same line, same columns),
  not by index, so a list that grew underneath a step still steps exactly one hit; a current match
  that did not survive the re-scan falls back to the newest. Stepping wraps both ways.
- **Revealing reuses §34's scroll, then an ordinary selection (`Terminal::reveal_line` + `app`).** A
  match already on screen is left exactly where it is — stepping between two hits on one screenful
  must not jerk the view — and one that is off screen is **centred**, so it arrives with context
  above and below it (an output span, by contrast, is scrolled to the *top*, since it is the start
  that matters there). The terminal only says whether the line could be shown; the match's own
  absolute line and columns are what `app` selects. That is why this feature needs no rendering and no
  clipboard work at all: the grid highlights a selection and Copy copies one, whatever put it there.
  (As shipped here `reveal_line` handed back the viewport row, because a selection addressed rows;
  since §40 it addresses lines, so a match's coordinates go straight into one untranslated.)
- **The bar floats; it does not push (`ui/terminal.rs`).** The grid's row count *is* the remote pty's
  size, so a bar that took height would resize the remote every time it opened. Instead it is an
  overlay in the existing stack, anchored to the grid's top-right by the same transparent-container
  trick the context menu uses (§10) — and since a container paints and captures nothing, a click or a
  wheel anywhere outside the bar still reaches the grid below. It carries the query field, a live
  `n / total` (or an explicit "No results"), the two steppers and the shared close ✕ (§10). The
  arrows are drawn ↑ / ↓ rather than the editor's ‹ / › because in a scrollback the direction *is*
  the meaning: ↑ walks back into history, ↓ forward toward the live prompt. **Enter steps ↑**, since a
  new query already put the user on the newest hit, so back into history is where anything is left to
  find.
- **While the bar is open it owns the keyboard (`app.rs::on_key`).** The keyboard subscription fires
  independently of widget focus, so without a guard every character typed into the field would ALSO
  be sent to the remote — searching would type at the shell's prompt. The guard is the same one the
  inline rename fields use (§18, §19): while `search` is `Some`, nothing reaches the channel, and Esc
  closes the bar. Ctrl+Shift+F is matched on the *physical* key (so it holds on any layout, like the
  copy/paste bindings) and taken *before* that guard, so pressing it again while the bar is up
  refocuses the field. Plain Ctrl+F is left to the shell — it is readline's forward-char. Closing the
  bar keeps the current match **selected**, so what was found can still be copied.
- **A press on the grid dismisses it, like the menus (§10).** That press takes the focus off the
  bar's field, and the guard above is blind to focus — so a bar left up would leave every keystroke
  swallowed by a field without a cursor. Dismissing it there is what keeps "the keyboard is either
  the bar's or the shell's" true at all times, with no third, dead state.
- **A session change closes it.** `clear_grid_interaction` drops the bar with the selection and the
  menus: a bar left open across a disconnect would be searching a scrollback that no longer exists,
  and would go on swallowing the keyboard.

### What is deliberately NOT here

- ~~**Highlighting every match at once.**~~ **Shipped in §39.** As built here, only the current hit was
  marked, because "marked" meant the one selection the grid already paints; washing the others needed a
  second, list-shaped highlight in the renderer, which §39 added (a per-frame mask over the visible
  cells, the current hit still keeping the selection's own fill).
- **Matches across a wrapped line.** A hit is found within one grid *row*, so a phrase straddling the
  wrap of a long logical line is missed (its halves are separate rows), and a cell's combining marks
  are not searched — only its base glyph. Joining wrapped rows would mean a second coordinate space
  between the search and the selection.
- **Regular expressions, whole-word and case-sensitive toggles.** The narrow, predictable rule the
  editor's find already sets (§32) — ASCII case-insensitive substring — is the whole vocabulary here
  too, and for the same reason: it is the behaviour that never surprises.
- **Searching the alternate screen's own scrollback.** There is none to search: a full-screen program
  (vim, less, tmux) manages its own pages and the engine keeps no history for it (§23), so the bar
  finds only what is on that screen. Its own search is the one to use there.

---

## 36. The last input and query gaps — DA3, and DECKPAM where it is safe (v4.0.0)

With the engine swap (§23), `modifyOtherKeys`, kitty keyboard (§25), OSC 8 (§24), OSC 133 (§34) and
scrollback search (§35) all done, the terminal-compatibility audit had four items left outside the
engine's own ceiling: DA3, the answerback string, blink, and DECKPAM. This section closes all four —
two by writing them, two by *deciding* them, with the evidence for the decision. Closing an item by
refusing it is only honest if the refusal is written down beside the ones that shipped.

### DA3, the tertiary device attributes (`CSI = c`)

The engine's `identify_terminal` answers DA1 (no intermediate) and DA2 (`>`), and drops the `=`
intermediate to a debug log — so, exactly like the three queries of §33, DA3 falls to cmote's own
stream scanner (`term/query.rs`). The scanner gained a `CSI =` state beside its `CSI >` one, and the
reply is DECRPTUI: `DCS ! | <eight hex digits> ST`.

- **The parameter rule is shared.** Both private queries cmote answers are only *themselves* in their
  default form (`CSI > q` / `CSI > 0 q`, `CSI = c` / `CSI = 0 c`); a non-zero parameter on the same
  final byte is a different private sequence. That test now lives in one place (`default_params`), so
  the two arms cannot drift apart.
- **SECURITY — the unit id is a constant, on purpose.** On DEC hardware those eight digits were the
  terminal's *serial number*. Reporting anything derived from the machine (a serial, a MAC, an install
  id) would hand every host the user logs into a stable fingerprint of their computer, off a query
  they never see. cmote answers `00434D45` from every install — site `00` (it has no DEC-assigned
  site) and `434D45`, which is `CME` in ASCII. The reply identifies the *program*, never the person,
  and the reasoning sits in the doc comment so nobody later "improves" it into a real id.

### DECKPAM, for the keys that cannot lose a meaning (`ESC =`)

Application-keypad mode asks for the numpad's own keys as SS3 sequences (`ESC O <final>`) instead of
the characters they print, so a program can tell keypad Enter from main Enter. The engine tracks the
mode bit, so this needed no scanner — only a seam getter (`Screen::application_keypad`) and a branch
in the encoder, reading the mode out of the grouped `keymap::Modes` beside DECCKM.

- **Only the unambiguous keys are diverted:** Enter `M`, `*` `j`, `+` `k`, `,` `l`, `-` `m`, `/` `o`,
  `=` `X`. These keys never navigate, so honouring the mode on them takes nothing away.
- **The digits and the decimal point are deliberately left out, and that is the whole design.** They
  are the keys whose meaning flips with NumLock, and terminfo's `smkx` sets DECKPAM — so *every*
  ncurses program has the mode on for its entire run. Diverting the digits to `ESC O p`…`y` would stop
  a NumLock-on numpad from typing numbers inside vim, less and `pm2 ls`: precisely the regression the
  NumLock digit fix (§9) exists to prevent. xterm makes the same call by default — its `numLock`
  resource lets NumLock override application keypad mode — so this is parity, not a shortcut.
- **The branch sits after the kitty hand-off and is guarded on the unmodified form.** Kitty has its
  own, complete keypad story (§25), and a Ctrl/Alt/Logo combo on a numpad key keeps whatever the
  ordinary paths make of it, so nothing a program bound for itself is swallowed by the mode.
- **The navigation role needed nothing.** With NumLock off a numpad key already encodes as its
  navigation key following DECCKM — and since `smkx` sets DECCKM *and* DECKPAM together, the bytes a
  program expects (`ESC O B` for down, …) are the ones DECCKM alone already produces.

### What is deliberately NOT here

- **The answerback string (ENQ, `0x05`).** A legacy host sends ENQ and the terminal types a fixed
  string back into the shell's input. cmote refuses it, for the reason its default is empty in xterm
  too: the trigger is a *single ordinary byte*, so any binary output that happens to contain `0x05` —
  `cat` of a binary, a corrupt download, a stray progress stream — would inject characters into the
  shell as if typed. That is a remote-driven side effect on the user's input, the same family as the
  OSC 52 clipboard writes and the bell cmote already drops (§12, §23). Legacy identification is worth
  far less than that. Recorded as policy, not as a gap.
- **Blink (SGR 5 / 6).** The compatibility audit used to call this a cmote *choice* — "the engine
  stores the bit, cmote draws steady". That was wrong, and checking it was part of this step: vte
  parses SGR 5/6 into `Attr::BlinkSlow` / `BlinkFast`, but `alacritty_terminal` 0.26's
  `terminal_attribute` has **no arm for them** and its cell `Flags` carry **no blink bit at all** — so
  the attribute is dropped before cmote could see it. It is an engine limit, and the audit now says
  so. Honouring it would take a scanner beside the engine (as `modkeys` is) tracking SGR 5/6 per cell
  *and* a repaint timer cmote deliberately runs for nothing — the same call made for the cursor.
- **DA3 as anything but a constant, and any other `CSI =` sequence.** See the security note above.

## 37. Closing a tab returns you to where you were (v4.0.0)

Tabs (§26) kept `active` as an **index into the strip**, so closing the tab on screen fell back to
strip arithmetic: keep the index, or step back one if the last tab went. With three or more tabs open
that is nearly always wrong. Close the shell you were working in and the window lands on whichever
chip happens to sit next door — frequently a home tab opened minutes ago and never looked at — while
the session you were in *before* this one sits two chips away, untouched. The strip's order says
where a tab **sits**; it says nothing about where the user has **been**.

This section adds the second order — the activation order — and makes a close walk back along it.

### The order is its own module, and it is pure

`mru.rs` holds one type: `Mru`, a stack of tab **ids**, least recently activated first, the tab on
screen last. It knows nothing else — no `Tab`, no strip index, no iced type — so the whole rule is
unit-testable without a window, and `App` stays the only place that reconciles ids with positions.

- **Ids, not indices.** Indices shift on every removal; a tab id is monotonic and never reused
  (§26), so an entry in the order cannot silently come to mean a different tab.
- **A stack of visits, not a log of them.** `touch` removes any existing entry before pushing, so
  re-activating a tab re-dates its visit instead of leaving a stale one further down that would come
  forward out of turn later. Length always equals the number of open tabs.
- **A `Vec` and a linear scan, deliberately.** A window holds a handful of tabs; an index or a deque
  would cost more to read than it saves to run.

### One rule covers both close cases

`forget(id)` drops the closed tab and returns the **top of what is left** — not "the tab before the
closed one". That one answer is right in both directions, which is why there is no branch on whether
the closed tab was active:

- Closing the **active** tab pops the top, so the answer is the tab the user was on before it. This
  is the whole point of the section.
- Closing a **background** tab (its own "×", from a strip where another tab is on screen) leaves the
  top where it is, so the answer is the active tab itself — and `App` re-activating what is already
  on screen changes nothing. Closing a tab off-screen must never move the window.

### Where it plugs in

`App` gained one field (`recent: mru::Mru`) and touches it at the four places that already changed
`active`: startup (the first home tab is the first visit), `open_tab`, `open_editor` (§32 — so
closing an editor returns to the session the file was opened from) and `select_tab`. `remove_tab`
asks `forget` which tab to bring forward and resolves that id to an index, keeping the old strip
arithmetic as an unreachable fallback rather than leaving `active` pointing anywhere if the order and
the `Vec` ever disagreed.

`remove_tab` also now carries the window geometry (`window_size` / `window_focused` / `modifiers`)
onto the tab it brings forward, exactly as `select_tab` does — read **before** the removal, since
when the tab being dropped is the active one that is the last moment its copies exist. Without it, a
tab brought forward by a close painted against whatever size it last saw until the next resize.

### What is deliberately NOT here

- **No keyboard shortcut.** A Ctrl+Tab "last tab" cycle is the obvious neighbour of this order and
  costs almost nothing to add on top of it — but the strip is mouse-only by choice (§26), and every
  keystroke cmote claims is one the remote shell no longer receives. Left for when it is asked for.
- **Not persisted across runs.** cmote does not restore the tab set on startup, so an activation
  order from the previous run would name ids that no longer exist. It is session state, and stays
  out of `settings.json` (§31).
- **No depth limit, and no visible history.** The order holds one entry per open tab and shrinks with
  them, so it cannot grow without bound; and it is a fallback rule, not a feature with a UI. The
  strip still shows exactly the tabs, in the order they were opened.
- **The quit flow is untouched (§30).** Quitting closes every tab, so which one would have come
  forward is moot; closing the *last* tab is still a request to quit, not a fallback.

## 38. The strip's order is the user's — files beside their session, and drag to rearrange (v4.0.0)

§37 fixed *which* tab a close brings forward. This section is about the other half of the strip's
order: **where a tab sits**, and who decides. Two changes, both of them the same idea — the strip
should read the way the user thinks about their work, not the order events happened to arrive in.

### An editor tab opens beside the session it came from

An editor tab (§32) went on the **end** of the strip, however far that was from the session whose
file it holds. Open a file from the leftmost of four sessions and its chip landed fifth, with three
unrelated sessions between the file and the shell it belongs to — and every subsequent tab pushed
them further apart. `open_editor` now inserts at `editor_slot(session)`: **just past the session's own
chip, and past any editor tabs already grouped there.**

- **Past the existing group, not immediately after the session.** Opening three files in a row reads
  left to right in the order they were opened. Inserting each one directly after the session would
  stack them up backwards, which is the wrong answer for "I'll open these three and work through
  them".
- **The group ends at the first chip that is not its own.** The scan only skips editor tabs whose
  `editor.session` is *this* session, so another session's tab — or an editor the user has dragged in
  (below) — stops it. A group is a run, not a claim on the rest of the strip.
- **A file whose session has already gone takes the end**, as it used to: there is no chip to sit
  beside (the tab closed while the load was in flight), and a guessed slot would be worse than the
  end.

### Drag a chip to move it

The strip was fixed in opening order. Now **the press that selects a tab also grabs it**: travel to
another chip and release, and the grabbed tab takes that chip's slot. No separate handle, no modifier
— dragging a tab is the same gesture as clicking one, continued.

- **The gesture is reported by per-chip pointer events, not pixel arithmetic.** `on_press` grabs,
  `on_enter` names the chip under the pointer, `on_release` drops. `ui/tabs.rs` never needs to know
  how wide a chip laid out — which matters, because iced does not expose a widget's laid-out bounds
  (the same wall §10's dialog centring works around).
- **The reorder happens once, on the drop — deliberately.** Shuffling the chips live under the
  pointer is the flashier option and is the reason the bounds problem bites: chip widths vary with the
  label, so moving a wide tab onto a narrow one (or the reverse) can leave the pointer sitting over
  the slot it just came from, which swaps it back, which puts it under the first slot again — a
  ping-pong between two positions with the pointer perfectly still. Committing on release cannot
  oscillate. The chip that would receive the drop wears a blue outline instead, so the target is
  visible without the strip rearranging itself under the hand.
- **The drag's state is ids, not positions.** `TabDrag { grabbed, over }` holds two tab ids and they
  are resolved to strip positions only at the moment of the drop, so a tab closing mid-gesture (the
  "×" is *inside* the chip being dragged) can never move the wrong tab — it just resolves to nothing
  and the drop is dropped.
- **`remove` + `insert` for the move**, which gives the familiar feel in both directions: dragged
  right, the tab lands where the hovered chip was and the chips it passed shuffle left; dragged left,
  the reverse.
- **`active` is a strip position, so it is re-found after the move** by the id of the tab that was on
  screen — not by assuming that is the grabbed one. Ordinarily it is (the press selected it), but a
  close confirmation can leave another tab active, and following the id costs one `position` call.
- **Two ways out of a gesture, both cheap.** A release anywhere over the bar drops (the last chip
  hovered wins the slot, so a drop on the padding or in the gap between two chips still lands
  somewhere sensible); the pointer *leaving* the strip abandons the move entirely. Dragging back onto
  the grabbed chip clears the target, so changing your mind mid-drag also leaves the order alone.
- **A press that never travels is just a click.** The drag arms with no target, so press-and-release
  on one chip selects and reorders nothing — the gesture costs the old behaviour nothing.
- **The hover report says a different thing at rest.** Mid-drag it names the slot under the pointer,
  which is what the drop needs. At rest it says only "a chip has the pointer", which is what the
  cursor needs (§51) — the affordance the strip advertises with an open hand over a chip and a
  closed one while a tab is in flight. On Windows that took a good deal more than asking; §51 is
  the whole of it.
- **The activation order (§37) is untouched by a reorder**, because it is keyed by tab id. Where a tab
  *sits* and where the user has *been* stay independent — which is exactly why §37 used ids.

### What is deliberately NOT here

- **No live shuffle, and no floating "ghost" chip.** See the ping-pong above for the shuffle. A ghost
  that follows the pointer would need the drag's pixel position and the chips' widths, i.e. the
  measurement iced does not give — a custom strip widget's worth of work for polish, not function.
- **No dragging a tab out into its own window.** cmote is one window (§26); tear-off would mean a
  second window, a second grid, and splitting the shared vault / target list across both.
- **No keyboard reordering, and no "move left/right" menu item.** Same reasoning as §37's missing
  Ctrl+Tab: the strip is mouse-only by choice, and every shortcut cmote claims is one the remote
  shell no longer receives.
- **The order is not persisted.** cmote does not restore the tab set between runs, so there is no
  order to restore (§31).
- **Nothing groups or sorts the strip for the user.** The editor rule above places a *new* tab; it
  never rearranges existing ones. Once the user has dragged a chip somewhere, that is where it stays.

---

## 39. Every match on screen, washed — the find bar shows where else the query is (v4.0.0)

§35 shipped the scrollback find bar with one hit marked: the current one, revealed and turned into an
ordinary selection so the existing highlight and Copy served it with no rendering work at all. That
listed "highlighting every match at once" as deliberately absent — *worth it later*. This is later.
Every hit on the visible screen now carries a **wash** (a muted amber fill), and the current one keeps
the **selection's blue**, so the bar answers two questions instead of one: where you are, and where
else the query is. Walking a query with ↑ / ↓ stops being blind stepping — the next hit is already
visible before the step lands on it.

### A second coordinate space, resolved once per frame

A `Match` holds an **absolute** document line (§35), because it must survive new output pushing the
viewport down. A renderer can only paint **viewport rows**. `Search::visible` is the one place the two
meet: given the history depth, the display offset and the row count it returns `Highlight { row,
start_col, end_col }` for the hits that land on screen, using the same `absolute - history_size +
display_offset` mapping as §34's prompt ticks.

- **`Highlight` is a separate type from `Match`, on purpose.** They are both a line and a column span;
  one is a document coordinate and the other a screen one, and the day they are the same struct is the
  day one gets passed where the other belongs. Two types make that a compile error.
- **Scrolling needs no re-scan.** The washes are a projection of an absolute list, so wheeling through
  the history moves every wash with the text it belongs to. Nothing is invalidated, nothing recomputed
  but the projection.
- **The walk starts at the first visible line.** The match list is in document order (`Terminal::find`
  walks the grid oldest line first), so a `partition_point` skips the history above the viewport in log
  time and the walk stops at its bottom. This runs on *every frame the grid draws*, and the query that
  makes it matter is the ordinary one: find-as-you-type searches the first letter typed, and one letter
  over a full scrollback has tens of thousands of hits, nearly all off screen.
- **The current match is in the list like any other.** It is drawn as a selection *as well* (that is
  how §35 reveals it), and the renderer lets the selection's fill win — so the current hit stands apart
  without `visible` having to know which one it is. It also means a user who drags a new selection over
  the grid still sees every hit washed, the current one included, rather than losing it from the set.

### In the renderer: a mask, not a list (`ui/grid.rs`)

The grid takes the visible highlights the way it already takes the prompt rows — owned, recomputed
each frame — and flattens them into a **row-major `Vec<bool>` over the visible cells**, the same index
space the Ctrl-hover link run (§24) already uses. The run planner then tests one boolean per cell.

- **Why a mask.** A per-cell walk of the match list is `cells × matches`, and neither factor is small
  in the case that matters (one letter typed, hundreds of hits per row). The mask is `cells + matches`
  built once, `O(1)` per cell after. An empty list allocates nothing and every lookup misses — which is
  the shut-bar case, i.e. almost always.
- **The fill order is the whole trick, and it lives in `cell_style`:** faint, then inverse/cursor, then
  **match**, then **selection**, then conceal. Because `CellStyle` is the run-grouping key, each fill
  seals its cells into their own run exactly as a selection already did — no new draw path, no second
  pass over the grid, one more branch in a function that was already resolving four of them.
- **Different hue, not a paler shade.** The current hit is blue (the selection), the others amber. Two
  brightnesses of one colour is what a dim screen or a colour-blind eye loses first; two hues survive
  both.
- **Out-of-grid highlights are dropped, not wrapped.** A resize between the scan and the frame can
  leave a hit pointing past the last row or column; the mask skips the row and clips the span, because
  a row-major write that ran off the end of a row would silently paint the row below it.
- **`Marks` groups the selection and the mask into one argument.** `plan_runs` had reached the count
  where a seventh loose `Option` is a mistake waiting to happen at a call site, and the two are always
  consulted together and resolved the same way — a fill that replaces the cell's own background.

### Where it plugs in

`ui/terminal.rs::view` already had the bar's state (`Modals::search`) and the screen; it resolves the
highlights there — where both the bar and the viewport's numbers are already to hand — and hands them
to `grid()`. The bar closed means `None` means an empty vector means no washes. **`app.rs` is
unchanged apart from a test**: the wash is a pure function of state the view already had.

### What is deliberately NOT here

- **New output does not join the washes until the next step or keystroke.** The match list is rebuilt
  on a query change and before each step (§35) and never on arriving output, so a hit printed since
  then is neither in the count nor washed. Rescanning the whole grid on every frame of a build log's
  output is the one thing §35's "no index to invalidate" simplicity cannot afford; the fix, if it ever
  matters, is to rescan on a step *and* on becoming idle, not per frame.
- **No overview marks in the scrollbar gutter.** A "where in the whole history are the hits" ruler
  beside the scroll thumb is a genuinely useful next step and a different feature: it needs the match
  list mapped to the *document*, not the viewport, and a thumb that is currently a read-only mark
  (§23) would start inviting clicks.
- **The wash does not survive closing the bar.** Esc drops the `Search`, so the washes go with it and
  only the current hit's selection stays (§35 chose that so what was found can still be copied).
  Keeping every wash after the bar is gone would leave marks on the grid with nothing on screen
  explaining them.
- **Nothing dims the unmatched text.** A "focus mode" that fades everything but the hits would have to
  rewrite every cell's foreground, which is the one thing the renderer must not do — a program's own
  colours are its own (§9).

---

## 40. The selection speaks document lines — text that scrolls stays selected (v4.0.0)

Three features had grown into absolute document coordinates — the OSC 133 prompt marks (§34), the
search matches (§35), the washes over them (§39) — while the thing they all end up *becoming*, the text
selection, still addressed **viewport rows**. That mismatch had visible costs. Drag over some output,
scroll, and the highlight stayed parked on the rows while their contents slid out from under it. Select
a command's output taller than the screen and only the first screenful was selected, because rows off
the screen had no coordinates to be selected *by*. Copy read the visible grid, so a copy could never
say more than the screen did.

So `Selection`'s endpoints are now document positions: `Spot { line, col }`, where `line` is the same
absolute index the marks and the matches use. Nothing about the *shape* of the selection changed — it is
still a stream selection in reading order — and no widget, message or keybind moved.

### The one door between the two spaces (`term/screen.rs`)

The pointer is on screen and the text is in the document, so exactly one conversion has to exist, and
it now exists exactly once: **`Screen::line_at(row)`** = `history_size + row - display_offset`. It is
the same arithmetic §34's ticks and §39's washes are placed by, written down in one function that both
`Screen::cell` and `ui::selection::Cell::spot` read through.

- **`Screen::line_cell(line, col)` is the read that does not care where the viewport is.** The engine
  keeps scrolled-off lines on the *negative* grid lines below the live screen's line 0 (§23), so a
  document line maps onto the grid by subtracting `history_size`; anything outside `-history_size ..=
  screen_lines - 1` is a line the session no longer has. `Screen::cell` — the renderer's per-cell read
  — is now `line_cell(line_at(row), col)`, so the viewport and document readers cannot drift apart.
- **`Cell` and `Spot` are two types, and that is the point.** `Cell` is where the pointer is (row 0 is
  the top visible line); `Spot` is where the text is. `Cell::spot(screen)` is the only crossing, so a
  viewport row cannot reach a selection without passing through the conversion — the same discipline
  `Match` / `Highlight` keep (§39), and the reason this refactor was a series of compile errors rather
  than a hunt for wrong highlights.

### The renderer resolves the other way, once per row (`ui/grid.rs`)

`plan_runs` asks the selection about a document line, so `Marks` carries the frame's `top_line`
(`screen.line_at(0)`) and the planner adds the row it is drawing. One addition per row, not per cell;
`Marks` already existed to group the fills (§39), and this is the coordinate they are resolved against.
Nothing else in the renderer moved — a selected cell still seals into its own run through `CellStyle`,
the fill order (match, then selection) is untouched, and the wash layer needed no change at all,
because a `Highlight` was *already* a projection.

### What that buys, feature by feature

- **A drag holds its text.** `on_grid_pressed` anchors at `hover_cell.spot(screen)` and `on_grid_moved`
  extends to the same, so the endpoints are lines from the moment they are made. Scroll away and back:
  the highlight is on its own text, and Copy takes what was dragged over, not what is now on those rows.
- **Select-command-output is whole (§34's deferred item, closed).** `OutputSpan` carries absolute
  `start_line` / `end_line`, and `locate_output` no longer clamps to the visible rows — revealing (which
  screenful you are looking at) and selecting (which lines are selected) became separate concerns, which
  is all that limit ever was. A forty-line output on a twenty-four-row screen copies forty lines.
- **A search match needs no translation.** `reveal_line` returns *whether* the line could be shown
  rather than which row it landed on, and the match's own line and columns become the selection (§35).
- **The copy path reads the document.** `selected_rows` walks `start.line ..= end.line` through
  `line_cell`, so both the plain-text and the styled-HTML copy (§10) reach into the history unchanged —
  they share that geometry, which is exactly why neither had to be touched.

### What is deliberately NOT here

- **No auto-scroll while dragging past the screen edge.** The head can only be a cell the pointer is
  over, so a selection still cannot be *extended* beyond the visible rows by dragging — you scroll,
  then drag further. What §40 fixes is that such a selection now survives the scroll; growing it by
  hovering the edge is a timer-driven behaviour and its own feature.
- **A resize still invalidates a live selection.** A reflow re-wraps the history, so the line count
  changes and the recorded lines no longer point at the same text — the reason §34 drops the prompt
  marks on resize. The selection is left alone rather than cleared (as it was before, when it pointed
  at whatever reflowed onto its rows). `ponytail:` clearing it on a reflow would be the honest thing;
  it needs a decision about the far more common height-only resize, which reflows nothing.
- **No word / line double- and triple-click selection.** Still the drag, the output span and the search
  match. Absolute coordinates make it *easier* (a word is a run on one line), but it is a new gesture
  and a new set of boundary rules, not part of the coordinate change.
- **Nothing reads the document below `history_size + screen_lines`.** There is no persistence: what the
  engine has evicted at the scrollback cap (§23) is gone, and a selection reaching that far simply
  contributes nothing for those lines rather than pasting blanks in their place.

---

## 41. Inline images — sixel pictures in the scrollback (v4.0.0)

The compatibility plan (§5, §7) had one item left with real UX value: **graphics**. A program that
wants to show a picture in a terminal — `img2sixel`, `chafa -f sixel`, gnuplot's sixel terminal, timg,
matplotlib's sixel backend, `lsix` — writes it into the byte stream as a sixel DCS. `alacritty_terminal`
carries none of that, and the entry read as "needs an engine fork *and* a compositor in the renderer",
which is why it sat at the bottom of the list for so long.

Half of that was wrong, and the reason is worth writing down: **the engine's DCS hooks are no-op debug
logs**. A sixel payload is already followed to its terminator and dropped, so it cannot corrupt the
grid and there is nothing to fork. What was actually needed was the tactic cmote has used four times —
*scan the sequence the engine ignores out of the same bytes* (the cwd §17, modifyOtherKeys §9, the
identity queries §33, the OSC 133 marks §34) — plus the coordinate §40 had just finished building.

So: **cmote decodes sixel itself and composites it over its own grid.** Both halves of a picture's
position — where it is anchored, and which cells it owns — are things the tree already had.

### The decoder, in-house (`term/sixel.rs`)

Sixel is a self-describing palette format in printable ASCII: one character carries six vertical
pixels, `#Pc;Pu;Px;Py;Pz` defines a colour register, `!Pn` repeats, `$` returns to the left edge of the
band and `-` drops to the next. That is why **sixel is the format that needed no new dependency** — it
is the one inline-image protocol whose payload is not a PNG or a JPEG. Decoding it in-house is the same
call the `.ppk` parser is (§7): a small, fully specified format is cheaper to own than to depend on.

- **Two passes over the payload, one grammar.** `walk` is the only place the command syntax is written
  down; `canvas_size` measures through it and `paint` draws through it, so a measurement and a painting
  of the same bytes cannot disagree. Measuring first means the canvas is allocated **once**, at a size
  already checked against the caps — the alternative (growing a row-major RGBA buffer sideways as the
  picture reveals its width) would restride every row on every growth *and* size an allocation from a
  number the remote chose before checking it.
- **The raster attributes are the sender's crop.** With `"Pan;Pad;Ph;Pv` present the canvas is `Ph×Pv`,
  so a picture whose last band is two pixels tall reports 62 rather than the 66 its bands span. Without
  them the extent of the pixels actually *painted* is used, measured from set bits only, so the trailing
  blank columns emitters pad a band with do not widen the image.
- **A colour introducer selects the register it defines.** Not obvious from the format's description —
  the two `#` forms read like separate commands — but every emitter relies on it: `#0;2;100;0;0~`
  defines red and expects the very next sixel to *be* red. Missing it painted whole pictures in one
  colour, and a test caught exactly that.
- **DEC's HLS measures hue from blue.** 0° blue, 120° red, 240° green, where every modern HSL formula
  starts at red — so the angle is rotated by 240° onto the standard wheel. HLS payloads are rare, but a
  picture drawn in the wrong primaries is unmistakable.
- **An unset pixel is transparent, not black.** Sixel's `P2` nominally chooses between the two; cmote
  draws over its own grid, so "the terminal's background shows through" *is* what background means here,
  and it is the honest answer for the emitters that ask for transparency — which is most of them.

### The picture is anchored in the document, and reserves real cells (`term/graphics.rs`, `term/mod.rs`)

A placement is an **absolute document line** plus a column — the coordinate §40 made a first-class
citizen. That single choice is what makes an image behave: scroll away and back and it is still on its
own text, because nothing ever has to move it. It is the same coordinate the prompt ticks (§34) and the
search hits (§35) live in.

The cells underneath are then genuinely reserved, by feeding the engine sequences **as if the remote had
sent them** — `CSI <cols> X` (ECH) to erase exactly the box the picture covers, then LF per row (CUD on
the alternate screen, for the reason below), then CR:

- The engine knows nothing about images, so unless the cells are claimed the shell's next line of output
  is written straight over the picture. With them claimed, the grid under an image is ordinary blank
  cells: it scrolls, it evicts at the scrollback cap and it reflows exactly as text does.
- Reserving through VT sequences rather than reaching into the grid means the reservation obeys the
  scroll region, the autowrap mode and the character set precisely as the program's own output would —
  and the cursor ends up at the left margin *below* the picture, which is why a prompt lands under an
  image the way it does in a terminal with native sixel.
- Both counts round **up**, so the reserved box is never smaller than the pixels; the renderer then
  clips the picture to that box, so an under-reservation could only ever crop an image, never let it
  creep onto the row below.

`process` now has two scanners that fire mid-chunk, so their events are merged into one **offset-ordered**
list (`splits`) — the engine can only be advanced forwards, and applying all the marks and then all the
images would place the second kind at the wrong point in the stream. The two kinds want that offset on
opposite sides of their own bytes, which is the subtlety in this section: a **picture** is applied *past*
its DCS (it goes where the cursor is, which is only right once everything before it has been drawn),
while an **erase** is applied *before* its sequence (which pictures it takes depends on where the screen
ends and the scrollback begins, and `CSI 3 J` drops the engine's whole history — asking afterwards is
asking a terminal that no longer remembers).

### The renderer paints it, and only where it belongs (`ui/grid.rs`)

`image_bounds` is the reverse of the run planner's projection: a document line back onto a row, against
the frame's `top_line`. The row offset is **signed**, so a picture anchored above the viewport is drawn
with its top off screen and a tall image scrolls smoothly through the view instead of popping into
existence once its first line arrives. Each picture is drawn at its **native pixel size** (no scaling,
so no resampling blur), snapped to the pixel grid, clipped to the intersection of its reserved box and
the visible grid, and composited after the text so the row it sits on cannot hide it.

The store holds an **iced image handle** rather than raw pixels — the one place `term/` looks up at the
GUI, and a deliberate trade: the renderer caches its GPU texture against the handle's identity, and the
widget is rebuilt every frame and can cache nothing itself, so minting the handle once at decode time is
the difference between one upload per picture and one upload per picture *per frame*.

### Programs have to be told, or none of it is used

Two answers, both new, and without them the feature would work and never be reached:

- **DA1 now advertises sixel.** The engine answers `CSI ? 6 c` itself, and attribute **4** is how a
  terminal says it draws pictures — what chafa's auto mode, `lsix` and ranger's previewer read at
  startup. So cmote *amends the engine's own reply* on its way out (`query::with_sixel_attribute`):
  sending a second DA1 would leave the program parsing one of them as input, and suppressing the
  engine's would mean cutting bytes out of an inbound stream mid-sequence.
- **XTSMGRAPHICS (`CSI ? Pi;Pa;Pv S`) is answered** from the limits the decoder actually enforces — 256
  colour registers, and the largest image it will accept. A *set* is honestly refused (status 3) with the
  value cmote will in fact keep to, and ReGIS (item 3) is answered "unknown item" rather than given a
  geometry it could never honour.

### Bounded, because every number comes off the wire

Decoded pixels are the only unbounded memory a remote can hand cmote, so (§12): parameters saturate
instead of wrapping; a payload past 16 MiB is abandoned while the DCS is still followed to its
terminator; an image past 4096×4096 or 4 Mpx is **refused whole** rather than clipped, because a clip
would show the user a silently truncated picture; painting is bounds-checked per pixel, so a raster
attribute that disagrees with the payload can only lose pixels; and the store evicts oldest-first past
64 pictures or 64 MiB. A picture cmote will not decode reserves no cells either, so a refusal leaves the
screen exactly as it was.

### Lifecycle — a picture goes when its text goes

- **`CSI 2 J`** takes the pictures on the visible screen; **`CSI 3 J`** takes the ones in the scrollback.
  A shell's `clear` sends both and leaves nothing, while a `2J` at a prompt leaves the plots further up
  the history alone — the same split the erase makes in the text.
- **RIS (`ESC c`)** takes everything: the session starts over.
- **A resize** takes everything, because a reflow moves the document out from under every anchor — the
  trade §34's prompt marks already make.
- **The alternate screen has its own page of pictures**, with its own lifecycle — see below.

### The alternate screen draws too, on a page of its own (v4.0.0)

`ranger`'s previews and `mpv --vo=sixel` draw on the **alternate screen**, and until this they were the
one thing sixel support did not reach. It was written down as a limit rather than papered over, on the
grounds that the alternate page needs a coordinate space of its own and §40 spent its whole length
collapsing two spaces into one. That premise turned out to be **wrong, and pleasantly so**: the
alternate screen keeps no history, so `history_size` there is 0 and the absolute document line of row
`r` is exactly `r`. It is not a second space — it is the **same** space with the history at zero. The
anchor arithmetic, `image_bounds`, the clipping and the compositing are all unchanged, and the
renderer needs no branch: `Terminal::images` hands it whichever page is up.

What the page really needed was a **lifetime**, which is a second store (`graphics::Store`), and four
rules that differ from the primary screen's:

- **A screen swap empties it, in either direction.** The pictures belong to the program that drew them:
  quitting `ranger` must not leave its preview painted over the shell, and starting the next program
  must not show it the last one's screen. The primary screen's pictures are untouched by either swap —
  a `vim` session in the middle of a scrollback of plots leaves every one of them exactly where it was.
- **`CSI 2 J` takes all of them**, because there is no history there for the erase to spare. `CSI 3 J`
  says nothing about a scrollback that does not exist, so it is ignored.
- **A new picture replaces the one whose box it overlaps.** A full-screen program redraws the same pane
  every time the selection moves, so the picture arriving is the *successor* of the one there, not a
  second one beside it — without this the store would fill with the frames of a video, each hidden
  behind the next.
- **A glyph appearing in a picture's reserved box retires it.** This is the closest cmote gets to what a
  terminal with native graphics has for free, where the pixels live in the cells and writing a character
  erases them. cmote's pictures sit *beside* the grid, and the box was blanked when the picture was
  placed — so a glyph in it means the program has repainted over the picture. `ranger` moving from an
  image preview to a text one is exactly that, and it announces itself no other way: it repaints the
  pane in place, with no erase and no swap. A chunk that *placed* a picture sits the sweep out, so a
  program writing its image and the rest of its frame in one write does not blank its own picture the
  instant it arrives.

And one rule that differs in the reservation itself: **the rows are stepped with CUD (`CSI B`), not
LF**. LF at the bottom of the screen scrolls, which on the primary screen is the point — that is how a
picture's cells become scrollback — and on the alternate page is ruin: a page with no history throws
the scrolled-off row away for good and drags every other picture's anchor row out from under it, and a
picture reaching the bottom is the *normal* case there, since a full-screen video is exactly one. CUD
stops at the margin instead. The cost is that the cursor is left on the last row rather than below the
picture, which no full-screen program notices — they all position absolutely.

**What is still not right there**, and is cheap to say out loud: a pane cleared to blanks with nothing
then drawn in it leaves the picture up until the next repaint or swap (nothing distinguishes that from
an untouched box); a picture is retired *whole* rather than having the covered part cut out of it,
which is the same trade as "not reflowed, dropped"; and a program that scrolls the alternate page
itself moves the text out from under its pictures, which is only noticed when the new text lands in one
of their boxes.

### What is deliberately NOT here

- **No kitty graphics protocol and no iTerm2 inline images (OSC 1337).** Both carry PNG/JPEG payloads,
  so both need an image-format decoder — a parser fed bytes straight off the wire, which is a security
  decision and a dependency decision, not a rendering one. Sixel is the format that needed neither, and
  it is what the sixel-capable tools already speak. The placement, reservation, compositing, eviction
  and capability-advertising machinery here is protocol-agnostic: adding kitty later is a decoder plus a
  scanner arm, not a rethink.
  **This still holds after §53 brought those decoders into the tree.** The refusal was never "cmote
  owns no PNG parser" — it is that a REMOTE must not get one run on bytes it pushed into the terminal
  stream unasked. §53 decodes only a file the user pointed at and asked to open, one at a time, under
  caps, with the format chosen by magic bytes; nothing about that reaches the escape-sequence path,
  and the two are not the same decision wearing different clothes.
- **A picture is not reflowed, it is dropped.** `ponytail:` a terminal with native graphics re-lays its
  images on resize; cmote drops them rather than leave one floating over whatever text landed on its old
  line.
- **The pixel aspect ratio (`P1` and `Pan`/`Pad`) is ignored** — square pixels, as every modern terminal
  draws. DECSDM (`?80`, sixel scrolling mode) is likewise not honoured: cmote always scrolls, which is
  the modern default and what emitters assume.
- **A selection over a picture copies nothing.** The reserved cells are blank, so an image region yields
  blank text — truthful (there *is* no text there), but it means a copy cannot capture a picture. Saving
  an image out of the scrollback would be its own feature.
- **Nothing is drawn beyond the retained scrollback**, and past the `SCROLLBACK` cap the absolute anchors
  drift for the same reason §34's marks do: once the history stops growing, `history_size + row` no
  longer names a fixed line. It takes 10 000 lines of output to reach, and it is a property of the
  coordinate, not of the images.

---

## 42. Select by word and by line — the double and triple click (v4.0.0)

Selecting text in cmote's grid needed a **drag**, and only a drag. Every terminal ever shipped also
selects a **word** on a double click and a **line** on a triple, and its absence was the kind of gap
nobody files a bug about — they just drag carefully across a path, every single time, and think less of
the program for it.

Nothing new had to be invented for it. §40 had already turned the selection into a pair of document
positions, and §34 had already established that *anything* can build a selection and the grid will
highlight it and Copy will copy it. So a double click is one question: **which cells is that word?**

### Counting the presses is cmote's job (`ui/selection.rs`, `app.rs`)

The grid sits inside a `mouse_area`, which reports each press on its own and counts nothing — so the
tally lives here: `Clicks` remembers the last press's **cell**, when it happened and what it counted as,
and escalates single → double → triple inside a **500 ms** window (Windows' own `GetDoubleClickTime`
default, so cmote feels like the rest of the desktop rather than inventing a timing). A fourth press
cycles back to single, so leaning on the button does not escalate forever.

Consecutive presses must land on the **same cell**, not merely within a few pixels as a general-purpose
widget asks: on a grid the cell *is* the target, and it is the cell the word then expands from. Nudging
the pointer inside a cell must not break a double click; crossing into the next one must.

`press` is handed the current instant rather than reading the clock itself, which is the only reason
this timing has tests at all.

### A word is what a shell session holds (`ui/selection.rs`)

The whole double-click rule is one predicate: a word character is **any alphanumeric** — in any script,
so CJK and accented text need no special case — plus the punctuation `_-./~+=@%&#?:`. That set is chosen
for what is actually on an SSH session's screen, and it means a double click returns each of these
**whole**:

```
/etc/ssh/sshd_config      https://example.com/a?b=1      root@10.0.0.1:22      KEY=value
```

which is nearly always what is about to be pasted straight back into the shell. Deliberately *absent*
are the shell's own separators — space, quotes, brackets, `|`, `;` and `,` — so a double click inside a
list or an argument takes the one item under the pointer. The trade is that a prose sentence's trailing
`.` or `:` is swept up with the word before it; xterm does the same, and it is a far cheaper annoyance
than a path arriving in three pieces.

Two details the tests pin down. A double click on **blank space selects nothing** (`None`, not a
one-cell span) — a click on nothing should leave the screen as it was. And a wide glyph's trailing half
carries no text of its own, so the question is passed to the lead cell in the column before it;
without that, every CJK word would end after its first character.

### A line means the LOGICAL line, which is why the engine's wrap flag surfaced (`term/screen.rs`)

Output that runs past the right margin occupies several rows and is still **one line**. A triple click
that took only the row under the pointer would hand back half a command, so `Screen::line_wrapped(line)`
is new on the seam: the engine sets a flag on a row's last cell at the moment output wrapped, and keeps
it correct through a reflow — that flag is exactly how it re-joins wrapped rows at a new width. A
whole-line selection walks back to the first row of the run and on to its last.

Reading that flag also fixed something older, and quietly worse: **a copy across a wrap used to paste a
newline into the middle of a path.** `extract` and the HTML copy now join a wrapped row to the next one
with *nothing* (`Row::wrapped` carries the flag to both, so the two can never break in different
places), and the trailing-blank trim is skipped on a wrapped row — there is no padding to trim there,
and a blank in the middle of a logical line is a space, not padding. Every other terminal unwraps on
copy for the same reason: a pasted command has to be the command that ran.

### One cell can be a real selection (`ui/selection.rs`)

`anchor == head` used to mean "nothing selected", which is right for a drag — a bare click deselects in
every terminal — and wrong for a one-letter word (`cd a`). The two are indistinguishable by their
positions, so a `Selection` now records **what made it**: a `Drag` collapses to empty, a `spanning`
range does not. Copy stays disabled after a bare click and enabled on a one-character word.

That distinction also **fixed two older one-cell blind spots**, both found by asking who else builds a
selection from a range rather than a drag. A **one-character find-bar query** (§35) matches a single
cell, and a **command that printed a single character** (§34) has a one-cell output: each was revealed
or located correctly and then read as "nothing selected" — no highlight, Copy greyed out. Both now go
through `Selection::spanning`, which is the same call the word and line expansions use.

### What is deliberately NOT here

- **Dragging on from a double click does not extend word by word.** xterm grows the selection a word at
  a time when the pointer moves after a double click; cmote's word and line selections simply do not
  drag (`selecting` is left false). Not laziness: the pointer is already parked on the word, so
  extending from the press cell would collapse the whole span on the first stray pixel of movement.
  `ponytail:` word-granular dragging.
- **A double click on blank space or on a separator run selects nothing**, where some terminals select
  the run of spaces. Nothing is the more useful answer at a terminal, and the cheaper one.
- **No configurable word characters.** The set above is a constant with its reasoning written beside
  it. A settings-file knob for it would be a preference nobody asked for yet (§13's rule).
- **The wrap-aware copy is only as good as the engine's flag.** A line whose wrap flag a reflow moved is
  re-joined wherever the flag now says — which is the engine's own reflow answer, and the same one its
  own selection uses.

---

## 43. A resize invalidates what was anchored to the grid (v4.0.0)

Everything §40 gained by anchoring the selection in **absolute document lines** rests on those numbers
meaning the same thing from one frame to the next. A **resize** is where they stop: re-wrapping the
scrollback at a new width changes how many lines the history holds, so a line number recorded before the
resize names *other text* after it.

`Terminal::resize` already knew this. It drops the prompt marks (§34) and the inline images (§41) on a
reflow for exactly this reason. What it could not reach is the state that lives a layer up, in the tab:
**the selection and the find bar's match list**. So a drag over a path, a window dragged narrower, and
the highlight was still on screen — over whatever the reflow had moved onto those lines. Copy then put
text on the clipboard that the user never selected, with nothing on screen to say so. A highlight that
lies is worse than no highlight at all.

### The selection is dropped, not remapped (`app.rs`)

`on_grid_reflowed` is the one place a reflow's fallout is handled, called from the single path a window
resize *and* a files-pane resize both already take (§19) — so one of the two cannot be fixed while the
other stays broken. It drops the selection and any drag with it.

Dropping rather than mapping is the deliberate half. The engine's reflow is the only thing that knows
where a given cell ended up, and it exposes no such map; reconstructing one would mean tracking each
selected cell across a re-wrap that can split a row in two or join two into one. The cost of getting
that wrong is silent: a plausible-looking highlight over text the user never chose. The cost of dropping
it is one re-drag, which is what every terminal that reflows asks for.

### The find bar is re-scanned instead (`app.rs`, `term/search.rs`)

The bar is *not* dropped, because it does not need to be: its washes are rebuilt from the match list on
every frame, and `Terminal::find` can produce a fresh list from the reflowed grid at any time — which is
already what a step does before it moves (§35). So a resize re-scans the same query, and `refresh` keeps
the current match by identity wherever it survived, falling back to the newest hit where it did not.
Without the re-scan the washes stay put over moved text, which is the same lie as the selection's, drawn
in a different colour. The revealed match's own highlight goes with the selection above; the next step
puts it back.

### Two smaller things the old grid owned (`app.rs`)

- **The multi-click tally** (§42) counts presses that land on **one cell**, and that cell shows different
  text now — so it starts over. Otherwise a press there within the 500 ms window would expand a word off
  the back of a single click the user made on something else entirely.
- **The hovered cell** is resolved again from the last known pointer position against the new grid,
  exactly as a pointer move would (§10). The pointer has not moved, but the cell under it has, and a
  press can arrive before the next move does — a keyboard resize, a window snap — which on a shrunken
  grid would otherwise anchor at a row that no longer exists.

### What is deliberately NOT here

- **No remapping of a selection through the reflow**, for the reason above. `ponytail:` if the engine
  ever exposes its reflow mapping, this is where a survived selection would be built from it.
- **A height-only resize drops the selection too**, though only a *column* change re-wraps: dragging the
  bottom edge of the window moves lines between the screen and the history without renumbering them.
  Keying on any grid-size change keeps one rule here and the same rule `Terminal::resize` uses for the
  marks and the pictures, at the cost of a selection that a vertical drag could have kept. `ponytail:`
  the narrower condition.
- **The scrollback cap's own drift is untouched.** Past `SCROLLBACK` lines the history stops growing and
  `history_size + row` stops naming a fixed line, so every absolute anchor drifts as output arrives —
  §34's marks, §41's images and the selection alike. That is a property of the coordinate, not of the
  resize, and it takes 10 000 lines of output to reach.

---

## 44. The find bar keeps up with live output (v4.0.0)

§43 closed one half of a two-part hole: a match list is built once, and it describes the document *as
it was at that moment*. A resize was the loud half. **Output arriving** is the quiet one, and it goes
wrong in two ways at once:

- **Hits the bar has never seen.** A `tail -f` or a long build prints the query again and again; none
  of it joined the count, and none of it was washed (§39), until the user stepped or retyped the query.
  The bar answered a question about the past while the answer scrolled by in front of it.
- **Stored lines that stop pointing at their text.** Once the history is at its `SCROLLBACK` cap
  (§23) it stops growing: each line that scrolls off drops the oldest one, so the text that was at
  absolute line *N* is at *N-1*. Every stored match drifts one line further from its text per line of
  output — and the washes drift with them, onto text that never matched. §43's *what is deliberately
  NOT here* listed that drift as untouched; for the find bar, it now is, because a fresh scan re-derives
  the lines rather than adjusting them.

### Marked on output, scanned on the next frame (`app.rs`)

The output arm does **not** scan. `Terminal::find` walks every retained line — up to 10 000 of them,
cell by cell — and a flood of output arrives as dozens of `SshEvent::Output` chunks per frame, so a scan
per chunk would spend the frame searching instead of drawing, exactly during the moment the terminal has
the most to do. Instead the arm sets one flag, `Tab::search_stale`, and the flag subscribes to
`iced::window::frames()` — so a burst of chunks collapses into **one** scan on the next frame.

That is the same shape the copy toast has used since §10 and the quit drain since §30: the frame clock
exists only while there is work for it, and clearing the flag is what removes it again. No timer is
created and none has to be cancelled. Three details fall out of it:

- **An empty query and a closed bar never start the clock**, so an ordinary session — no bar, or a bar
  with nothing typed in it — pays nothing at all for this.
- **Only the ACTIVE tab's flag is consulted.** A background tab's bar is not on screen; its flag stays
  set and is honoured the moment the user comes back to it, which is the first frame that can show it.
- **The flag is cleared unconditionally by the re-scan**, before anything else. A tick that arrives
  after the bar closed in the same batch still has to stop the ticking.

### A re-scan is not a step (`app.rs`)

`rescan_find` rebuilds the list and nothing else: no reveal, no scroll, no selection. Three callers now
share it — a reflow (§43), a frame tick with output waiting, and a step, which scans first so a hit
printed since the query was typed can be stepped onto. Only the step reveals afterwards, because only
the step is something the user asked for. A shell printing under an open bar must not drag the viewport
about while the user is reading a hit up in the history; what changes on screen is the count and the
washes.

### What is deliberately NOT here

- **No incremental scan.** Every re-scan is a full walk. The obvious optimisation — scan only the lines
  that arrived — cannot fix the cap drift, because the engine reports no count of the lines it evicted,
  so the older matches would still need renumbering by a figure nothing hands out. `ponytail:` the whole
  document, once per frame, while a query is live and output is flowing.
- **The current hit can jump at the cap.** Drift breaks the identity `refresh` keeps the current match
  by (§35), so past 10 000 lines a re-scan falls back to the newest hit — the bar behaves as though the
  query had just been typed. Correct rather than pretty, and the next ↑ walks back from there.
- **A revealed match's selection is not re-pointed.** The selection is the user's, and it may not even
  be the match any more — they can drag a new one with the bar open. Moving it on output the user did
  not ask for would be the surprise; leaving it is the same drift §43 already documents, in the one case
  (a full scrollback) that has it.
- **§34's prompt marks and §41's images still drift at the cap.** Neither can be re-derived: they are
  recorded from OSC events as they arrive, not read back off the grid, so there is nothing to re-scan.
  The find bar is fixable here precisely because its list is derivable from the text.
- **The bar does not open itself, and nothing searches while it is closed.** No background matching, no
  "N new matches" badge on the tab chip (§34's dot is a command status, not a search one).

## §45 — More than one account on one connection (v4.0.0)

`rec.michoacan` accepts `cme` and nothing else: root logs in over SSH nowhere sane. So the way to be
root there is to become root **after** logging in — `sudo -i` — and until now cmote could only do that
the way any terminal can: by the user typing it, into the one shell, and losing `cme` in the process.

An SSH session authenticates ONCE, as one user, and everything on it runs as that user. Becoming
another account is therefore not an SSH matter at all: it is a *program* (`sudo`, `su`) run on the
connection, which holds a short conversation — a password, perhaps a one-time code — and then replaces
itself with a login shell for the other account. This section makes that a first-class thing: a session
holds a SET of shells, one per account, each with its own terminal, and the status bar says which one
is on screen.

What it deliberately does not do is pretend the file panes came along. They did not — see the NOT list.

> **STATUS: the UX is withdrawn; the machinery stays.** There is no way to START an elevation from the
> app any more — the "Log in as…" button, the context-menu item, the elevate dialog and the account
> switcher are all gone, and with them the app-side state they fed (`ElevateDialog`, the `Elevate*`
> messages, `IdentityChoice`, the cached sudo password). The approach is being reconsidered after §46
> met a two-factor server: the terminal handled it, but the file side could not (see §46's NOT list),
> and the shape of the dialog is entangled with assumptions that may not survive the rethink.
>
> Everything below the UI line is untouched and still compiled: `elevate.rs`, `ssh/asuser.rs`,
> `ssh/shellfs.rs`, the shell SET and its credential conversation in `ssh/shell.rs`, the `Elevate*`
> commands and events in `bridge.rs`, and the app-side identity list, parked `Workspace`s and switch.
> Their tests stay too, driven directly rather than through a dialog — they are what will keep the next
> attempt honest. `elevate::valid_user` carries an `#[allow(dead_code)]` and a note saying why it was
> kept rather than deleted.
>
> Nothing survives above the line. The status bar's read-only account label was the last piece and has
> since gone too: it repeated, on the same row, the `user@` the centred endpoint already carries. With
> it went the two fields that existed only to feed it — `Identity::user` and `Tab::login_user`.

### One shell was one channel; now it is a set (`ssh/shell.rs`)

`stream()` used to hold the shell channel, await it and write to it. `shell::Shells` now holds them all,
keyed by an identity number the GUI assigns (`bridge::LOGIN_IDENTITY` for the account the session logged
in as, then 1 upward). Two problems came with the set, and the shape is the answer to both:

- **Awaiting N channels.** `Channel::wait` needs `&mut`, so awaiting several in one `select!` would hold
  N mutable borrows across an await while the same loop wants to write to them. Every channel is
  `split()` instead: the reading half moves into a task of its own that forwards each message down one
  shared mpsc tagged with its identity, and the writing half stays in the map. The session loop awaits a
  single receiver, and writing borrows nothing reading holds — so the `select!` keeps the shape it had
  however many shells are open.
- **Output that is not output.** While `sudo` is still asking, the channel's bytes are a credential
  conversation, not something to draw. A shell is `Elevating` before it is `Live`, and only a `Live`
  shell's bytes become `SshEvent::Output`. What was buffered when the conversation ends is flushed as
  output, so the account's greeting and its first prompt are drawn rather than swallowed — and
  `IdentityReady` goes out **first**, because the GUI builds that identity's emulator when it hears it
  and output for an identity that has none was dropped. Sending the flush first therefore lost exactly
  the two things it exists to carry, and left a freshly elevated terminal blank but for its caret. The
  GUI now also builds an emulator on demand for an identity that has none, so neither side alone can
  lose those bytes.

Three commands are untagged on purpose and one is newly tagged, which is the whole routing rule:
`Input` goes to the SELECTED shell (the GUI says which with `SelectIdentity`, on the same ordered
channel, so a switch and the keystrokes after it cannot cross); `Resize` goes to EVERY shell, since they
share one window and an off-screen pty laid out for an old size would be wrong the moment it came
forward; `Output` carries its identity, because the shells not on screen keep running and a build left
in `cme`'s shell must go on filling `cme`'s scrollback. `Reply` is new and named: a query answer (§23)
must reach the shell whose program is blocked on it, which is not necessarily the one being looked at.

### Why the elevation gets its own channel, and not the shell it has (`elevate.rs`)

The obvious implementation is to type `sudo -i` at the live prompt and watch for a password prompt. That
is what makes it dangerous: if sudo never asks — already root, sudoers refuses, a slow command still
running — the password goes to whatever owns the pty, which may echo it, log it, or take it as a command
line. cmote runs the elevation with `exec` on a channel of its own, so the process on the other end IS
`sudo` and nothing else. A secret written there cannot reach a shell, a running command or a history file.

Recognising the questions is then only about labelling, and `elevate.rs` is the pure text of it —
`&str` in, `String` out, no channel and no session, so the two judgements that carry risk are unit
tested line by line:

- **The password prompt is named by cmote itself.** `sudo -p 'cmote-password:'` turns the one question
  that can be predicted into an exact string match, instead of guessing at `[sudo] password for cme:` in
  whatever the remote's locale is. The user is shown "Password:" — the marker is an internal token.
- **A second factor is recognised by its wording,** because a PAM module sudo knows nothing about asks
  it. Only the tail after the last newline can be a question (a program asking leaves the cursor on the
  line it asked on; a terminated line is output), it must end in `:` or `?` after escape sequences are
  stripped, and it must contain a credential word.
- **When in doubt, say nothing.** A shell prompt CAN end in a colon. Guessing "credential prompt" over
  one would put a secret field in front of the user and land what they typed on a root shell's command
  line; guessing the other way costs them one thing — typing the code into the terminal themselves,
  exactly as they do today. So an unmatched prompt raises no dialog, and a tail ending in `$ # % >` ends
  the conversation for good: from then on nothing on that channel is ever read as a question again.
- **The account name is validated, not merely quoted** (`valid_user`). It is the one string in cmote
  composed into a command that runs on a remote machine as another user; `root; rm -rf /` is refused at
  the field, and quoting is the second line of defence rather than the only one.

### Switching accounts is a swap, not a re-purposing (`app.rs`)

An identity that is not on screen keeps its own `Workspace`: terminal, selection, drag state, click
tally, find bar. The fields for whichever identity IS on screen stay exactly where they were — as
`Tab`'s own fields — so the thousands of lines that read `self.terminal` never learn that identities
exist; `Tab::exchange` moves a whole view in and the live one out in one step. That one function is the
thing that has to be complete: a `Workspace` field without a line there would leak one account's state
into another's pane, which is why it is a `swap` of every field rather than anything cleverer.

The consequence worth having: `cme`'s scrollback, cwd, prompt marks and find bar are all still there
when the user comes back to it, and a long build keeps printing into them while root's shell is on
screen.

### The status bar's account label (`ui/terminal.rs`) — gone

Nothing of this section is left above the UI line. The label outlived the switcher it belonged to for a
while: plain text at the head of the right group, the account whose shell was on screen, read from the
identity list. What it did NOT do is tell the user anything the bar was not already saying. The centre
zone is the session's `user@host:port`, on the same row, a few centimetres away — so the account was
printed twice, and two labels for one fact read as though they could disagree. They could not: with the
switcher withdrawn a session has exactly one shell, so `current_user` could only ever return the account
the endpoint already names.

Removing it took `Identity::user` and `Tab::login_user` with it, since the label was the only reader of
either. That is deliberate rather than tidy-up: a name nothing displays is a name nothing keeps honest,
and the elevation that brings a second account back is what should add it, beside whatever shows it.
What stays is the machinery that cannot be re-derived — the identity NUMBERS, the parked workspaces and
the routing that keeps a background shell's output out of the foreground grid.

What stood here before that, and is withdrawn: a "Log in as…" button beside that label and the same item in the
terminal's context menu, both raising `ElevateOpen`; a select in place of the label once a session held
two accounts; and the elevate dialog itself — one conversation in two faces, a FORM (which account,
`sudo` or `su`) until something was asked and then the remote's QUESTION with a masked field under it,
owning the keyboard while it held a secret so that nothing typed into it also reached the shell behind
it. The reasoning is preserved in git (`feat: a Log in as… button, and an elevation that survives two
factors`), which is where the next attempt should start reading: the keyboard-ownership rule and the
"never a password field over a live prompt" rule are properties of the problem, not of that dialog.

### The password is typed once; a one-time code never is

*Half of this is withdrawn with the dialog: the GUI's own cached password went with `ElevateDialog`,
since nothing can ask for one now. The SESSION's copy stays — `Shells::answer` still reports whether an
answer was the first factor's, and `Accounts::set_secret` still keeps it for `sudo -S` on a file channel
(§46). The rules below are why they are shaped the way they are, and the next attempt inherits them.*

The sudo password is kept for the connection's life in a `Secret` (redacted in `Debug`, wiped on drop)
and dropped when the session ends — never to the vault, never to a profile: a sudo password is usually
the account's own login password, and persisting it would turn a session-lifetime secret into one at
rest (§12). A second elevation that asks for the same password is answered from it with no dialog. A
one-time code is asked for every single time, which is what "one-time" means.

**Which question is which cannot be read off the wording**, and getting that wrong was a real bug on
exactly the machines this feature exists for. sudo substitutes its own `-p` text for every *standard*
prompt in its PAM stack, so on a two-factor server the second factor is asked for under `cmote-password:`
as well — one label, twice, in a conversation going perfectly well. cmote used to read the repetition as
"that was refused", and drew three wrong conclusions from it: the dialog told the user their good
password had been rejected, the cached password was thrown away, and the code they then typed would have
been cached in its place — from where §46's file layer would have fed a spent one-time code to `sudo -S`.

Two rules replace the inference, and neither looks at the wording:

- **A refusal is what the program SAID.** `elevate::refusal` reads the lines printed between the last
  answer and the next question, and reports the remote's own line when one of them reads as a rejection
  ("Sorry, try again.", "Authentication failure"). A further factor prints no such thing. It rides the
  `ElevatePrompt` event, so the GUI is told rather than guessing, and the dialog shows the remote's words
  rather than a canned sentence — "Sorry, try again." and "user cme is not allowed to execute…" ask
  completely different things of the user. Matching nothing shows nothing, which is the safe direction.
- **Only the FIRST FACTOR of an elevation is a password.** The cache answers that one and no later one,
  and only that one's answer is cached — on both sides: the GUI's copy for the next elevation, and the
  session's copy that `sudo -S` replays on a file channel (§46). A later factor is a code whatever it is
  dressed in, so caching it once had a spent one-time code standing in for the connection's password.
  *Factor*, not *question*: a question the program re-put after refusing the answer is the same factor
  again, so `ssh::shell` counts only the questions that follow no refusal. That is what lets a password
  corrected on the second go still be kept, and it is the same count §46 reads to decide whether the file
  side can follow the account at all.

And because the wording cannot say it, the dialog does: a further question with nothing refused carries
the line *"Nothing was refused — the remote is asking for one more answer."* cmote cannot name the factor,
but it knows those two facts, and together they are the difference between a user who types their code and
one who retypes their password at a prompt that has already had it. In cmote's own colour, not the
notice amber — nothing is wrong.

### What is deliberately NOT here

- **The folder tree, the files pane and the transfers are still the LOGIN account's.** They do not go
  through the shell at all: each opens its own channel and asks sshd for the `sftp` subsystem, which
  sshd starts as the account the session authenticated as. `sudo` in a terminal cannot reach that
  process, so root's terminal beside `cme`'s file panes is the honest state of things until §46 gives
  the file layer its own elevation. `Workspace` deliberately does not park them: splitting one view per
  identity now would only pretend they differ.
- **A localized second-factor prompt may raise no dialog, and that costs the elevation.** The vocabulary
  covers English plus a few common spellings; anything else falls through. This is the safe direction of
  the two — the alternative is a password field over a live root prompt — but the earlier claim here, that
  the question then "appears in the terminal where the user answers it directly", was simply false: an
  `Elevating` channel draws nothing and takes no keystrokes, so an unrecognised question leaves the dialog
  waiting until it is cancelled. Handing an unrecognised conversation to the grid is a real gap, not a
  decision, and it is not built.
- **A refusal cmote cannot see is a question asked twice with no explanation.** `refusal` reads the
  program's own words, so a sudo configured with no failure message (or one worded outside the list)
  re-asks silently. The user sees the same question again — exactly what a terminal shows — and answers it.
  What that costs is the cache: the wrong password is not dropped, and the corrected one is not stored in
  its place, because a re-ask and a further factor are indistinguishable without that line. So the next
  elevation offers the stale password once (of sudo's three tries) before asking. Erring this way is
  deliberate: the other way round would cache a one-time code as the connection's password.
- **A shell prompt ending in `:` with no second factor configured would be read as a question** if it
  also contained a credential word (a cwd called `~/code`, say). `ponytail:` the residual case of the
  heuristic above; the elevation still works, the user sees one spurious field and cancels it.
- **Two sudos means two authentications.** sudo's credential cache is per-tty by default
  (`tty_tickets`), and each shell has its own pty, so the cached password saves the typing but not the
  second factor. This is what will make §46 ask for a code again.
- **No way in at all, for now.** The right-click item and the status bar's button were the two, and both
  are withdrawn — see the status note at the top. A per-profile "elevate on connect" was §47, which is
  part of what the rethink covers.
- **An identity is not a tab.** It shares the connection, the tab strip stays one chip per session
  (§26), and the MRU (§37) knows nothing about accounts. Closing an elevated shell is `exit` at its own
  prompt, or cancelling the dialog that opened it.

## §46 — The file panes follow the account you switched to (v4.0.0)

§45 left a session with more than one shell but only one set of eyes on the filesystem: elevating gave
a root terminal and left the folder tree, the files pane, every transfer and the editor reading as the
login account. That was not an oversight in the implementation — it is what SSH does — and closing the
gap is this section.

> **STATUS: reachable only when §45's UX returns.** Nothing here was removed and nothing here changed:
> `Accounts`, the elevated `sftp-server` channel, the shell fallback and the per-account backends are
> all still compiled and still tested. But no account can be elevated into any more, so every listing
> runs as the login account and the code below waits. A two-factor server is part of why the UX is being
> reconsidered — see the second-factor bullet in the NOT list, which is the sharpest constraint any new
> approach has to answer.

### Why sudo in the terminal could never fix it

The file panes never touch a shell. Each opens a channel of its own and asks sshd for the **`sftp`
subsystem**, and sshd starts that subsystem itself, as the account the session authenticated as. There
is no command line in that path to put `sudo` in front of. So the only way to read files as another
account is to stop asking sshd for the subsystem and run the same program ourselves:

    sudo -n -u root -- /usr/lib/openssh/sftp-server

`sftp-server` speaks the SFTP protocol on its stdin and stdout, which is exactly what the subsystem
provides. Run it that way and every feature built on SFTP — the tree, the pane, resume, tree transfers,
the editor's atomic save — works unchanged, as the other account, with no second authentication to the
server. Three things had to be solved to make that real: finding the binary, authenticating sudo without
a terminal, and a fallback for a remote that has no such binary to run.

**Finding it.** Packaging moves it (`/usr/lib/openssh`, `/usr/libexec/openssh`, `/usr/lib/ssh`,
`/usr/libexec`, …), so cmote looks in the usual places and then asks sshd's own configuration —
`Subsystem sftp` names the program it starts for the login user. The path that comes back from
`sshd_config` is REMOTE-CONTROLLED text about to be composed into a command that runs as another
account, so `elevate::valid_program` whitelists it rather than escaping it: absolute, ordinary path
characters, no `..`, and a file name that mentions `sftp`. `internal-sftp` — sftp implemented inside
sshd — is refused, because it is not a program and cannot be run as anyone.

**Authenticating it.** There is no pty on that channel, deliberately: SFTP is binary, and a pty would
translate line endings and interpret control bytes in the middle of it. So sudo cannot prompt, and
`sudo -S` is used instead — it reads the password from stdin one byte at a time, stopping at the
newline, which leaves the rest of the stream to the program it execs. The ORDER is the safety property,
and it is written as a list of attempts (`Entry::attempts`):

> The password is written **only after** a non-interactive (`-n`) attempt has been refused for the want
> of one.

Because sudo holding a valid credential — a cached ticket, NOPASSWD — does not read its stdin at all. A
password sent on a guess would therefore not reach sudo: it would land in `sftp-server`'s input, as
protocol garbage, on a process running as root. Getting that order wrong is the one way this feature
could have leaked a secret, so the verdict is only ever set by an OBSERVED refusal, and it is remembered
per account so the wasted first attempt is paid once rather than per click. The password itself is the
one §45 already keeps in memory for the connection: `ssh::shell` now reports whether an answer was the
question cmote NAMED itself (`-p cmote-password:`), and only that one is kept. A one-time code never is.

### The chain, and what each link costs

1. **Elevated SFTP** (`asuser`) — the whole feature set: typed listings, metadata, atomic saves.
2. **Shell commands** (`shellfs`) — for a remote with no `sftp-server` binary to run. `ls`, `cat`,
   `wc -c`, `find`, `mkdir`, `mv`, `rm` under the same sudo. The bytes need no encoding: an exec channel
   is binary and there is no pty, so `cat file` puts the file's exact bytes on the channel and
   `cat > file` writes exactly what is sent — no base64 layer, no new dependency. It costs metadata
   (`ls` text carries no owner, size or time into the pane) and the timestamps and mode a copy would
   have been stamped with, and it inherits the text-is-a-guess caveats the pre-§46 `ls` fallback had.
3. **Nothing, said plainly.** Where sudo itself refuses, every operation fails with the remote's own
   words and the panes list nothing. They never quietly fall back to the login account's files while the
   terminal says root: a pane that lies about whose eyes it is using is worse than an empty one.

### What moves when you switch account

The panes are NOT parked per account the way the terminal is, and that is a deliberate difference. A
scrollback is a record of what an account did, so it belongs to that account; a folder is a **place**,
and the ordinary reason to become root is a file in the folder you are already looking at. So the path
stays and the contents are read again through the new account's eyes (`Explorer::reread`,
`App::reread_panes`). Both panes are emptied first — `cme` cannot see inside `/root`, root sees keys in
`/etc/ssl/private` that `cme` cannot — so nothing another account listed is on screen while the new
listing is in flight, and a failure leaves them empty beside the reason. The re-listings are sent AFTER
`SelectIdentity` on the one ordered channel, so a listing can never be answered by the account being
left.

The editor is the exception, and the interesting one: an editor tab keeps the identity it was OPENED as
for its whole life (`Editor::identity`), and its save names that account. A file read as root is a
root-owned file; saving it as whoever happens to be on screen minutes later would fail — or, on a
chrooted server, succeed against a different file of the same path.

### Structure

`Accounts` (in `ssh::asuser`) holds one entry per identity and is where everything LEARNED about an
account lives: its sftp session (one per account, kept for the connection, since a tree asks many small
questions), where `sftp-server` is, whether sudo wants a password, whether SFTP works at all. Every file
command in the session loop now begins by resolving that account into one `Browse` / `Files` value, so
`browse`, `upload`, `download` and `edit` never learn that accounts exist — they match on a backend,
exactly as they used to match on "sftp session or exec channel".

One structural rule came out of russh: `client::Handle` is not `Clone`, so a spawned task cannot open a
channel for itself. The shell backend needs one channel per command, so `asuser::Channels` lets a task
ASK the session loop for one (the reverse of the pattern §27's forwards already use). Which brings the
one way to deadlock this design, written at the top of `asuser.rs`: a `Runner` may only be awaited from
a spawned task, never from the loop that serves it. Work that must happen inline — the discovery probe,
opening an sftp session — uses `exec_inline`, which borrows the loop's own handle.

### What is deliberately NOT here

- **`su` cannot serve the file layer.** It reads a password from a terminal only, and this channel has
  no pty by design. A `su` identity whose account needs no password works; otherwise the panes report it
  and stay empty. `sudo` is the supported path.
- **`requiretty` in sudoers blocks it too**, for the same reason — a pty would corrupt the protocol, so
  cmote will not request one to work around it. It falls to the shell backend, which fails the same way,
  and the reason shown is sudo's own.
- **Two sudos, two authentications**, as §45 predicted: the terminal's sudo and the file layer's sudo
  have separate credential caches (`tty_tickets`, and the file channel has no tty at all). The cached
  password covers it silently.
- **A second factor puts the files out of reach, and is said so up front.** The earlier claim here — that
  such a server "will ask again on the first file operation" — was false: there is nowhere to ask. That
  channel has no dialog behind it and carries a binary protocol, and the code the terminal used is spent.
  What actually happened was two ten-second handshake timeouts per account, a channel burnt on each (which
  on a server near its `MaxSessions` took the LOGIN account's file access down with it: `Failed to open
  channel (ConnectFailed)`), and then empty panes that said nothing. So `ssh::shell` counts the FACTORS an
  elevation took — distinct questions, not questions asked, since a refused one is the same factor over
  again — and more than one has the session mark that account `denied` in `Accounts` before any listing.
  Both backends are refused with one sentence saying why, which is what the panes then show. The terminal
  is unaffected: it holds a real conversation and can answer anything.
- **The shell backend has its own copy loops**, not the SFTP ones made generic. `ponytail:` making
  `upload`/`download` generic over a filesystem trait would have meant rewriting working transfer,
  resume and conflict code (§16, §17, §19) with no way to test it against a real server — so the
  duplication went into the path that runs on almost no server, rather than the risk into the path that
  runs on all of them.
- **No metadata on the shell backend.** No owner or size in the pane, no mtime/mode stamped on a copy;
  a listing that way is names and types. The reason is in `shellfs`'s header.
- **The panes do not label themselves with the account.** The status bar's account control already says
  which one the session is showing, and the panes always agree with it — so a second label would only be
  a second thing to keep in step. A FAILURE names the account and the remote's reason, which is the case
  where it matters.
- **Nothing is cached across a switch.** Going to root and back re-lists both panes each time, one round
  trip per open folder. Correct and simple; a per-account listing cache is the obvious optimisation if it
  is ever felt.
- **Still no auto-elevate on connect, and no vault-stored sudo password.** Both are §47, which is where
  the profile format changes.

## §48 — Splitting the window (v4.0.0)

Tabs (§26) gave the window many sessions and showed one at a time. That is the right trade when the work
is sequential — read a log, then go and fix the thing — and the wrong one when it is not: watching a build
on one host while editing on another meant clicking back and forth and holding the other side in your
head. This section makes the window divisible. A split cuts it into **regions**, each with its own tab
strip and its own tab on screen, so two sessions can be watched at once.

The strip gained two buttons at its right-hand end: cut this region in two and put a fresh one **beside**
it, or **below** it. The fresh region opens as a whole small application — one tab, sitting on the saved
target list — because the reason to ask for a split is almost always to go somewhere else, and that is
where going somewhere else starts.

### One cut, offered from one place

A window holds **at most one split**: one region beside the original, or one below it, never both and never
a split of a split. The two buttons appear only while the window is whole — which is also the only time
there is a single region to offer them from, the original one at the top left. Make the cut and they go
from both strips; close the second region and they come back.

Two windows' worth of work side by side is the case this section was asked for. A grid of four is a
different tool: at that point the regions are small enough that a terminal in one is a few columns wide, the
tint that says which region has the keyboard is competing with three others, and every question below —
where a dropped file goes, what the title bar names, which region a picker's answer belongs to — gets an
answer that is right less often. Refusing the second cut is what keeps all of those answers cheap.

`App::splittable` is the one place the rule lives, and it is a **count of the region tree** rather than a
flag: `regions.len() == 1`. Counting cannot fall out of step with the thing it counts. It also gets the far
end of the feature right for free — close the *original* region and the split one inherits the whole window,
which makes it the top-left region, and it may split. The rule follows the shape of the window, not the
history of how it got there.

The controls are **absent rather than greyed out**, so whenever one is on the strip it works. And the rule
is checked twice, in two different places, for two different reasons: `view` asks it to decide whether to
draw the buttons at all, and `apply_split` asks it again to refuse a cut that got past them. The second is
not belt-and-braces — the monitor is measured asynchronously, so two quick presses both leave while the
window is still whole, and the second arrives to find that it is not. Refusing on arrival is what makes the
rule hold; checking it in `request_split` would check it before the race rather than after.

### The window grows; it does not divide what you had

A split asks the OS to **double the window** along the way it cuts: beside → twice as wide, below → twice
as tall. So the region being split keeps the size it had, and the new one is its equal.

This is the difference between a split being free and a split being disruptive. Halving would reflow the
shell already on screen to half a window — a hard reflow, which drops the selection, throws away the find
bar's match list and re-wraps every line of scrollback (§43). Growing costs the old region half a
divider's width and nothing else.

The size asked for is **clamped to the monitor** first, which is why the flow is two steps rather than one:
`Message::Split` asks the OS how big the screen is, and `Message::SplitSized` does the cutting once the
answer is back. Past the edge of the screen there is no way to reach a region and — since a region's only
handle is the region itself — no way to drag the divider back either. A screen that cannot be measured is
not a reason to refuse the split, only a reason not to clamp against a number we do not have.

### Routing: an event belongs where it happened, not where the keyboard is

This is the one genuinely new idea in the section, and everything else follows from it.

Every region is on screen at once, so "which region is this event for?" stops being answerable from the
layout. The obvious answer — send it to the region holding the keyboard — is **wrong**, and wrong in a way
that would have been easy to ship. A left press inside an unfocused region produces two messages: the
press itself, and the focus change. The press arrives **first**, because `pane_grid` lets a region's own
widgets see an event before it looks at the event itself. Routed by focus, that first click into a split
would land in the *previously* focused region's terminal — clobbering a selection there and starting a drag
nobody asked for.

So `view` wraps everything a region draws in `Message::In(pane, …)`, using iced's `Element::map`. Every
message a region's widgets raise carries the region it came from, and `App::update` applies it there.
`update_in(pane, message)` is the match `update` used to be, with "the active tab" now meaning "the active
tab OF THIS REGION".

The shape of a message is what says where it goes, and there are three shapes:

| Shape | Where it goes | Examples |
|---|---|---|
| `Message::In(pane, …)` | that region | every click, every strip gesture, every dialog inside a region |
| the App's own, unwrapped | the App, sometimes fanned out to every region | SSH events (by tab id), window resize / focus, the frame clocks, the quit flow, the split gestures, the raw pointer that catches a divider double-click |
| anything else, unwrapped | the region holding the keyboard | the keyboard, above all — a subscription has no region of its own |

A pleasant consequence: **nothing inside a region had to change.** Not the strip, not the terminal, not a
dialog, not the file panes. The wrapper carries the one fact any of them would have needed, which is why
§48 touched `ui/tabs.rs` for two buttons and a tint and left every other view file alone.

Two kinds of message had to learn to **fan out** rather than pick a region. The OS window's focus, because
focus reporting is a promise made to the program in each shell (§23) and there is now one visible shell per
region; and the two frame clocks — the copy toast's dwell (§10) and the find bar's re-scan (§44) — because a
region left un-ticked would keep a toast on screen for good. Both are clocks or facts about the window, not
gestures, so neither has a region to belong to.

### Geometry: one place turns a window into a row and column count

`ui::split::regions` asks `pane_grid`'s own layout node for each region's rectangle, and `App::relayout`
hands every region's on-screen tab that rectangle **less the strip above it**. It runs after anything that
can change a region's shape: a window resize, a divider drag, a split, a region closing, a tab coming
forward.

Using the widget's own node to measure is the whole reason `pane_grid` is under this feature rather than a
hand-built tree of rows and columns. A terminal cannot exist until something tells it the exact pixel box
it fills, because that is what fixes its size (§9); a hand-rolled tree would have had to do that arithmetic
twice, once to draw and once to measure, and the two copies would drift the first time a constant moved.
`SPACING` and `MIN_SIZE` are constants rather than arguments for the same reason — a divider drawn at one
spacing and measured at another leaves every grid a column short of its region.

A divider drag stores only the **ratio**, never a pixel count, so a share of the window survives a window
resize instead of becoming a stale measurement of a size the window no longer is.

The pointer needed no work at all, and that is worth saying because it is the thing most likely to have
broken. Every pointer coordinate in cmote was already **widget-local**: `mouse_area::on_move` reports
`position_in(bounds)`, and the two places the grid widget reads the raw cursor immediately subtract its own
laid-out bounds. Both are still right inside a region, because a region's bounds are just a smaller
rectangle. The dialog drags are the same story — they apply pointer *deltas* and clamp against the space
they are drawn in, which is now a region rather than the window, and the App-level overlays (§26, §30) go on
being measured against the whole window because that is still what they float over.

### A double-clicked divider goes back to the middle

A share dragged by hand has no way back. The window is **grown** by a split and never divided, so nothing
in the feature ever re-centres a seam once it has moved — the only route to an even split was to close the
region and cut again. A **double-click on the divider** is that way back, and it is the gesture every
desktop already spends on "reset this handle".

Catching it is the interesting part, and it is the one press in the whole window that reaches no widget.
`pane_grid` **captures** a press on a seam to start its own resize gesture and publishes nothing — no
`on_click`, no `on_resize` until the pointer actually moves. So there is nothing to hang an
`on_double_click` on: a `mouse_area` wrapped round the frame never sees the press, because iced hands the
event to the child first and a captured event stops there; an overlay strip drawn *over* the seam sees it
first instead and would then have to reimplement the drag it just stole.

So the press is taken off the **raw event stream**, the same `event::listen_with` that already catches
window focus (§23) and the file drop (§29), and the geometry decides what it was: `ui::split::seam_at`
runs `pane_grid`'s own hit test — `split_line_bounds`, widened by the same `LEEWAY` the drag uses — so a
double-click lands exactly where a drag would have. A press anywhere else in the window breaks the run,
which is what keeps a double-click in a shell (§42) from touching the divider.

Two details are the whole of the care here:

- **The position and the moment arrive separately.** iced's press event carries no coordinates, so the
  move stream supplies where the pointer is and the press supplies when. That means one message per
  pointer move — the only such stream in cmote — so `subscription` asks for it **only while the window is
  split**. An undivided window has no seam to hit and pays nothing; a split one pays a field store per
  move, and the repaint iced was going to do anyway.
- **A drag cannot be half of the gesture.** A drag ends with the pointer still on the seam it moved, so
  the nudge-nudge of placing a divider by hand would otherwise read as a double click and throw away the
  share just set. `on_divider_dragged` therefore **forgets the press holding it** rather than blocking the
  next one — the counter is reset, so the double click *after* a drag is two fresh presses and still
  works. Blocking would have cost the user a whole extra click every time they had touched a divider.

The tally itself is `ui::selection::Clicks`, §42's own multi-click counter, made generic over what it
counts: a grid **cell** there, a **seam** here. The rule is the same in both — consecutive presses must be
on the same target, not merely within a few pixels — and a seam is the better target of the two, since a
divider is hundreds of pixels long and the pointer may wander anywhere along it between the two presses.

A third press is a `Triple` and does nothing: the shares are already even, and leaning on the button
should not keep re-doing it.

### Two inversions, each contained in one place

Neither is avoidable and both are a standing invitation to an off-by-ninety-degrees bug, so each is written
down exactly once:

- **iced names a split after its divider.** Two regions side by side are parted by a *vertical* line, so a
  user's "split horizontally" is `pane_grid::Axis::Vertical`. `ui::split::Way::axis` is the only place the
  two vocabularies meet.
- **Material Icons does the same with its glyphs.** `vertical_split` is the picture of two regions side by
  side. The constants in `ui/tabs.rs` are named for what the button *does*, with the codepoint beside them.

### What closing does

Closing a region's **last tab** closes the region, its room goes back to the region beside it, and the
window **gives the OS back the space the split asked for**. Closing the last tab of the **only** region is
still a quit (§30) — it would empty the window otherwise. The live session and unsaved editor confirmations
sit in front of both, unchanged.

The shrink is the exact mirror of the grow, and it follows from the same rule: **the surviving region keeps
the box it already has.** A split hands the region being cut its own size back and puts an equal one beside
it; a close takes the departing region's share and the seam away again. Nothing on screen reflows in either
direction.

Which axis to shrink along never has to be worked out, because the survivor's own rectangle **is** the new
window size. With two regions the survivor already spans the whole window along the axis they share, so its
box differs from the window on exactly the axis the split was made along, and on that axis by exactly what
the split added. One measurement answers both questions.

This is deliberately *not* "halve the window". Between the split and the close the window may have been
resized by hand and the divider dragged well off centre, and halving would be arithmetic performed on a
number the user chose. Measuring the survivor respects both, at the cost of one read of the layout node
before the region goes — after it, the survivor's rectangle is already the whole window and there is
nothing left to read the shrink off.

The one clamp is `settings::MIN_WINDOW`: a divider dragged near the end of its travel can leave a survivor
narrower than the smallest window cmote will reopen, and a size the settings file refuses to remember (§31)
is a window that jumps back to its old size on the next run. The floor is shared rather than restated for
exactly that reason.

There is no separate "close this split" button. A region is defined by the tabs in it, so closing them is
closing it; a button that discarded several at once would need a confirmation of its own and a rule for
what it does to their sessions.

### Structure

- `ui/split.rs` (new) — the frame. `Way` and its inversion, the seam metrics, `regions` for measuring,
  `frame` for drawing, and `seam_at` for asking which divider a raw press landed on. Knows nothing about
  tabs.
- `app.rs` — `Region` is new and almost entirely **lifted, not written**: a window used to *be* a strip of
  tabs, so the tab list, which one is on screen, the activation order (§37) and the strip drag (§38) moved
  off `App` onto `Region` unchanged. `App` kept what there is genuinely one of — the region tree, the focus,
  the window size, the target list, the vault, the id counter, the quit flow — and, for the divider
  double-click, the pointer's window position and the seam click tally.
- `ui/selection.rs` — `Clicks` became generic over what it counts, so §42's grid cells and §48's seams share
  one piece of timing arithmetic instead of two.
- `ui/tabs.rs` — two buttons at the right end of the bar, and a dimmer fill for a region that does not hold
  the keyboard. The buttons are pushed to the far right rather than sitting by the "+", because chips grow
  with their labels and a control that can be pushed out of reach is one a user has to close a tab to get at.
  A strip is *told* whether to draw them (`splittable`) rather than working it out: whether the window may be
  cut is a fact about the window, and a strip can only see its own region.

Tab ids stay **app-wide** and are never reused, so an id names exactly one tab however the window is split.
That is what lets a session's events, an editor's parent and the quit drain all keep working by identity
while positions moved underneath them.

### What is deliberately NOT here

- **No second split, and so no grid of regions** — one cut, from the undivided window, for the reasons above.
  `pane_grid` would nest them arbitrarily deep; the refusal is cmote's, not the widget's.
- **The split layout is not remembered between runs.** `settings.json` keeps the window size (§31) and is
  told about both the grow and the shrink, so a restart comes back the size the window was left — with one
  region. Persisting the tree would mean persisting which tab is in which region, and a tab is a session
  that no longer exists.
- **No keyboard shortcut for splitting**, matching the strip's mouse-only rule (§26). Every modifier
  combination is a key the shell has a claim on, and inventing a global one is a decision about what to take
  away from the remote.
- **A region cannot be dragged onto another**, though `pane_grid` offers it. Both regions are strips of tabs,
  and the gesture would have to say what happens to both.
- **A tab cannot be dragged from one region's strip to another's.** The drag is reported by the chips' own
  pointer events (§38) and a chip belongs to one strip, so a pointer that wanders into another region's strip
  reports nothing there and the release drops the tab where it was. Correct, but a real limitation: moving
  work between regions meant opening it again. **Lifted in §52** — not by teaching the drag to cross the
  seam, which would need the pixel arithmetic §38 exists to avoid, but by a right-click menu on the chip
  that sends the tab to an area by name.
- **A dropped file goes to the region holding the keyboard**, not the region it was dropped on. iced's
  `FileDropped` window event carries no position, so there is nothing to route on.
- **A file picker's answer goes to the region holding the keyboard.** In practice that is the right region,
  because the click that opened the picker also focused it, and a native dialog holds the input while it is
  up. It would be wrong only if the focus moved between opening the picker and answering it.
- **A region can be dragged narrower than a usable terminal, down to `MIN_SIZE`.** `grid_size` clamps to at
  least one cell from there, exactly as it always did for a window dragged down to nothing. A window too
  small to divide at all is left to that same clamp rather than given a new refusal path of its own.
- **No maximize-this-region.** `pane_grid` has one; the dividers already do the job, and a maximized region
  would be a second kind of "which one is showing" to keep in step with the focus.
- **The regions do not label themselves.** The focused region's strip is lit and the others are dimmed, and
  the title bar names the focused region's session (§17) — a window has one title bar however many regions
  are in it. A per-region caption would be a third thing saying the same as the tint and the chip.

---

## §49 — Filtering the target list (v4.0.0)

The home screen (§14) lists every target you have ever connected to, alphabetically, and it grows the way
those lists always grow — one row per machine, forever, because a machine you stopped using is exactly the
one you will want again in six months. At a dozen rows the list is a menu; at sixty it is a haystack you
scroll, and the click that opens the wrong row is a connection to the wrong server. This section puts a
**filter box above the list**: type, and only the rows matching what you typed stay on screen.

### One pattern, two rules — and the wildcard is the switch between them

`glob.rs` holds the whole rule, and it is two rules chosen by what was typed:

- **No `*` and no `?` — a fragment.** The pattern matches anywhere in the text, so `prod` finds
  `web-production-01`. This is what makes the box usable one keystroke at a time. Whole-string matching from
  the first letter would blank the list until the pattern was finished, which is the opposite of a quick
  filter — the point is to narrow *while* typing, not to type a name in full and be told whether it exists.
- **A `*` or a `?` — a glob over the whole text**, the way a shell glob matches a whole filename. `prod*` is
  the rows that *begin* with prod, `*.db` the ones that end with `.db`, `web-0?` a row with exactly one
  character where the `?` is. This is where **anchoring** comes from, and a fragment cannot express it: a
  fragment is free to match in the middle, so there is no way to say "at the start" without a second syntax.

Making the wildcard the switch means the mode is visible in what was typed rather than hidden in a toggle
somewhere. The alternative — matching a fragment always, wildcards included — was rejected because it makes
a trailing `*` mean **nothing** (a fragment already matches with anything after it), so a user typing the
shell habit `prod*` would get an answer that quietly ignored half of what they wrote.

Matching is **case-insensitive** under both rules: a host list is typed in whatever case its naming scheme
happened to use, and nobody filtering it is trying to make that distinction.

The glob itself is the classic **two-pointer walk with one backtrack point**, not a recursion and not a
regex build. Only a `*` can be wrong recoverably — everything else either matches the character in front of
it or fails outright — so the walk remembers the last `*` it passed and how much text that star had
swallowed, and on a mismatch it feeds the star one more character and carries on. That is what lets `*b`
find its `b` at the *end* of `abab` instead of stopping at the first one. Only the **last** star needs
remembering: an earlier star that has to give more ground is reached again by the same rule. Comparison is
by `char`, not by byte, because `?` means one character the user can see — a byte-wise `?` would match a
third of an emoji.

### The pattern is tried against both halves of the row

`Target::matches` asks the rule about the **name** and about the **`user@host:port` endpoint**, and either
hit keeps the row. Both are on screen, so filtering by only one of them would hide rows whose match the user
can *see* — and the two are searched for different reasons: the name for what the machine is for (`build`,
`staging`), the endpoint for where it actually is (a subnet, a login, a port). It matters too that every
target *starts out* named after its endpoint (§14), so a list nobody has renamed is all endpoint.

That method lives on `Target` rather than in the view, which is what lets `app` ask the **same** question of
the selected row that the list asked of every row.

### A filter that hides the selection lets go of it

Every shortcut on this screen acts on the **selection**, not on what the pointer is over: F2 renames it,
Enter opens it, Delete asks to remove it. So a selection left behind a filter is one keystroke away from
renaming or deleting a row that is not on screen, and a confirmation naming a target the user cannot see
reads as a bug rather than as the warning it is. `on_home_filter` therefore drops the selection — and the
context menu anchored to it — the moment the pattern stops matching it. Nothing is lost: re-selecting is the
same click that selected it in the first place.

The context menu is placed by the selected row's **index among the rows on screen**, not its index in the
saved list (§14's fixed `ROW_HEIGHT` arithmetic, since iced does not expose a laid-out position). A
filtered-out selection is simply not found there and the menu is not drawn, which is the right answer: there
is no row for it to point at.

### The box takes the keys it needs and leaves the rest

iced's keyboard subscription delivers only the events **no widget captured**, and a focused text input
captures what it uses. That single fact does the whole keyboard split for free, in both directions:

- **Backspace and Delete are captured** by the field, so editing a pattern cannot reach the list — the
  Delete key raises no delete prompt while the cursor is in the box. This is why the box needed no mode of
  its own.
- **Enter is NOT captured**, because the field is given **no `on_submit`** — iced's text input only captures
  Enter when it has a submit message to publish. So Enter falls through to the screen's key handler and
  **opens the selected target** while the cursor is still in the box: type, press Enter, connect.
- **Ctrl+F** puts the cursor in the box — the browser's shortcut for the same thing. The terminal's find bar
  answers to Ctrl+Shift+F (§35) because a live shell has a claim on plain Ctrl+F; the home screen does not.
- **Esc empties the box** and puts the whole list back. From *inside* the box it takes two presses: iced's
  text input unfocuses on Esc and captures the event, so the first press hands the keyboard back and the
  second one arrives at the screen. That is the widget's behaviour, disclosed rather than fought.

A `shown of total` tally appears beside the box once something is typed, so a short list reads as *filtered*
rather than as *targets missing*, and the empty state says which of the two kinds of empty it is — nothing
saved yet (answered by connecting somewhere) or nothing matching (answered by editing the pattern).

The pattern is **per tab**, like the selection: two regions of a split window (§48) are two places to be
looking for two different machines, and one filter shared between them would move under a user who never
touched it. It is not persisted — a filter is a way of getting somewhere, not a setting.

### What is deliberately NOT here

- **No regular expressions.** A glob is what this list needs: names are `role-environment-number`, and `?`
  and `*` cover every question anyone asks of them. A regex engine would be a dependency, a syntax to
  explain, and a way to type something that matches nothing for reasons that need reading.
- **No fuzzy matching.** It would rank rather than filter, and ranking fights the alphabetical order the
  list is read by (§14) — the row you are looking for would move as you type.
- **No `{a,b}` braces and no `[a-z]` classes.** Both are shell-glob features; neither earns its explanation
  here, and `*`/`?` plus a fragment already reach every row.
- **The filter does not survive leaving the screen** in any way but staying in the field — it is per tab and
  in memory, not in `targets.json`. Nothing about a machine changed, so nothing belongs in the store.
- **No filtering by what is not on the row.** Auth kind, key path and the remembered-secret flag are not
  matched, only the two strings the user can see. A filter that matched invisible fields would hide rows for
  reasons the screen never shows.

---

## §50 — The keyboard follows what you act on (v4.0.0)

§20 gave the window one keyboard and three stops for it — the shell, the folder tree, the files pane — and
made every move between them explicit: a click, Ctrl+Tab, or Esc. Explicit is right for a ring, and it is
wrong for the two moments where the user has *already said* where they are working and only the ring has not
caught up. This section handles those two.

### Typing at a prompt means the prompt

The panels answer to the arrows, the Page keys, Home/End, Tab, Enter, F2, F5 and Esc. Not one of them
answers to a plain character — there is no type-ahead in either panel — so a letter arriving while a panel
holds the keyboard could only ever have been meant for the shell. It was dropped: the panel swallowed
everything (§20's "a focused panel keeps every key"), nothing happened on screen, and the first character of
a command disappeared. Usually several, because nothing about a swallowed keystroke says it was swallowed —
the user finds out when the echo they expected is missing and has to work out how much of what they typed
survived.

Now typing **hands the keyboard to the shell and goes with it**. The focus moves before the key is
dispatched, so the letter that asked for the move is the letter that reaches the prompt rather than being
spent on the switch.

What counts as typing is `is_typing`, and it is deliberately narrow — two conditions, both required:

- **A `Character` key, never a `Named` one.** Enter, Tab, the arrows, F2, Esc, Backspace and Delete are all
  `Named`, and every one is a panel's own key. Writing the rule on the *produced text* instead — the obvious
  alternative, since winit hands one to most keys — would catch Enter, which carries `"\r"`, and take the
  tree's "send the shell there" away from it.
- **No Ctrl, Alt or Logo.** Those make a combination, not a character: the files pane's Ctrl+A takes the
  whole listing (§21), and Ctrl+Tab is the way out of a panel at all. Shift is let through, since a capital
  is as much typing as a small letter.

`ponytail:` on Windows AltGr arrives as Ctrl+Alt, so an AltGr character — `@` on an AZERTY layout — reads as
a combination and does not on its own hand the keyboard over. The letters around it do, which is the case
that matters: a command starts with a word.

The rule is one-way. Typing in the shell never moves the focus *to* a panel, because a panel has nothing to
type into (its rename fields are modal and take the keyboard whole, §18, §19).

### A command from the terminal's surface means the terminal

The grid's right-click menu — Copy selection, Paste, Upload…, and Open / Copy link on a link cell (§10, §17,
§24) — and the status-bar buttons that duplicate the first two used to leave the keyboard wherever it was.
The case that shows why that is wrong is **Paste**: pasting a command while the files pane held the focus put
the text at the prompt and left the *next* keystroke — the Enter that runs it — going to the pane.

`on_terminal_command` now puts the ring back on the shell for every item of that menu. Paste is the sharp
case, but the reading covers the rest: a copy of the scrollback, an upload into the shell's own directory, a
link followed out of its output are all work on the terminal, and none is a reason to keep the keyboard
parked on a panel.

**Ctrl+V is that same command off the keyboard**, so it is answered the same way — from wherever the ring
is, and it brings the ring with it. That means it moved *above* the focus dispatch in `on_key`: left in the
copy/paste block below it, it was only ever reached with the shell already focused, so a paste asked for
while a panel held the keyboard was dropped on the floor with no echo to say why. Neither panel claims
Ctrl+V, so nothing is taken from them. Ctrl+Shift+V is the same shortcut and pastes the same plain text
(`is_paste` covers both, matched on the physical key so it holds on AZERTY and Dvorak).

**Ctrl+C is deliberately not treated that way.** It reads the terminal's own selection, or — with nothing
selected — is the interrupt for the remote. Neither is text going *in*, which is what this whole section is
about; and of every unclaimed shortcut, "copy what is selected here" is the one a panel has the best claim
on the day it wants it.

**An item does this; the right-press that opens the menu does not.** Opening the menu is a question about
what is under the pointer, and every way of leaving it unanswered — Esc, a click on the dismiss layer —
leaves the window as it was, keyboard included. Only choosing an item is an act on the terminal. (A LEFT
click on the grid has always focused it, §20; that is unchanged.)

Both moves go through `set_focus`, so **focus reporting** sees them like any other (§23): a program that
asked for `?1004` hears `CSI I` when typing or a menu command brings the keyboard back, exactly as it would
for a click on the grid. Reporting the ring rather than the OS window is what makes that consistent.

### What is deliberately NOT here

- **No other shortcut reaches across the focus.** Ctrl+V does, because it is text going into the shell;
  every other unclaimed combination still lands wherever the ring is and does nothing there. Widening that
  would be deciding, in advance, that the panels will never want those keys.
- **No focus move for the panels' own context menus.** A menu item on the tree or the files pane acts on
  that panel, and the panel already had the keyboard when it was right-clicked; there is nothing to take
  back.
- **The right-press does not focus the grid**, per above.
- **Typing does not move the focus away from the shell**, since there is nowhere it would go.
- **No type-ahead in the panels.** Letters could plausibly jump to the entry that starts with them — the
  file-manager habit — but that is exactly the key this section gives to the shell, and the shell has the
  better claim: a terminal is a thing you type at. If type-ahead is ever wanted it needs its own way in (a
  panel-local search field), not a quiet reversal of this rule.

---

## 51. The hand over everything you can pick up (v4.0.0)

§38 made every chip in the strip a drag handle and asked iced for the two cursors the web has
taught everyone to read: `Interaction::Grab`, an open hand, over a chip at rest, and
`Interaction::Grabbing`, a closed one, while a tab is in flight. On Windows that asked for
something the operating system does not have, and the strip has been showing the four-arrow *move*
cursor for both states ever since — a control whose cursor said "this can be moved" but never said
whether you had hold of it.

**Two surfaces wear the hand**, and the second is the reason this is a section rather than a fix:

- a **tab chip**, which drags along the strip to a new slot (§38); and
- a **dialog header**, which drags the card around the window (§10) — and which said nothing at all
  before, arrow at rest and arrow while dragging. A header LOOKS like a title bar, and title bars
  are not reliably draggable, so the affordance was invisible: the way to find out was to try.

They share one implementation and one pair of messages, so whatever becomes grabbable next says it
the same way by calling the same module. That is the point of an affordance — the user learns it
once, on the chips, and it holds everywhere.

Three facts stack up, and none of them is visible from the Rust side:

- **Windows ships no hand cursors.** `IDC_*` has an arrow, an I-beam, a four-arrow move, resize
  arrows, a wait ring, a help arrow — and `IDC_HAND`, which is the POINTING finger used for links,
  not a hand that can hold something. There is no open palm and no fist anywhere in the set.
- **winit therefore collapses the two into one:** `CursorIcon::Grab | Grabbing | Move | AllScroll
  => IDC_SIZEALL`. Asking for two different hands got one four-arrow, so press and release changed
  nothing on screen.
- **iced exposes no custom cursor.** winit 0.30 can build one from pixels (`CustomCursor`), but
  `iced_winit` only ever calls `window.set_cursor(CursorIcon)` and iced hands out no winit `Window`.
  There is no seam to pass an image through.

Which is exactly the situation every browser is in — and they solve it by shipping their own
bitmaps (Firefox's `widget/windows/res/grab.cur`, Chromium's own resources). cmote does the same,
with **its own two drawings** bundled in `assets/` and decoded into `HCURSOR`s at start-up.

### The two hands are drawings (`assets/`)

`assets/cursor-grab.png` and `assets/cursor-grabbing.png`, **drawn at 64×64** on a transparent
background, bundled into the binary with `include_bytes!`, decoded at start-up and resampled to
whatever size Windows asks for.

They began as `const` **ASCII art** — one character per pixel, `#` outline, `.` fill, space for a
hole — for three good reasons: the shapes are reviewable in a diff (a cursor committed as a `.cur`
is a blob nobody reads again), the repository stays free of a third-party asset and its licence
(cmote is MIT; Firefox's cursors are MPL-2.0), and the whole thing cost about a hundred bytes of
`const` per hand. Two rounds of drawing them that way settled the argument the other way:

- **A cursor is a drawing, and a drawing wants a drawing tool.** Nudging a fingertip by moving `#`
  characters in a grid is slow, and it does not let you *see* what you are doing until the app is
  launched. Both attempts were geometrically fine and looked, in the author's word, alien.
- **The licence argument does not survive contact with our own art.** It was an argument against
  bundling *someone else's* cursor. A PNG drawn for cmote is cmote's, under cmote's licence, and
  brings nothing into the tree with it.
- **What is lost is reviewability**, and that is the real cost of the change. The shapes can no
  longer be read in a diff, so the tests now stand in for that: whatever is bundled decodes, is
  square, the same size as its twin, covers the hotspot, and draws something without drawing
  everything — plus that halving one keeps its shape and its colour at the edges. They skip a hand
  that is not there rather than demand one, so "not drawn yet" is a state the build allows.

They are bundled rather than read from beside the executable, like the fonts (§9, §19): §11 promises
one portable binary. Redrawing a hand is therefore *overwrite the file, rebuild* — no code change,
and `cursor_from` takes the size from the image so even a different one needs none.

**An empty file means that hand has not been drawn yet**, and the window says so honestly: nothing
is painted, the subclass is never fitted, and `grab_interaction` flips from "ask iced for nothing"
to asking for `Grab` / `Grabbing` — which winit collapses to the four-arrow **move** cursor. That is
strictly the behaviour the strip had before §51: it cannot tell hovering from holding, but it does
say the thing can be moved. The same fallback covers an unreadable PNG and the moment between the
window opening and the boot task installing the cursors. The files have to EXIST for
`include_bytes!` to compile, which is why an empty one is the placeholder rather than no file.

Decoding is the `png` crate: pure Rust, its inflate backend already in the tree, so §11's
no-C-toolchain build holds. `image` is present but iced pulls it in deliberately **without codecs**
(§41), so it cannot read a PNG, and enabling its `png` feature would only add the same crate behind
another layer. The decoder is asked to normalise to 8-bit RGBA, so a hand exported as a palette, as
greyscale, or with a `tRNS` chunk all arrive the same way, and the alpha is passed through as drawn.

Three details that are not decoration:

- **Drawn at 64, shown at whatever Windows wants.** `SetCursor` does not scale, so handing over the
  64×64 artwork would show a double-size cursor on an ordinary display. `SM_CXCURSOR` is the one
  number to ask: it already accounts for the display's scaling *and* for the user's own cursor-size
  setting in Accessibility. On a 100% display it is 32, which halves the artwork exactly; at 200% it
  is 64 and the drawing is used as drawn, which is the point of drawing it large. It is asked
  **for the window's own DPI** (`GetSystemMetricsForDpi` + `GetDpiForWindow`), because iced runs
  per-monitor-DPI-aware and plain `GetSystemMetrics` answers for the DPI the session logged in at —
  on a 200% panel beside a 100% primary that would make the hands half the size of every other
  cursor on the screen. The resampler averages **weighted by alpha**: a transparent pixel still
  carries a colour, usually black, and averaging it in unweighted rings every soft edge with a dark
  halo. Read once at start-up (`ponytail:` moving the window to a differently-scaled monitor keeps
  the size it booted with, until `WM_DPICHANGED` is handled — the subclass already sees it).
- **Fitted to a FRACTION of that box, not to the box** (`COVERAGE`). `SM_CXCURSOR` is the size of
  the box a cursor is drawn in, not of the drawing in it, and the standard arrow uses barely two
  thirds of its own — a 32×32 arrow bitmap carries a glyph about twenty pixels tall with empty space
  around it. Artwork that fills its box edge to edge therefore reads as visibly bigger than every
  other cursor on screen, which is what the first fitted build looked like on a real desktop. The
  hands are scaled to about the arrow's own footprint instead: 21 pixels where the box is 32. The
  cursor bitmap is simply made that size — nothing requires it to BE `SM_CXCURSOR`, and padding the
  drawing out to one would move the hotspot for nothing. One constant to turn if it still reads
  large.
- **One hotspot for both shapes** (`HOTSPOT`), given in the DRAWING's own pixels and scaled with the
  image. It is the middle of the drawn hand — between the two shapes' centres of area, on the palm of
  each — so press and the hand closes without the pointer appearing to jump; a hand cursor is aimed
  with its middle, since it has no tip to aim with. A test pins it on an opaque pixel of both
  drawings, so a redrawn hand that no longer covers it fails the build rather than quietly clicking
  somewhere the user is not pointing.
- **Straight alpha, not premultiplied.** That is what 32-bit icons and cursors are documented to
  use, and the `ponytail:` note in `decode` says what to change if a soft-edged hand ever comes out
  haloed.

### Painting it takes one Win32 seam, and only one

winit answers `WM_SETCURSOR` itself: whenever the pointer is over the client area it calls
`SetCursor` with whatever icon iced last asked for. So a cursor set from anywhere else is undone on
the next mouse move. The window is **subclassed** (`SetWindowSubclass`) and that one message is
answered first — while a hand is wanted, this module sets it and returns TRUE, so winit's handler
never runs for it. Every other message, and every moment no hand is wanted, is passed straight
through with `DefSubclassProc`; nothing else about the window's behaviour changes.

Two consequences worth stating:

- **A handle asks iced for NO interaction on Windows** (`grab_interaction` answers `None`). This
  looks backwards and is the crux: iced tells winit to change the cursor whenever the requested
  interaction CHANGES, which is precisely at the hover and at the press — the two moments the hand
  is supposed to change. Asking for `Grab` would therefore stomp the hand with `IDC_SIZEALL` at
  exactly the wrong instants. Asking for nothing means winit never touches the cursor over the
  handle and the subclass owns it outright. Off Windows the same function answers `Grab`/`Grabbing`,
  because those platforms have the real thing — which is also why every handle goes through this one
  function rather than naming an `Interaction` itself.
- **A press that never moves still closes the hand, and a pointer that comes to rest gives it back.**
  `WM_SETCURSOR` arrives with pointer MOVEMENT, so between one message and the next there is nothing
  to answer, and both directions matter. Pressing without moving has to close the hand, so the state
  setters call `SetCursor` directly. The mirror image turned up with §52's close buttons: the move
  that lands the pointer on a chip's "×" carries its own `WM_SETCURSOR`, which is answered BEFORE the
  enter event reaches iced, so the hand would otherwise stay until the user moved again. When no hand
  is wanted, `apply` therefore asks the window the same question Windows would (`SendMessageW(hwnd,
  WM_SETCURSOR, …)`): the message re-enters the subclass, which now passes it to winit's handler,
  and the button's pointing finger goes on at once. Both calls are safe from `update`, which runs on
  the thread that owns the window.

Failure is silent by construction. If the cursors cannot be built or the subclass cannot be
installed, the handles keep the cursor they had before and nothing else in cmote depends on it.

### What the module is told, and why the claim is NAMED

`cursor` is told three things and works the rest out itself: a handle took the pointer, a handle
lost it, and something is or is not being dragged. Dragging outranks hovering, so the hand stays
closed wherever the pointer has got to — that is what says the gesture is still live, and it is why
a dialog dragged clean out from under the pointer does not open its hand halfway through the move.

`set_dragging` is a bare bool, so a chip press (§38) and a header press (§10) are indistinguishable
there — a drag is a drag. `App` answers all of it, not `Tab`: there is one pointer and one window,
so a card dragged across a split must not change hands on the way.

The HOVER, though, is a **claim held by one named handle** — a tab's id, or `cursor::HEADER` for a
dialog header. It began as an anonymous count, on the reasoning that the cursor question is only
"is the pointer on something that can be picked up"; §52 corrected that, and the correction is worth
keeping written down because the reasoning was sound and the conclusion was still wrong:

- **A count survives the ordering trap, which was the original point.** Two handles report the same
  mouse move — one left, one entered — and iced dispatches them in the widgets' layout order, not
  in the order the pointer crossed them. Moving right to left along the strip, the chip being
  ENTERED is asked first, so a bare flag would be set and then immediately cleared by the chip being
  left. A named claim survives it just as well and more directly: the exit names the handle being
  left, that handle no longer holds the claim, and so it takes nothing away.
- **A count cannot survive a handle that VANISHES.** iced publishes a widget's `on_exit` from the
  widget itself (`mouse_area`), so a widget that has left the tree publishes nothing at all: press a
  dialog's ✕ while the pointer is on its header, close a chip under the pointer, or send a tab to
  another region (§52), and the exit that would have let go is never raised. The count stayed at one
  and the window went on wearing an open hand over the terminal, the buttons, everything. Leaving a
  whole region of handles (`hover_reset`, wired to the strip's own exit) healed the chip case only,
  and nothing healed the dialog case at all.

So the claim is **re-asserted every frame by the handle drawing itself**: `view` brackets the build
with `frame_begin` / `frame_end`, each handle calls `drawn(id)` as it lays itself out, and a claim
that was not redrawn is dropped. The frame is the only place that knows what still exists, which is
why the hand is the one piece of state in cmote that a view path writes to. A modal's backdrop says
`covered()` on the same pass, because a chip under a scrim is a live widget still reporting the
pointer and still cannot be picked up.

### What is deliberately NOT here

- **The splitters keep their resize arrows** — the tree's and the pane's handles, and the split
  divider (§48). They are named `SplitterGrabbed` and they are grabbed, but what they do is
  RESIZE, and `↔` / `↕` say which axis while a hand would not. Swapping them for the hand would
  trade information for consistency. The web draws the same distinction: `col-resize` for a
  splitter, `grab` for something you carry.
- **Nothing else gets a custom cursor.** The grid keeps its I-beam, the buttons their arrow. This
  section exists because a specific pair of cursors is missing from Windows, not to start dressing
  the app's pointer.
- **The rubber band and the text selection are not grabs.** Pressing empty space in the files pane
  (§21) or dragging across the grid (§40) starts a SELECTION — nothing is picked up and nothing
  moves — so neither wears a hand.
- **No `.cur` or `.ani` files, and no `SetSystemCursor`.** A `.cur` would carry a hotspot and save
  the `CreateIconIndirect` dance, but it is a format no drawing tool exports and no reviewer can
  open; a PNG is both. `SetSystemCursor` changes the cursor for *every application on the machine*
  and is not something a terminal client gets to do.
- **No DPI variants.** One 32×32 pair, which is `SM_CXCURSOR` at every normal scaling; Windows
  scales it like any other cursor. A 48×48 set for 200% displays is a straight addition to the art
  if it is ever wanted.
- **No drag-and-drop invented to have somewhere else to put the hand.** The two surfaces that wear
  it are the two that already dragged. Nothing was made draggable for the sake of the cursor.

---

## 52. Sending a tab to another area of the window (v4.0.0)

§48 cut the window in two and gave each half its own strip. §38 let a chip be dragged along its
strip. Neither let a tab **cross the seam**: the drag is reported by the chips themselves — the
pointer entering one names the slot — so a gesture that starts in the left strip hears nothing from
the right one and the release drops the tab where it was. A tab opened in the wrong half had to be
closed and opened again, which for a live session means dropping it.

**A right press on a chip opens a menu**, and every row in it is one sentence: send *this* tab to
*that* area. Two groups of up to three rows each:

```
Move to main area            greyed — it is already there
Move to right area
Move to bottom area
─────────────────────────────
Duplicate to main area
Duplicate to right area
Duplicate to bottom area
```

The right press does **not** select the chip. Acting on a tab the user is *not* looking at is the
reason the menu is on the chip rather than on the tab already showing, so opening it must not change
what is on screen — the opposite of the strip's left press, which selects because a press *is* a
selection there (§26).

### Areas, not regions

The menu names **places on screen** — main, right, bottom — and never `Pane`, `Region` or "split".
A `pane_grid::Pane` is an opaque index that survives a window being cut and made whole again while
meaning something different each time; what the user can point at is a corner of the window.

With one cut allowed (§48) the vocabulary is closed, which is what makes this honest rather than a
simplification: there is the region at the top left, and there is the one the cut made, which is
either to its right or below it. `ui::split::areas` reads that off the **laid-out rectangles** — the
same `Node` `regions` measures with — so it cannot drift from what is drawn, and "main" keeps
meaning the top-left one even after the *original* region is the one that closed.

### What is offered, and what is greyed

**An undivided window offers all three**, because two of them are one cut away and choosing one is
what makes the cut — the same cut the strip's own split buttons make, monitor measurement and window
growth included. **A split window offers only the two it has**: the third would mean closing a
region and cutting the other way round, which is more than a menu row can promise.

Rows that are there but cannot act are **dimmed, not dropped**. With at most four rows in a group, a
menu whose items move about between openings is harder to use than one with a grey row in it — and
the grey is itself the explanation. Three rules dim a row:

- **Move to the area it is already in.** Nothing to do.
- **Move the only tab of a region into an area that would have to be cut.** The cut and the collapse
  behind it cancel out: the window would grow, the tab would land in the new half, the old half
  would empty, close, and hand the room back. All that for a window that flickered. Re-checked when
  the cut is actually made, not only in the menu, because the monitor is measured asynchronously and
  a tab can close in between — the same reason §48 re-checks `splittable` on arrival.
- **Duplicate anything that is not a session.** A copy is a second connection; a home tab has none
  to make again, and a second editor on one remote file is two dirty buffers racing to save it.

### A move is a lift, not a close and a reopen

`take_tab` is `remove_tab` with the ending taken out: the same strip bookkeeping (the activation
order, which tab comes forward, the window geometry handed to it) with the `Tab` handed back instead
of dropped. A close is now that plus the drop; a move is that plus a push. **The session never
notices** — its channel, emulator, scrollback, panels and forwards travel with the struct, because a
tab has always owned all of it (§26) and nothing about a session was ever indexed by region.

The moved tab arrives **on screen** in its new strip and takes the keyboard with it (§50). The user
has just said where they want this tab; a move that left it hidden behind whatever was showing there
would have to be followed by a hunt through the strip to find it.

**A move that empties its old region closes it** (§48), and the window is whole again — which makes
this the way back from a split without closing anything. Send the last tab across, and the seam goes
with it. That is why the "would empty a region" rule dims only the *cut* case: emptying a region
into one that already exists is not a mistake, it is the merge.

### A duplicate is a fresh connection

A session is a socket and a remote process. Neither can be forked from this end, so **Duplicate
dials again** — it is "open this connection a second time", not "clone this tab".

- **The credential comes from the vault**, by exactly the route the home list's Open takes (§16):
  the stored target is read, the form is filled, and a remembered secret is pulled in — unlocking the
  vault behind the master-passphrase prompt if it is locked. It deliberately does **not** reach into
  the source tab's own form field, though the plaintext is sitting right there while the session is
  up. A duplicate can do no more than the user could do by hand, and a password typed once and never
  stored still has to be typed again — which is the promise "Remember" is the opt-in to (§12).
- **It dials when nothing is left to type**, and stops at the pre-filled form when something is.
  Validation alone is not that test: it accepts an *empty* password on purpose, because some servers
  do (§7). Dialing on an empty field would spend an authentication attempt to arrive back at the same
  form with a failure on it. Every other method needs no field — a key's passphrase, a
  keyboard-interactive challenge and an agent's confirmation are all asked for *during* the connect,
  exactly as they would be from the form's own button.
- **It waits for its own worker first.** A tab is born with no channel to the SSH task: the worker is
  started by the subscription list, which iced rebuilds only *after* the update that created the tab
  has returned, and it checks in with `SshEvent::Ready` a moment later. Dialing in the same breath as
  making the tab therefore failed with "SSH worker is not ready yet". So the dial is **armed**
  (`pending_connect`) and fired by that `Ready`. The pre-filled form is what shows in between — and
  what is left behind, usably, if a worker never arrives at all.
- **It opens where the original is standing.** The source shell's cwd (§17's OSC 7 announcement) is
  carried to the copy and replayed as a `cd` when its shell opens — the same mechanism a reconnect
  resumes with (§22), pin included — and it **outranks** the target's remembered directory: the user
  pointed at a shell, not at a machine.
- **The carry names its own endpoint.** A copy that stopped at the form is a form, and a form can be
  edited; change the host, press Connect, and a path from another filesystem would otherwise be typed
  at a stranger's shell. It is taken by the first `Connected` either way, and used only if that
  session is the one it was made for.
- **The files pane is not carried.** It opens at its own remembered directory and is pinned there
  until the shell settles, exactly as on a reconnect (§22) — the two panels drift apart on purpose,
  and Sync and Reveal (§19) are the deliberate ways to bring them back together.
- **A copy made into its own strip lands beside its original**; sent to the other region there is no
  "beside", so it goes on the end. Either way it opens on screen, since a connection dialing is
  something to watch.

### The menu hangs from the strip, not from the pointer

Every other context menu in cmote opens at the click. This one cannot: a right press publishes no
position (iced's `mouse_area` reports the button, not where it was), and the raw event stream that
would supply one is asked for **only while the window is split** (§48) — it is the single
subscription in cmote that costs a message per pointer move, and switching it on for every window
would make an undivided one pay for a menu it opens once in a session.

So it hangs from the **region's own top-left corner, just under the bar**; the tree's menu is
anchored to its panel for the same reason (§18). It is always on screen, it needs no stored point,
and it follows a divider dragged while it is open, which a remembered pointer position would not. It
is drawn over the **whole window** rather than inside its region: a menu offering to send a tab
across a seam should not be clipped by that seam.


### The hand had to learn what is still on screen

Moving a tab out of a strip is the case §51's hand cursor could not survive, and fixing it fixed two
older ones with it.

iced publishes a widget's `on_exit` **from the widget**, and only when it is still in the tree
(`mouse_area::update` compares the cursor against its own bounds). A handle that DISAPPEARS under
the pointer therefore says nothing: send a tab to another region and its chip is gone, close a tab
under the pointer, or — the oldest of the three — press a dialog's ✕, which destroys the very header
the pointer is resting on. §51 held the hover as an anonymous count, so an exit that never came left
the count at one and the window wore an **open hand over everything**: the terminal, the buttons,
the whole frame. The strip's own exit healed the chip cases if you happened to move off the bar;
nothing healed the dialog case.

The fix is to make the hand a **claim held by a named handle** — a tab's id (app-wide and never
reused, §26) or `cursor::HEADER` — and to have it **re-asserted every frame**:

- `App::view` brackets the whole build with `cursor::frame_begin` / `frame_end`;
- every handle calls `cursor::drawn(id)` as it lays itself out — one line in `chip_view`, one in
  `dialog::header_bar`;
- a claim that was not redrawn is dropped, because a handle that is not on screen cannot be under
  the pointer.

Naming the claimant also replaces the count's original job outright: the exit that arrives out of
order (iced dispatches enter and exit in layout order, not in the order the pointer crossed the two
handles) names the handle being LEFT, which no longer holds the claim, so it takes nothing away.

A handle is also not uniform. A chip carries its own **"×"**, a header its **✕**, and those are
buttons: the pointer is over something to CLICK, and an open hand there offers to drag the control
that closes the tab or dismisses the dialog. The handle's own `mouse_area` cannot see the difference
— it reports the pointer anywhere inside its bounds, children included — so each control says so
itself (`control_entered` / `control_exited`) and wins while it has the pointer. It is a **second
named claim** rather than a flag on the first, because iced updates a child BEFORE its parent:
arriving on a chip directly over its "×" raises the control's enter first, and one shared flag would
let the chip's enter, arriving second, clear a block that is still true. A drag still outranks both —
the pointer crossing a close button mid-gesture must not drop the closed hand.

`ui::dialog::backdrop` says `cursor::covered()` on the same pass, which drops any claim that is not
the header's: a chip behind a modal is a live widget still reporting the pointer, and a hand over
something the click cannot even reach is the same lie in the other direction. That under-claims
rather than over-claims — close the dialog with the pointer resting on a chip and the hand comes
back only once the pointer leaves and returns, because the chip never stopped believing it was
hovered — and a missing hand over something draggable is the smaller of the two lies.

This is the one piece of state in cmote a **view** path writes to. It is justified by there being
nowhere else to put it: the frame is the only thing that knows which handles exist, and iced offers
no hook for a widget leaving the tree.

### Structure

- `ui/split.rs` — `Area` (`Main` / `Right` / `Bottom`), `Area::way` (the cut that would make it,
  `None` for main), and `areas` (which region each one currently is, read off the rectangles).
- `ui/tabs.rs` — `on_right_press` on the chip, and `context_menu`: the rows, their labels, and the
  clamp that keeps the panel inside the window's right edge. It draws `Destination`s and works
  nothing out — availability depends on the region tree, which a strip cannot see.
- `cursor.rs` — the hover became a named claim with `drawn` / `frame_begin` / `frame_end` /
  `covered`; `ui/tabs.rs` and `ui/dialog.rs` each gained the one line that says "still here", and
  `Message::GrabEntered` / `GrabExited` gained the name of the handle raising them.
- `app.rs` — `StripMenu` (which strip, which chip), `destinations` (the rules above), `move_tab_to`,
  `duplicate_tab_to`, `take_tab` (lifted out of `remove_tab`), `seed_form` (lifted out of
  `open_selected_target`, so Duplicate fills the form by the same route the home list does),
  `open_copy_of`, `ready_to_dial`, `Carry`, and `SplitSeed` — which is what lets one split flow serve
  three openings: a home tab, a tab moved in, or a copy made there.

### Deliberately not

- **No dragging a chip across the seam.** The drag is built out of per-chip pointer events with no
  pixel arithmetic anywhere (§38); making it cross a strip means one strip knowing where the other's
  chips laid out, which is exactly the measurement iced does not expose and §38 was designed to avoid.
  The menu says the same thing in one click.
- **No third area.** One cut remains one cut (§48). A menu that could produce a second cut would be
  making a window-layout decision behind a row of text.
- **No keyboard shortcut.** The strip is mouse-only by design (§26), and this is a strip gesture.
- **No "move all tabs" and no "move to a new window".** The first is a bulk edit nobody asked for;
  the second is a second OS window, which is a far larger decision than a menu row (cmote is one
  window, §1).
- **The copy authenticates as the login account.** An elevated shell (§45) lives on the connection it
  was raised on; a fresh connection starts where every connection starts, and the copy can be
  elevated again by the same route the original was.
- **The copy does not inherit the original's forwards, find bar, selection or panel sizes.** Those
  belong to a session or to the target's remembered state (§22, §27), and the copy gets the target's
  the same way any other connect to it would. Only the directory is carried, because only the
  directory is the thing the user was looking at.

---

## 53. A picture opens as a picture (v4.0.0)

§32 gave a file two ways into a tab: a **double-click** in the files pane, and the menu's **Edit…**.
Both went to the text editor, because the text editor was the only viewer there was. Double-click a
`.png` and cmote pulled the whole file off the server, tried to read it as UTF-8, failed, and put up
*"This file is not text in a supported encoding (UTF-8 or UTF-16)."* — true, useless, and a wasted
transfer. The pane already knew that file was a picture: it had drawn it a picture icon (§19).

**A picture now opens in a picture tab.** Same double-click, same menu row (relabelled **Preview**
when the file is one), a new kind of tab: the file's name and dimensions along the top, the image on
a grey ground below it, scroll to zoom and drag to pan.

### The editor's read-only twin

- **A `Tab` now carries a `preview: Option<Preview>`** beside its `editor`, and a `Screen::Preview`
  beside `Screen::Editor`. Everything §32 established about a tab that is not a session holds
  unchanged: no connection and no SSH worker of its own, parented to the session it was opened from,
  its read sent on **that** session's channel and routed back by the viewer tab's id.
- **A sibling of the editor, not a state inside it.** The two share the tab shape and the read that
  fills it, and nothing else: an editor has an encoding to preserve, a dirty flag, changed-line
  marks, a theme, a find bar and a save path, and a preview has none of them, because it cannot
  write. Folding a read-only thing into a read-write one buys a dozen fields that are always empty
  on one of the two — the shape §16's queue was pulled out of `Tab` to escape.
- **Read-only is most of what makes it small.** No Save, so no dirty dot, no unsaved-close prompt,
  no encoding to persist, no account to pin for a later write, and no `parent_gone` flag — that flag
  exists to disable Save. The tab's whole keyboard is Ctrl+W and Escape, and both of them close it.
- **The load is one command for both viewers.** `EditLoad` became **`FileLoad`**, `editor_id` became
  `viewer_id`: what the two want off the network is identical — this file, whole, read as this
  account (§46) — and what they do with the bytes is entirely a GUI-side matter. `EditSave` kept its
  name, because only one of them saves.

### Which tab opens, and which decoder runs, are two different questions

This is the load-bearing distinction, and the two answers come from different places on purpose.

- **The tab is chosen by the EXTENSION**, in one place (`App::open_viewer`), because it has to be
  chosen before a single byte has been read. Both entry points send the same message and arrive at
  that one function, so the rule cannot end up half-applied — the double-click and the menu row can
  never disagree about what a `.png` is.
- **The decoder is chosen by the file's MAGIC BYTES** (`preview::decode`), never by its name. The
  name is the remote's to pick, and letting it steer which parser runs would hand an attacker the
  only decision that matters here. The pleasant side effect is that a mislabelled file simply opens:
  a `.jpg` carrying PNG bytes previews fine, and the toolbar says PNG — the one place a user would
  ever find that out.
- **The picture set is `files`' image table minus SVG.** SVG is a picture by icon and text by
  nature: it is XML, the editor can genuinely edit it, and a preview could only refuse it (drawing
  one is a layout engine, not a decoder). Everything else in that table opens here **even where
  cmote has no decoder** — a TIFF gets *"cmote does not preview TIFF — it previews PNG, JPEG, GIF,
  BMP and WebP"*, which is a better answer than the editor's truthful *"not text in a supported
  encoding"*. The table is `pub(crate)` and derived from, not copied, so the icon and the dispatch
  cannot drift apart as extensions are added.

### The fence around the decoder

`image` was already in the tree — iced pulls it in **without codecs** for §41's sixel compositing.
This takes it as a direct dependency purely to turn five decoders on: **PNG, JPEG, GIF, BMP, WebP**.
Nothing new enters the dependency graph, and the no-C-toolchain portable build (§11) holds.

That narrows §41's refusal rather than reversing it, and the difference is the whole point: §41 is
about bytes a remote **pushed** into the terminal stream unasked, and kitty graphics and OSC 1337 are
still not implemented. This is a file the user pointed at and asked for. It is fenced three ways:

- **The format comes from the leading bytes**, and the reader is pinned to it (`with_format`, not
  `with_guessed_format`), so the payload cannot talk it into a second opinion.
- **`image::Limits` caps the decode** at 8192 per side and 128 MiB of allocation, checked before a
  buffer is reserved — so a header declaring 30000×30000 is refused rather than allocated for. 8192
  is doing two jobs: it bounds the bomb, and it is the smallest maximum texture size still found on
  hardware cmote runs on, so a picture that decodes is a picture that can be drawn.
- **The fetch is capped at `preview::MAX_SIZE`, 32 MiB**, off the server-reported size before a byte
  moves, and again as the bytes arrive in case that size was a lie.
- **Every decoder not listed is one that cannot be reached** — TIFF, AVIF, EXR, DDS, HDR, TGA, PNM,
  farbfeld and QOI are all left off, and each is a parser that is not compiled in.

### The ceiling belongs to the caller now

`edit::MAX_SIZE` is 8 MiB — generous for a config file, mean for a photograph. So **`limit` rides
the `FileLoad` command** rather than sitting as one constant in the network layer, and both readers
take it as a parameter. The `shellfs` path took the same change, which closed a real hole rather
than a theoretical one: it had its own `MAX_READ = edit::MAX_SIZE`, so a preview opened while the
files pane was elevated (§46) — reading through `cat` rather than sftp — would have silently kept
the editor's ceiling, and the same photograph would have opened or been refused depending on which
account the pane happened to be showing.

### Small decisions worth stating

- **One `Screen::Viewer` and one `Tab::viewer`, not two of each.** A viewer tab was modelled as
  `Screen::Editor`/`Screen::Preview` beside `editor: Option<…>`/`preview: Option<…>` — four values
  agreeing about one thing, with "exactly one of these is open" maintained by hand. The kind is now
  `enum Viewer { Editor, Picture }` and the screen says only WHETHER a viewer is open, which is all
  it ever meant: every arm that branched on the screen went straight on to unwrap the matching
  field. What that buys is not brevity. It is that `Tab { screen: Screen::Editor, preview: Some(…) }`
  used to be constructible and nothing rejected it, and the fork between the two kinds was written
  out five times (open, orphan, route an event, label the chip, draw the tab). The enum is still two
  whole structs — an editor has an encoding, a dirty flag, changed-line marks, a theme and a save
  path, and a picture has none of those because it cannot write — so this is NOT the "fold the
  read-only one into the read-write one" that was refused when §53 landed; that would have meant a
  dozen always-empty fields. What the two share (`session`, `path`, "your parent is gone", the chip
  label) is stated once on the enum, and that is what most of the old fork sites were actually
  reaching for.
- **Zoom and pan are iced's**, not cmote's: `image::viewer` already scroll-zooms about the pointer
  and drag-pans, and it keeps the scale in the widget's own state. So the model carries no zoom
  level and nothing has to be reset — there is exactly one picture per tab for the life of the tab,
  which is what makes that free. Limits: out to a third (a big scan fits a small region), in to 10×
  (far enough to read a screenshot's smallest text, which is why anyone zooms one).
- **A picture opens at 1:1 if it fits and contained if it does not, and centred either way**
  (`ContentFit::ScaleDown`). Two rules, and the second one is the one that is easy to get wrong.
  Bigger than the body: shrunk until the whole of it is on screen at once, so there is nothing to
  scroll to before you have seen the picture — never `Cover` (crops) and never `Fill` (a 2:1
  photograph squeezed into a 4:3 body is a photograph of something that does not look like that).
  Smaller than the body: left alone, because upscaling invents detail the file does not contain and
  the request was to see the file. This shipped as `ContentFit::Contain` first, which is the same
  thing for large pictures and silently WRONG for small ones — `Contain` fits upward too, so a 32×32
  favicon opened as a 600-pixel wall of soft squares. Worth stating because the mistake is invisible
  in the case anyone tests by eye: every fit that scales at all handles a photograph identically, and
  it is the icon that tells them apart. Four tests in `ui/preview.rs` ask the constant itself what
  size a given picture comes out in a given body, so the variant cannot drift back.
- **Centring is the widget's**, not a container alignment: `image::viewer` splits the leftover room
  evenly on both sides of the image. Panning is therefore inert at the opening fit — with nothing
  hidden, there is nowhere to drag to — and wakes up only once someone has zoomed past the body.
- **Decoded once, on arrival**, into an iced image handle held on the model — the same trade
  `term::graphics` makes for the sixel images (§41). The alternative is keeping the pixels and
  rebuilding a handle every frame, which re-uploads the texture on every paint. `ponytail:` that
  decode runs on the GUI thread, so a big picture holds the window for the length of it — bounded on
  both ends (32 MiB in, 8192 per side out), so the worst case is a fraction of a second on a file
  the user asked for and is already waiting on. Moving it off-thread is an async task plus a message
  and a route home; worth doing if a real picture is ever felt to stutter, and not before.
- **The toolbar reports what the file IS**: format, pixel dimensions, and the size of the FILE — not
  of the decoded pixels, which would be a bigger number the user has no way to recognise.
- **A preview still loading when its session ends is failed, not left spinning.** The read it is
  waiting on can never arrive, so `orphan_viewers` says so. One that already has its picture is
  untouched: the image is decoded and in memory, and it is as good as it was a moment ago.
- **The menu row is relabelled, not duplicated.** "Edit…" promises a buffer and on a picture that
  promise cannot be kept, so a picture's row reads "Preview". It sends the identical message — the
  decision stays `App`'s alone — and only the wording follows the file, so the menu never says one
  thing and does another.

### Deliberately not

- **No SVG, and no TIFF, HEIC, AVIF or ICO.** SVG is text and belongs to the editor. The rest are
  formats whose decoders are deliberately not compiled in; each is refused **by name**, which is the
  useful half of supporting it.
- **No editing, no rotate, no save-as, no copy-to-clipboard.** A preview answers "what is in this
  file". Everything past that is an image editor, and cmote is an SSH client.
- **No thumbnails in the files pane.** That is a listing that fetches every picture in a directory
  to draw it, which is a different feature with a different cost — the icon says "picture" and the
  double-click shows it.
- **`ponytail:` transparency shows a flat mid-grey, not a checkerboard.** The convention is
  unambiguous and needs a tiled custom widget; one tone that loses to neither light nor dark artwork
  buys most of the clarity for none of the work. Deliberately not the panels' near-black, which
  would swallow exactly the dark artwork transparency is most often used for.
- **No animation.** An animated GIF shows its first frame. Playing one means a frame clock, a decode
  loop and a pause control — a media player, not a preview.
