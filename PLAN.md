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
targets on a home screen — metadata only, no secrets — plus an optional
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
§10, and remembers per target where the shell and both panes were so a reconnect resumes
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
shape, focus reporting, and 10 000 lines of scrollback with a draggable scrollbar, §23/§116; it also
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
a side pane holds it, or choosing an item off the grid's menu, hands it back to the shell (§50);
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

## §1 — Locked decisions

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
| Credentials | Secrets **session-only** — held in memory, `zeroize`d on drop, never written to disk (§12). Connection *targets* (no secret) are saved so the home screen can list targets (§14) |
| Auth order | The chosen method first (`publickey` / `password` / `keyboard-interactive` / `agent`), then chain into `keyboard-interactive` while the server still offers it — 2FA / OTP and challenge-response (§7); driven by what the server accepts |
| File picker | `rfd` — native open-file dialog for the key file (Win32 on Windows, `NSOpenPanel` on macOS) |
| Errors | `anyhow` at the app boundary; typed `thiserror` enums deferred until a real API needs them |
| Config location | `known_hosts` **and** `targets.json` in `./cmote-data/` beside the exe, falling back to `%LOCALAPPDATA%\cmote` (Windows) or `~/Library/Application Support/cmote` (macOS) if that dir is read-only |

---

## §2 — Why these choices (didactic)

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
  connection *targets* (host / port / user / auth kind / key path — no secret) ARE
  saved so the home screen can list targets (§14); persisting the secrets themselves,
  encrypted at rest, stays a deliberate later feature (§16), not a v1 gap.

---

## §3 — Tech stack + versions (mid-2026)

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
| `serde` / `serde_json` | 1.0 | serialize `targets.json` — saved targets + the per-target session snapshot (§14, §22) | `derive` on the target structs; a corrupt store is logged and treated as empty, never a crash |
| `open` | 5 | launch an OSC 8 hyperlink in the OS browser (§24) | pure Rust, no C toolchain; hands the URI to PowerShell `Start-Process` as data (an env var), never a shell command line — the `cmd /C start` inject path is behind an off-by-default `insecure` feature we do not enable. cmote still gates the scheme to http/https/mailto first (`link`) |
| `portable-pty` | 0.9 | a pseudo-terminal on THIS machine, for the local shells the home screen's Local bar opens (§103) | wezterm's crate. ConPTY on Windows, `forkpty` on macOS, one API — and §2 says both, so hand-rolling meant two backends of a thing whose traps are all in the lifecycle. Pure Rust, no C toolchain. Two costs paid knowingly: it brings **`winapi`**, a second Windows-bindings crate beside `windows-sys` (the taskbar note in §54 declared a COM vtable by hand to avoid exactly that), and its `serial2` / `winreg` dependencies are **not optional** in 0.9, so a serial-port pty cmote never opens is dead weight in the binary. It also has a real bug cmote works around: `WinChildKiller::kill` inverts its success test, returning `Err(last_os_error())` when `TerminateProcess` succeeded — so `local::pty::close` drops the result and says why |
| `anyhow` | 1.0 | app-level error handling (`Result<_, anyhow::Error>`) | context-rich errors, `?` everywhere |
| `thiserror` | 1.x | *(deferred)* typed error enums for module boundaries | add when a module becomes a real API |
| `tempfile` | 3 | *(dev-dependency)* temp dirs for tests writing `known_hosts` fixtures (§13) | test-only; not linked into the shipped binary |

Versions above are the ones actually resolved by `cargo add` at scaffold time and
recorded in `Cargo.lock`. We keep **caret (`^`) requirements** in `Cargo.toml` and
rely on the **committed `Cargo.lock`** for reproducible, auditable builds (§12) —
that is the idiomatic reproducibility guarantee for a binary crate, so hard `=`
pins are unnecessary.

---

## §4 — Architecture — the async ↔ GUI bridge (core pattern)

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

## §5 — Repo layout (single crate, many small files)

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
    ├── taskbar.rs        mirror the active tab's command progress onto the Windows taskbar button — `ITaskbarList3` declared by hand, since `windows-sys` ships no COM interfaces (§54)
    ├── explorer.rs       the remote folder tree's model: nodes, expansion, path arithmetic (§18)
    ├── files.rs          the files pane's model: one directory, batched listings, icon categories (§19)
    ├── forward.rs        the pure port-forward spec: kind (L/R/D) + bind/target, parse / validate / label / serialise (§27)
    ├── glob.rs           the home filter's text rule: a fragment until `*` or `?` is typed, then a whole-text glob; case-insensitive (§49)
    ├── integration.rs    the OSC 7 / OSC 133 block a remote's rc file can be given, its markers, and the install / remove edits (§17, §34)
    ├── link.rs           opening an OSC 8 hyperlink safely: the scheme allow-list + the OS browser launch (§24)
    ├── mru.rs            the tabs' activation order (ids, most recent last): a close falls back to the previous visit (§37)
    ├── palette.rs        the terminal colour scheme (default fg/bg + xterm-256), shared by the renderer and the colour-query answerer (§9, §23)
    ├── panes.rs          the tree and the file pane as one pair, and only what spans them: reveal/follow, re-read, what a deletion means, the remembered layout, the shared `.*` toggle — returning the listings to ask for rather than sending them (§18, §19, §22)
    ├── paths.rs          data-dir resolution: `cmote-data/` beside the exe if writable, else `%LOCALAPPDATA%\cmote` / `~/Library/Application Support/cmote` (§11)
    ├── preview.rs        the picture tab's model: which files open as a picture, and the fenced decode — sniff by magic bytes, cap the dimensions and the allocation, name the format in every refusal (§53)
    ├── targets.rs        load/save `targets.json`: saved connection targets + the per-target session snapshot; corrupt file → treated as empty (§14, §22)
    ├── secret.rs         the session-secret wrapper (`Secret` over `zeroize`): passwords / passphrases held in memory, wiped on drop, never logged (§12)
    ├── ui/
    │   ├── mod.rs         view helpers, incl. the shared `elide_middle` path/name cut (§22); host-key / passphrase / error dialogs (§8, §7, §6)
    │   ├── connect.rs     the connection form (host/port/user/auth/key)
    │   ├── dialog.rs      shared modal-dialog chrome: header (title + ✕, the drag handle and its hand cursor) / body / footer, and `Card` — where a floating dialog sits and how a header drag moves it, once for every dialog in the app (§10, §26, §51)
    │   ├── explorer.rs    the folder-tree pane, its splitter and its context menu (§18)
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
    │   ├── osc.rs         frame OSC strings out of the stream — one chunk-safe machine, shared by every scanner below that reads one (§17, §34, §54, §55)
    │   ├── iterm.rs       read the parts of iTerm2's OSC 1337 namespace cmote honours — an ALLOW-LIST, so a key nobody vetted does nothing (§55)
    │   ├── cwd.rs         scan OSC 7 / OSC 9;9 out of the output stream: the remote cwd (§17)
    │   ├── progress.rs    scan OSC 9;4 out of the stream: how far along the remote command says it is (§54)
    │   ├── protect.rs     scan DECSCA / DECSED / DECSEL out of the stream: the cells a program asked us not to wipe (§56)
    │   ├── cancel.rs      find the sequence the engine would read as something ELSE — DECSLRM's `s`, which its save-cursor arm takes — so `process` can cancel it in flight (§57)
    │   ├── rect.rs        scan the VT420 rectangular ops out of the stream — DECERA / DECSERA / DECFRA / DECCRA (§58), DECCARA / DECRARA / DECSACE (§59), DECRQCRA (§60) — plus the corner arithmetic they all resolve through and the checksum one of them reports
    │   ├── graphics.rs    scan the sixel images out of the stream and anchor each to a document line, capped and evicted oldest-first (§41)
    │   ├── sixel.rs       decode a sixel payload into RGBA pixels — in-house, no image-format dependency (§41)
    │   ├── keymap.rs      GUI key events → the bytes a terminal sends; legacy or kitty per the active mode (§9, §25)
    │   ├── kitty.rs       encode a key event in the kitty keyboard protocol's CSI u form (§25)
    │   ├── mouse.rs       pointer events → the xterm mouse reports a program that asked for them expects (§9)
    │   ├── modkeys.rs     scan `CSI > 4 ; p m` out of the stream: the remote's modifyOtherKeys level (§9) — and answer `CSI ? 4 m` with it (§61)
    │   ├── osc133.rs      scan the OSC 133 shell-integration marks out of the stream: prompt lines, command state, output ranges (§34)
    │   ├── query.rs       answer the identity queries the engine drops — XTVERSION, DECRQSS, XTGETTCAP, DA3, XTSMGRAPHICS — and amend its DA1 to advertise sixel (§33, §36, §41)
    │   ├── screen.rs      the engine-agnostic Screen/Cell/Color view the app reads through — incl. a cell's OSC 8 link, the kitty flags, the viewport↔document line mapping and whether a line wraps into the next (§9, §16, §23, §24, §25, §40, §42)
    │   └── search.rs      find text anywhere in the scrollback: a row flattened for searching, the match list, which is current, which are on screen (§35, §39)
    ├── transfer.rs       the ONE transfer slot and everything queued behind it: the batch being set up, the file / folder / download queues, the collision questions, resume — including the one a dropped session hands to the next — and an OS drop settling into all of it (§16, §17, §19, §21, §29)
    └── bridge.rs          SshCommand / SshEvent enums + channel wiring (§4)
```

---

## §6 — Connection + authentication flow

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

## §7 — Key handling (PEM / OpenSSH / PPK)

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

## §8 — Host-key verification (security)

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

## §9 — Terminal emulator

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
  leaves a scrolled-back view stationary. A thin **scrollbar** rides the grid's right padding gutter
  whenever there is history; its thumb reports position and history depth (`screen::history_size`),
  sized as the viewport's share of the whole document with a floor so a deep history still shows a
  mark. Since §116 it is also GRABBABLE — press and drag it to move the view, press the bare track to
  jump there — which is what turned it from an indicator into a control and put a thumb at the live
  bottom, where §23 drew none. Selecting across the
  scrolled view already works (extract reads the same offset the grid draws). §23 is complete.
- **Security note**: rendering untrusted server bytes is safe here — the engine
  *interprets* escapes into grid state; it never executes anything. We deliberately do
  **not** honor dangerous sequences (e.g. clipboard-write OSC 52) in v1.

---

## §10 — UI (iced)

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
  place: a dark rounded pane of a fixed width (set by the longest item any of them
  carries), full-width items that highlight on hover, a **dimmed** label for a disabled
  item (a transparent button gives no other signal — the folder tree's "Copy relative
  path" is disabled without a cwd), and one `dismiss_layer` taking the caller's cancel
  message. Positioning stays per-screen, because the three anchor differently: the
  pointer, a row index, the pane's right edge.
  - The home screen's menu is the one place this **deliberately overrides** that screen's
    "take every colour from the theme" rule (§14). The rule exists to stop a surface that
    themes its background but not its foreground; this pane sets *both*, so it stays
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
  a dozen surfaces write to the clipboard — both pane headers, the details card, four
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
    panes without swallowing a press aimed at what it covers, and it needs no dismiss button
    to get out of the way.
- **Terminal** (`Screen::Terminal`, done): a fixed-height status bar in three
  equal-width zones — **Copy / Paste** on the left, the live session's `user@host:port`
  centered, and on the right the pane toggles, a **Tunnels** button (§27, its label
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

## §11 — Portability / config / build

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

## §12 — Security

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

## §13 — Testing (AAA pattern, 80% target on logic)

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

## §14 — Saved connection targets (v1.3)

The home screen (`ui/home.rs`) is the landing screen: a list of previously used
connection **targets**, so reconnecting is a click instead of re-typing the form.

- **What persists — metadata only, never secrets in this file (§12).** A target records
  `name`, `host`, `port`, `user`, `auth_kind`, (for key auth) `key_path` and — when the target
  presents one — the OpenSSH `cert_path` (§7), the panes' `show_hidden` preference, and a
  `remember_secret` flag. A certificate is public data like the key *path*, so it rides here;
  no password and no key passphrase is ever written to `targets.json`. This keeps the §12 "the safest secret is the one never
  persisted" guarantee for this file **and** keeps it fully portable — a `targets.json` copied
  to another machine leaks nothing. The user enters the secret on the form each time, unless it
  was remembered. *(Opt-in, PORTABLE encrypted-at-rest secret persistence now exists — a
  master-passphrase `age` vault, `secrets.age`, separate from this file; see §16. The
  `remember_secret` flag here is only the hint that such a secret can be pre-filled.)*
- **Store** (`targets.rs`): `targets.json` in the shared data directory
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

## §15 — Coding conventions — DECIDED: idiomatic Rust

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

## §16 — Deferred (with upgrade paths)

- **Credential persistence (secrets at rest)** — *done (v3.0), as a PORTABLE opt-in.* Saved
  targets carried metadata only (§14); a password / key passphrase is now optionally kept too,
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
  session state machine — its own screen (home / connect / a live shell), terminal, panes and
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
  history, typing snaps back to the live bottom, and a thin **scrollbar** in the grid's right gutter
  shows position and depth — read-only until §116 made it draggable. That was the last §23
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

## §17 — Remote working directory + file upload (v1.4)

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
`files_path`, the pane sizes and the sort for every saved target, and `terminal_path` for exactly
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
  - It reaches for nothing — no SSH channel, no dialog buffer, no panes. Each call returns
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
  `transfer::mark_refused`, an error wrapper the reporting end reads back with `transfer::was_refused`.
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

## §18 — Remote folder explorer (v2.0)

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
  entries; the pane's `.*` checkbox only decides whether the rows are drawn, so
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

### The pane (`ui/explorer.rs`)

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
  below wears the twin button and its own F5 (§19), so each pane refreshes the one that has
  focus. A closed or already-loading branch is skipped — nothing changes under rows you cannot
  see, and a fetch in flight will bring the fresh listing itself. Beside ↻ sits a **collapse-all
  button** (`unfold_less`, `Explorer::collapse_all`): it closes every branch back to the root's own
  children — the clean top-level view after a deep dive — while leaving the cached listings in
  place, so a re-opened branch draws instantly (though opening re-lists it in the background, as
  above). The root itself stays open; closing it would shrink the pane to a single `/` row.
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
  subsystem. On success both panes re-list the affected parent, and a files pane sitting inside a
  deleted folder steps up to the nearest surviving one.
- **Recursive transfers (v3.0).** *Upload folder…* sends a picked local folder tree-and-all into
  the menu's folder; the files pane's own *Download folder…* is the mirror. See §17 for the
  merge-and-per-file-collision protocol they share.
- **Failures are a notice line**, under the tree, not the error screen: a directory the
  user may not read must not tear down a working shell (the same call as an upload
  failure, §17). The path is the user's own, so naming it is what makes it actionable.
- **Fixed colours, like the grid.** Every surface in the pane sets background *and*
  foreground together, so contrast does not depend on the system light/dark preference —
  the trap §14 documents.
- **The menu opens under the cursor.** A right-press carries no coordinates, so the pane
  is wrapped in a `mouse_area` that tracks the pointer — the same trick the terminal grid
  uses (§10). A child `mouse_area` only captures *presses*, so the rows' own click
  handlers do not swallow the moves. The anchor is **frozen into the open menu** rather
  than read live: the pane keeps reporting moves while the menu is up (the dismiss layer
  above it handles no moves, so they fall straight through), and a menu that tracked them
  would slide out from under the cursor before an item could be reached. The menu is laid
  out right-aligned with a
  padding of `pane width − pointer.x − menu width`: since the pane's right edge is the
  window's right edge, that puts the menu's left edge under the cursor, and clamping the
  padding at a minimum slides a menu opened near the edge back inside the window instead
  of letting it hang off. Placing from the pointer rather than from a row index (what the
  home screen does, §14) is also what makes it correct on a scrolled tree — iced does not
  expose the scrollable's offset, but the pointer needs no such correction.

---

## §19 — Remote files pane (v2.1)

An **icon grid of every entry in one directory**, in the browser strip under the terminal.
The strip runs the window's full width; the pane fills it, save for the folder tree's column
on the right when that is shown (§18). The tree answers "where am I in the filesystem"; this
answers "what is actually in here". Same three-way split — a pure model
(`files.rs`), a pure view (`ui/files.rs`), and the network calls (`ssh/browse.rs`,
`ssh/download.rs`) — so the rules that matter are unit-testable with no server.

### The pair has an owner (`panes.rs`)

The tree and the pane are two models, but a good deal is true of **both at once**, and that half
had no home: it lived in `app`, in eighteen methods that reached into the two models and sequenced
them by hand — one of which said so in its own comment, *"Done here rather than in a model because
it spans both panes."* `panes::Panes` holds the pair and owns exactly the operations that are about
the pair: revealing a directory (tree opened down to it AND pane pointed at it — one without the
other is a bug, not a halfway state), following the shell, re-reading for another account (§46), the
remembered layout (§22), and what a deletion means.

Deletion is the one that shows why order is a rule and not a detail: the pane must step out of a
folder that is gone **before** anything re-lists, or the first refresh asks the server to list a
directory that has just been removed. That sequence is now one method with a test, instead of a
comment.

Two things it deliberately does **not** do. It does not forward the panes' own methods — both
models stay public, and a caller that wants to scroll the tree scrolls the tree; re-typing a hundred
single-pane methods would make it wide and shallow and would earn nothing. And it does not touch a
channel: operations that need the network return `Fetches` — the listings to ask for — and the
caller turns those into commands. That is `transfer::Queue`'s shape (§16), for the same reason, and
it is what lets every rule above be answered in a test with no window and no server.

The `.*` toggle is the clearest case of a coupling that had nowhere to live. It is **one setting for
both panes**, and it is held by the tree — so `files::rows` cannot answer "what should I show"
without it, and nine call sites used to fetch the flag off the other model and hand it over.
`panes.rows()` states that once. There are now zero expressions in `app` that touch both models.

The layout is two rows under the status bar: `terminal | tree` on top, the files pane
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
  session snapshot as the `.*` filter and the pane sizes, so the grid reopens in the order a
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
  **Sync** button, either pane's **"Open in terminal"**, the tree's Enter, or the replay a
  reconnect does (§22). Every one of those is a deliberate act, never a side effect of
  browsing.
- The catch the two sources create: the shell re-announces its directory at *every prompt*,
  so a naive "follow the cwd" would drag the pane back from a browse on the next keystroke.
  `Files::follow` therefore acts only when the announced directory differs from the last one
  followed — a repeat is not a move — while `Files::show` (a browse) is unconditional. Last
  one wins: browse and the pane moves; move the console and the pane follows the `cd`.
- **The tree carries the same guard, and the two must agree** (`Explorer::reveal_if_new`
  against its own `revealed`, `Files::follow` against `followed`). Two panes, one question —
  "has the shell actually moved?" — answered in two places, which is two chances to disagree.
  They did, on the reconnect path: see §22's pin, which held the pane and not the tree.

### No `remote::Location` module — and why (v4.0.0)

An architecture review proposed lifting the shell/tree/pane coordination — `on_sync`,
`on_reveal`, `browse_to`, `refresh_remote_dir`, the resume pin and the shell-follow — into one
`remote::Location` owning "where the panes point relative to the shell". It was explored and
rejected. Recorded so a later review does not re-suggest it.

- **It would have to own `Explorer` and `Files`, which are used everywhere for reasons that
  have nothing to do with location.** Scroll offset, pane width and reserved space, visibility,
  hidden-file mode, the context menus, the inline rename, the selection and the rubber band —
  around a hundred call sites in `app.rs`, against the eight that are about *where the panes
  point*. Owning them means an `explorer()` / `explorer_mut()` pair carrying ninety per cent of
  the traffic straight through: a module whose interface is as wide as what it hides, which is
  the definition of shallow.
- **Not owning them is worse.** The alternative is free functions taking `&mut Explorer`,
  `&mut Files` and a path — three or four arguments to move two lines of state, with the
  invariants still living in the caller.
- **The peers are already the deep modules.** `explorer.rs` and `files.rs` each own a pane and
  hand back listing requests; `transfer.rs` works because the state it owns is *only* used by
  transfers. A third layer mediating two widely-used peers is not a deepening, it is a wrapper.
- **What is genuinely shared is one field**, `App::resume_cwd`, and merging the two panes'
  follow-guards behind it would move state *out* of the pane modules and *into* `app.rs` — the
  wrong direction, since `app.rs` is the file the review flagged for being 11k lines.

The exploration was not wasted: it found the pin covering only half of what it was for, and
Reveal stranding the panes when pressed against it. Both are fixed below.
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
    than as a move, and a real `cd` after it still carries both panes.
  - **It ends a reconnect resume still settling** (`resume_cwd = None`, §22), the rule
    `move_shell_to` already follows and for the same reason: the pin holds the panes against the
    shell's login announcements, and the user saying out loud where the panes go outranks that.
    Left armed, the pin swallowed the settle as "already there" and stranded the panes at the
    login directory with no further announcement coming to put it right — the exact drift this
    button exists to close, caused by pressing it. Nothing is spent when there is no announced cwd,
    since there is then no ask to outrank.
  - **Disabled when there is nothing to do:** no announced cwd (§17 — it takes OSC 7), the strip
    hidden (the tree goes with the pane, so a press would change nothing anyone can see), or both
    panes already there. "Both" is three terms rather than Sync's one: the pane can be on the cwd
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
- Colours are per category and fixed, like everything else in these panes (§18).

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
  rather than replaced. Both panes react to `RenameDone`.
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
  the left edge is **clamped against the pane's width** (v2.2) — the pane is a fixed
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
  reachable the same way in both panes — menu, header button, F5 — and each key/button acts on
  the pane that has focus.
- **The `.*` toggle is the tree's.** One flag (`Explorer::show_hidden`) filters both
  panes, and each header carries a checkbox that shows and flips it — so hiding dot-files
  hides them everywhere, and the pane still has the control when the tree is collapsed.
  Toggled on, it hides *nothing*: every name the server reported is shown, dot-prefixed or
  not, whatever attribute the far side considers hidden or system. The two exceptions are
  `.` and `..`, dropped at ingest (`explorer::is_dot_link`) because they are this folder
  and the one above it rather than entries in it — a tree row for `..` would walk back up
  itself. SFTP omits them and `ls -A` leaves them out; the guard makes it true regardless.

---

## §20 — Keyboard focus and entry details (v2.1)

Two panes now sit beside the shell, and both want the arrow keys. This section is the
answer to "who gets the keystroke", plus what the files pane shows about the entry the
keyboard just landed on.

### One focus for the window

- **Three stops: shell, tree, files pane** (`app::Focus`). A session opens with the
  **shell** focused — that is what a terminal is for — and `clear_grid_interaction` puts it
  back there whenever a session starts or ends.
- **A click focuses what was clicked.** Each pane's own `mouse_area` reports a
  `PanePressed`, so an empty patch of pane focuses it just as a row or a cell does, and a
  press on the grid hands the keyboard back to the shell. In the files pane that press also
  **clears the selection** — a cell's own `mouse_area` swallows the press that lands on it,
  so one that reaches the pane missed every cell, which is the click-away every file
  manager deselects on.
- **Ctrl+Tab cycles**, Ctrl+Shift+Tab the other way, skipping panes that are hidden — a
  stop you cannot see is a dead press. It is read *before* anything else on the terminal
  screen, because it is the way out of a pane that is swallowing keys.
- **A focused pane keeps every key it could mean**, not just the ones it uses. A pane that
  swallowed only the arrows would leave Tab completing paths at a prompt the user is not
  looking at. **Esc** hands the keyboard back to the shell from either pane — and so does
  **plain typing**, which no pane answers to and the shell always does (§50).
- The focused pane wears a one-pixel ring (`ui::explorer::focus_border`, shared by both),
  which is the only thing that tells the two panes apart at a glance.

### Walking the panes

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
  aimed at. Both panes report their scroll offset (`Scrolled`) and share one rule,
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

## §21 — Selecting many entries at once (v2.1)

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

## §22 — Resuming where you left off (v2.2)

A reconnect to a saved target used to drop you at the shell's login directory with both
panes at the root. This section remembers where the last session was — the shell and the
files pane, each on its own — and puts you back there, and gives the folder tree a path
header so both panes name the same place.

### One snapshot, remembered per target

- **`SessionState` is the one place that names what persists per target** (§14): the two
  paths (`terminal_path`, `files_path`), the `.*` filter, and the two pane sizes
  (`explorer_width`, `files_height`). It is a transfer struct; `Target` keeps the fields flat
  (so the JSON stays flat and a pre-v2.2 `targets.json` loads unchanged), all optional and
  omitted when absent. Target metadata, never a secret — §12 is untouched. Adding another
  remembered value is one field on `SessionState` and `Target`, one line each in capture /
  restore / `set_session`.
- **The pane sizes stay per target; the WINDOW size does not.** The tree width and pane
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
  (the OSC scanner, §17), but the pane path and pane sizes are GUI state the app owns — they
  never appear in the byte stream, so the scanner stays a scanner and the snapshot lives with
  the target.
- **So the shell resume only exists on a shell that announces.** Everything else here works on
  any remote — the pane path, the pane sizes, the `.*` filter and the sort are the GUI's own
  state — but `terminal_path` can only ever be what the shell said. On a plain bash it is
  therefore always `None`, forever, and the rule above quietly keeps whatever was there. That
  reads as a broken feature rather than a missing shell hook, which is what §17's shell-integration
  dialog exists to close: install it once per server and the resume starts working from the next
  connection.

### Putting you back

- **`App::restore_session` applies the snapshot before the first listing**: the `.*` filter
  and the two pane sizes go straight onto the panes (a size clamped to the same window
  fraction a splitter drag is, and only once the window size is known), and the two resume
  paths come back for the caller to drive the rest.
- **The pane reopens at `files_path`** (root as the fallback) and the tree reveals the
  chain down to it, so both panes start on the resume point.
- **The shell is resumed with a `cd`** typed in exactly as the tree's "Open in terminal"
  does (§18) — quoted, POSIX-assumed, visible in the scrollback. Nothing to replay leaves
  the shell at its login directory, the previous behaviour.
- **Both panes are pinned while the shell settles.** The shell announces its login directory
  *before* the replayed `cd` runs, so without a guard that announcement would drag them off a
  divergent `files_path`. `App::resume_cwd` holds the cwd we are waiting for: until the shell
  reaches it, `SshEvent::Output` moves neither pane; once it does, both follow-guards are seeded
  (so they stay put now but follow the next real `cd`) and the pin lifts. An explicit move by the
  user — Sync, "Open in terminal", Reveal — lifts it early.
- **The tree used to sit outside the pin, and that was the bug.** It followed every announcement
  while the pane was held, so a resume walked it to the login directory and then on to the
  replayed one, opening each chain in turn and asking the server for a listing of every folder
  along both — to land somewhere the pane had deliberately not gone. The two panes are meant to
  open a session agreeing on the resume point, and one of them was leaving before the user saw it
  there. `Explorer::set_revealed` is the tree's half of the seed, the exact mirror of
  `Files::set_followed`, and the reveal now happens *inside* the not-pinned arm rather than in
  front of the whole match.

### The folder tree shows the path too

- **The tree pane's header now shows the current directory**, the same `Files::path` the
  files pane shows — the two views are synchronised, even though the tree's selection can
  sit elsewhere.
- **It wraps across up to two lines**, because the pane is narrow and a deep path would
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

## §23 — Terminal engine swap: vt100 → alacritty_terminal (v3.0)

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
  a side pane reads as a focus-out too — the remote is blind to cmote's panes, so it should hear
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
  **Two of those sentences are no longer true: see §116**, which made the bar draggable and therefore
  had to draw it at the live bottom as well. The auto-hide was right for an indicator and wrong for a
  control.
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

## §24 — OSC 8 hyperlinks (v3.0.0)

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

## §25 — Kitty keyboard protocol (v3.0.0)

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

## §26 — Multiple sessions in tabs (v3.0.0)

The window held one session. Now it holds a **strip of tabs**, each a **fully independent**
session: a tab can sit on the home list while another runs a shell, dial a second connection while
the first stays live, and each keeps its own terminal, folder tree, files pane, selection and
dialogs. Two open connections no longer mean two windows.

### The split: `Tab` and `App`

The whole single-session state — everything `App` used to be — moved wholesale onto a **`Tab`**
struct, keeping its `update` / `view` / `title` and every helper unchanged. A new, thin **`App`**
owns a `Vec<Tab>`, the active index, and the two things that must be **shared**, not duplicated:

- the saved-target list (`targets::Targets`) — one file on disk; a rename or delete in one tab's
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

## §27 — Port forwarding (v3.0.0)

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

## §28 — Host-key mismatch override (v3.0.0)

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

## §29 — Drag-and-drop upload (v3.0.0)

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

## §30 — Confirmed, clean quit (v3.0.0)

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

That pairing rests on the shell answering EOF, which every remote cmote can reach does and three of the
four **local** shells do not — see §104.

### What is deliberately NOT here

- **No quit on a non-last tab close.** Closing a tab when others remain is unchanged (§26): an idle tab
  goes at once, a live one asks the per-tab Disconnect confirmation. Only the *last* close is a quit.
- **No forced kill.** Quit waits for a clean disconnect (bounded by the timeout); it never SIGKILLs a
  session to leave faster.
- `ponytail:` the drain waits on the **live (Terminal-screen) tabs only**. A tab still handshaking has
  no shell to disconnect and its worker unwinds when its link drops; the timeout covers any straggler.

## §31 — App-wide window size, and pane-handle feedback (v4.0.0)

Two small layout niceties. cmote already remembered the pane sizes per target (§22); it did
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
  (`splitter_active` = `dragging || splitter_hovered`, on both pane models). Resting it is the
  pane grey `SPLITTER_BG`; active it is the brighter `SPLITTER_HOVER`, shared by both handles so
  they feel identical. Hover is fed back by the bar's own `mouse_area` `on_enter`/`on_exit`
  (`SplitterEntered`/`SplitterExited`), which touch only the highlight — no relayout, so no grid
  refit. This is the hand-rolled equivalent of what a `pane_grid` splitter gives for free; cmote's
  splitters are custom `mouse_area` bars (they drive the pty reflow), so the feedback is explicit.

---

## §32 — Remote text editor in a tab (v4.0.0)

A **basic text editor** for a remote file, opened in **its own tab** in the strip (§26). Until now
a tab was always a session (a home list or a live shell); now a tab can also be an editor, so the
one strip manages both. The editor opens a file over SFTP, shows it with **line numbers** and a
**changed-line** gutter, and can **save**, **save as** a new remote file, and **close** — asking
about unsaved changes first. It is deliberately small: no syntax highlighting, no find/replace, no
split panes. It is for the "just fix this line in the config" job that otherwise means launching
`vi` in the shell.

Same three-way split as the panes (§18, §19): a pure model (`editor.rs`), a pure view
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
- **Edit events are correlated by a `viewer_id`, not the session id.** The worker tags every
  event with the *session* tab's id (§26), but a loaded/saved file belongs to the *viewer* tab that
  asked. So `FileLoad` / `EditSave` carry that tab's id and the matching
  `FileLoaded` / `EditSaved` / `*Failed` echo it back; `App` routes those to the tab whose id
  equals the `viewer_id`, whichever session produced them. Two editors loading at once cannot cross
  their bytes.

  The field is `viewer_id` rather than `editor_id` because §53 gave the picture tab the SAME load
  path: one `FileLoad` serves both, carrying the asking tab's own size ceiling (`edit::MAX_SIZE`
  versus `preview::MAX_SIZE`), and the bytes come back undecoded because what they decode into is
  the viewer's business.

### Opening a file

- **Two ways in, both from the files pane (§19).** A new **Edit…** item on the entry context menu
  (files only, disabled on a directory or a multiple selection), and a **double-click on a file** —
  which until now did nothing (double-click only browsed *into* a directory). Both emit
  `FilesMessage::OpenStarted(path)`, which the tab turns into an App-level `ViewerOpen` carrying the
  parent session id and the path; `App` creates the viewer tab **right beside the session it came
  from** (§38), makes it active, and sends `FileLoad` on the parent's channel. Both names lost their
  `Edit` prefix in §53, when the picture tab started using the same route.
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
  size (`on_scroll`, first frame included), and after any cursor move `App` runs the panes' own
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

## §33 — Answering the identity queries the engine drops (v4.0.0)

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

## §34 — Shell-integration prompt marks — OSC 133 (v4.0.0)

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

## §35 — Finding text in the scrollback (v4.0.0)

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

## §36 — The last input and query gaps — DA3, and DECKPAM where it is safe (v4.0.0)

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

## §37 — Closing a tab returns you to where you were (v4.0.0)

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

## §38 — The strip's order is the user's — files beside their session, and drag to rearrange (v4.0.0)

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

## §39 — Every match on screen, washed — the find bar shows where else the query is (v4.0.0)

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

## §40 — The selection speaks document lines — text that scrolls stays selected (v4.0.0)

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
`Screen::cell` and `ui::selection::ScreenSpot::to_doc` read through.

- **`Screen::line_cell(line, col)` is the read that does not care where the viewport is.** The engine
  keeps scrolled-off lines on the *negative* grid lines below the live screen's line 0 (§23), so a
  document line maps onto the grid by subtracting `history_size`; anything outside `-history_size ..=
  screen_lines - 1` is a line the session no longer has. `Screen::cell` — the renderer's per-cell read
  — is now `line_cell(line_at(row), col)`, so the viewport and document readers cannot drift apart.
- **`ScreenSpot` and `DocSpot` are two types, and that is the point.** `ScreenSpot` is where the pointer is (row 0 is
  the top visible line); `DocSpot` is where the text is. `ScreenSpot::to_doc(screen)` is the only crossing, so a
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

## §41 — Inline images — sixel pictures in the scrollback (v4.0.0)

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
list (`interruptions`) — the engine can only be advanced forwards, and applying all the marks and then all the
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
  **§70 then made the compatibility matrix say the same thing**, which it had not: its `iTerm 1337 File`
  row went on billing for the decoder long after §53 paid for it, so it read ❌ where the truth was 🛑 —
  a key `term/iterm.rs` declines by allow-list and by payload cap, with a test on both. Kitty graphics
  stays ❌ there, and §70 corrects why: `f=24` / `f=32` are raw RGB/RGBA and need no decoder at all, so
  its cost is the protocol.
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

## §42 — Select by word and by line — the double and triple click (v4.0.0)

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

## §43 — A resize invalidates what was anchored to the grid (v4.0.0)

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

## §44 — The find bar keeps up with live output (v4.0.0)

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

> **STATUS: the UX was withdrawn and §47 replaced it.** Read what follows as the record of the
> withdrawal, which is still worth having — the machinery it describes is what §47 was able to build
> on, and the reasons the first UX was pulled are the reasons the second one is shaped as it is. The
> way in is now ONE control (the status bar's Account button) and one dialog, not the four the
> paragraphs below list; see §47.
>
> **The withdrawal, as it stood.** There was no way to START an elevation from the
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

### The conversation is a value, not a match arm (`elevate::Handshake`)

The state machine that reads sudo's replies — what has been said, what has been asked, how many
DISTINCT things were asked for, and which single answer may be kept — lives in `elevate`, as a plain
struct with `on_bytes(&[u8]) -> Step`. `ssh::shell` holds the channel and does nothing with the bytes
but hand them over and act on the `Step` that comes back.

It was written the other way first: seventy-odd lines inside a match arm in `ssh::shell`, operating on
a `&mut String` and three counters and touching no socket at all — yet unreachable by any test,
because `Shells::new` takes a `russh::Channel` and so needs a real server to exist. The predicates it
calls (`prompt`, `refusal`, `looks_like_shell`, `reason`) were all tested; the thing that SEQUENCES
them was not, and sequencing is where the rule lives.

The rule in question is the one worth the move. **A one-time code asked for under cmote's own `-p`
marker must never be kept as the connection's sudo password.** sudo substitutes its `-p` text for
every standard prompt in its PAM stack, so on a two-factor machine the marker appears twice — and
answering "that was the password" the second time handed the code to the file layer, where it could
only ever be refused. Being cmote's own prompt is necessary but not sufficient; `factors == 1` is what
tells the two apart, and `factors` is not `asked`, because a question re-put after a refusal is the
same factor over again (which is also why a *corrected* password is still cacheable). Thirty lines of
comment explained all that and nothing checked it. There are now seven tests that play whole
conversations in memory: the ordinary one, the corrected password, the second factor under the marker,
a prompt split across chunks, the question bound, answering when nothing was asked, and the refusal
notice.

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
and dropped when the session ends — never to the vault, never to a target: a sudo password is usually
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
- ~~**No way in at all, for now.**~~ The right-click item and the status bar's button were the two and
  both were withdrawn. **§47 put one back** — the status bar's Account button, and one dialog rather
  than the four controls this section had — and with it the per-target "elevate on connect" this entry
  was waiting for.
- **An identity is not a tab.** It shares the connection, the tab strip stays one chip per session
  (§26), and the MRU (§37) knows nothing about accounts. Closing an elevated shell is `exit` at its own
  prompt, or its ✕ in the accounts dialog (§47).

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

#### The shell backend is written against a trait, and where that stops

`asuser::Exec` is two methods — `stdout` and `succeeds` — implemented by `Runner`. Every function in
`shellfs` whose whole content is "compose a command, read the reply" takes `&impl Exec` rather than
`&Runner`: the listings, the metadata reads, the mutations, the free-name probe.

The reason is testability, and it is not a small one. A `Runner` that will answer anything needs a
live session, so **none of that code could be reached by a test at all** — including the quoting,
which is a security boundary, and the `ls` parsing, which is a compatibility one. Those two want
testing *together*, because the pair of them is the entire backend: a `Script` double records what
it was asked to run and answers out of a canned reply, so one test asserts both the command that
went out and the parse of what came back. A folder called `'; rm -rf ~` is now checked as a composed
command rather than by reading `shell_quote` in isolation and trusting the call sites.

**`Exec` deliberately has no `stream`.** That returns a `russh::Channel` — a foreign type nothing
but the real runner can produce — so putting it on the trait would make the trait implementable only
by the thing it exists to stand in for. The four functions that move bytes (`read_all`, `write_all`,
`fetch`, `send`) therefore still take a concrete `&Runner`, which is the same line `shellfs`'s own
`ponytail:` note draws: **the copy loops stay two, not one generic pair.** Making *those* generic
would mean rewriting working transfer, resume and conflict code (§16, §17, §19) with no way to test
it against a real server, and that refusal is unchanged.

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
- ~~**Still no auto-elevate on connect, and no vault-stored sudo password.**~~ **Both are §47**, which
  is where the target format changed: `Target::elevate` remembers the account, the program and whether
  to do it on connect, and the password — opt-in — goes in the vault under a second key shape.

## §47 — The way back in to another account (v4.0.0)

§45 gave a session a SET of shells, one per account, and then its UX was withdrawn: the "Log in as…"
button, the context-menu item, the elevate dialog and the account switcher all went, leaving the
machinery compiled, tested and unreachable. §46 was what pulled them — it met a two-factor server the
terminal handled and the file side could not — and the note it left said the shape of the dialog was
"entangled with assumptions that may not survive the rethink". This is the rethink, and the answer is
smaller than what it replaces: **one control and one dialog**.

Four things were decided before any of it was built, and they are worth recording as decisions rather
than as consequences of the code:

- **One way in.** The status bar's **Account** button, and nothing else. §45 spread the same job over
  four controls, so there were four things to keep in step and four places to look for the one that
  was missing. A right-click entry on the home screen was considered and rejected: the home screen is
  where a connection is chosen, not where a session is steered.
- **Configured in two places, because they are two different moments.** The dialog has a "do this on
  every connection to this target" checkbox — the decision made while looking at the machine — and the
  connect form has the same three fields, for the decision made before there is a session at all.
- **The account on screen is named on the button**, not on a label beside it. §45 had a read-only
  label and it was removed for duplicating the centred endpoint. This does not duplicate it: the
  endpoint names the account the session AUTHENTICATED as, and after an elevation that is no longer
  who is typing.
- **A sudo password may be kept, opt-in, in the vault.** This is a deliberate relaxation of a rule
  §12 and §45 both wrote down — that a sudo password lives in RAM and nowhere else — and it is
  written plainly here because a relaxation nobody records is a rule nobody kept.

### One dialog that lists, asks and answers

`ui/elevate.rs` is three things in the order a user meets them, and putting them in one card is what
retires §45's four controls:

1. **The accounts this session has**, each named, the one on screen marked with a dot, and every
   elevated one carrying a ✕. Switching is a click on the name of the thing switched to; the one
   showing is plain text rather than a dead button. The login account has no ✕ — ending it is what
   Disconnect does, and a second way to end a session is a way to end one by accident.
2. **Who to become**: `sudo` or `su`, an account, and the two checkboxes.
3. **The credential conversation.** sudo's questions arrive one at a time as `ElevatePrompt` and the
   dialog puts the remote's OWN wording to the user, with its words about the previous answer above
   it when there were any — which is the only thing that tells "wrong password, try again" from "now
   the second factor", since sudo dresses every prompt in its stack in cmote's `-p` text (§45).

The stage is a state (`Asking` / `Answering` / `Waiting`) rather than a pair of booleans, because
while a question is outstanding the dialog must show that question and nothing else: with two flags
there was a reachable arrangement where a "Log in as…" button sat over an elevation already running.
Pressing Account while a question is up shows the question rather than a blank form, which is one
`is_answering` test at the top of the opener and would otherwise have thrown an answer away.

### What is stored where, and why the two halves are not the same question

| | where it lives | when it is written |
|---|---|---|
| the account, the program, "on every connection" | `targets.json`, as `Target::elevate` | when the elevation is ASKED for |
| the password | `secrets.age`, keyed `sudo:<account>@<endpoint>` | when the elevation SUCCEEDED, and only then |

The preference is written on the way out because it says what the NEXT connection should try, and a
refused attempt is still what the user asked for. The password is the opposite: a wrong one must
never be stored, so it is held in a `PendingElevation` — a `Secret`, zeroized on drop — from the
moment it is sent until the elevation resolves, and then either stored or dropped. That is the same
rule §16 keeps for the connect secret, one layer up.

**The vault needed a second key shape, not a second file.** It has held one since §16 — the endpoint —
and a prefix keeps the two apart for good: an endpoint is `{user}@{host}:{port}`, so one can only
begin `sudo:` if a login name does, and a login name with a colon in it would already have broken the
endpoint scheme that is a target's identity. Keyed by BOTH endpoint and account, because they are
different secrets: `sudo` asks for the caller's password and `su` for the target account's, and one
target may be used to become more than one account. §110 argued the rest: the vault has no format
version and a second key shape needs none, since an older cmote reads the extra entries as endpoints
it has no target for — which is exactly what it already does with an entry whose target was deleted.

### The rule that says when a password may be kept at all

**A one-time code must never be kept as a password**, and the number that settles it already existed.
`elevate::Handshake` counts DISTINCT factors — a question re-put after a refusal is the same factor
over again, so a corrected password still counts as one — and §46 already read that number to decide
whether the FILE side may follow an account: one factor means a password a file channel can replay to
`sudo -S`, more means a second factor it can neither ask for nor reuse.

§47 reads the same number for the same reason one layer up, so `SshEvent::IdentityReady` carries it
now. One number, two decisions, one rule. Two factors means nothing is stored — and the target's flag
is set to what the vault ACTUALLY holds afterwards, so the dialog never opens promising a hands-free
elevation that cannot happen.

### The account is vetted at three boundaries, and one of them is a file

`elevate::valid_user` is the check that keeps anything but a plain login name out of the command line
`ElevateKind::command` composes — the one place cmote builds a remote command from something the user
typed (§12). It is applied:

- in the dialog, on submit, where a refused name is reported under the form and nothing is sent;
- on the connect form, where `ConnectForm::elevation` returns `None` rather than passing a name on;
- **on the way OUT of `targets.json`** (`Elevation::usable`), because that file is one the user is
  invited to edit (§22). An account a hand-edit put there is remote input as far as this check is
  concerned, and one it refuses is a stored preference cmote declines to act on — not an error to
  report, since nobody asked for anything.

That third boundary is why the function survived §45's withdrawal. It was kept with a
`cfg_attr(not(test), expect(dead_code, …))` and a note saying the next implementation would need
exactly it; being an `expect` rather than an `allow`, it became a build error the moment a real caller
appeared, which is how it got deleted at the right time (§111).

### The connect form's half

Three fields, and the first is the gate: **Become** (an account, blank by default), then — only once
it names somebody — **With** (`sudo` / `su`) and **Become it on connect**. Blank is not "stay put":
it says nothing, so connecting from a form that never mentioned an elevation does not erase what a
target remembers. Clearing the field, and the dialog's own checkbox, are the two ways to change it.

The form's Tab ring already skipped controls that are not on screen (a passphrase field belongs to key
auth), and it decided that from the auth method alone. Two things decide it now, so the five
signatures that took an `AuthKind` take a `FormShape` — the method, and whether an elevation is being
asked for — which is one struct rather than the next argument.

### Files

- `src/ui/elevate.rs`, new — the dialog: the account rows, the form, the question, and the status
  bar's button label. Three tests of its own.
- `src/targets.rs` — `Target::elevate`, the `Elevation` type, `set_elevation`,
  `set_elevation_remembered`, `Elevation::usable`. Three tests.
- `src/vault.rs` — `elevation_key`, the prefix and the argument for it, and a test-only constructor
  with scrypt turned down so the app's tests can use a real vault rather than a stand-in.
- `src/ui/connect.rs` — the three fields, `FormShape`, four new focus stops, two tests.
- `src/app.rs` — `Modal::Elevate`, eleven messages, `PendingElevation`, and the flow: open, submit,
  prompt, answer, settle. Eight tests.
- `src/bridge.rs`, `src/ssh/shell.rs` — `IdentityReady` carries `factors`.
- `src/elevate.rs` — `ElevateKind` is serialized with a saved target; `valid_user`'s dead-code
  escape is gone.
- 1,486 tests green, clippy `-D warnings` clean, `cargo fmt --check` clean.

### Not done

- **The elevation is not offered on a local session** (§103), the same absence Tunnels has and for the
  same reason: becoming another account is a program run on a CONNECTION, and there is none. A local
  shell's own `sudo` still works by being typed, which is what any terminal offers.
- **One target remembers ONE account.** "Do this on every connection to this target" cannot mean two,
  and a second elevation from the dialog replaces what the first remembered. A target used as two
  accounts in turn is possible — the dialog opens on whichever was last saved and the other is a
  re-type — but not remembered as a pair.
- **A stored connect password is not reused for `sudo`**, even though for `sudo` it is by definition
  the same secret — the caller's own. Reusing it would send a password stored for SSH to a program
  that was never named when it was stored, which is a purpose boundary this section is not willing to
  cross silently. The cost is that a user who wants a hands-free `sudo` ticks a second box and types
  the same password once.
- **A locked vault is not unlocked FOR an elevation.** If the master passphrase has not been given
  this session, the stored password is simply not offered and the question is put to the user. An
  elevation is not the moment to interrupt with a second, unrelated question.
- **The dialog reports a failure; it does not diagnose one.** What it shows is the remote's own words
  ("not in the sudoers file", "3 incorrect password attempts"), which is the right answer for a
  policy cmote does not know — and nothing is said about which of `sudo` and `su` might have worked
  instead.
- **The file panes still cannot follow a two-factor account** (§46's own Not done). §47 changes
  nothing there: it is the same rule about the same number, and a file channel still cannot ask for a
  code.

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

The panes answer to the arrows, the Page keys, Home/End, Tab, Enter, F2, F5 and Esc. Not one of them
answers to a plain character — there is no type-ahead in either pane — so a letter arriving while a pane
holds the keyboard could only ever have been meant for the shell. It was dropped: the pane swallowed
everything (§20's "a focused pane keeps every key"), nothing happened on screen, and the first character of
a command disappeared. Usually several, because nothing about a swallowed keystroke says it was swallowed —
the user finds out when the echo they expected is missing and has to work out how much of what they typed
survived.

Now typing **hands the keyboard to the shell and goes with it**. The focus moves before the key is
dispatched, so the letter that asked for the move is the letter that reaches the prompt rather than being
spent on the switch.

What counts as typing is `is_typing`, and it is deliberately narrow — two conditions, both required:

- **A `Character` key, never a `Named` one.** Enter, Tab, the arrows, F2, Esc, Backspace and Delete are all
  `Named`, and every one is a pane's own key. Writing the rule on the *produced text* instead — the obvious
  alternative, since winit hands one to most keys — would catch Enter, which carries `"\r"`, and take the
  tree's "send the shell there" away from it.
- **No Ctrl, Alt or Logo.** Those make a combination, not a character: the files pane's Ctrl+A takes the
  whole listing (§21), and Ctrl+Tab is the way out of a pane at all. Shift is let through, since a capital
  is as much typing as a small letter.

`ponytail:` on Windows AltGr arrives as Ctrl+Alt, so an AltGr character — `@` on an AZERTY layout — reads as
a combination and does not on its own hand the keyboard over. The letters around it do, which is the case
that matters: a command starts with a word.

The rule is one-way. Typing in the shell never moves the focus *to* a pane, because a pane has nothing to
type into (its rename fields are modal and take the keyboard whole, §18, §19).

### A command from the terminal's surface means the terminal

The grid's right-click menu — Copy selection, Paste, Upload…, and Open / Copy link on a link cell (§10, §17,
§24) — and the status-bar buttons that duplicate the first two used to leave the keyboard wherever it was.
The case that shows why that is wrong is **Paste**: pasting a command while the files pane held the focus put
the text at the prompt and left the *next* keystroke — the Enter that runs it — going to the pane.

`on_terminal_command` now puts the ring back on the shell for every item of that menu. Paste is the sharp
case, but the reading covers the rest: a copy of the scrollback, an upload into the shell's own directory, a
link followed out of its output are all work on the terminal, and none is a reason to keep the keyboard
parked on a pane.

**Ctrl+V is that same command off the keyboard**, so it is answered the same way — from wherever the ring
is, and it brings the ring with it. That means it moved *above* the focus dispatch in `on_key`: left in the
copy/paste block below it, it was only ever reached with the shell already focused, so a paste asked for
while a pane held the keyboard was dropped on the floor with no echo to say why. Neither pane claims
Ctrl+V, so nothing is taken from them. Ctrl+Shift+V is the same shortcut and pastes the same plain text
(`is_paste` covers both, matched on the physical key so it holds on AZERTY and Dvorak).

**Ctrl+C is deliberately not treated that way.** It reads the terminal's own selection, or — with nothing
selected — is the interrupt for the remote. Neither is text going *in*, which is what this whole section is
about; and of every unclaimed shortcut, "copy what is selected here" is the one a pane has the best claim
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
  would be deciding, in advance, that the panes will never want those keys.
- **No focus move for the panes' own context menus.** A menu item on the tree or the files pane acts on
  that pane, and the pane already had the keyboard when it was right-clicked; there is nothing to take
  back.
- **The right-press does not focus the grid**, per above.
- **Typing does not move the focus away from the shell**, since there is nowhere it would go.
- **No type-ahead in the panes.** Letters could plausibly jump to the entry that starts with them — the
  file-manager habit — but that is exactly the key this section gives to the shell, and the shell has the
  better claim: a terminal is a thing you type at. If type-ahead is ever wanted it needs its own way in (a
  pane-local search field), not a quiet reversal of this rule.

---

## §51 — The hand over everything you can pick up (v4.0.0)

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
  on a 200% pane beside a 100% primary that would make the hands half the size of every other
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

## §52 — Sending a tab to another area of the window (v4.0.0)

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
notices** — its channel, emulator, scrollback, panes and forwards travel with the struct, because a
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
  until the shell settles, exactly as on a reconnect (§22) — the two panes drift apart on purpose,
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
anchored to its pane for the same reason (§18). It is always on screen, it needs no stored point,
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
  clamp that keeps the pane inside the window's right edge. It draws `Destination`s and works
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
- **The copy does not inherit the original's forwards, find bar, selection or pane sizes.** Those
  belong to a session or to the target's remembered state (§22, §27), and the copy gets the target's
  the same way any other connect to it would. Only the directory is carried, because only the
  directory is the thing the user was looking at.

---

## §53 — A picture opens as a picture (v4.0.0)

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
- **The decoder is chosen by the file's MAGIC BYTES** (`preview::decode_image`), never by its name. The
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
  buys most of the clarity for none of the work. Deliberately not the panes' near-black, which
  would swallow exactly the dark artwork transparency is most often used for.
- **No animation.** An animated GIF shows its first frame. Playing one means a frame clock, a decode
  loop and a pause control — a media player, not a preview.

---

## §54 — A remote command says how far along it is (v4.0.0)

A build, a `dd`, an `apt upgrade` and a `rsync` all know their own progress, and until now cmote had
no way to hear it. The convention ConEmu introduced and Windows Terminal adopted is one OSC:

```
ESC ] 9 ; 4 ; st ; pr   BEL | ST

  st = 0   remove progress            pr ignored
  st = 1   this share is done         pr = 0..100
  st = 2   the work failed            pr optional — stays where it was
  st = 3   working, share unknown     pr ignored
  st = 4   paused / wants attention   pr optional — stays where it was
```

`alacritty_terminal` treats it as an unknown OSC and drops it, so this is the beside-the-engine
tactic once more (§17, §33, §34, §41): scan the same bytes, keep a reading, draw it.

### Where it shows

**Two surfaces, and the split is deliberate.** The chip carries a 3 px bar along its bottom edge, for
**every** tab — a background tab's build is exactly the thing worth seeing without switching to it.
The Windows taskbar button carries the **active** tab's reading only, because there is one button and
there can be many tabs, and "the one you are looking at" is the only rule that needs no explanation.
Neither surface appears at all until a command reports, so a session whose commands never send this
looks precisely as it did before.

The bar is laid **over** the chip in a `stack`, not added to its row: a bar that took up layout would
make every chip resize the moment a command started, and the whole strip would twitch.

### Decisions worth stating

- **Implementing `9;4` did not reopen `OSC 9`.** OSC 9 is multiplexed — `9;9` is the Windows cwd
  announcement cmote has read since §17, `9;4` is this, and a bare `9;<text>` is a desktop
  notification which stays **refused**. The line is not the OSC number, it is whether the effect
  escapes the tab: progress cannot leave the chip it belongs to, a notification lands on the user's
  desktop and outlives the session. Recorded in TERMINAL_COMPATIBILITY_PLAN part 6, along with the two
  other spellings of the same refusal (`OSC 777`, `kitty 99`).
- **A malformed report is a no-op, never a reset.** Every byte here is chosen by the remote, so an
  unknown `st`, a non-numeric field, an `st=1` carrying no share, and a number too big for a `u32`
  all leave the previous reading untouched. The alternative — falling back to a default — would hand
  a remote a way to *blank* a real reading by sending rubbish, which is a worse primitive to offer
  than an ignored sequence.
- **A share is clamped, not believed.** The number is drawn, so a claimed 4 000 000 000 is 100.
- **The command ending is judged in stream order, inside the scanner.** One read off the wire can
  carry the end of one command, a fresh prompt, and the first report of the next. Clearing the bar
  after the chunk — the obvious place — would wipe the *new* report and leave a working tab looking
  idle. So `progress` asks `osc133::ends_command` payload by payload as the framer hands them over.
  That predicate is a five-line seam on `osc133` rather than a copy of its grammar: the module that
  defines the marks keeps owning what one means.
- **There is no `clear` on the interface.** Both endings that matter arrive in the stream (the
  remote's own `st = 0`, and §34's `D`). The one place a caller might reach for it is `resize`, and
  `resize` must **not**: it drops the prompt marks and the inline images because both are anchored to
  grid positions a reflow invalidates, whereas a progress reading has no place on the grid at all.
  Wiping a running command's bar because the window got wider would be a bug, so the method that
  would let that happen does not exist.
- **`st = 3` (indeterminate) is a full-width dimmed bar, not an animated pulse.** Animating it means
  waking the whole window on a timer to move a few pixels on a strip. The dimming is what
  distinguishes it from a genuine 100%.
- **The taskbar is hand-rolled COM, because `windows-sys` ships no COM vtables.** It has the
  `TaskbarList` CLSID and nothing to call on it, so `ITaskbarList3` is declared in `taskbar.rs`:
  the IID, the vtable laid out in its true order (IUnknown's 3, then ITaskbarList's 5, then
  ITaskbarList2's 1, so `SetProgressValue` is slot 9 and `SetProgressState` slot 10). The
  alternative was the full `windows` crate for two calls. The HWND is the one `cursor.rs` already
  stashes for §51, so no new window plumbing was needed.

### Deliberately not

- **No desktop notifications, in any of their four spellings** — see above and
  TERMINAL_COMPATIBILITY_PLAN part 6. This is a decision, not a deferral.
- **No progress in the status bar.** That bar already shows the file transfer queue's progress
  (§16, §17), and two unrelated progress bars in one strip is how a user learns to read neither.
- **No history of readings, and no rate or ETA.** The remote reports a share; inventing a
  time-remaining from it would be cmote guessing about work it cannot see.
- **`ponytail:` a tab whose remote sets progress and then dies WITHOUT shell integration keeps its
  bar** until the next report or the tab closes. `st = 0` and §34's `D` both cover the honest cases;
  a shell with no integration configured gives nothing to hook, and inventing a timeout would put a
  bar's lifetime in cmote's hands rather than the reporter's.

---

## §55 — A script says "look here" (v4.0.0)

iTerm2's OSC 1337 is not one sequence. It is a private namespace — a `key=value` grab-bag sharing one
OSC number, about twenty keys deep:

```
ESC ] 1337 ; SetMark                        BEL | ST
ESC ] 1337 ; CurrentDir=/home/user          BEL | ST
ESC ] 1337 ; SetUserVar=gitBranch=<base64>  BEL | ST
ESC ] 1337 ; SetProfile=Production          BEL | ST
```

So "support OSC 1337" is not a decision anyone can take in one go, and taking it in one go would have
been actively dangerous. **Two of those keys are decisions cmote had already made, wearing a different
costume**, and a generic implementation would have quietly reopened both:

| key | what it actually is |
|---|---|
| `Copy=<base64>` | a system-clipboard write — **OSC 52 write**, refused (TERMINAL_COMPATIBILITY_PLAN part 6) |
| `SetProfile=` / `SetColors=` | a theme repaint — the **fixed-scheme** refusal (§6) |
| `SetBackgroundImageFile=` | both of the above, plus a remote naming a file for cmote to **decode** (§41) |

That is the finding that shaped this section, and it is why `term/iterm.rs` is an **allow-list rather
than a parser with a policy bolted on**. A key not named there produces nothing, so a key iTerm2 adds
tomorrow is refused by default rather than by anyone remembering to refuse it.

### What is honoured

**`SetMark` — a navigable bookmark on the current line.** This is the one item in the namespace that
is genuinely additive rather than a second spelling of something cmote has. §34's marks are
*prompt-derived*: OSC 133's A/B/C/D bracket a command, so nothing in that vocabulary can mark a point
**mid-output**. `SetMark` can — before each test suite, each stage of a build, each retry — and a
script that drops them turns a wall of output into something navigable.

It lands on machinery that already existed: absolute line indices that ride the scrollback, the
left-gutter tick, and Ctrl+Shift+Up/Down.

**`CurrentDir=` — a third spelling of the working directory.** Read in `term/cwd.rs` beside OSC 7 and
OSC 9;9, *not* in `term/iterm.rs`, because the only thing that differs between the three is the prefix
and the path arithmetic should not exist twice. A dotfile written for iTerm2 now gets cwd tracking
(§17) with nothing else configured.

**`SetUserVar=gitBranch=<base64>` — the branch, on the chip.** iTerm2 lets a shell set any named
variable and reference it from a title template; cmote has no template, so the allow-list is applied a
**second time, to the names**. Only `gitBranch` is kept — the name iTerm2's own git integration uses.

That narrowness is the security property, not a shortcut: a remote cannot make cmote hold a map whose
keys it chose, so there is no unbounded store needing a bound. The value is drawn as a small pill on
the chip, after the endpoint label.

### Decisions worth stating

- **A bookmark is not a prompt, so it is stored apart.** The tempting shortcut was to push `SetMark`
  lines into `Prompts::marks` and get the ticks and jumps for free. It is wrong: nothing about a
  bookmark has a command state, an exit code or an output span, so `output_at_prompt` must not resolve
  one — a click on a bookmark's tick has no command to select, and a merged list would eventually
  hand it somebody else's. `user_marks` is its own ring.
- **…but a JUMP treats them alike.** Being reachable is the entire point of dropping a bookmark, and a
  user pressing Ctrl+Shift+Up is asking for "the last interesting line", not for a particular *kind*
  of interesting. So `visible_rows` and `visible_user_rows` are two answers (they are drawn in
  different colours) while `jump` chains both lists into one.
- **Amber for a bookmark, cyan for a prompt, and the bookmark draws last.** A shell hook that emits
  both on the same line should show as the bookmark: a prompt is derivable from the shell's own marks,
  whereas a bookmark is something a script went out of its way to say. Same geometry, so one simply
  draws over the other.
- **The tick projection is one function.** `project` is shared by both lists deliberately — an
  arithmetic that drifted between them would put one kind of tick a row off the line it marks.
- **A bookmark is grid-anchored, so it joins the interruption advance.** Its whole content is the line it
  arrived on, so `process` merges it into the same offset-ordered list as the prompt marks and the
  inline images (§34, §41) and applies it at its own point in the stream. A chunk carrying a prompt
  mark *and* a bookmark puts each on its own line, which is the case that list exists for.
- **The payload cap is part of the refusal.** `MAX_PAYLOAD` here is far below what an `iTerm2 File=`
  inline image needs. That key is refused, and refusing to *buffer* it is the cheapest possible way of
  meaning it — a megabyte of base64 overruns the cap, the framer abandons it, and cmote holds none.
- **The branch pill sits BESIDE the endpoint label, never in place of it.** This is the load-bearing
  decision of the variable half, and it is a spoofing one. The value is chosen by the remote and drawn
  in the tab strip — chrome cmote owns, which the user reads to know *which machine they are typing
  into*. Given the label outright, a remote could name itself `prod-db-01` and be believed. In its own
  dimmer pill, after the label, it reads as an annotation on the tab: the remote gets to say what
  branch it is on, and does not get to say what host it is.
- **A value that will not decode leaves the branch alone; an EMPTY value clears it.** Those must be
  different answers, which is why `parse_user_var` is three-valued (`None` / `Some(None)` /
  `Some(Some)`). An empty value is the shell reporting it *left the repository*, and a stale branch
  under a directory that has none would be a lie. Rubbish, though, must not be a way to wipe a real
  reading — the same rule §54 applies to progress.
- **Sanitised on the way IN, not on the way out.** Control characters stripped (the window title's rule
  since §23, and for the same reason) and the length capped, both before the value is stored, so the
  bound holds however it is later drawn. The cap counts `chars`, not bytes, so a name of multi-byte
  glyphs is cut at a character boundary rather than panicking a slice.
- **`base64` is taken as a dependency rather than hand-rolled.** It is the same crate at the same
  version `alacritty_terminal` already pulls in, so it adds nothing to the graph and nothing to
  compile. The input is remote-controlled, and a decoder is exactly the small, boundary-condition-heavy
  code that should not be written a second time in the world.

### Deliberately not

- **`Copy=`, `SetProfile=`, `SetColors=`, `SetBackgroundImageFile=`** — see the table above. Refused
  because they are closed decisions in a new costume, and each is pinned by a test **by name**, so the
  refusal is checked rather than merely intended.
- **`StealFocus`, `RequestAttention=`, `ClearScrollback`** — refused on the line §54 drew: a remote may
  change what its own tab looks like and nothing more. `StealFocus` raises the window;
  `RequestAttention` flashes the taskbar button, which is an *interrupt demand* rather than §54's
  reading about work the user started; `ClearScrollback` destroys the user's own record of the session.
- **The keys that are simply redundant** — `CursorShape=` (DECSCUSR already, and OSC 50 as well),
  `ClearScrollback` (`CSI 3J` already), `ReportCellSize` (`CSI 14t` already — **not `CSI 16t`**, which
  this line was wrong about until §60 and cmote does not answer), `ShellIntegrationVersion` (OSC 133).
  Nothing is gained by a second spelling of a sequence that already works.
  **§71 moved the first and third of those from ❌ to 🛑** and pinned both by name. Redundancy is the
  reason; the allow-list is the mechanism; and a refusal nobody checks is a refusal a later reader
  removes as a courtesy — which is a likelier end for these two than for any dangerous key here.
- **`AddAnnotation=`, `SetBadgeFormat=`** — real features with no consumer in cmote. A note attached to
  a line range, and a watermark over the grid, are each their own section if ever wanted.
- **No user variable other than `gitBranch`, and no title template.** iTerm2's variables are useful
  because a configurable template interpolates them; building that here means a settings surface, a
  parser for the template, and a decision about how much remote-chosen text may appear in the strip.
  The one variable with an obvious reader is honoured, and the rest wait for a real want. Adding a
  second name later is one entry on the list.
- **The pill does not go in the window title.** The title is what the OS shows in the taskbar preview
  and the Alt-Tab list, i.e. outside the tab — so remote-chosen text there escapes further than the
  strip, and §54's line applies.

---

## §56 — The labels a program asked us not to wipe (v4.0.0)

A VT220 had **two** erases. The plain one wipes everything; the *selective* one leaves alone whatever
the program marked as protected first:

```
CSI 1 " q     DECSCA — protect what is written from here on
CSI 0 " q     DECSCA — stop protecting (Ps 2 means the same)
CSI ? Ps J    DECSED — selective erase in the display
CSI ? Ps K    DECSEL — selective erase in the line
```

It exists for data-entry forms on a serial line. The program draws the labels once inside a protected
run — `Name:`, `Address:` — the user types into the blanks unprotected, and the next record costs a
single `CSI ? 2 J`: the typed fields clear and the labels stay put. At 9600 baud, not redrawing the
form was the whole point.

Nothing much emits it today, because full-screen programs repaint every frame instead. It sat in the
matrix as a ❌ that had never been looked at properly, and looking at it turned out to be worth more
than the feature: it is the first thing cmote could not add the way it added everything else.

### Why the usual tactic does not work here

Every compatibility addition since §17 has the same shape. The engine drops a sequence, so cmote scans
the same bytes for it and keeps the answer **beside** the grid: a working directory (§17), a reply to an
identity query (§33), a prompt mark's line number (§34), a picture's anchor (§41), a progress reading
(§54), a branch name (§55).

Protection cannot be kept beside the grid, because it is not one answer — it is **per-cell state**. A
bitmap of protected cells would have to be re-aligned every time the grid moved underneath it: every
scroll, every `IL`/`DL`, every reflow on resize, every swap to the alternate page and back. Keeping a
second grid in step with the engine's grid *is* re-implementing the grid, and it would drift the first
time a program did something the re-implementation had not thought of.

The other obvious route is to fork the engine — add a `PROTECTED` flag and two `vte` arms, maybe eighty
lines. Rejected: it buys one dead VT220 feature and costs a vendored fork of two crates forever, and the
whole point of the seam in `term/mod.rs` is that the engine stays swappable and unpatched.

### What worked: borrow a bit the engine is not using

`alacritty_terminal` stores each cell's attributes in a `Flags: u16` and **names fifteen of the sixteen
bits**. Bit 15 is free.

So cmote sets bit 15 on `grid.cursor.template` — the pen — while DECSCA is armed. Every cell the engine
prints is stamped from that template, so from then on the engine carries protection *itself*, as if it
were bold: through scrolling, through insert/delete, through reflow, through the alternate-screen swap.
There is no map to keep aligned, because protection lives in the same place as the glyph it belongs to.

It is invisible in both directions, which is what makes it safe rather than a trick:

- **The engine never reads it.** `Cell::is_empty` tests named flags with `intersects`, so an unknown bit
  cannot make a blank cell look occupied — which would otherwise have quietly changed line-wrap
  trimming and what a copy yields.
- **The renderer never draws it.** `ui/grid.rs` and `screen.rs` match named flags too.
- **`Cell::reset` clears it**, so protection dies with the cell's content. A plain erase leaves the cell
  reusable and unprotected, and a form cannot accumulate ground nothing can clear.

The one hazard is a future `alacritty_terminal` naming a sixteenth flag. That is caught at **build
time**, not at runtime:

```rust
const _: () = assert!(
	Flags::all().bits() & protect::PROTECTED_BIT == 0,
	"the engine has claimed the flag bit cmote borrows for DECSCA protection — pick another"
);
```

A collision would otherwise surface as text that cannot be erased and one attribute coming out wrong — a
symptom nobody would trace back to a bit mask.

### The one thing the engine does do to the flag word

`SGR 0` assigns it whole: `Attr::Reset` sets `Flags::empty()`. On a real terminal DECSCA is independent
of SGR, so `CSI 0 m` inside a protected run must not unprotect the rest of it.

So the scanner reports **every SGR seen while the pen is armed**, and `mod.rs` puts the bit back on the
far side of it. Deliberately over-reported: re-asserting a bit that is still set is a no-op, whereas
working out which SGR lists contain a reset means parsing colour specs, where a `0` can be a colour
index (`38;5;0`) rather than a reset. Over-report and stay correct. An unarmed stream — every ordinary
session — reports nothing at all, so the common case costs `process` no splits.

### Two smaller decisions

**The offsets point the other way.** Every other split-fed scanner reports the offset the sequence
*starts* at, because the engine is advanced up to it and the cursor then names the line the event
belongs on. `protect` reports **one past** the final byte: a pen change has to land after the SGR that
wiped it, and an erase after the engine has ignored the sequence. Same loop, opposite side of the
boundary.

**The erase writes cells directly**, which breaks the rule §41 set in `reserve_cells` — inject VT
sequences, because erasing and scrolling are the engine's business. Two reasons it cannot hold here. The
engine's plain `CSI 2 J` on the primary screen does not blank the viewport at all, it **scrolls it into
history** (`Grid::clear_viewport`), which would carry the protected cells off with everything else. And
the per-run alternative — position with CUP, blank with ECH — would move the cursor across a screen this
erase is defined never to move it on, dragging in origin mode and clearing the pending-wrap flag. The
honest version of "blank these cells and nothing else" is to blank these cells. What gets written is
what the engine's own erase writes: the pen's background colour and no glyph.

### What it cost, and what it unblocked

One new module (`term/protect.rs`), two methods in `term/mod.rs`, one arm on `Interruption`. The region
arithmetic is a pure function over row and column numbers, so all six shapes of DECSED/DECSEL are
tested without building a terminal.

It also moved a wall. **DECSERA** (`CSI Pt;Pl;Pb;Pr $ {`) — selective erase of a rectangle — was listed
under §5's VT420 rectangular ops as an engine limit. The missing piece was per-cell protection, and that
now exists, so DECSERA is a fourth shape for `protect::spans`. Left unbuilt because nothing asked for
it, which is a different sentence from the one that was there before.

### Not done

- **DECSCA is not reported by DECRQSS.** A program can set protection and cannot ask what it is. §33
  answers `DCS $ q m` from the pen's SGR, and protection is not an SGR attribute, so this would be a new
  selector rather than a field on an existing reply.
- **No protected-cell awareness anywhere else.** `IL` / `DL` / `ICH` / `DCH` move cells around and take
  protection with them, which is right: DECSCA makes a cell unerasable, not immovable.
- **`CSI ? 3 J` does not exist.** Plain `CSI 3 J` drops the scrollback, and protection is a property of
  cells on the screen — history is not erased a cell at a time.

---

## §57 — Refusing a sequence without paying for it (v4.0.0)

The next ❌ down the matrix was left/right margins, and it is a different animal from everything above
it. Two sequences, one final byte:

```
CSI s           SCOSC   — save the cursor position (the ANSI.SYS spelling, universal)
CSI Pl ; Pr s   DECSLRM — set the left and right margins (VT420)
```

A real VT420 tells them apart with a mode: `s` means margins only once **DECLRMM** (`CSI ? 69 h`) is
set, and save-cursor otherwise. cmote does not have margins to give. The engine's scroll region is
vertical only — `set_scrolling_region(top, bottom)` — and horizontal margins are not one more arm to
add: they change what printing does at the right edge, what wrapping does at the left, and what `IL`,
`DL`, `ICH`, `DCH`, `SU`, `SD` and every line feed at the bottom of the region operate on. That is the
grid's whole job, so DECSLRM stays out, and the engine agrees: mode 69 is not in its list, and DECRQM
answers `0`, "not recognised". A program that asks is told the truth.

### The gap that was not a gap

The problem is the program that does not ask, because refusing DECSLRM was not free. `vte`'s dispatch is

```rust
('s', []) => handler.save_cursor_position()     // vte-0.15.0/src/ansi.rs:1737
```

which never looks at its parameters. So `CSI 5;70 s` **saved the cursor** — and the engine keeps one
saved-cursor slot, shared by `CSI s` and `ESC 7`, so it overwrote whatever the program had put there. A
program that saves the cursor, updates a status line, and restores would land wherever the margin
request happened to sit. The bug surfaces as a cursor that jumps, nowhere near the sequence that caused
it, and the matrix's old note — "unreachable in practice, a conformant emitter never sends it" — is a
guarantee about other people's software.

This is a shape none of the earlier compatibility work had: **the engine does not ignore the sequence,
it acts on it wrongly.** Scanning it out and keeping the answer beside the grid, the move that carried
§17 / §33 / §34 / §41 / §54 / §55 / §56, has nothing to offer here — the problem is not what cmote
fails to do with the bytes, it is what the engine does with them. So the bytes have to be stopped.

### Cancelling a sequence in flight

`term/cancel.rs` is a chunk-safe CSI state machine looking for one shape, and `process` splits its
advance at the offending **final byte**: advance the engine up to it, feed it `CAN` (0x18) in place of
it, resume after it. One small state machine, and four lines in the loop.

Two details carry the design.

**Feeding nothing in place of the byte would be the bug, not the fix.** The parameters have already
reached the engine's parser, which is sitting in its CSI-parameter state waiting for a final byte — so
the *next* one in the stream would be taken as this sequence's. `CSI 5;70 s` followed by `hello` would
dispatch `('h', [])` with parameters 5 and 70: set mode 5, set mode 70, print `ello`. The sequence has
to be **ended**, and CAN is how the ANSI state machine ends one: 0x18 in the CSI-parameter state routes
to `anywhere()`, which runs `execute` and returns to Ground with no dispatch, and `execute` has no arm
for CAN. Not SUB (0x1a), which takes the same transition but is *defined* to be displayable — this
engine ignoring it today is not a promise. Not a final byte that merely has no arm, like `('p', [])`,
because that rests on the absence of an arm, which is the kind of thing a version bump adds. CAN is a
cancel in the machine itself, and there is a test for each half: one that fails if the byte is not
withheld, one that fails if the CAN is not fed.

**A parameter is what makes it DECSLRM.** The mode that would settle it is one the engine never
accepts, so the parameter count is the only evidence in the bytes. It costs the reading of `CSI 0 s` as
a save-cursor, which nothing writes — every save-cursor in the wild is the bare `CSI s`, and that one
is untouched and still works. A private marker or an intermediate rules the sequence out too:
`CSI ? Pm s` is XTSAVE, which the engine drops harmlessly on its own.

The offsets are a third convention, and worth naming as such: a prompt mark reports where its sequence
**starts** (§34), a selective erase reports the byte **one past** the end (§56), and a cancel reports
**the final byte itself** — because that byte is the one being replaced. The interruption loop clamps
`offset.max(start)` now, since a cancel is the first split that leaves `start` past its own offset.

### What it cost

One module (`term/cancel.rs`, 15 tests), one `Interruption` arm, five engine-level tests. No new state, no
buffering, no rewriting of the stream — the chunk is still fed to the engine as slices of the caller's
bytes, with one byte swapped on the way past.

A ❌ that costs nothing is worth more than most ✅s, and the fourth way into a compatibility gap —
next to "scan it out", "borrow a bit", and "accept the engine's limit" — is **refuse it properly**.

### Not done

- **The margins themselves.** Unchanged and not planned — but the reason has since been corrected, and
  it is a price rather than a wall. This section first said margins would mean re-implementing the
  grid. They would not: `Processor::advance` is generic over `Handler` and `Term` merely implements it,
  so a wrapper can sit between the parser and the engine; `Cursor::input_needs_wrap` is public, so even
  the pending-wrap decision is reachable; and §58 supplied the band scroll (a rectangular copy plus an
  erase). It is about twelve of `Handler`'s 71 methods overridden and the rest forwarded — 400–600
  lines. What it is *not* is safe to keep: every method of that trait has a default empty body, so a
  forwarding gap, today's or a future version's, compiles cleanly and silently drops a sequence — the
  §57 hazard again, minus the `const` assertion that made §57's catchable. Nothing outside a
  conformance suite emits DECSLRM. Costed in TERMINAL_COMPATIBILITY_PLAN part 5.
- **No other misparse is known.** This module has one member because the audit found one. The near
  neighbours were checked and are clean: `CSI ? Pm r` (XTRESTORE) and `CSI ? Pm s` (XTSAVE) both carry
  a marker the engine has no arm for, and DECSTBM's `r` is the vertical region the engine really
  implements. `CSI ? 5 W` (DECST8C) and `CSI Ps SP k` (SCP) are parsed and dropped into empty default
  handlers, which is a *third* failure shape — the arm exists and does nothing — and harmless.
- **cmote does not tell the program it refused.** There is no reply for DECSLRM to fail with, and a
  program that wants to know can ask DECRQM about mode 69 and be told `0`.

---

## §58 — Acting on a box instead of a line (v4.0.0)

Everything a terminal erases, it erases in lines. `CSI K` takes part of one, `CSI J` takes the rest of
the screen, and the selective pair §56 added take the same shapes. A VT420 could also act on a **box**:

```
CSI Pt;Pl;Pb;Pr $ z                          DECERA  — erase the rectangle
CSI Pt;Pl;Pb;Pr $ {                          DECSERA — erase it, leaving protected cells (§56)
CSI Pch;Pt;Pl;Pb;Pr $ x                      DECFRA  — fill it with one character
CSI Pts;Pls;Pbs;Prs;Pps;Ptd;Pld;Ppd $ v      DECCRA  — copy it somewhere else
```

Top, left, bottom, right — 1-based, inclusive, and 0 or omitted meaning the edge of the page. These
were the block operations of a forms terminal: clear a field, rule a line of `-` across a box, scroll a
sub-window by copying it up one row.

`vte` has no arm for any of them. Its CSI dispatch matches the `$` intermediate in exactly two places,
both of them DECRQM, so all four fall through unhandled and are dropped whole.

### Why this was cheap, and why it was not before

It sat in §5 as an engine limit for the whole life of the audit, and by the time it came round it was
not one — §56 had already built the hard half. Writing cells straight into the engine's grid, and
knowing which of them a program had protected, were the two problems worth solving, and both were
solved for the selective erase. What was left is a grammar and some arithmetic: one more chunk-safe CSI
scanner, two pure functions over row and column numbers, and four small methods that walk a box.

The geometry is deliberately pure, as `protect::spans` is. `area` resolves corners against a page size;
`copy_extent` works out how much of a source fits at its destination. Neither touches a terminal, so
every case of defaulting, clamping and trimming is tested directly instead of inferred from a screen.

### Three rules decided where they are decided

**A rectangle nobody could draw is a no-op.** An *end* past the edge of the page clamps to it — a
program sized for a bigger screen gets the part of its rectangle that exists. A *start* past the edge
does not clamp, and neither do crossed corners (bottom above top, right left of left) get swapped: both
yield nothing. Clamping a start back onto the last row would erase a row the program never named, and a
rectangle cmote invented is worse than one it declined.

**DECFRA's character is an allow-list**, 32–126 and 160–255 — printable ASCII and printable Latin-1,
the ranges xterm allows. Anything else drops the whole sequence. "Fill four hundred cells with U+0000"
is not a request worth honouring on the way to finding out what the renderer does with it (§12).

**DECCRA reads its source out whole before writing anything.** The two rectangles may overlap, and the
maximally overlapping case — copy a box over itself, one row up — is exactly what the sequence exists
for. DEC defines the copy as if it went through a buffer, so cmote uses a buffer rather than working out
which direction to walk in. Whole cells move, so colour, attributes, the OSC 8 link and DECSCA
protection all travel with the glyph. That last one is right on its own terms: protection makes a cell
unerasable, not immovable.

Protection otherwise divides the family the way §56 divided the erases: only DECSERA respects it.
DECERA, DECFRA and DECCRA go straight through a protected cell, exactly as the plain `CSI J` does — two
verbs, and the plain one is the stronger.

### The one limit left in, and disclosed

**Origin mode is refused, not approximated.** With DECOM set, these corners are counted from the top of
the scrolling region rather than the top of the page — and the engine keeps `scroll_region` as a private
field with no accessor. Placing the rectangle at the page's rows anyway would put it on the wrong lines,
so `apply_rectangle` drops every one of these while origin mode is on. That is §57's rule applied a
second time: doing nothing is a correct refusal where acting on a guess is a wrong action, and one line
of check beats a shadow copy of the region — which would mean tracking DECSTBM, the engine's own
clamping rules, and every reset that widens the region back out. It is marked `ponytail:` in the code
rather than left to be discovered.

### What it cost

One module (`term/rect.rs`, 30 tests), one `Interruption` arm, four methods and eight engine-level tests. The
overlapping copy, the protection split, the pen-attributed fill, the trimmed copy, the origin-mode
refusal and the undrawable rectangle each have one.

### Not done

- ~~**DECCARA / DECRARA** (`$ r` / `$ t`), and the **DECSACE** (`CSI Ps * x`) that picks their
  extent~~ — **done in §59**, below.
- ~~**DECRQCRA** (`CSI Pid;Pp;Pt;Pl;Pb;Pr * y`) — the rectangle checksum a conformance suite blocks
  on~~ — **done in §60**, below, on the geometry this section built.
- **No page parameters.** DECCRA's `Pps` and `Ppd` are ignored: cmote has one page, which is what
  clamping a page number to the number of pages a terminal has comes to.
- **No damage plumbing**, as in §56 — cmote repaints from the grid each frame, so a direct write needs
  none.

---

## §59 — Changing how a box looks without moving what is in it (v4.0.0)

§58 built the half of the VT420 rectangular family that changes what the cells **hold**. This is the
half that changes what they **look like**, leaving every character exactly where it stands:

```
CSI Pt;Pl;Pb;Pr;Ps… $ r      DECCARA — turn attributes on and off across an area
CSI Pt;Pl;Pb;Pr;Ps… $ t      DECRARA — flip them
CSI Ps * x                   DECSACE — pick which SHAPE those two act on
```

It is what a forms terminal used to highlight a field: underline a column of entry blanks in one
write, reverse the row under the cursor as it moved down a menu, then flip it back. Doing the same
with SGR means repainting the text, and repainting the text was the expensive thing.

`vte` has no arm for any of the three. It matches the `*` intermediate in **no CSI at all**, and `$`
only in the two DECRQM spellings, so all three fall through unhandled — like the four before them.

### DECSACE is a mode, and modes belong to whoever sees the order

DECSACE picks between two readings of the same four corners. Under `2` they are a **rectangle**, the
box §58 already draws. Under `0` or `1` — the default, and what a terminal powers up in — they are a
**stream**: the wrapped run from the top-left corner out to the end of its row, every whole row
between, then in from the start of the last. The shape a mouse selection has.

That makes it the first thing in this family that is state rather than work, and it is kept in the
**scanner**, not in `Terminal`. The scanner is the one place that reads a DECSACE and the DECCARA
after it in stream order, so the ordering is free there and would have to be reconstructed anywhere
else. Each attribute request therefore leaves `term/rect.rs` already carrying the extent that was in
force when it arrived, and `term/mod.rs` never has to hold a mode or reason about when it changed.

Two smaller calls, both made the same way as §58's: a value DEC never defined (`CSI 9 * x`) leaves the
extent where it was rather than guessing at a shape, and **RIS resets it while DECSTR does not** —
because DEC's published DECSTR list does not name DECSACE, and inventing a reset is the same kind of
guess as inventing a rectangle.

### The extent changes a rule, not a walk

The easy assumption is that the extent only decides which cells get visited. It also decides whether
there are any. A rectangle whose right corner is left of its left one is undrawable and yields
nothing (§58's rule). The **same numbers** as a stream are ordinary: `CSI 1;70;5;10;4$r` underlines
from row 1 column 70, round the wrap, to row 5 column 10. So left and right are only compared when
the run is confined to a single row, and the extent is a parameter of `area` rather than a mode it
reads — the call site is what says which family the corners belong to, and the four operations of
§58 pass `Rectangle` because DECSACE does not govern them.

### Only the bits it names

`Ps` here is a small DEC-defined subset of SGR: `0` for all of them, then bold, underline, blink and
reverse — never a colour, never a glyph. The selectors fold into a `Change { on, off, flip }` at
parse time, later winning over earlier as in an SGR, so the walk over the cells costs the same
however long the list was. An unknown selector (`3`, italic, which DEC never gave DECCARA) is
**ignored while the rest of the list still applies** — the opposite of the rule for a malformed
*number*, and deliberately: a number that will not parse leaves cmote unable to say which cells were
meant, while `3` is a perfectly clear request for an attribute this sequence cannot name.

DECRARA gets the shorter table — `0 1 4 5 7` and no off-forms — because "off" has no meaning for a
verb that flips, and reading `24` as an underline toggle would flip an attribute on a request that
plainly said "off".

The rule that matters most is at the other end. `attribute_rect` sets **named bits one at a time**
and never assigns the flag word. Assigning it would be the obvious way to write "all attributes off",
and it would silently unprotect a form the moment a program underlined it — cmote's DECSCA protection
rides bit 15 of that same word (§56). A test pins it: underline a protected label, then selectively
erase the row, and the label has to still be there.

### Blink is read and dropped

`alacritty_terminal`'s flag word has no bit for blink. The fifteen it names cover inverse, bold,
italic, dim, hidden, strikeout, five underline styles and the wide-character marks, and nothing
blinks. So DECCARA's `5` / `25` and DECRARA's `5` are parsed, accepted and dropped in the one place
that translates cmote's mask to engine names — the same call already made for DECSCUSR's blinking
cursor shapes (§2), and the honest one while there is nothing to store it in. A program asking for
blink and underline together still gets its underline.

### What it cost

Three arms in an existing scanner, one mode field, two selector tables, one method, and 24 tests. The
extent's two shapes, the undrawable-rectangle-that-is-a-valid-stream, the protection bit, the
"all off" that spares italics, the dropped blink and the reversal-with-no-off-forms each have one.
Both load-bearing halves were checked by breaking them: forcing `columns_on` to ignore the extent
fails the stream tests, and assigning the flag word fails the protection test.

### Not done

- ~~**DECRQCRA** (`* y`) is still the only piece of the family left~~ — **done in §60**, below.
- **Origin mode** is refused here as it is for the rest of §58's family, and for the same reason.
- **`term/mod.rs` grew again** (~2700 lines). `interruptions()` now takes six positional lists and could use
  a struct; the six scanners could plausibly become one. Neither is urgent, both are noted.

---

## §60 — Answering a question about a box (v4.0.0)

The last row of the rectangular family, and the only one that writes bytes back down the pty:

```
CSI Pid ; Pp ; Pt ; Pl ; Pb ; Pr * y      DECRQCRA — report a checksum of the rectangle
DCS Pid ! ~ XXXX ST                       DECCKSR  — the answer
```

§58 built the geometry and §59 the mode beside it; this needed neither. What it needed was a number
that is right to the digit, which is a different kind of work from everything above it.

### A checksum you derive is worth less than no checksum at all

Every other sequence in this family could be reasoned out from its definition: a rectangle is a
rectangle, an erase blanks cells. A checksum has no such property. Its only use is being compared —
a conformance suite prints the four digits it got beside the four a real VT420 gave — so an
implementation that is *plausible* fails exactly as loudly as one that is absent, and costs the work
as well. Two implementations that both "sum the characters" disagree on whether the sum is negated,
whether attributes count, what a blank weighs, and what happens at sixteen bits. There are four
places to be wrong and no way to notice from the inside.

So it was not derived. It was **copied**, from xterm's `xtermCheckRect` with no extension bits set —
the mode xterm arrived at by comparing against screenshots from a real VT520, which makes it DEC's
answer by way of the one implementation everybody tests against. The primary source was read, not
remembered:

- a cell weighs its character code, plus 0x04 if DECSCA protected, 0x08 hidden, 0x10 underlined,
  0x20 reverse, 0x40 blinking, 0x80 bold;
- a cell finishing at exactly 0x20 — a plain space, nothing added — is dropped, **except the first
  cell of the rectangle**, which always counts, so an empty area reports one space rather than zero;
- the total is taken mod 2^16 and **negated**, which is why a page of ordinary text reports a number
  just under 0x10000, and is the single detail most likely to come out backwards.

The trim compares the *finished* value, attributes included. An underlined blank weighs 0x30 and
survives — a cell you can see is a cell that counts. That falls out of doing it in the right order,
and would not have from doing it in the obvious one.

### Three places it cannot match, named rather than hidden

**Blink** has no bit in `alacritty_terminal`'s flag word (§59 found this the hard way), so 0x40 never
lands. **A DEC charset cell** — `ESC ( 0` then `q` for a box-drawing rule — reaches the grid already
translated, so cmote weighs U+2500 where xterm weighs the `q` it remembers seeing; reversing the
translation would be a guess, since a program that wrote U+2500 directly would then be weighed wrong
in the other direction. And **a never-written cell** reads exactly like a written blank, because the
engine's grid starts full of blanks and has no "drawn" bit; xterm has one and skips those cells, so a
rectangle whose first cell is virgin reports 0xFFE0 where xterm reports 0x0000.

That last one is the interesting divergence, because it is bounded to a single term: **every
rectangle that begins on a written cell agrees to the digit**, which is every rectangle a suite
checksums after painting one. Naming the bound is worth more than the 0x20.

### It reads the page, which is the whole objection

Ask about a one-cell rectangle and the reply is `-(character + attributes)`. That inverts in one
subtraction. A program can walk the page a cell at a time and recover every character on it, so a
hostile file `cat`ed into the terminal can read back what the commands before it left on screen.
This is a screen readback, and cmote has refused readbacks before.

It is answered anyway, and the reason is a boundary rather than a preference. **Every byte on that
page arrived from the pty the reply goes back down** — the remote wrote it, or the remote's own echo
did. Nothing is learned that the far end did not already have. Contrast OSC 52's read form, refused
outright since §9: the *local* clipboard holds what the user's other applications put there, which
the remote has never seen. One crosses a boundary cmote is standing on; the other does not.

Two properties keep it that way, and both are enforced rather than trusted:

- **the visible page only.** Corners resolve through `area`, which clamps to `screen_lines()`, and no
  spelling of a corner reaches a retired line. A test scrolls text into history and checks the answer
  is the page's, and that a bottom corner of 99 clamps rather than reaches.
- **grid cells and nothing else.** No size, no title, no working directory, no clock — nothing about
  cmote or the machine it runs on enters the number. It repeats what the remote already said.

### A query in a scanner full of commands

DECRQCRA shares its `*` intermediate with DECSACE and differs by one final byte, so it belongs in
`term/rect.rs` rather than with §33's answerers, whatever the plan said for two sections. Three
things followed from putting it there, and all three were the point:

- **The split gives it correct timing for free.** Its offset is one past the final byte like the
  rest, but for a second reason: it *reads*, so it must answer from the page as it stood where the
  question sat. A chunk carrying `AB`, the query, and then `ZZ` over the top reports the checksum of
  `AB`. A test pins that with the value the wrong answer would have had.
- **The reply goes into the engine's own buffer**, pushed at the interruption point, so a DSR and a checksum
  asked for in one write come back in the order they were asked — no second reply path to keep in
  step with the first.
- **Origin mode costs it the rectangle and not the reply.** The rest of the family is refused
  outright under DECOM, because the corners would be region-relative and the engine keeps its
  scrolling region private. A query cannot take that exit: a program that gets no answer waits on a
  terminal that has already moved on (§33). It answers `0000` — the checksum of no cells, which is
  what it could actually reach.

The grammar has one trap worth naming: **the corners start at parameter 2**, since `Pid` and `Pp`
come first. `Pp` is then ignored, as DECCRA's two are and for the same reason — which also settles
DEC's "`Pp` = 0 means all of page memory", because with one page the page is all of them.

### What it cost

One scanner arm, a nine-line accumulator, one reply formatter, one method reading the grid, and 19
tests. The negation, the trim, the exempt first cell, the sixteen-bit wrap, the four hex digits, the
attribute weights, the protection weight, the split timing, the scrollback clamp, the ignored extent
and the origin-mode reply each have one. Three load-bearing parts were checked by breaking them:
dropping the negation fails ten tests, removing the trim fails three, and letting origin mode swallow
the query fails the one that exists to catch it.

### Not done

- **`CSI Ps # y` (XTCHECKSUM)**, the sequence that selects xterm's extension bits, is not implemented.
  cmote computes the DEC-compatible default and only that; a program that asks for a different
  variant gets the default anyway, which is better than a mode nothing honours.
- **The never-written cell** stays indistinguishable from a written blank. Fixing it means a
  sixteenth flag bit, and bit 15 is already cmote's (§56) — so it would mean shadow state beside the
  grid, which is the shape §56 turned down.
- **`term/mod.rs` is ~2930 lines.** The note from §59 stands unchanged, and is now a little louder.

### The audit that followed

With the last CSI row closed, the OSC and CSI tables of `TERMINAL_COMPATIBILITY_PLAN.md` part 8 were swept
against `vte-0.15.0`'s dispatch arms and `alacritty_terminal-0.26.0`'s `Handler` impl — the question
being *which trait methods are left at their empty default*, which is the only reading that catches a
sequence the engine parses and then silently drops. Six rows disagreed with the code, and the doc now
records each with the row that was wrong:

- **Three worked and were not written down.** OSC 50 (`CursorShape=`, a third spelling landing in the
  same `cursor_style.shape` DECSCUSR uses), OSC 9;9 (the Windows cwd spelling — missing from a Windows
  client's own table) and the ANSI form of DECRQM (`CSI Ps $ p`, not only the `?` one).
- **One row contradicted another.** iTerm's `ReportCellSize` was refused as redundant to `CSI 16t`,
  which this same document records as unanswered three tables further down. The honest argument is
  `14t` ÷ `18t`.
- **Two called a refusal *policy*** when the engine dropped the sequence first — OSC 22 and
  XTPUSHCOLORS / XTPOPCOLORS. True as a stance, wrong about who performs it, and worth correcting
  precisely because §57 is a section about that difference.

The one real gap: **`CSI ? 4 m`** (XTQMODKEYS) is a query nothing answers. `vte` dispatches it to
`report_modify_other_keys`, the engine leaves the default empty, `term/query.rs` does not cover it and
`term/modkeys.rs` reads only the set form — so a program that asks waits out its timeout, the exact
failure §33 exists to prevent. Closed in **§61**, below.

---

## §61 — Answering the one question the audit found (v4.0.0)

§60's audit turned up exactly one thing that was work rather than wording:

```
CSI ? 4 m          XTQMODKEYS — "what modifyOtherKeys level are you at?"
CSI > 4 ; Pv m     the answer
```

`vte` dispatches it to `report_modify_other_keys`; the body of that method in the `Handler` trait is
empty and `alacritty_terminal` never overrides it. So the question was parsed, dropped, and the
program that asked sat waiting for a reply that was never coming — §33's founding complaint, hiding in
a sequence §33 never listed.

### The module that holds the state is the module that answers

The obvious home was `term/query.rs`, beside XTVERSION and DECRQSS and the rest of §33's answerers.
It went in `term/modkeys.rs` instead, for the reason DECSACE went in `term/rect.rs` (§59) and the
checksum went with it (§60): **the level lives there**, and that scanner is the one thing in cmote
that sees the sets and the questions in the order the stream put them. Answer from `query.rs` and the
reply would have to read the level after the whole chunk was scanned; answer from `modkeys.rs` and it
is read where the question sat. A chunk carrying `CSI > 4 ; 2 m` then the question reports 2; one
carrying the question then the set reports 0. Both are what a terminal reading in order would say, and
a test asserts both in one write — the load-bearing half, proved by deferring the answer to the end of
`feed` and watching that test fail.

### The answer is the order, said back

xterm replies to XTQMODKEYS with an XTMODKEYS control — the *set* form, not a bespoke report. That is
worth copying and worth understanding: what comes back is exactly the sequence that would put the
terminal into the state it is in, so a program can save the reply and write it back verbatim on the way
out, without knowing what any of it means.

It is also what settles the scope question. XTMODKEYS carries **seven** resources —
`modifyKeyboard`, `modifyCursorKeys`, `modifyFunctionKeys`, `modifyKeypadKeys`, `modifyOtherKeys`,
`modifyModifierKeys`, `modifySpecialKeys` — and cmote holds state for one. Because the reply *is* a set
control, there is no spelling of "I do not have that resource": an answer for resource 1 would be cmote
asserting a level for a knob `keymap.rs` does not have, which a program could then act on. So the other
six draw **silence**, which is the third time in three sections that an invented number lost to a
missing one. In practice they are not asked; resource 4 is the one editors probe.

### The cost of the `?` marker

The scanner used to open its parameter run on `>` alone. It now opens on `?` too — which means every
DECSET and DECRST in the stream, by far the most common private CSI there is, enters the run, buffers a
few digits and is abandoned on its `h` or `l`. That is the same toll `>` already paid, and a test pins
that a page of `\x1b[?1049h\x1b[?25l\x1b[?2004h` draws no reply and does not disturb the level. A
second parameter drops the sequence outright (§54's rule): XTQMODKEYS takes one, so `CSI ? 4 ; 1 m` is
not the sequence it looks like.

### What it cost

One enum, one field, one method, one line in `process`, and 9 tests. Both rules were checked by
breaking them: deferring the answer to the end of the chunk fails the stream-order test, and answering
every resource fails the two that exist to stop it.

### Not done

- **The other six XTMODKEYS resources** stay unanswered, and that is a decision rather than a deferral
  — it reverses only if cmote grows a real resource to report.
- **Ordering against the engine's own replies** is still approximate, as it is for every §33 answer:
  cmote's replies are appended after whatever the engine wrote for the chunk. A write that queries the
  cursor position and then the modifier level gets both, in engine-then-cmote order rather than stream
  order. No program has been seen to care, and fixing it means the split machinery §58 built.


## §62 — A refusal that nothing performs (v4.0.0)

The compatibility matrix had one column doing two jobs. A **❌** meant "a program cannot use this", and
a parenthetical *(policy)* in the note said whether that was a gap or a decision. Worse, the two rows
§60 had corrected carried *(policy, and free)* — a second footnote, on the footnote, marking the case
where cmote agrees with a refusal it does not carry out. Everything a reader needed in order to trust
the row was in the prose, and the column said only "no".

So the refusals took marks of their own: **🛑** where cmote's own code refuses, **🤷** where nothing
does. The second one is the interesting half. It reads as a shrug because that is the honest posture:
the sequence dies upstream — no `vte` dispatch arm, or a `Handler` method `alacritty_terminal` leaves at
its empty default body — so cmote is never offered it, pays nothing to refuse it, and has no test
pinning the refusal because there is no code to pin. It is a stance, not a guarantee.

### The split had to be re-derived, and that is what found things

A mark in a status column reads as *verified*, which meant every refusal had to be checked against the
crates rather than inherited from the note beside it. Eighteen rows, the same method §60 used, and two
of them turned out wrong in exactly the §60 way — a refusal the document credited to cmote that cmote
does not perform:

- **`CSI 1–10 t`** — iconify, move, resize, raise, maximize, fullscreen — read as cmote holding its own
  window against a remote. In fact `vte`'s `('t', [])` arm handles **14 / 18 / 22 / 23** and sends every
  other parameter to `unhandled!()`. There is no `Handler` method for window manipulation to leave at a
  default; the sequence has nowhere to go at all.
- **ENQ answerback** read the same way, and it *is* a decision (§36 argued it at length: a lone `0x05`
  in binary output would type a string into the shell). But `vte`'s `execute` matches HT / BS / CR / LF
  / VT / FF / BEL / SUB / SI / SO and drops `0x05` to a `debug!`. What refuses answerback is that nobody
  ever wrote the reply.

Both were true as stances and wrong about who was doing the work — the failure shape §57 exists to name,
turning up twice more the moment the column was made to state it.

### One row got harder, not easier

The split also cut the other way, and this is the part worth keeping. **OSC 52 write** is the single
refusal in the matrix that the engine actively hands over. `alacritty_terminal`'s `config.osc52`
defaults to `Osc52::OnlyCopy` — documented upstream as *"a compromise between entirely disabling it (the
most secure) and allowing paste"*, which is not a refusal — and cmote never sets it. So a remote's
clipboard write is parsed, base64 and all, raised as `Event::ClipboardStore`, and stopped by the
**catch-all arm** of `Replies::send_event`.

That works, and has always worked. But it is the weakest 🛑 on the page: an inherited default plus a
fall-through, where the iTerm keys next to it have a named arm and a `refuses_*` test each. The read
direction is refused by the same inherited default, one line above cmote's code rather than in it.
Setting `osc52: Osc52::Disabled` explicitly would move both refusals to the engine boundary and cost one
line — recorded in §7 of the compatibility document rather than done here, because this section changed
no code.

### Why a mark and not a better sentence

The counter-argument was real: the ❌/🛑 split is about *support*, the 🛑/🤷 split is about *mechanism*,
and mixing axes in one column is how the six wrong rows of §60 happened. What settles it is who reads
which. A note is read by someone already suspicious of the row; a column is read by everyone, in one
pass, without deciding to. The mechanism is the thing this document keeps getting wrong, so it belongs
where it cannot be skipped — and the notes now spell out, on every one of the eighteen rows, either the
code that performs the refusal or the exact place the sequence dies.

### What it cost

No code, no tests: a documentation section. Eighteen rows re-derived from `vte-0.15.0` and
`alacritty_terminal-0.26.0`, eleven 🛑 and seven 🤷, plus the legend, §6's heading, §7's new hardening
item and the §8 closing prose.

### Not done

- **`osc52: Osc52::Disabled`** was not set when this section was written — the one security item the
  pass turned up, waiting on a word rather than on a design. **§63 set it**, and the word came in the
  same breath as reading this list.
- **Images and DECSLRM keep plain ❌.** Both are refusals, but each is also a real cost — a PNG/JPEG
  decoder dependency (§41), a 71-method delegating wrapper that degrades silently on an engine bump
  (§5). A 🛑 would claim the price is not part of the reason, and it is.
  **Half of that stopped being true in §53 and was corrected in §70.** The decoder dependency was taken,
  for the file preview, so the price on iTerm2's `File=` had already been paid and the row is a 🛑 that
  `term/iterm.rs` performs twice over. Kitty graphics keeps ❌ but for the protocol rather than the
  parser, and DECSLRM's price is untouched — the reasoning above stands, it was the invoice that moved.
- **The other six XTMODKEYS resources** (§61) are a refusal with no mark at all, having no row.


## §63 — Saying the refusal out loud (v4.0.0)

§62 split the matrix's refusals into **🛑** (cmote refuses it) and **🤷** (nothing does), and the one
row that would not sit still under that question was **OSC 52**, the remote clipboard — the oldest
refusal in the project and, it turned out, the least stated.

### What was actually there

`alacritty_terminal`'s `Config` carries an `osc52` field. cmote never set it, so it sat at the crate's
default:

```rust
/// This option is the default as a compromise between entirely
/// disabling it (the most secure) and allowing `paste` (the less secure).
#[default]
OnlyCopy,
```

Under `OnlyCopy` the *write* direction is **allowed at the engine**. A remote's `OSC 52 ; c ; <base64>`
was parsed whole and raised as an `Event::ClipboardStore`, and the single thing standing between that
and the user's clipboard was the last arm of `Replies::send_event`:

```rust
// Everything else — the clipboard pair, the bell, a colour *set* — needs no reply
// and carries nothing we surface, so it is dropped.
_ => {}
```

Which works. It has always worked, cmote has never touched the clipboard on a remote's word, and no
behaviour changed in this section. What was wrong is subtler and worth naming: **a fall-through cannot
say "refused".** The arm drops the event because it drops everything it does not recognise, so nothing
in the code asserts a decision, nothing fails if a later edit starts handling `ClipboardStore` for some
well-meant reason, and the comment is the only place the reasoning lives. Next to it in the same
document sit the iTerm 1337 keys, each with a named arm and a `refuses_*` test — the contrast is what
made this row look thin.

The read direction was thinner still. `OnlyCopy` refuses `clipboard_load`, so the read *was* stopped at
the boundary — but as a **side effect of allowing the write**. cmote's strongest refusal, the one §6
argues hardest (a remote must not learn what the user's other applications put on the clipboard, which
it has never seen), was being performed by an upstream compromise's spare half.

### One field, and a function to put it in

```rust
osc52: Osc52::Disabled,
```

`Disabled` makes both handlers return inside the engine, before an event exists. The catch-all stays,
now as the second line rather than the only one: if an engine bump changed that field's meaning, or a
`Config` edit dropped it, the events would arrive again and would still be discarded.

The `Config` itself moved out of `Terminal::new` into a named `engine_config()`, and that move is the
point rather than tidying. A literal inside a constructor cannot be asserted; a function can:

```rust
assert_eq!(engine_config().osc52, Osc52::Disabled);
```

That is the test the section exists for, and **deleting the field is how it was checked** — with the
field gone the assertion fails, which is the intended tripwire. The same run proved the limit of the
other test: `a_remote_clipboard_request_draws_no_reply` feeds both directions on the wire and still
passed with the field removed, because at cmote's own boundary `Disabled` and `OnlyCopy`-plus-catch-all
are indistinguishable — the event is dropped either way, and no reply appears either way. That is
exactly why the guarantee had to be pinned on the *field* and not on the behaviour. A behaviour test
here can only confirm the outcome; it cannot see which of two mechanisms produced it.

The kitty-keyboard flag beside it got the same treatment for free (`kitty_keyboard: true`, §25). It is
the other place where cmote overrides a crate default on purpose, and turning it off would fail
silently — `keymap`/`kitty` would go on encoding `CSI u` reports from a flag stack the engine no longer
maintains, and programs would simply never be told the protocol is available.

### What it cost

One field, one extracted function, a doc comment on `Replies` saying the pair no longer arrives, and 3
tests. `term/mod.rs` is ~3020 lines. No behaviour changed, which is the honest summary: this section
moved a decision from a comment into code that can fail.

### Not done

- **The bell and the colour sets still ride the catch-all.** Both are refusals §6 argues for, and
  neither has a field to state them in — the engine has no `bell` switch, and a colour *set* is refused
  by `ui/grid.rs` painting from `palette` alone rather than by anything declining it. Structural rather
  than stated, and the structure is load-bearing enough that a test would have to assert a rendering
  fact instead of a config one.
- **`Osc52::Disabled` is asserted, not proven end to end.** The proof that a remote cannot reach the
  clipboard is the engine's early return plus cmote never calling a clipboard API on a remote's behalf;
  no test can watch the Windows clipboard from inside the suite.

## §64 — The mark that hid two answers (v4.0.0)

§62 gave the matrix's refusals marks of their own and §63 hardened the one refusal that turned out to be
inherited rather than stated. Both passes only ever looked at ❌ rows. **§64 asked the same question of
the partial rows**, starting with the three left in §8's OSC table — and a partial turns out to be the easiest
mark to leave alone, precisely because it admits up front that something is missing. It draws none of the
suspicion a ❌ or a 🛑 does.

### One mark over two opposite answers

`OSC 4` (palette entry) and `OSC 10 / 11 / 12` (default foreground / background / cursor) each do two
unrelated things, and cmote's answer to the two is opposite:

- the **query** is answered, in full, and accurately: `Replies::send_event` catches
  `Event::ColorRequest`, `report_color` resolves the slot through `palette::xterm_256` or
  `DEFAULT_FG` / `DEFAULT_BG` — the same const table `ui/grid.rs` paints from — and the engine's closure
  formats the reply. Three tests already pinned it, one per role. This is the half that matters in
  practice: `OSC 11 ?` is how a program picks a light or dark colourscheme, and cmote's answer is exactly
  what the screen shows.
- the **set** is refused, for the reason §6 gives: the theme is chrome the *user* chose. `set_color`
  records the value in `Term::colors`, and **nothing in `src/` ever reads that table** — grep returns no
  hit. The renderer cannot read it even in principle: the style resolver in `ui/grid.rs` takes a cell and
  a const table, and is never handed a terminal to ask.

One mark averaged those into "partial", which tells a reader neither. **Each is now two rows** — `4
(query)` ✅ beside `4 (set)` 🛑, and the same for `10 / 11 / 12` — following the split `OSC 52 (write)` /
`(read)` has carried since §62. A composite mark in one cell (`query ✅ / set 🛑`) was the first attempt
and is worse: the status column exists to be read down in one pass, and a cell holding two marks makes
the reader stop and parse prose to find out which applies to the thing they came for. Two rows cost one
line and answer at a glance.

The split also makes an inconsistency impossible to miss rather than merely visible: `4 (set)` and **row
104** are now adjacent-in-kind — identical mechanism, identical mark — where before one was a 🛑 and the
other was hidden inside a partial, only because it happened to have a query half attached to it.

### A cost this document invented

The `OSC 10 / 11 / 12` note justified the refusal with a price: *"**set** recorded by the engine and
never read — a full repaint for no change"*, and §6 said the same at more length. It is not true.

```rust
fn set_color(&mut self, index: usize, color: Rgb) {
    // Damage terminal if the color changed and it's not the cursor.
    if index != NamedColor::Cursor as usize && self.colors[index] != Some(color) {
        self.mark_fully_damaged();
    }
    self.colors[index] = Some(color);
}

fn mark_fully_damaged(&mut self) {
    self.damage.full = true;
}
```

One bool. And cmote calls neither `damage()` nor `reset_damage()` anywhere, so that bool is written and
never read — it will simply stay `true` for the life of the terminal, harming nothing. A repaint does
happen when a colour set arrives, for the same reason it happens for any output: bytes arrived. The
marginal cost of the refusal is the bool.

This is **§60's failure with the signs reversed**. There, the mechanism was invented and the outcome was
right — rows credited to cmote's policy that the engine had already dropped. Here the mechanism was
right and the *reason* was invented. Both come from the same habit: writing down what a sequence ought to
cost rather than reading what it does. A refusal this cheap needs no cost argument at all, and giving it
one made a policy look like a performance trade.

### Two tests, and why they assert the set landed

```rust
let mut terminal = Terminal::new(10, 40);
assert!(terminal.process(b"\x1b]4;3;rgb:ff/00/00\x07").is_empty());
assert!(terminal.term.colors()[3].is_some());
assert_eq!(
	terminal.process(b"\x1b]4;3;?\x07"),
	b"\x1b]4;3;rgb:8080/8080/0000\x07".to_vec()
);
```

The middle line is the point. Without it the test would pass just as happily if `vte` had rejected
`rgb:ff/00/00` outright, or if a future parser change stopped dispatching OSC 4 sets — a green test
proving nothing, which is the failure §63 was about. `Term::colors()` is the crate's public accessor, so
the test can say *the engine stored red* and then *the answer is still yellow* in the same breath.
`a_default_colour_set_does_not_move_the_query_answer` does the same through `OSC 11`, the direction that
matters most, since that is the one a program reads to choose its own contrast.

Checked by breaking it: changing `report_color`'s palette arm to return red fails
`a_palette_colour_set_does_not_move_the_query_answer` on the exact reply bytes. Reverted.

The renderer's half needs no test — there is no route for a remote colour to reach the resolver, which is
a stronger guarantee than an assertion. The *reply* half had no such structure: `report_color` sits in
the same file as the listener that receives the set, and nothing but habit kept the two apart.

### The third partial, checked and left alone

`iTerm 1337 SetUserVar` is the one partial in that table that earns the mark as written — partial **by
design**, one honoured name. Re-read against `term/iterm.rs`: the name is matched whole against
`HONOURED_VAR = b"gitBranch"` before anything is decoded, so there is deliberately no map for a remote to
fill; the value is base64-decoded, UTF-8 checked, control-stripped and cut at `MAX_VALUE_CHARS = 32`
counted in `chars` so a multi-byte branch name cannot panic; and a value that fails to decode returns
`None` rather than `Some(None)`, so rubbish cannot clear a real reading. Every claim in the row holds.
Widening the allow-list was considered and dropped: each new name needs somewhere on the chip, which
already carries the endpoint label, the branch pill, the status dot and the progress bar — and a name
with no reader is a store with no purpose.

### What it cost

Two tests (~30 lines with their comments), plus corrected rows and paragraphs across
`TERMINAL_COMPATIBILITY_PLAN.md` parts 6 and 7, part 8's legend, the two rows themselves — now four — the closing
audit prose, the header state paragraph and the Evidence appendix. 1041 tests. **No behaviour changed and
no answer changed**: the two partial rows became a ✅ and a 🛑 apiece because that is what they had been all
along. The rows just stopped claiming more than they knew, and the cheapest refusal in the project stopped
being described as an expensive one.

### Not done

- **Honouring the sets was considered and refused, twice over.** Fully honouring them means threading a
  per-tab mutable palette into every cell lookup in `ui/grid.rs` (today a pure function of the cell plus
  a const table), building the reset paths so `104 / 110 / 111 / 112` become real, and answering
  precedence against the user's own scheme — for the reward of letting a remote set foreground equal to
  background, or repaint to mimic another host. §6 already says no.
- **Honouring the set in the *reply* only would be worse than either extreme**, and is worth naming so it
  is not proposed later as a compromise: the query would then promise a colour the grid does not paint,
  so a program choosing its contrast from the answer chooses against a background that does not exist.
  The current asymmetry — set ignored, query honest about what is painted — is the useful one.
- **The bell is still the last refusal riding the catch-all alone**, unchanged from §63's list: the
  engine has no field to state it in, and unlike the colour sets there is nothing structural behind it
  either — only the `_ => {}` arm. If any of these ever grows a pin, that is the one.
- **The other partial rows outside the OSC table have not had this treatment** — DECSTR, locking shifts,
  DECRQSS, XTGETTCAP, DECCOLM, blinking cursor, DECSDM, synchronized output, BEL. Each is a partial for
  its own reason and each would need the same read-the-crate pass to say whether its note still holds.

## §65 — Asking the partials which part (v4.0.0)

§64 split the two partial colour rows and left the obvious question hanging: there were eight more partial rows in
the matrix, and none of them had ever been audited the way §60 audited the ❌ and 🛑 ones. This section
audits all ten, and the result is lopsided enough to be worth stating first: **seven were two answers
hiding under one mark, one was a refusal wearing a partial's clothes, and two were honestly partial.**

### Why the partials were the least examined rows

A ❌ invites someone to close it. A 🛑 invites someone to check whether the refusal is real — which is
exactly what §62 and §63 did. A partial invites nothing. It has already admitted that something is missing, so
nobody asks *which part*, and a note of five words ("SO / SI + designation only") is never felt as thin.
That is how the one real finding below sat in plain sight for this long.

### The seven splits

Each is now two rows, following the `(query)` / `(set)` shape §64 settled on:

| Was | Now | What the audit found |
|---|---|---|
| `SetUserVar` partial | `=gitBranch` ✅ + any other name 🛑 | the name is matched whole **before** anything is decoded, so there is no map for a remote to fill — pinned by `only_the_one_honoured_variable_name_is_kept` |
| `CSI ! p` partial | DECSCA part ✅ + the rest ❌ | `vte` has `('p', [b'$'])` and `('p', [b'?', b'$'])` and **no `('p', [b'!'])`**, so a soft reset leaves origin mode, autowrap, the keypad, the scrolling region and the pen untouched. A gap, not a policy |
| locking shifts partial | `SI`/`SO` ✅ + `LS2`/`LS3`/`LS1R`… ❌ | `execute` maps only SI and SO; `esc_dispatch` has no arm for `n`, `o`, `~`, `}`, `\|`. G2/G3 can be designated (`ESC * B`) and never invoked |
| mode 3 partial | side effects ✅ + column resize 🤷 | the engine's `deccolm` clears the grid and region on purpose and declines the resize itself — cmote is never asked, so it is a 🤷, the same shape as `CSI 1–10 t` |
| mode 12 partial | the mode ✅ + the blink 🛑 | engine tracks and DECRQM reports; `CursorShape` carries no blink and cmote runs no animation timer, and `Event::CursorBlinkingChange` hits the catch-all. Both halves of the refusal are cmote's |
| mode 80 partial | behaviour ✅ + the mode 🤷 | `NamedPrivateMode` has no 80, so it is `Unknown(80)` — logged, ignored, and honestly reported `NotSupported` |
| mode 2026 partial | batching ✅ + abort timeout ❌ | see below |

`BEL` was the eighth: not a partial at all, but a refusal. `vte` dispatches it, `alacritty_terminal`
implements it (`bell()` raises `Event::Bell`), and cmote's catch-all drops it — §6's rule, performed by
cmote's own code. Re-marked 🛑, and noted as the last refusal in the document standing on a fall-through
alone: OSC 52 got a field in §63, and the colour sets have a renderer that structurally cannot read them.

`DECRQSS` and `XTGETTCAP` keep their partial mark, and their notes now say what the mark means: every request draws
a valid reply, and one setting (`m`, from the live pen) and two capabilities (`TN`, `Co`) carry data. The
rest answer "not reported" rather than guessing. That is partial in the plain sense — one answer, given
where it can be given truthfully.

### The finding: a remote can hold the screen still

Mode 2026 read *"parser batches; engine mode is a no-op; cmote already atomic"*. All three clauses are
true, and together they are reassuring in a way the row does not deserve.

Synchronized output lives in `vte`'s `Processor`, not in the engine. `BSU` (`CSI ? 2026 h`) starts
buffering the stream; `ESU` flushes the whole buffer through the parser inside one `advance`. So far so
good — the frame really is atomic, which is what the row was celebrating. But a program can crash between
the two, and `vte` plans for that:

```rust
const SYNC_UPDATE_TIMEOUT: Duration = Duration::from_millis(150);

pub fn advance<H>(&mut self, handler: &mut H, bytes: &[u8]) {
    while processed != bytes.len() {
        if self.state.sync_state.timeout.pending_timeout() {
            processed += self.advance_sync(handler, &bytes[processed..]);
        } else { /* normal parse */ }
    }
}
```

The timeout is *checked* on the way in, and never *expires* on its own: `advance_sync` only extends the
buffer and looks for `BSU`/`ESU`. Expiry is the application's job — `Processor::sync_timeout()` returns
the `Instant`, and the application calls `Processor::stop_sync()` once it passes. **cmote calls
neither.** There is no hit for `sync_timeout`, `stop_sync` or `pending_timeout` anywhere in `src/`.

So: a remote sends eight bytes and stops writing. cmote's PTY reader has nothing to read, so `advance` is
never called again, so the buffered bytes are never flushed, so the grid keeps rendering the pre-BSU
state. The screen is frozen until the remote sends `CSI ? 2026 l` or pushes 2 MiB (`SYNC_BUFFER_SIZE`,
whose overflow path does flush).

Worth being precise about the severity, in both directions. Nothing leaks, no state is corrupted, the
session is alive, the user can still type, switch tabs, or disconnect — it is a stuck *picture*, not a
stuck client, and any remote that can do this can already scribble on the screen. But it is a
remote-triggered effect on cmote's own window that outlives the remote's writing, which is the category
§6 spends its whole length refusing, and it is the only one that arrived by omission rather than by
decision.

### Not done

- **The 2026 timeout is not driven yet.** The fix is small and the shape already exists in the codebase:
  subscribe to `iced::window::frames()` while an update is pending — how `SnackbarTick` and `QuitTick`
  are driven — and call `stop_sync` once `sync_timeout()`'s instant passes. §65 was an audit; taking this
  is a change to `term/mod.rs` and `app.rs`, and belongs in its own pass with its own test.
- **`CSI ! p` stays a ❌.** Implementing DECSTR beside the engine would mean cmote reproducing the
  engine's own reset semantics through fed sequences — origin, autowrap, keypad, scrolling region, pen —
  with no way to verify it against what the engine believes. `ESC c` works, and programs that care mostly
  send that.
  **Undone in §72**, which found both halves of this wrong: the fed sequences are a TRANSLATION rather than a
  reimplementation, so nothing gains a second writer, and `xterm-256color` — the terminfo entry cmote asks
  for — opens both `is2` and `rs2` with `\E[!p`, so the programs that care were sending it all along.
- **The other locking shifts stay a ❌ too.** Invoking G2/G3 needs a shift-state cmote does not model, for
  charsets `vte` will not designate anyway (only ASCII and line drawing).
- **`BEL` is still unpinned.** A test at cmote's boundary cannot tell "the event was dropped" from "the
  event never arrived" — the §63 wire-test limit exactly — and there is no config field to assert
  instead. Stated in §6, marked 🛑 in §8, guarded by nothing.
- **No test was added by this section at all.** It moved marks and corrected notes; the one thing it
  found that deserves code is the item above.

## §66 — Retiring a mark (v4.0.0)

§64 and §65 audited the partial rows and split nine of them. Two were left standing as "genuinely partial" —
DECRQSS and XTGETTCAP — and this section asks what they would be if the class did not exist, splits them,
and deletes the mark.

### What the last two really are

Both are protocols cmote answers **completely** and reports **narrowly**. Every DECRQSS request draws a
reply; only the SGR selector carries data. Every XTGETTCAP request draws a reply; only `TN` and
`Co`/`colors` carry values. Nothing hangs, nothing lies.

The interesting part is what the declined half is, once a row has to say. Three candidates:

- **🛑** — a refusal cmote performs. No: nothing decided that a program should not learn the scroll
  region. There is no policy here to point at, and §6 does not mention either sequence.
- **🤷** — refused upstream, nothing offered. No either: these are cmote's own scanners (`term/query.rs`),
  so cmote *is* offered the request and chooses what to answer.
- **❌** — not supported. Yes, and uncomfortably so, because it is work rather than a stance.

Writing the ❌ down is what exposed the thing this section is really about:

```rust
/// A DECRQSS request, reduced to what cmote can answer (§33). `Sgr` is the one setting cmote reads
/// back truthfully … every other setting (cursor shape, scroll margins, conformance level) is
/// `Unsupported`, because cmote either renders it fixed (a block cursor drawn by inverting the
/// cell) or the engine does not expose it.
```

"cmote renders it fixed (a block cursor drawn by inverting the cell)" **stopped being true in §60**, which
found DECSCUSR and OSC 50 both working and the shape drawn from `cursor_style.shape`. The seam has exposed
`Screen::cursor_shape` ever since. So DECSCUSR is answerable from state that already exists; `" q`
(DECSCA) is answerable from the protection bit §56 owns; `r` (DECSTBM) needs one new seam getter over a
region the engine already keeps. Likewise XTGETTCAP's "the wire values are ambiguous" is true of exotic
capabilities and not of `Tc` / `RGB`, the two a shell actually asks about, which cmote supports.

None of that is large and none of it is urgent — a program that gets an honest "no" behaves correctly. The
point is that **it was invisible**, and it was invisible because the row said "partial" and everybody read
that as "fine".

### Why the class had to go rather than be tidied

The pattern across four sections:

- **§60** — six rows wrong about *who* performed a behaviour.
- **§62** — two more, plus the ❌/🛑/🤷 split so a row's mechanism sits in the column.
- **§64** — two rows averaging two opposite answers into one partial, plus a cost invented to justify one half.
- **§65** — seven more of the same, one refusal mismarked as a partial, and a real gap (mode 2026's
  undriven abort timeout) sitting under a note that read as reassurance.

Every one of those is the same failure: **a row making a claim too wide to be wrong.** The partial mark is that failure
promoted to a mark. It says "something here is incomplete" and thereby answers, in advance, the only
question worth asking of a row — *which part, and who decided?* A ❌ invites someone to close it. A 🛑
invites someone to check the refusal is real. A partial invites nothing.

So the matrix now runs on one rule: **one row, one answer, one mechanism.** Four marks, and any row that
wants to say two things becomes two rows — which `OSC 52 (write)` / `(read)` had been doing since §62
without anyone noticing it was a rule.

### What it cost

Four rows where there were two, a four-mark legend, and every mention of the retired class reworded across
the header state paragraph, §6, §7, §8's legend and closing prose. Markdown only — **no code changed and
no answer changed**; the two ❌ halves are inherited work items, not regressions. `PLAN.md`'s own §62-§65
keep the mark in their prose because they are dated history, and history is the one place a retired
notation still reads correctly.

### Not done

- **Neither inherited gap is closed.** DECRQSS's three answerable selectors (`SP q`, `" q`, `r`) and
  XTGETTCAP's `Tc` / `RGB` are named in §7 and left there. Each is small, each needs a test, and neither
  belongs in a section that changed no behaviour.
- **Six ✅ rows still carry a "but only…" clause** — `OSC 0`, `OSC 8`, `ESC ( ) * +`, `r` (DECSTBM),
  `SP q` (DECSCUSR) and `CSI ? 4 m`. By the rule above each is two rows. They were left alone on purpose:
  their second halves are stated in their own notes, which is exactly what a partial never did. Splitting them
  is bookkeeping, and worth doing the next time one of them is touched for a real reason.
- **The rule is not enforced by anything.** Nothing fails if a future row says two things at once — the
  same class of exposure §63 fixed for the clipboard, and unfixable here, since a markdown table has no
  test. The nearest thing to a pin is that the legend now states the rule in the same breath as the marks.

## §67 — What a tick was allowed to mean (v4.0.0)

§66 retired the partial mark and left four. One of them was still loose: **✅** meant "full", which is a
claim about completeness that a row makes and a reader takes on trust. §67 narrows it to **supported**, and
moves the burden into the note: the row says *how much*, and an empty note becomes the strong claim rather
than the lazy one.

### Why "full" was the wrong word

The failure this document keeps finding is a row read generously. §60's six rows were right about *what*
and wrong about *who*. §64's colour rows averaged two answers. §65's partials had never been asked which
part. Each survived because the reader supplied something the row had not said.

"Full" invites exactly that, and it does it in the column where it is least visible. A row reading

    | 5n / 6n | Device status report | ✅ | |

says: this works. Which parameters? Both of the two named, and — as it turns out — not the private
spelling `CSI ? 6 n`, which reaches no arm in `vte` at all. The row was not wrong. It just could not be
wrong, which is the property §62 through §66 spent four sections removing from every other mark.

### The sweep

Reading every ✅ row and asking "supported *how much*" turned up this:

| Row | Mark | What the sweep found |
|---|---|---|
| `1005` (UTF-8 mouse) | ✅ | **no row at all** — engine-tracked, read off `Screen::mouse_encoding`, supported since the mouse shipped and never written down |
| `CSI ? 6 n` (DECXCPR) | ❌ | no row either — `vte` has `('n', [])` and no `('n', [b'?'])` |
| `5n` / `6n` | ✅ ✅ | one row for two different reports, now one each |
| `CSI g` (TBC) | ✅ | had a row and an empty note, which under the new definition claimed all six parameters; it is `0` and `3`, the others hitting `unhandled!()` |

And one row that was right for a reason it did not give: **`ESC % G`** read "engine is always UTF-8",
which is true, but `vte`'s `esc_dispatch` has no `%` arm whatsoever — the sequence reaches nothing. Same
outcome, different mechanism, and the mechanism is the half that tells you `ESC % @` cannot switch back
either.

Seven rows gained the extent they had been leaving to the reader: `I`/`Z` and `ESC H` (whose stops the
engine keeps, eight apart at power-on), DECSCUSR's three shapes, the mouse's three buttons and vertical
wheel (66/67 not encoded), the title stack's single title and 4096-entry cap with the oldest dropped, and
DSR's two reports.

### What it cost

Markdown only. Two new rows, one row split into two, seven notes made explicit, one legend paragraph.
**No code changed and no answer changed** — every one of these sequences behaved this way before the row
admitted it.

One correction inside the section itself, which belongs here rather than in a silent edit: the first pass
added a `CSI g` row and claimed TBC had none, having searched the table for "Clear tab" when the row said
"Tab clear". The row existed, with an empty note. That is the same generous reading this section is about,
committed while writing it — and it is the argument for the sweep rather than against it, since an empty
note is precisely what stops a row from being findable by what it does.

### Not done

- **The bare-✅ rows were spot-checked, not re-derived.** Roughly thirty rows carry ✅ with an empty note
  and now assert completeness by doing so. The obvious hiding places were checked against `vte` — cursor
  movement, erase, insert/delete, the ANSI modes — but "I looked and saw nothing" is weaker than §65's
  reading of every dispatch arm, and the next audit should start there rather than trust this sentence.
- **Six ✅ rows still carry a "but only…" clause** and are two rows each by §66's rule — unchanged from
  that section's list. §67 makes them *legal* (a ✅ with a stated extent is exactly what the mark now
  means) without making them *ideal*.
- **`CSI ? 6 n` is a real ❌ nobody will miss.** Answering it would mean inventing a page number cmote
  does not have, and the standard spelling already works.

## §68 — Paying the rest of the rule (v4.0.0)

§66 set the rule — one row, one answer, one mechanism — and named six ✅ rows that broke it, then left
them alone. The reason given was that they *said* their second half in the note, unlike a partial, which
said nothing. That was true, and it was also exactly the defence every partial row had been making for
five sections.

### The seven pairs

| Row | Split into | The decision it forced |
|---|---|---|
| `OSC 0` | title ✅ + the icon half ❌ | the icon name is dropped by `vte` wherever it is spelled, so `OSC 1`'s row now covers both |
| `OSC 8` | http/https/mailto ✅ + any other scheme 🛑 | `link.rs`'s `ALLOWED_SCHEMES` is an allow-list; the link is still **drawn**, never launched, because the scheme picks which local program the OS starts |
| `ESC ( ) * +` | `B` / `0` ✅ + any other final ❌ | UK, Dutch, Finnish and the rest hit `unhandled!()`; nothing designates them and nothing would draw them |
| `Ps SP q` (DECSCUSR) | shape ✅ + blink 🛑 | `vte` carries the blink (`blinking: id % 2 == 1`) and the engine stores it — cmote's seam drops it, so this is a refusal cmote performs |
| XTSMGRAPHICS | read ✅ + set 🛑 | `graphics_reply` answers a set with `status 3`: the decoder's limits are not a remote's to move |
| `ESC =` (DECKPAM) | the encoded keys ✅ + the numpad digits 🛑 | NumLock is the user's switch; `keymap::encode` leaves the digits outside its guarded branch on purpose |
| `CSI ? 4 m` | resource 4 ✅ + the other six ❌ | the reply is itself an XTMODKEYS control with no way to say "not mine", so silence beats an invented level — honest, and still nothing tracks them |

One row needed no split. **DECSTBM**'s "vertical only" is not a second answer: the horizontal margins are
a different sequence, DECSLRM, with a row of its own since §57. The note points at it now instead of
carrying it.

**The first row in that table did not survive the section.** §69 built `OSC 1` and refused the icon half of
`OSC 0` on its own terms, so the pair is ✅/🛑 today and the ❌ above is what it read on the day of the
split. Left as written, because the point of this section is what splitting a row makes visible, and this
row made it visible faster than any of the other six.

### Why it was worth the edit

Three of the seven second halves are refusals **cmote performs** — an allow-list, a seam that drops a flag
the engine had already stored, a failure status cmote writes itself. Four are gaps nobody had marked.
Before the split, all seven read as the same thing: a clause after a semicolon.

That is the argument for the rule in one line. **A second half left inside a note is a decision nobody has
had to make.** The mark forces someone to choose between ❌, 🛑 and 🤷, and choosing is what turns up the
`ESC % G` sort of finding — a row that was right for a reason it had never given. This document's whole
audit history (§60's six wrong rows, §62's two, §64's two, §65's seven, §66's two, §67's four) is what
happens when a note is allowed to hold what a column should.

### What it cost

Markdown only. Seven rows became fourteen, one row gained a cross-reference, and the legend's closing
paragraph now says the cost is *paid* rather than admitted. **No code changed and no answer changed** —
every one of these fourteen rows describes behaviour that predates the split.

### Not done

- **`OSC 8`'s refused schemes are unpinned.** `link.rs` has tests for the allow-list itself, but nothing
  asserts that the grid still *draws* a refused link rather than hiding it — the visible half of the
  decision. Small, and worth it the next time §24 is touched.
- **The XTMODKEYS six are still a gap, not a plan.** Tracking them means holding six more resources for
  programs that essentially never ask; the ❌ is honest and the work is not obviously worth doing.
- **Rows whose "extent" is about output fidelity were left alone on purpose** — the mouse's three buttons
  and vertical wheel, the kitty protocol's best-effort alternate keys. Nothing a program *sends* gets a
  different answer there; the limit is in what cmote's own encoder produces, which is an extent under
  §67's definition of ✅ rather than a second answer under §66's rule. That line is worth keeping in mind
  the next time someone is tempted to split a row: the test is whether a program can send something and
  get a different answer.

## §69 — The tab has a name now, and OSC 0 does not get to write it (v4.0.0)

§68 split `OSC 0` from `OSC 1` and marked the icon-name half **❌**, on a note that read: the sequence is
"dropped wherever it is spelled", and "nothing is lost: cmote shows no icon name anywhere, so there would
be nowhere to put it".

The first clause was right. The second was a claim about cmote's own UI, written from the sequence's 1980s
meaning — the label X11 put under an *iconified* window, a thing Windows does not have and winit exposes
no API for. What terminals actually settled on is different, and iTerm2 made it the norm: **OSC 2 names
the window, OSC 1 names the tab**, and OSC 0 (the older spelling) means both at once. cmote has had a tab
strip since §26.

So there was somewhere to put it, and the row that said otherwise was the second half of a pair that had
existed for exactly one section. §68's closing argument was that a second half left inside a note is a
decision nobody has had to make; this is the same finding one level down. Once the halves had their own
rows, one of them turned out to be a feature and the other a refusal — and the merged row had been quietly
asserting that neither was true.

### Why it was worth building

`App::Tab::strip_label` returns the **endpoint**. Open two shells on the same host and the two chips read
identically — `user@host` and `user@host` — and the only way to tell them apart is to click one. That is
the exact gap an icon name fills: `vim`, a long build, a tmux window, anything that names *itself*.

The window title could not have covered it. It is one string for the whole window (§48 gives it to
whichever region holds the keyboard), so it says what the focused tab is doing and nothing about the other
five.

### How it is read

`vte` has no OSC 1 arm at all — the code falls through `osc_dispatch` to `_ => unhandled(params)` and is
logged away — so nothing in the engine ever offered it. That is the same shape as the cwd (§17), the
prompt marks (§34), the progress reports (§54) and the 1337 namespace (§55), and it gets the same answer:
a scanner over `term/osc.rs`'s shared `Framer`, which is the fifth one now. `term/icon.rs` is 228 lines,
most of them tests and reasoning.

Two details worth naming:

- **The prefix is matched whole — `1;` and nothing else.** `10;`, `11;`, `12;`, `104;`, `110;`, `112;`
  and `1337;` all begin with a `1`, and the last of those is a namespace cmote genuinely reads. A
  `strip_prefix(b"1")` would have swallowed every one of them.
- **An empty name clears it**, rather than drawing an empty suffix. That is how a program hands the chip
  back when its command exits, and a shell that sets one on `preexec` and clears it on `precmd` gets the
  behaviour a user would expect without cmote guessing at command boundaries.

### The two refusals, which are the same decision from either side

**The name is appended to the chip, never substituted for it.** The label reads `user@host — vim`. This is
§55's rule, already carried by the branch pill and written in its own comment: the endpoint is what says
*which machine this is*, so remote-chosen text must never be readable as the start of it. A remote that
could rename its own chip could dress a staging box as production.

**The icon half of OSC 0 is declined** — and this one is worth the paragraph, because on the face of it it
is free. OSC 0 sets the icon name and the window title to the *same string*, so cmote already holds those
bytes; honouring the icon half costs one `or_else` and no parsing at all. The reason not to is what sends
it. Debian's stock `PS1` carries `\[\e]0;\u@\h:\w\a\]`, which fires OSC 0 **on every prompt of every
session**. Honour the icon half and every chip permanently reads `user@host: ~` — the endpoint that is
already on the chip, plus the directory that is already in the title bar — with no room left for the one
thing an icon name is worth having for.

So the refusal is about noise, not risk, which is precisely why it needed writing down: a later reader
finding `1;` matched and `0;` not would otherwise read it as an oversight and close it.

It is a **🛑** and not a **🤷**. `vte` does drop the icon half, but it drops it by handing cmote the title
and saying nothing about the rest — the bytes are in cmote's hands when the decision is made, and it is
`term::icon`'s prefix match that declines them. §57 is the whole section on that difference.

### What the pins found

The refusal is held by two tests, and the second is the one that carries the argument:
`osc_0_moves_the_title_and_leaves_the_icon_name_alone` asserts the window title **moved** before asserting
the icon name did not. A test that only checked the icon name would pass just as well if `OSC 0` had
stopped being parsed altogether — the same trick §64 used for the colour sets, where the engine is shown
to have *stored* the value before the answer is shown to ignore it. Both were pinned by making `parse`
accept `0;` as well and watching exactly those two fail, and nothing else.

One test was written wrong and the code corrected it. `control_characters_are_stripped_from_the_name`
originally asserted that `ESC [ 31 m` inside a name came out as `[31mb`. It comes out as **nothing**: an
ESC inside an OSC payload either opens the ST terminator or invalidates the sequence, and `Framer`
abandons the payload rather than guessing. The real behaviour is stricter than the one asserted, so the
test now pins *that*, with a second half proving the scanner still works on the sequence after — a
malformed name costs its own sequence and nothing more.

### One rule extracted, because it got a second caller

`iterm.rs` had a private `sanitize` — strip control characters, cap the length in `chars` — with a comment
saying it was the same rule as the window title's, written out again. `icon.rs` needed exactly it. Rather
than a third copy, it moved to `term/osc.rs` as `sanitize(text, max_chars)`, beside the framer, with each
caller naming its own cap because the surfaces genuinely differ (a branch pill has different room from a
tab label).

That module is where it belongs for a reason it already records: `osc.rs` exists **because** the OSC
framing had been copied three times and had already drifted between the copies. This is the same lesson,
one layer up, caught at two copies instead of three. `sanitize_title` in `term/mod.rs` was left where it
is — it has no cap, and giving it one is a decision about the title bar, not a refactor.

### What it cost

One new file (`term/icon.rs`, 228 lines), and small edits to four others: a module declaration, a struct
field, one `feed` call in `process`, one accessor, and eight lines in `strip_label`. Sixteen new tests,
1041 → 1057 in the suite, green on `cargo check --all-targets` / `test` / `clippy -D warnings` / `fmt`.

No engine change, no widget change, no new dependency. The chip's layout is untouched — the name goes
inside the existing label, not into a second pill, because two remote-chosen pills on a chip that already
elides at 48 characters would be a crowding problem dressed up as a feature.

### Not done

- **A full reset (RIS) clears neither the title nor the icon name.** This is not new and it is not the
  icon name's doing: `alacritty_terminal::Term::reset_state` assigns `self.title = None` **directly**,
  without raising the `ResetTitle` event cmote's listener watches, so cmote's copy of the title has
  survived RIS for as long as cmote has had one. The icon name now behaves the same way, which was the
  deliberate choice — a user seeing one survive a reset and the other not would be looking at a bug
  whichever way it had been guessed. Fixing it properly means a flag on the reply buffer and clearing
  both from one place; worth doing, and worth doing to *both*, which is why it is not a footnote to this
  section's feature.
- **`strip_label` has no test.** The append-not-substitute rule lives in `app.rs`, where constructing a
  `Tab` means most of the app; the scanner and the terminal boundary are pinned, the last four lines are
  not. The same gap `OSC 8`'s drawn-but-refused links have (§68), and it wants the same answer: a seam in
  `app.rs` worth testing through.
- **Nothing writes an icon name into the shell hook.** §17's `integration.rs` can install the OSC 7 and
  OSC 133 emitters into a remote's rc file; it could offer the same for a `preexec` that names the running
  command. Deliberately not bundled here — this section is about reading what a remote already sends, and
  a hook that makes remotes send more is a separate decision with a separate consent question.


## §70 — A mark that outlived its argument (v4.0.0)

§69 corrected a row's **answer**. This one corrects a row's **reason**, which is subtler and invisible to
the method every sweep since §60 has used. The row was:

```
| iTerm 1337 File | Inline images | ❌ | a PNG/JPEG payload, so it needs an image-format decoder —
                                        cmote's own images are sixel, which needs none (§5, §41) |
```

Every clause of that was true the day it was written, in §41.

### What changed underneath it

§53 shipped the file preview and took **`image 0.25` as a direct dependency** with five codecs turned on
— PNG, JPEG, GIF, BMP, WebP. §53's own text was careful about the line it was crossing: it says it
*narrows* §41's refusal rather than reversing it, and §41's "What is deliberately NOT here" was amended in
the same pass to say the refusal still holds. What neither noticed is that a row in the compatibility
matrix was still charging for a dependency that had just been paid for elsewhere.

So the row was not wrong about the world. It was wrong about the **price**, and a mark that rests on a
price is only as good as the last time anyone checked the invoice.

### Why nine sweeps did not catch it

§60 through §69 all work the same way, and it is the right way: re-derive each mark from the crates. Read
`vte`'s dispatch arms, ask which `Handler` methods `alacritty_terminal` leaves at their empty default,
mark accordingly. Against the crates this row read correctly — inline images are not supported, and no
amount of re-reading `vte` says otherwise.

The change was in cmote's own `Cargo.toml`, made for a different feature, in a section about opening a
file the user picked. **A note that names a cost has to be re-read whenever that cost is paid somewhere
else**, and nothing in the audit method points at `Cargo.toml`.

### What is left when the price is taken out

Exactly what §41 wrote down in advance, and it never needed the decoder to state:

> The refusal was never "cmote owns no PNG parser" — it is that a REMOTE must not get one run on bytes it
> pushed into the terminal stream unasked.

The difference is **consent, not caps** — which matters precisely because caps are copyable and consent is
not. Every bound §53 put around the preview decoder could be lifted into an `OSC 1337 File=` handler in an
afternoon; none of them would answer the question that actually separates the two:

| | §53's preview | `OSC 1337 File=` |
|---|---|---|
| who chose the bytes | the user pointed at the file | the remote pushed them |
| how many | one, on demand | unbounded, at line rate |
| where the format comes from | the leading bytes, reader pinned to them | whatever the payload says |
| what a bad decode costs | one preview tab | the terminal the user is working in |

There is a second difference the old note never made either. Sixel's decoder is `term/sixel.rs` — six
hundred lines cmote wrote, auditable in this tree, on a format whose payload is printable ASCII. PNG,
JPEG, GIF and WebP are third-party parsers on a path a remote drives. They are pure Rust, so this is not
the memory-unsafety class of problem; panics and decompression bombs are the live one, and `image::Limits`
bounds a decode the user asked for rather than a stream arriving as fast as the link allows.

### And the refusal was already there, twice over

`term/iterm.rs` has declined this key since §55, by two independent mechanisms:

- the **allow-list** `parse` never matches `File`, at any size — the same property that makes a key
  iTerm2 invents tomorrow safe by default;
- **`MAX_PAYLOAD` = 4096** sits deliberately below what a base64 image needs, so a real payload overruns
  the shared framer's cap and is abandoned mid-flight. cmote never holds it.

One test asserts both — `refuses_the_inline_image_key_without_even_buffering_it` feeds the `File=` prefix,
then `MAX_PAYLOAD + 1` bytes, then the terminator, and asserts nothing came out; then it feeds `SetMark`
and asserts the scanner still works, so a flood costs the flood and nothing else.

That is the definition of **🛑** as §54 wrote it: refused by cmote's own code, pinned by cmote's own test.
The row has been in the wrong column since before that column existed.

### The neighbour that did not move

Kitty graphics keeps **❌**, but its note was wrong in both directions and is rewritten:

- **The decoder was never the cost.** Kitty's `f=24` and `f=32` payloads are raw RGB and RGBA — no
  decoder at all. Only `f=100` is PNG. So "its payloads are PNG/RGBA chunks, so it needs an
  image-format decoder" billed for a parser that half the format space does not use.
- **The cost is the protocol**, and it is real: chunked transmission (`m=1` continuations), image ids,
  placements, deletion commands, unicode placeholders, animation. §41's placement, reservation,
  compositing and eviction machinery is protocol-agnostic and would still serve — it is the scanner
  above it that is large.
- **Nothing in cmote refuses it.** Kitty graphics arrives as an **APC** string (`ESC _ G … ESC \`), and
  `vte`'s parser routes `State::SosPmApcString` to `anywhere`, whose match is `0x18 | 0x1A` → execute and
  return to ground, `0x1B` → escape, and `_ => ()`. Every payload byte is dropped without a single
  `Perform` method being called (`vte-0.15.0/src/lib.rs`, :182, :359, :377, :438).

So the two rows genuinely differ, and the old shared note — *"same reason as kitty: a PNG/JPEG payload"* —
was wrong about both of them. One is a decision cmote enforces; the other is a cost cmote has not paid,
and a refusal nothing performs.

### What moved

Documentation only, no code. Two rows change mark — `iTerm 1337 File` in the OSC table and
`iTerm2 inline images (OSC 1337)` in the graphics table, both ❌ → 🛑 — and kitty's note is rewritten in
place. Six prose passages that named the decoder as the reason are corrected: §0's header, §41's paragraph
there, §5's bullet (now **two** bullets, since the three protocols no longer share an answer), §62's
summary, §7's, and §8's "Shape of it". §6 gains an entry for the key, and the `term/iterm.rs` evidence
bullet gains the second mechanism and the test that pins it.

Matrix after: 163 rows, **❌ 32 → 30**, **🛑 21 → 23**, ✅ 101 and 🤷 9 unchanged. Verified mechanically —
no row carrying two marks, no row carrying none.

**A correction to the counts themselves.** The scan used through §69 read field 4 of every table row,
which is the Status column in §8's four-column tables and the **Note** column in the three-column
"Graphics, window ops, keyboard, C0" table — so it silently counted mark glyphs that happened to appear
in prose. The figures quoted in §69 (145 rows, 92 / 26 / 20 / 7) were that script's; the real totals are
163 rows, 101 / 30 / 23 / 9. No count is written into either document, so nothing in the text was wrong —
only numbers quoted in passing. The fixed script reads each table's own header row for the column named
`Status`, which is the only version worth keeping.

### Not done

- **Kitty graphics is ❌ and unpinned**, which is now stated rather than implied. Nothing in cmote
  declines it, so if a future `vte` ever dispatched APC the sequence would arrive with no arm to meet it.
  Same shape as OSC 22 and `kitty 99`, and the same answer would serve: a scanner arm that drops it on
  purpose, or nothing at all, with the row saying which.
- **Half of `File=`'s refusal rides a cap chosen for the namespace**, not for this key. If a later key
  ever needs a bigger payload, raising `MAX_PAYLOAD` would quietly demote this refusal to the allow-list
  alone — still correct, still tested, but one mechanism where there had been two. That wants a comment
  at the constant, not a change to it.
- **There is no sweep for expired reasons.** §70 exists because a reader asked about one row. The check
  is cheap and mechanical — every note that names a dependency or a cost, re-read against `Cargo.toml`
  and the tree — and it is nobody's job yet. It is also the only class of error the crate-by-crate method
  structurally cannot find, which is the argument for making it somebody's.


## §71 — Two refusals with nothing behind them (v4.0.0)

§70 corrected a row whose reason had expired. §71 takes the row directly beneath it, which had been
carrying two keys, one dash and one word:

```
| iTerm 1337 `CursorShape` / `ReportCellSize` | — | ❌ | redundant — … |
```

The word was right. The mark was not, and the row was two rows.

### Two problems, and neither was the word

**It states one answer for two different kinds of sequence.** `CursorShape=N` is a *set*: a remote asks
cmote to change something. `ReportCellSize` is a *query*: a remote asks cmote to say something. §68's
split test applies exactly — a program can send either one alone and get a different kind of nothing back
(one changes nothing, the other says nothing), and the reasons are not the same reason.

**And ❌ was the wrong column**, for the reason §70 had just established one row up. `term/iterm.rs` is an
**allow-list**: every key it does not name is refused by cmote's own code, on bytes cmote framed and is
holding at the moment it declines them. That is the definition of 🛑. The catch-all row two lines below —
`iTerm 1337 (every other key)` — has said 🛑 since §55, so the document was already contradicting itself
about these two keys; it just did it quietly, in two tables that nobody reads side by side.

### Why they are refused rather than built

Both were live options. Neither is expensive.

**`CursorShape=N`** uses the same numbering OSC 50 does — 0 block, 1 beam, 2 underline, confirmed in
`vte`'s `b"50"` arm — and cmote honours *two* spellings of that instruction already: DECSCUSR
(`CSI Ps SP q`) and OSC 50, both dispatched by `vte` onto the engine's single `cursor_style.shape`, which
`term/screen.rs` reads. Taking a third would mean reaching that field from **outside** the engine, since
cmote's scanner has no route in except feeding the engine synthetic bytes the way §41 feeds ECH + LF to
reserve an image's cells. That works, and it is the wrong thing to do here: it would make a second
**source** for one piece of state rather than a second spelling of the first, and two sources for one
field is the arrangement in which they eventually disagree and the renderer has to pick. One field, one
writer, is worth more than a spelling no program needs — a program that wants a bar cursor has two here
that work.

**`ReportCellSize`** is different in kind, because honouring a query means **replying**, and a reply is an
advertisement. cmote is not short of the answer: the GUI sets the cell size through
`Terminal::set_cell_pixels`, and `CSI 14t` is answered by multiplying it by the grid. What makes this
spelling the wrong one to answer is *why it is asked* — in iTerm2 the question is asked in order to size
an **inline image**, which is `File=`, which §70 refuses. Answering precisely and then dropping the
picture is a worse outcome for the sender than silence, because silence is what lets it fall back to a
protocol cmote does draw.

That is the standard this document already holds itself to in two other places. §41 refuses an oversized
sixel outright instead of clipping it, because *"a refusal draws nothing; a clip would silently misreport
what the host sent"*. XTMODKEYS answers only the one resource cmote tracks, because the reply format is
itself an XTMODKEYS control and *"there is no way to answer 'not mine' except by not answering"*. A cell
size handed to a tool whose next move cmote will drop is the same mistake in a third costume.

And it is not a vendor key being singled out: **`CSI 16t` is the standard spelling of the same question**,
and cmote answers that no more than this one.

### What declaring it cost

No behaviour changed — the refusals have been in force since §55. Four tests, 1057 → 1061:

- **`refuses_the_two_keys_that_would_only_repeat_an_answer_cmote_already_gives`** (`term/iterm.rs`) — the
  allow-list, both keys by name, beside the `refuses_*` tests for the dangerous ones.
- **`the_iterm_spelling_of_the_cursor_shape_is_not_honoured`** (`term/screen.rs`) — §64's shape. Asserting
  the shape after the sequence would pass against a terminal that ignored everything, so a spelling cmote
  honours moves the shape first and the refusal is the shape **surviving**.
- **`the_iterm_spelling_of_the_cell_size_question_gets_no_answer`** (`term/mod.rs`) — asserts the silence,
  then answers `CSI 14t` on the same terminal, so the silence is shown to be a decision rather than
  ignorance.
- **`osc_50_is_a_second_spelling_of_the_same_shape`** (`term/screen.rs`), which is not about the refusal
  at all. OSC 50 has been ✅ since §60's audit found it working, and had never had a test. Refusing a
  fourth spelling *on the grounds that two already work* is worth nothing if the two are only assumed to.

### What breaking it showed

Prefix-matching `CursorShape=` in `parse` failed **exactly one** test — the allow-list one — and pointedly
**not** the `screen.rs` boundary test. That is correct, and worth writing down: nothing plumbs a cursor
shape from the scanner, so the boundary test pins *"no effect end to end"* while the allow-list test pins
*"not matched"*. Two different guarantees that should fail for two different reasons, which is what having
both is for.

`ReportCellSize` has no one-line break available, because there is no reply path to break. Its pin is the
other shape instead — assert the silence, then prove the channel works in the same test.

### The mark says who, not how bad

These are the first 🛑 rows in the document with **no danger behind them at all**. Everything else in that
column is there because something would go wrong: a clipboard read, a theme repaint, an effect escaping
the tab, a parser run on pushed bytes. These two are there because nothing would.

That is worth stating, because a column of stop signs reads as a severity ranking and it is not one. **🛑
says cmote's own code performs the refusal and a test checks it. Nothing more.** §69's icon half of
`OSC 0` had already come close — refused for noise rather than risk — and these two go the rest of the
way, refused for redundancy.

Which is exactly why they are pinned by name, and why their reasons live in `term/iterm.rs`'s header
rather than only in the compatibility document. A refusal with a threat behind it defends itself. A
refusal whose whole reason is *"we already answer this"* is the one a later reader deletes as a courtesy
to a program that did not need the favour.

### Not done

- **`CSI 16t` is a gap, and is now named as one in its row.** cmote holds the cell size and does not
  answer the standard question either — `vte`'s `('t', [])` arm handles 14 / 18 / 22 / 23 and sends the
  rest to `unhandled!()`, so the sequence dies in the parser. Closing it would be a scanner shaped like
  `term/modkeys.rs` plus a reply, and it is genuinely unclear it is worth paying for: `CSI 14t` ÷
  `CSI 18t` gets a program the same number, and that is the pair sixel tools already send. Named so the
  next reader decides it instead of inheriting it.
- **A fifth mark was considered and refused.** The 🛑 column now mixes *"a remote must not read your
  clipboard"* with *"we already answer this twice"*, and a reader sorting by mark gets both. Splitting it
  would mean a new class to keep consistent across two rows, in a document that spent §66 through §68
  *removing* marks for exactly that reason. The notes carry the difference; this records that the choice
  was made rather than missed.
- **The synthetic-bytes route makes a second source easy to add by accident.** §41 feeds the engine ECH +
  LF as if the remote had sent them, which is the right tool for reserving cells and would also be the
  mechanism by which some future key quietly became a second writer of engine state. The argument in this
  section is about *sources*, not about this key — worth remembering the next time a scanner needs the
  engine to change its mind.

## §72 — The reset that reached nothing (v4.0.0)

§65 audited the partial rows and left `CSI ! p` — DECSTR, the soft reset — split into a ✅ for the DECSCA
bit and a ❌ for everything else, with a reason attached:

> **`CSI ! p` stays a ❌.** Implementing DECSTR beside the engine would mean cmote reproducing the
> engine's own reset semantics through fed sequences — origin, autowrap, keypad, scrolling region, pen —
> with no way to verify it against what the engine believes. `ESC c` works, and programs that care mostly
> send that.

Three sections walked past that paragraph. This one reads it again, and finds it wrong twice — once about
the work, once about the sequence.

### Wrong about the work

"Reproducing the engine's own reset semantics" is not what happens. Every item on DEC's DECSTR list is a
mode, a region, a pen or a character set that the engine takes an **ordinary sequence** for. So cmote does
not reproduce anything: it feeds the engine the long spelling of the reset and lets the engine do it.

```
CSI 0 m       the pen back to default (SGR)
CSI ? 25 h    cursor visible (DECTCEM)
CSI 4 l       replace rather than insert (IRM)
CSI ? 6 l     absolute origin (DECOM)
CSI ? 7 h     autowrap (DECAWM)
CSI ? 1 l     normal cursor keys (DECCKM)
ESC >         numeric keypad (DECNKM)
ESC ( B …     G0-G3 all ASCII, then SI to make G0 the active set
CSI r         the scrolling region back to the whole page (DECSTBM)
CSI H         home, so the save below is of the corner
ESC 7         the SAVED cursor to home, carrying the pen just reset (DECSC)
```

That is the whole implementation — a constant, plus the CUP that puts the real cursor back. The engine
stays the only writer of its own state, which is §71's argument used the *other* way round: that section
refused a fourth spelling of the cursor shape because honouring it would have needed a second **source**
for one field, and this one is safe for exactly the reason that one was not. There is no new source here.
There is one new *spelling*, and it resolves into the spellings that already existed.

DECSCA, the eleventh item, was already cmote's and stays where it was. The rest of DEC's published list —
KAM, DECNRCM, DECAUPSS, DECSASD, DECKPM, DECRLM, DECPCTERM — names state that neither `vte`, nor the
engine, nor cmote models at all, so there is nothing left stale by not sending it. Seven items that cost
nothing because the terminal they belong to is not the one being emulated.

And "no way to verify it against what the engine believes" had already stopped being true. Every item is
observable from outside: DECRQM answers `?1 / ?6 / ?7 / ?25` and `4` straight out of the engine's own
`TermMode`, DECRQSS `m` rebuilds the pen cmote paints with, the scrolling region shows through origin
mode, the charset shows in a printed glyph, and the keypad has a seam `app.rs` already reads. The tests
below check the engine's beliefs, not the bytes that were fed.

### Wrong about the sequence

"Programs that care mostly send `ESC c`" is the part that should have been checked rather than assumed.
cmote asks the remote for `TERM=xterm-256color` (`ssh/client.rs`, `ssh/shell.rs`) and answers XTGETTCAP
`TN` with the same name. Here is what that terminfo entry says, read locally:

```
$ infocmp -1 xterm-256color
        is2=\E[!p\E[?3;4l\E[4l\E>,
        rs1=\Ec\E]104\007,
        rs2=\E[!p\E[?3;4l\E[4l\E>,
```

`is2` is the initialisation string and `rs2` the reset string, and **both open with `\E[!p`**. Every
`tput init`, every `reset`, every ncurses program's startup was sending cmote a soft reset it dropped on
the floor. RIS is `rs1`, one line up — and `reset` runs `rs1` then `rs2`, so it happened to work, which is
how a gap this well-trafficked stayed invisible.

The failure it caused is the classic one. A full-screen program dies leaving a scrolling region set and
origin mode on; the user types `reset` or the shell's prompt hook runs `tput init`; on any other terminal
the screen comes back, and on cmote it stayed broken. The row said "a **gap**, not a policy — nothing here
refuses it", which was accurate and made it sound theoretical.

### Two departures from DEC, both deliberate

**Autowrap goes back ON, where the VT510 manual says a soft reset turns it off.** The manual describes
hardware nobody is emulating. `xterm-256color` declares `am` — this terminal wraps — and its `rs2` sends
this sequence *without* a following `\E[?7h`, so on the terminal cmote claims to be, a soft reset cannot
be what leaves wrapping off, or `tput init` would break every program that ran it. Power-on default is the
honest reading, and the engine's own power-on default (`TermMode::default()`) has `LINE_WRAP` in it.

**The cursor is put back where the reset found it.** DECSTR does not move the cursor. But the engine's
`set_scrolling_region` ends in `goto(0, 0)` — right for DECSTBM, which is defined to home the cursor, and
wrong for a reset that has to borrow DECSTBM to clear the region. So the position is read before anything
moves and restored with CUP once origin mode is off and coordinates are absolute again. The one thing that
does not survive the round trip is the pending-wrap flag, which a reset has no business preserving.

Both are in the `soft_reset` header rather than only here, because both look like bugs to a reader who
knows the manual and not the reasoning.

### What it cost

One enum variant (`protect::ProtectRequest::SoftReset`), ~20 lines of code under ~60 lines of comment in
`term/mod.rs`, and eleven tests. 1061 → 1072, green on `cargo check --all-targets` / `test` /
`clippy -D warnings` / `fmt`.

The scanner needed nothing new. `term/protect.rs` has matched `CSI ! p` since §56 — it clears the pen, so
it cleared DECSCA — at exactly the right offset, one past the final byte. What changed is that it now
reports the whole reset instead of the smallest part of it. Writing a second scanner for the same sequence
in a module of its own was the alternative, and would have meant two readers of one sequence, eventually
disagreeing about what it was.

Three of the eleven tests are about telling this sequence from its neighbours rather than about the reset:
DECRQM is `CSI Ps $ p` and `CSI ? Ps $ p`, the two arms `vte` **does** have for this final byte, so a
scanner matching on `p` alone would soft-reset the terminal every time a program asked what a mode was set
to. `a_mode_request_is_not_read_as_a_soft_reset` asks twice and checks the answer did not change.

### What breaking it showed

Dropping `CSI r` from the fed string failed **two** tests, not one: the scrolling-region test, as
intended — and the saved-cursor test, which had no business depending on it. `ESC 7` was landing on home
only because `set_scrolling_region` had homed the cursor a few bytes earlier. One item's correctness was
resting on another item's side effect, in a string where the whole point is that each line answers for one
line of DEC's list.

Fixed by sending `CSI H` before `ESC 7` — three bytes to make the dependency explicit rather than
inherited. The break test found it; reading the string had not, twice.

### The fourth way in

§56 named three ways to handle something the engine drops: scan it out and keep the answer beside the grid,
accept the engine's limit, or borrow a bit and let the engine carry it. This is the fourth: **translate
it**. Where the missing sequence is a *shorthand* for things the engine already takes, the work is a
lookup table rather than an implementation, and nothing gains a second writer.

§41 had already done this once — a picture's cells are reserved by feeding ECH and LF as if the remote had
sent them — and §71's last "Not done" bullet flagged the route as a way to acquire a second source of
engine state by accident. Both are right, and the difference is worth stating precisely: feeding is safe
when it *translates* a sequence into the engine's own vocabulary, and dangerous when it *originates* state
the engine did not ask for. DECSTR is the first case. A hypothetical `CursorShape=` handler would have
been the second.

It also does not quietly empty the ❌ column, because it only works where there is something to translate
into. Left-right margins, kitty graphics and blink have no shorthand relationship to anything the engine
does — they are capabilities, and §5 still costs them out as such.

### Not done

- **DECSACE is deliberately not reset**, and the row now says so as a live decision rather than a moot
  one. DEC's published DECSTR list does not name it, RIS does reset it (§59), and the temptation to
  "finish the job" by adding it is exactly the kind of small invention that makes a terminal's behaviour
  unpredictable for the program that read the manual.
- **The cursor style (DECSCUSR) is not reset either**, for the same reason: not on the list. xterm's own
  behaviour here is not something this section verified, and guessing at it would be worse than following
  the document both ends can read.
- **The fed string is not exercised under synchronized output.** If a remote opens `BSU` (mode 2026) and
  then sends `CSI ! p`, the fed bytes join the parser's sync buffer like any others — which is almost
  certainly right, and is the same exposure `reserve_cells` has had since §41. It is untested in both
  places, and it is really the mode 2026 timeout item (§65) wearing a different hat.
- **`CSI 16t` is still a gap**, unchanged from §71's list — named there, decided by nobody yet.

## §73 — The refusal that was not in the column (v4.0.0)

The question put to this section was narrow: should DECSLRM read ✅ or 🛑? The interesting part is
what the question left out. The row read

```
| s (DECSLRM) | Left / right margins | ❌ **safely** | …
```

and ❌ was not on the menu, correctly. §66 retired the partial mark because a row that says two things
cannot be checked; a mark with an adverb propped beside it says two things in a smaller space. "Safely"
is there because the mark alone was wrong and the note knew it.

### Not ✅, for two reasons — one of them new

**There is nothing to translate into.** §72 worked because a soft reset is a *shorthand*: every item on
DEC's list is a mode, a region, a pen or a charset the engine already takes an ordinary sequence for, so
cmote feeds the engine the long spelling and the engine stays the only writer of its own state. Margins
are a capability. Building them means the delegating `Handler` wrapper §5 costs out, and that wrapper
wraps lines, moves the cursor and scrolls a column band *itself* — cmote becomes a second writer of
engine state, which is precisely what §71 argued against and what §72 was built to avoid. The hazard is
also unguardable: all 71 `Handler` methods have default empty bodies, so a method left unforwarded, or
one a future `alacritty_terminal` adds, compiles clean and silently swallows a sequence. §57's borrowed
flag bit gets a `const` assertion at build time. This gets nothing.

**And §5's traffic claim was wrong.** It read "essentially nothing emits DECSLRM outside a conformance
suite". §72's lesson is that a reason can be wrong on the facts and stay unread for sections, so it was
checked rather than repeated. `xterm-256color` — the TERM cmote asks for — declares all four:

```
mgc=\E[?69l,
smglp=\E[?69h\E[%i%p1%ds,
smglr=\E[?69h\E[%i%p1%d;%p2%ds,
smgrp=\E[?69h\E[%i;%p1%ds,
```

That is the same shape as §72's finding, which is why the difference in the answer is worth writing
down. What made §72 real was *where the string sits*:

```
is2=\E[!p\E[?3;4l\E[4l\E>,
rs2=\E[!p\E[?3;4l\E[4l\E>,
```

`\E[!p` is in both, so every `tput init`, every `reset` and every ncurses startup sent it unasked. No
margin capability appears in any init or reset string. Those four go out only when an application
deliberately decides to use margins, and ncurses' own rendering never does. **Declared is not emitted.**
The conclusion survives; the sentence holding it up did not.

### 🛑, and the legend already said so

The legend defines ❌ as *a sequence that could still land*. This one cannot. `term/cancel.rs` cancels
the final byte in flight and feeds the parser its own CAN, `process` splits the advance to do it,
fifteen tests in the scanner pin the shapes and `a_cancelled_margin_request_prints_nothing_at_all` pins
it end to end. That is the best-pinned refusal in the document, sitting under the mark for "nothing
stops it".

And the argument was already written in the legend, in the 🤷 bullet:

> The distance between agreeing with a refusal and performing one is what §57 is about, and it is worth
> seeing in the column rather than reading for.

§57 **is** this row. The document names it as the reason the column splits 🛑 from 🤷, and then leaves
§57's own row outside the split — which is the same failure as §65's `BEL`, found by reading the legend
rather than the crates.

### One row, one answer — the gap moves rather than disappearing

Margins are still missing, and that is not the `s` row's business. The row that carries it is `? 69`
(DECLRMM) in the private-mode table: nothing in cmote refuses the mode, the engine has no arm for it and
answers DECRQM `0`, "not recognised". So the pair now reads

- `s` — the **request**, stopped by cmote's own code → 🛑
- `? 69` — the **capability**, missing and priced in §5, refused by nobody → ❌

and it reads in the right order. A program that asks first is told the truth and never spells `s` as
DECSLRM; the program that does not ask gets its request cancelled instead of its saved cursor stolen.

### What the mark had to give up

The 🛑 bullet said "a decision recorded in **§6**" and "it never becomes work". This decision is §5's —
price, not policy — and the capability behind it is still priced. Rather than move margins into §6,
where they would read as a stance cmote holds about margins (it holds none; they are simply expensive),
the bullet now says what the mark actually reports: **who performs the refusal**, with the section it
points at carrying **why it was taken**. The refusal itself still never becomes work. §6 gained a lead
paragraph naming its one exception, so the section's own title does not over-claim.

### A numbering slip, fixed in passing

§72's paragraph in the compatibility plan opened "**§72 names the fourth: translate it**". §57's
paragraph, six lines below it, already ended "'Refuse it properly' is the fourth way in". §56 named
three; §57 made four; translate is the fifth. Written last section, read this one — which is the whole
case for re-reading a section from the row below it.

### What it cost

Nothing in `src/`. The tests stand at 1072, none of them touched, because §57 built the mechanism and
§73 only says so in the column. The matrix rescan: 163 rows unchanged, ✅ 101 unchanged, ❌ 28 → 27,
🛑 25 → 26, 🤷 9 unchanged, and no row carrying two marks or none.

### Not done

- **The margins themselves.** Unchanged, and now with a corrected reason rather than a wrong one.
- **Nothing tells the user when a margin request is cancelled.** An application that reaches for
  `smglr` will draw into a page that never narrowed, and the output is simply wrong — quietly, which is
  the one thing the §57 repair does not fix. Disclosed rather than solved: a note in the terminal would
  be cmote talking over the remote, which no other refusal here does.
- **DECRQM for mode 69 answers `0`.** That is the engine's, and it is the reply that keeps a conformant
  program away from `CSI s`. Whether cmote should ever answer `4` ("permanently reset") instead — a
  stronger statement, and a claim about a mode cmote does not implement — is not decided here.

## §74 — The row where the answer was neither refuse nor shrug (v4.0.0)

Same question as §73, one row further down the same table: should DECST8C read ✅ or 🛑?

```
| ? 5 W | Tab stops every 8 columns (DECST8C) | ❌ | parsed and dropped — `vte` calls `set_tabs`,
  and `alacritty_terminal` never overrides the empty default (§5) |
```

`CSI ? 5 W` says *clear every tab stop, then set one every eight columns* — the state a terminal powers
up in, in one sequence rather than the dozen it otherwise takes.

### Not 🛑, and the reason is what §73 was about

§73's row was 🛑 because cmote's own code already stopped that sequence dead, and the mark had simply
never caught up with the code. Here nothing in cmote refuses anything: `vte` parses the sequence,
`alacritty_terminal` leaves `Handler::set_tabs` at the trait's empty default, and it dies there. Marking
that 🛑 would mean **writing refusal code first** — and a refusal has to be for something.

§57's refusal was a repair. DECSLRM shares its final byte with save-cursor and `vte`'s arm for that byte
ignores its parameters, so an unrefused margin request *stole a value the program meant to restore from*.
There is no such collateral here. `set_tabs` is its own trait method with its own empty body; nothing else
is touched by the drop. And there is no policy objection either — tab stops are inside the tab, they set
no chrome, they are not a second source for a field cmote already writes, and there is nothing a remote
gains by moving them. A 🛑 would be work spent to make a harmless sequence stay broken.

### ✅, because §72's question has a yes here

The question §72 introduced is not "can cmote implement this" but "**is this a shorthand for sequences the
terminal already takes**". §73 put it to margins and got no: a margin is a capability, the delegating
`Handler` wrapper that would build one makes cmote a second writer of engine state, and there is nothing
to translate into.

A tab-stop reset is the opposite. Every piece of it is already ✅ in the matrix:

- `CSI 3 g` — TBC, clear all stops. The engine has the arm.
- `\r` — carriage return, to column 0.
- `ESC H` — HTS, set a stop where the cursor is.
- `CSI 8 C` — CUF, step to the next one.

So `term/tabs.rs` scans DECST8C out of the stream and `term/mod.rs` feeds the engine the long spelling.
The engine's tab table stays private, keeps being rebuilt correctly on every resize by the engine itself,
and cmote never becomes its second writer. The two numbers cmote reads are the page's width and the
cursor's column, both off the existing seam.

### The measurement that decided how the walk is spelled

The natural way to walk a page is `CSI n G` — absolute column, no arithmetic. It is the wrong way here,
and finding out cost a probe rather than an argument.

`alacritty_terminal` funnels most cursor movement through one `goto(line, col)`, which adds the scrolling
region's top to the line it is handed. That is correct for CUP and VPA, which hand it a line from their
own parameter. It is wrong for the movements that hand it **the line the cursor is already on** — they get
the offset added a second time. Under a `CSI 3;7 r` region with origin mode on, cursor at `(3, 4)`:

```
CUU  CSI 1 A   (3,4) -> (4,4)     up one moves it DOWN one
CUD  CSI 1 B   (3,4) -> (6,4)     down one moves it down three
CHA  CSI 1 G   (3,4) -> (5,0)     a column move moves the row
HPA  CSI 1 `   (3,4) -> (5,0)     same arm, same defect
VPR  CSI 1 e   (3,4) -> (6,4)     aliased to CUD
CUF  CSI 1 C   (3,4) -> (3,5)     exact
CUB  CSI 1 D   (3,4) -> (3,3)     exact
VPA  CSI 2 d   (3,4) -> (3,4)     exact
CUP  CSI 2;1 H (3,4) -> (3,0)     exact
```

CR, CUF and CUB assign the column directly and never reach `goto`. So the walk is built out of exactly
those, and it cannot move the cursor's row **under any mode** — which also means it needs to know nothing
about origin mode, the scrolling region or the saved cursor, and reads no engine state to be correct.
A walk spelled with CHA would have looked right in every test that did not think to set origin mode first,
and would have dragged the cursor to the bottom of a region in real use. `term/mod.rs` has the test that
fails for that spelling.

Four ✅ rows in the compatibility matrix now carry the defect, two of them having had an **empty note**,
which under §67's rule is the strong claim that nothing is withheld. It is disclosed rather than fixed:
correcting it from outside means cmote writing the cursor the engine owns, which §71 and §73 both refused,
and it is an upstream bug that belongs upstream.

### The traffic, checked rather than assumed

§73's lesson was that a note naming a reason has to be re-read, so this one was checked before it was
written: **no terminfo capability emits DECST8C.** ncurses lays default stops down by hand — `clear_all_tabs`,
then `init_tabs` columns of movement and `set_tab`, over and over — which is the same walk cmote now feeds
the engine, only sent from the far end of the wire. So this is not §72, where `\E[!p` sat in `is2` and
`rs2` and every `tput init` was sending it. The traffic is programs that spell VT510 sequences themselves.
Worth saying plainly: the value here is smaller than §72's. What makes it worth building anyway is that
the cost is a scanner and a string, the ingredients were all already ✅, and the alternative was a sequence
that lands and silently does nothing.

### What it cost

- `src/term/tabs.rs`, new — the scanner and the walk, both pure and testable without a terminal.
- `src/term/mod.rs` — the seventh scanner in the split feed, a `Interruption::TabStops` variant, and
  `set_default_tabs`, which is five lines because the module above it is the whole of the thinking.
- Tests 1072 → 1090. Thirteen in the module (grammar, near misses, chunk splitting, the exact walk for
  four page widths, and one that asserts the walk emits **only** row-safe spellings), five end to end
  (the stops really move, the cursor comes back, the row survives origin mode, nothing is printed, and
  `CSI 5 W` without the marker is left alone).
- Matrix: 163 rows unchanged, ✅ 101 → 102, ❌ 27 → 26, 🛑 26 unchanged, 🤷 9 unchanged.

### Not done

- **The engine's origin-mode cursor defect.** Disclosed on four rows and worked around in the one place
  cmote controls. Reporting it upstream is the honest next step and is not done here.
- **A cursor waiting to wrap loses that flag** across the reset, because CR and CUF both clear it. Not
  detectable from outside the engine — a pending wrap looks like a cursor in the last column — and the
  same small loss §72's soft reset takes.
- **cmote answers DA1 as a VT102** (`CSI ? 6 c`, the engine's, plus §41's sixel attribute), while
  implementing a pile of VT400-and-later sequences: the whole rectangular family, DECRQCRA, and now this.
  xterm gates DECST8C on `terminal_id >= 400`. Whether the reply should say what cmote actually does is a
  real question and a separate one — it is a claim every program reads, not a row in a table.

## §75 — The stance that was living in a note (v4.0.0)

Third time the same question, one row further down again: should SCP read ✅ or 🛑?

```
| Ps SP k | Select character path (SCP) | ❌ | parsed and dropped — same shape: `vte` calls
  `set_scp`, the engine never overrides it. Bidi anyway, which cmote does not do |
```

`CSI Ps1 ; Ps2 SP k` selects the **character path** — `1` left-to-right, `2` right-to-left — and how the
data and presentation components track each other. It is the bidirectional-text control.

### Not ✅ — §72's question answers no

The question §72 introduced: is this a **shorthand** for sequences the terminal already takes? §74's tab
walk was, and got built out of TBC, CR, HTS and CUF. This one is not, and it is not even close. There is
no sequence in cmote's repertoire that sets a direction, no per-cell or per-line direction state to set,
and no renderer path that could act on one.

What is actually being asked for is a **second coordinate space**. A terminal that reorders has the order
the bytes arrived in and the order the glyphs are drawn in, and they are not the same. Everything §40
built on the absolute document coordinate reads the first while the user points at the second: the
selection, the find bar's match spans, the OSC 133 prompt marks, the Ctrl-hover link runs, the sixel
placements, the rectangular family's corners. A character path is that second space threaded through all
of them. This is §73's answer — a capability with nothing to translate into — with more behind it than
margins had.

### Not 🛑 — §74's question answers no

§74's test for a 🛑: does cmote's own code refuse this, and if not, would writing that code repair
anything? §57's cancel was a repair, because an unrefused DECSLRM stole the program's saved cursor. Here
`vte` parses the sequence in full — both parameters, into `ScpCharPath` and `ScpUpdateMode`, with a third
value for either reaching `unhandled!()` before the call — and hands it to `set_scp`, whose body in the
trait is `{}` and which `alacritty_terminal` never overrides. Nothing is stolen, nothing is misparsed,
nothing else is touched. A 🛑 would be refusal code written to keep a harmless sequence broken.

### 🤷, which the legend already describes word for word

> **🤷** is the same decision with **nothing behind it**: the sequence dies upstream — no `vte` dispatch
> arm, or a `Handler` method `alacritty_terminal` leaves at its empty default body — so cmote is never
> offered it and pays nothing to refuse it.

That is this row exactly, second clause. And the giveaway had been sitting in the note the whole time:
*"Bidi anyway, which cmote does not do."* That is a **stance**, and it was parked under ❌, the mark for a
gap that could still become work. Same shape as §73, where a refusal cmote's own code performed was
parked under the mark for a sequence that could still land. A note that argues with its own mark is the
cheapest finding in this document and the one it keeps producing.

### The work the row produced: the decision itself

§6's title is *deliberately excluded — policy, not gap*, and it had no entry for bidi. So a 🤷 pointing at
§6 would have pointed at nothing. §6 now states it, in the shape OSC 22's entry set: what is being
refused, why, who does the declining, and the closing sentence that the reasoning is why cmote *would*
refuse it while nothing in cmote has to.

One line of the renderer is what makes the stance real rather than aspirational, and it was written for
something else entirely. `ui/grid.rs` batches adjacent cells of one style into a run for drawing, and

```rust
let seals = is_wide || !glyph.is_ascii();
```

seals every non-ASCII cell into a run of its own — because a glyph the bundled font lacks needs the
shaping-and-fallback path and ASCII does not. The side effect is that the text shaper is never handed
more than one grapheme cluster at a time, so it has nothing to reorder. Glyphs go down one cell at a
time, in grid order, and a remote cannot make this terminal display bytes in an order other than the one
they arrived in.

Worth being precise about what that is and is not. It is **not** a refusal — the renderer never sees the
sequence, and a 🛑 would be the wrong mark for a property that falls out of font handling. It is why the
request has nothing to act on. And it is not a claim about what the remote did before sending: whatever
that host's shell or editor reordered is visible as sent, which is the honest behaviour for a terminal
that does not reorder.

### A pair that stopped being one

PLAN §57's audit put DECST8C and SCP together, as "a *third* failure shape — the arm exists and does
nothing — and harmless". That grouping was right about the shape and is now only half a group: §74 built
DECST8C, because the shape a sequence fails in says nothing about whether it has a translation. The pair
was about the engine; the answers were about the sequences.

### What it cost

Nothing in `src/`. Tests stand at 1090, untouched. Matrix: 163 rows unchanged, ✅ 102 unchanged, ❌ 26 →
25, 🛑 26 unchanged, 🤷 9 → 10, no row with two marks or none.

Four consecutive rows, four different answers, from one question asked of each: §72 translate, §73
refuse, §74 translate again, §75 shrug on the record.

### Not done

- **Bidi itself.** Refused, not deferred — which is the difference between this row and the DECLRMM one,
  where §5 quotes a price. No price is quoted here on purpose: the work is not in the sequence, so a line
  count would be a fiction.
- **No test pins the stance.** The renderer's sealing line is what keeps glyphs in grid order, and it is
  there for font fallback — so a future change to how runs are batched could relax it without anyone
  noticing this note. A test that draws a run of RTL codepoints and asserts one run per cell would pin
  it. Not written here; the row is a documentation change and `src/` was left alone deliberately.
- **The engine could grow `set_scp`.** 🤷 rows are stances, not guarantees, and this one has a sharper
  edge than most: if `alacritty_terminal` implemented bidi, the grid would start holding cells in an
  order cmote's renderer draws literally, and the divergence would be visible rather than inert.

## §76 — The parameter the refusal had been charging for (v4.0.0)

§75 marked SCP 🤷 and argued the case at length. The instruction after it was to implement the sequence
anyway. The concern was restated once and overruled, which is how this section came to exist — and the
re-derivation found something both earlier readings had walked straight past.

### What §73's question missed

Three sections had now asked the same thing of this row and got no from all three angles: no shorthand to
translate into (§72's test), nothing to repair (§74's test), so a stance with nothing behind it (§75).
The strongest of the three arguments was that a character path reaches into the engine — it reverses the
direction of the cursor's advance, autowrap, IRM insert, `ICH`/`DCH` and `EL`, none of which cmote owns.

That argument is true of **one of SCP's two update modes**, and the row had been charged for it whole.

```
CSI Ps1 ; Ps2 SP k     Ps1: 0 default   1 left-to-right   2 right-to-left
                       Ps2: 0 implementation-dependent
                            1 data to presentation
                            2 presentation to data
```

ECMA-48 gives a terminal a **data component** — the characters, in the order they arrived — and a
**presentation component**, where the glyphs go. `Ps2 = 2` says the presentation drives the data: edit the
drawing and the stored characters follow. That is the mode every objection above was about, and it is
refused, on §71's and §73's grounds exactly — it is cmote writing the engine's grid, over the only copy of
what the host actually sent.

`Ps2 = 1` says the opposite: the presentation is **derived** from the data. cmote already works that way
and has since it was written. The engine's grid is the data; `ui/grid.rs` builds every frame out of it and
stores no drawing anywhere. So a right-to-left character path is a rule about the derivation, and the
grid is untouched by it.

### What that made it

- `term/scp.rs` — the scanner, and the store. A path is recorded against the cursor's **absolute
  document line** (§40), which is the prompt marks' own anchor, so a line keeps its direction as the
  screen scrolls under it for free and for the same reason they do.
- `ui/grid.rs` — one call at the end of the run planner. The runs are built exactly as before, out of
  data coordinates, so the styles, the selection fill, the match wash and the link underline are all
  resolved before anything moves; then a right-to-left row's runs are mirrored.
- `ui::terminal::cell_under` — the same mirror on the way back in. Every call site that turns a pointer
  into a cell now goes through it, so a click on a mirrored line resolves to the data column the glyph
  was drawn from.

The piece that made this small is the one §75 said would make it large. A second coordinate space is
real, and it turned out to be **one function and one crossing point**: `scp::flip` is its own inverse and
is called by both sides, and `cell_at` was already the single place a pixel became a cell. §75 listed the
selection, the match spans, the prompt marks, the link runs, the sixel placements and the rectangle
corners as six places the space would have to be threaded through. Five of them never see a presentation
column at all, because the mirror happens after they have had their say. The sixth is disclosed below.

### One row became two

The compatibility matrix now carries `Ps ; Ps SP k` as ✅ and `Ps ; 2 SP k` as 🛑, which is the
one-answer rule doing the job it exists for: one mark averaging "we do this, except for the parameter
that would be dangerous" is exactly the row §66 retired the partial mark over. Matrix: 163 rows → 164,
✅ 102 → 103, ❌ 25 unchanged, 🛑 26 → 27, 🤷 10 → 9.

The lesson is narrower than "re-derive everything", which §70 through §75 have already said in five
different ways. It is this: **a refusal that rests on a parameter's cost has to name which parameter.**
Three sections wrote "SCP reaches into the engine" without asking which value of `Ps2` did the reaching,
and so charged the whole sequence for the expensive one.

### A debt paid on the way past

`interruptions()` took eight positional `Vec`s after this, and clippy's argument limit is seven. The fix was the
struct this document has had on its deferred list since §58: `Scanned`, one named field per scanner, with
the emptiness test — the fast path every ordinary chunk takes — as a method on it. Worth more than
silencing a lint: with lists that similar in type, two arguments transposed at the call site would have
compiled and then applied the wrong event at the wrong offset.

### What it cost

- `src/term/scp.rs`, new — scanner, store and mirror, all pure and testable without a terminal.
- `src/term/screen.rs` — the seam carries the store, so the renderer and the pointer path read one source.
- `src/term/mod.rs` — the eighth scanner, a `Interruption::Path`, `select_character_path`, and the store cleared
  on RIS and on both directions of the alternate-screen swap, since each renumbers what a line index
  means.
- `src/ui/grid.rs`, `src/ui/terminal.rs`, `src/app.rs` — the mirror and its inverse.
- Tests 1090 → 1115.

### Not done

- **Implicit bidi — the Unicode Bidirectional Algorithm.** Refused, and §6 now carries only that rather
  than the whole of SCP. ECMA-48's BDSM defaults to *explicit*, which means the sender has already
  ordered the characters; that is the half cmote implements. Under implicit mode the data-to-presentation
  mapping stops being a function of the line's direction and becomes a function of its content,
  recomputed on every write — a per-line table both sides would have to agree on, rather than one
  involution. `vte` does not name mode 8, so BDSM reaches nothing and cmote cannot be asked out of
  explicit mode, which makes the stance consistent rather than lucky.
- **An inline picture is not mirrored.** Placements are anchored by column and drawn separately from the
  runs, so a sixel on a right-to-left line stays where its columns say. Pictures are not characters and
  ECMA-48's path is about characters — but it is a divergence, and it is here rather than nowhere.
- **The mirror is over the page's full width.** A short line on a right-to-left path sits against the
  right edge. That is what a character path running that way means, and it will still surprise someone.
- **Nothing tells a program its `Ps2 = 2` was dropped.** SCP has no reply and no DECRQM mode, so a
  program that asks for presentation-to-data gets silence — the same disclosure §57 makes about a
  cancelled margin request.

## §77 — The refusal that described a different program (v4.0.0)

The question was what the difference is between two rows that both say "cursor". OSC 22 sets the MOUSE
POINTER — the arrow under the user's hand; OSC 50 sets the TEXT CARET — block, bar or underline, in a
grid cell. X11 called the mouse pointer a "cursor" (`XC_xterm`), which is why `vte` names its two
handlers `set_mouse_cursor_icon` and `set_cursor_shape` and why the rows read as near-duplicates. The
next question was whether OSC 22 could be supported if the shape were scoped to the terminal rather than
to the window. It could — and the scoping turned out to be the only thing on offer.

### Three reasons, checked

The row had read 🤷 since §54, under three reasons written into §6. Checking them against the code was
the whole of the work:

- *"A pointer shape is window-wide, so it fails the same test §54 applies to progress."* True of
  `winit::window::Window::set_cursor`, which is what the handler's NAME suggests and what a terminal
  built straight on the windowing layer would have to call. cmote never goes near it. The grid is an
  iced `mouse_area` (`ui/terminal.rs`), and `mouse_area::interaction` applies while the pointer is
  inside that widget's bounds and nowhere else.
- *"The pointer is already contested, and the arbitration is hand-rolled unsafe code."* The four
  contested shapes are `ResizingHorizontally` on the explorer splitter, `ResizingVertically` on the
  files splitter, and `Grab`/`Grabbing` on the dialog and tab-strip drags — the last two painted by
  §51's own `WM_SETCURSOR` subclass, because Windows ships no hand cursor. All four sit on widgets that
  are SIBLINGS of the grid and are never over it. There was no fifth voice to arbitrate, and nothing
  was added to that subclass.
- *"`none` is in the vocabulary, so a remote could hide the local pointer."* False.
  `cursor_icon::CursorIcon` — which is what `vte` resolves an OSC 22 name through, and what every
  terminal on this protocol uses — has no hidden variant, and its `from_str` has no `"none"` arm. The
  hazard cannot be spelled. iced has `Interaction::Hidden`; nothing can reach it from the wire.

Two of the three were arguing about an architecture cmote does not have, and the third was a claim about
a sibling crate that had never been opened. That is the finding, and it is not the same one §76 made:
there the refusal had charged the whole sequence for its most expensive PARAMETER; here the refusal had
been written for a plausible terminal that was not this one.

### What was actually underneath

Something was, which is why this is a split rather than a straight ✅. The five shapes worth having
describe **the content under the pointer** — `default`, `text`, `pointer`, `crosshair`, `cell` — and on
the grid that content is the remote's own output, so the remote is the one that knows. The rest of the
CSS cursor set divides into two refusals:

- `grab`, `grabbing`, `move` and the fourteen resize shapes are **cmote's own vocabulary**. Those exact
  shapes are what the two splitters and the two drag handles say, and §51 goes as far as drawing custom
  art for two of them. A remote painting `col-resize` over the grid teaches the user that a grid edge
  drags when it does not, and `grab` impersonates a cmote handle outright — §55's spoofed-chrome
  argument, one surface over.
- `wait`, `progress`, `not-allowed` and `no-drop` **speak for the client**. `wait` says cmote is hung;
  `not-allowed` says cmote is refusing the user's input. A remote must not be able to say either.

`help` and `context-menu` announce a menu that is cmote's; the remainder (`alias`, `copy`, `zoom-in`,
`vertical-text`, …) have no meaning inside a text grid and can be added later with a reason attached.

The allow-list **is the parser**: `term::pointer::Shape` names five variants and `from_css` matches
those five, so a refused shape has no value to be carried in and no later caller can get round the
list. That is `term/iterm.rs`'s construction for OSC 1337 and `link.rs`'s for URI schemes, third use.
Deliberately not a re-export of `CursorIcon` or of `iced::mouse::Interaction`: re-exporting either would
leave every refused shape nameable and keep it out by a check somewhere downstream.

### What it cost

- `src/term/pointer.rs`, new — the scanner on `term::osc`'s shared framer, the allow-list, the store.
  Fifteen tests, none of which needs a terminal.
- `src/term/mod.rs` — the ninth scanner, a `pointer_shape()` accessor, and the shape cleared on **both**
  directions of the alternate-screen swap, beside the pictures (§41) and the character paths (§76).
  That swap is exactly the moment a full-screen program starts or ends, which is the whole of the
  lifetime management this row needs. Three tests.
- `src/ui/terminal.rs` — one `.interaction()` on the grid's `mouse_area`, and `grid_interaction`, the
  arm-by-arm translation from the five shapes to iced's twenty-seven. Two tests, one of them asserting
  that no remote request reaches any shape cmote's own chrome uses.
- Tests 1115 → 1135. Matrix: 164 rows → 165, ✅ 103 → 104, ❌ 25 unchanged, 🛑 27 → 28, 🤷 9 → 8.

`Shape::Default` maps to `None` rather than to `Interaction::Idle`, which is the difference between
handing the question back and answering it: with no `interaction` set, `mouse_area` lets the widget
underneath decide, which is what the grid did before this row existed and what it must go back to doing
the moment a program hands the pointer back. `default` is on the allow-list precisely so that handing it
back is something a program can say on purpose.

### Not done

- **A refused name leaves the current shape alone** rather than resetting to `Default` — the rule `icon`
  keeps for a payload that is not an icon name. Safe, because the shape that survives is one this same
  remote asked for and this same list already passed, so a refusal cannot be turned into a way of
  clearing a shape. But a program that sets `text` and then asks for `wait` is left with `text`, which
  is not what it intended and which nothing tells it.
- **Nothing tells a program its shape was dropped.** OSC 22 has no query and no reply, so a request for
  `grab` gets silence — the same disclosure §57 makes about a cancelled margin request and §76 about a
  refused update mode.
- **RIS does not clear the shape.** The alternate-screen swap does, which covers the realistic case (a
  TUI that quits), but `ESC c` on the primary screen leaves the last shape standing. Deliberate rather
  than missed: clearing it would need this scanner to frame `ESC c` for itself, and the five shapes it
  can be holding are all benign when stale — which is a property of the allow-list, not luck. Worth
  revisiting only if the list ever grows a shape that is alarming to see for no reason.
- **The shape is per-terminal, not per-pane-under-the-pointer.** With split panes it would be the
  focused terminal's shape over any grid; there is one grid today, so the question has not been asked.
- **`vertical-text` is refused for want of a target**, not for danger: iced has no such interaction. It
  is the one refused name that is a toolkit limit rather than a decision, and it is in the same list as
  the decisions, which is the kind of thing §73 exists to catch. Left there on purpose, and here.

## §78 — The row that was in the right column for a borrowed reason (v4.0.0)

The question was whether `kitty 21` — colour by semantic name — could be supported. The answer is no,
and the answer was already no; what §78 found is that the row had never argued for it. Three sections in
a row had moved a mark (§75 ❌→🤷, §76 🤷→✅/🛑, §77 🤷→✅/🛑), so the useful outcome here is the other
one: **the mark was right, the sentence under it was on loan, and only writing the reason out in full
finds that.**

### What the sequence actually does

`OSC 21 ; key=value ; key=? ; key= ST` — three jobs in one sequence, any number of pairs at a time,
addressed by NAME rather than by number: `foreground`, `background`, `cursor`, `cursor_text_color`,
`selection_foreground`, `selection_background`, `visual_bell_color`, `transparent_background_color1`–`8`,
`color0`–`color255`. A pair with a colour SETS, a pair with `?` QUERIES, a pair with nothing RESETS.

### The argument that was on loan

The row read "same fixed scheme as 4 / 10 / 11 / 12 — the theme is cmote's, not the remote's". That is
exact for the set and reset pairs, and it is the same refusal `4 (set)` and `110`–`112` already carry.

But rows `4` and `10 / 11 / 12` are each **split**: query ✅, set 🛑. cmote answers colour queries
accurately on purpose — `report_color` resolves against `palette`, the same table `ui/grid.rs` paints
from — and refuses only the sets. The `kitty 21` row had copied the SET half's reason and stretched it
over a sequence that carries both halves, so the query half had no reason written anywhere.

That is §76's shape, one family over: there a refusal charged the whole sequence for its most expensive
PARAMETER; here a refusal charged the whole sequence for its most expensive HALF. The difference from
§76 and §77 is where it landed. Those two found an argument that was wrong and a mark that moved with
it; this one found an argument that was missing and a mark that survives being argued for properly.

### The query half's own reasons

Four, and they are about what asks rather than about what it would cost to answer:

- **It is a dialect query, not a generic one.** The five in `term/query.rs` — XTVERSION, DECRQSS,
  XTGETTCAP, DA3, XTSMGRAPHICS — are sent blind by programs that have concluded nothing about the
  terminal, which is exactly why sniffing them out of the stream pays. OSC 21 is sent AFTER a caller has
  concluded kitty, and nothing in this stack ever says kitty: cmote asks the remote for
  `TERM=xterm-256color`, answers XTVERSION with `cmote(<version>)`, and answers XTGETTCAP `TN` with
  `xterm-256color`.
- **For the keys cmote could answer it carries nothing new.** `foreground`, `background`, `cursor` and
  `color0`–`color255` all resolve through `report_color`, which is what `OSC 10` / `11` / `12` / `4`
  answer from today. It would be a second READER of one source, not a second WRITER of one field as
  `iTerm 1337 CursorShape` would have been (§71) — so unlike that row the two spellings could never
  disagree. They could never differ either, which is the whole of what a second spelling would buy.
- **The keys that would justify it have no single value here.** Selection changes only the BACKGROUND
  (`SELECTION_BG`, `ui/grid.rs`), so `selection_foreground` is whatever the cell already had; the cursor
  is drawn by INVERTING the cell, so `cursor_text_color` is per-cell too; `visual_bell_color`, the eight
  transparent-background colours and the mark colours name features cmote does not have. Answering any
  of them means inventing a colour, and `palette.rs` opens by saying why a terminal must not: an answer
  that disagrees with what the grid paints breaks the colour-scheme detection the query exists for.
- **It would be the first reply whose length the requester sets.** n `key=?` pairs produce n values in
  one reply. Every reply cmote writes today is one bounded answer to one question. Cappable — `query.rs`
  already holds `MAX_PARAMS` and `MAX_DATA` for exactly this — but a cost paid for a question nothing
  here is asking.

A fifth, weaker one, recorded because it is real: the reply syntax would have to be written from memory
against a spec not open here, and this document pins replies with tests. A reply in the wrong shape is
worse for a sender than silence, which is the same reasoning §71 used to leave `ReportCellSize`
unanswered.

### What would flip it

Written down rather than left for a later reader to re-find, because the counters are genuine:

- `selection_background` **is** answerable. `SELECTION_BG` is a constant in `ui/grid.rs`, and
  `palette.rs`'s own charter — one source of truth for the renderer AND the query answerer — says that
  is where it belongs. If it moves there for any other reason, one of the three interesting keys becomes
  free.
- **kitty's keyboard protocol does work here** (§25), so "nothing kitty lands in cmote" would overstate
  the first reason. A program that found one kitty protocol answering has some grounds to try another.
- `query.rs`'s own argument applies unchanged: a program that asks and hears nothing stalls until its
  timeout. That is the case FOR answering, and it is the reason this is a judgement about what asks
  rather than a wall.

So: if cmote ever advertises kitty anywhere, or if `palette.rs` grows the selection colours, this row
should be re-read. The implementable slice is small — a scanner on the shared `term::osc::Framer` and a
`kitty21_reply` formatter beside `query.rs`'s five — which is precisely why the reason had to be about
what asks and not about what it costs.

### What it cost

- No code. Tests unchanged at 1135, matrix unchanged at 165 rows / ✅ 104 / ❌ 25 / 🛑 28 / 🤷 8.
- `TERMINAL_COMPATIBILITY_PLAN.md` — the row rewritten to carry both halves and their two reasons, a §6
  subsection for the query half beside the fixed-scheme policy it is NOT an instance of, the §7 sentence
  that had stated the borrowed reason corrected in place, and a §78 paragraph in the header narrative.
- The mark **verified rather than assumed**, which is §77's habit applied to a row that did not move:
  `vte` 0.15.0's OSC arms are `0`/`2`, `4`, `8`, `10`–`12`, `22`, `50`, `52`, `104`, `110`–`112` and
  nothing else. No arm for 21, no cmote scanner, so 🤷 is the honest mark and not a shrug.

### Not done

- **The row is not split**, though `4` and `10 / 11 / 12` are. A row splits when the two halves ANSWER
  differently; both halves here answer 🤷, for different reasons, and two identical marks side by side
  would add a row to the matrix without adding an answer. Worth revisiting only if the query half is
  ever built, at which point the split arrives with the mark change that justifies it.
- **Nothing performs this refusal**, and nothing was written to. A scanner that saw OSC 21 only to drop
  it would be code with no behaviour, and the OSC 9 precedent does not transfer: `9;<text>` is declined
  by scanners that were already looking at `9;4;` and `9;9;`, so that refusal was free. Nothing in cmote
  looks at OSC 21 by accident.
- **`XTPUSHCOLORS` / `XTPOPCOLORS` (`CSI # p` / `# q`) were not re-read** in the same pass, though that
  row is downstream of the same fixed scheme and carries the same 🤷 for the same upstream reason. It
  has no query half, so the specific fault found here cannot apply — but that is an argument made from
  this chair, not one checked against the sequence.
- **The reply-syntax uncertainty is a fact about this session, not about the sequence.** It is recorded
  as a reason above, which slightly overstates its weight: it argues against implementing TODAY without
  the spec, not against implementing. Left visible rather than dropped, because the alternative is a
  reason list that reads stronger than it is — §73's complaint in a smaller size.

## §79 — The refusal nobody was performing (v4.0.0)

The instruction was to mark `kitty 99` — rich notifications — explicitly 🛑, because desktop
notifications are not something this project will let a remote raise. The decision was not in question;
it has been settled since §54 and written down in §6. What was in question, once the legend is taken at
its word, is who was carrying it out. Nobody was.

### What 🤷 was claiming

The legend is precise: 🛑 is a refusal **cmote's own code performs**, 🤷 is one that **dies upstream** —
no `vte` dispatch arm, or a `Handler` method left at its empty default body. §73 spent a whole section
on getting a single row into the right one of those two columns.

Read that way, `kitty 99` at 🤷 was making a claim about `vte`, and the claim was not quite true. `vte`
does not REFUSE OSC 99; it has no arm for it, which is a different thing. Nothing upstream considered
the sequence and declined it — the sequence simply arrived somewhere that was not looking. That is the
gap a mark cannot express, and it is the fourth way §75 through §78 have now found for a 🤷 to be wrong.

The same was true of `OSC 777`. And the bare `OSC 9;<text>` — which has read 🛑 since §54 — was only
just on the right side of the line: it is refused because `term/progress.rs` matches `9;4;` and
`term/cwd.rs` matches `9;9;`, so neither recognises it. Cmote's code does see the payload and does
decline it, which satisfies the legend, but it declines it by looking for something else. A refusal that
holds because of what two unrelated scanners happen not to match is one nobody wrote and nobody can find.

### What was built

`src/term/notify.rs`: one pure function, `refused(payload) -> Option<Spelling>`, naming the three
dialects of a single decision — ConEmu's `9;<text>`, urxvt's `777;notify;…`, kitty's `99;…`. It is
called from `term::progress::Reports::feed`, which returns immediately on a match.

**It changes no behaviour, and every row it backs says so.** Nothing in cmote could raise a desktop
notification if it wanted to: there is no toast code, no Action Center call, nothing. All three
spellings were going to be ignored, and after §79 all three are still ignored. What changed is that the
ignoring is now something cmote does on purpose, in a named place, checked by six tests that fail if a
later hand starts reading any of the three.

That is exactly the change §63 made on the OSC 52 row, and the parallel is worth stating because it is
the argument for writing code that does nothing. There the clipboard pair was refused only by the
listener's catch-all arm, with the engine's own `osc52` field sitting at its permissive default; §63 set
the field explicitly and called the old state "the weakest 🛑 in this table, now the most explicit". A
refusal that rests on nobody happening to match it is invisible to tests, and invisible to the next
person to touch the file.

### Why a function and not a scanner

Every other module in `term/` that reads the stream keeps something the app reads or answers a query the
engine dropped. A ninth scanner that kept nothing and answered nobody would be a no-op wearing a
scanner's clothes, and it would need `Terminal::process` to hand it a copy of every chunk to be that.

So the policy lives in a function and the calling is done by the module that already frames every OSC
payload cmote sees — `term::progress` — and that already owned the bare-OSC-9 half of this refusal
(§54). The cost is one early return on a path that already ran.

The load-bearing part is not the refusal but the **exclusion**. OSC 9 is multiplexed three ways, and two
of those ways are shipped features: `9;4;` is progress and `9;9;` is the Windows working directory. Both
are named in the classifier WITH their trailing separator — the same prefixes `progress.rs` and `cwd.rs`
strip — so the classifier and those two modules cannot disagree about which payload belongs to whom, and
a later tightening of this refusal cannot quietly take a shipped feature with it. Matching is on the
whole numeric field and never on a prefix of it, so `990;` and `999;` are not read as kitty's `99;`.

### What it cost

- `src/term/notify.rs`, new — the classifier and the `Spelling` enum. Six tests, none needing a terminal.
- `src/term/progress.rs` — one early return in `feed`, the header paragraph that explains why the
  refusal lives here, and one test that sets a progress report FIRST and asserts it survived both vendor
  spellings (§77's ordering, reused: the refusal is a decision about bytes cmote had).
- `src/term/mod.rs` — the module declaration, and one seam test asserting all three draw no reply, leave
  the tab's own state alone, and — the half worth pinning — never land their TEXT on the grid.
- Tests 1135 → 1143. Matrix: 165 rows unchanged, ✅ 104 and ❌ 25 unchanged, **🛑 28 → 30, 🤷 8 → 6**.

`OSC 777` moved with `kitty 99` rather than being left behind, and that was not scope creep but the
opposite: one function refuses both, so leaving 777 at 🤷 would have been the document disagreeing with
the code it describes. The instruction named one row; the mechanism that satisfies it covers two.

### Not done

- **The refusal is inert and will stay inert**, which is the honest reading of the whole section. If a
  reader wants a version of this that DOES something, the only candidates are a tab-local hint that a
  remote asked to notify — additive, contained, and nobody has asked for it — or nothing. Nothing is the
  right answer today and is what is here.
- **`term::progress` is an odd landlord for a notification policy.** It is the right *place* — the one
  module fed every OSC payload that also already performed half this refusal — but a reader looking for
  where notifications are declined will not guess it from the module name. Mitigated by naming §79 in
  both headers and in three rows, not fixed.
- **Only the `notify` module of OSC 777 is classified.** urxvt's 777 is a dispatcher and its other
  modules are unimplemented rather than refused, which is a different question with a different mark.
  The row says so; no row was added for them, because nobody has enumerated them.
- **Nothing tells a program its notification was dropped.** None of the three spellings has a reply, so
  a remote that asks gets silence — the same disclosure §57, §76 and §77 each make for their own row.
- **`vte`'s OSC payload buffer was noticed and not chased.** `MAX_OSC_RAW` is 1024 with the `no_std`
  ArrayVec, and the std build uses a plain `Vec`; whether a hostile stream can make the parser hold an
  unbounded OSC payload is a question about every OSC cmote sees, not about this row, and it is left
  open here rather than answered badly in passing.

## §80 — The half of a module that belongs to one platform (v4.0.0)

CI's macOS job has been failing since f3d7ea6 — the commit that landed §51's hand cursors on 10 August
— and the Windows job has been green for every one of those four days. Both facts have one cause, and
it is not the cursors.

### What the lint was saying

`cursor.rs` is two halves. One is a state machine over five atomics — which handle has the pointer,
which control on it is blocking, whether something is being dragged — and `app.rs` drives it on every
platform. The other is a Win32 seam: decode two bundled PNGs, fit them to `SM_CXCURSOR`, build an
`HCURSOR`, subclass the window and answer `WM_SETCURSOR` before winit does. That seam exists because
Windows ships no hand cursors and iced exposes no seam to pass a picture through (§51), so it is behind
`#[cfg(windows)]` and every other platform draws its own hands.

Everything the seam CONSUMES was written unconditionally. Off Windows the seam is not compiled, so
`Hand`, `hand()`, both `include_bytes!` drawings, `HOTSPOT`, `COVERAGE`, `Drawing`, `decode` and
`resampled` have no reader at all — and `dead_code` said so as an error, because CI runs clippy over the
shipped `x86_64-apple-darwin` target with `-D warnings`.

The lint was RIGHT, and that is what decides the fix. `allow(dead_code)` was the fast answer and would
have asserted something false: that these items are used and the compiler cannot see it. They are not
used. They are Windows'.

### The fix, and the one item that differs

Seven items become `#[cfg(any(windows, test))]`, and one — `COVERAGE` — becomes `#[cfg(windows)]`.

`test` is in the predicate deliberately. The hotspot check, "both hands are one square size", and the
resampler's alpha weighting are facts about two pictures and some arithmetic rather than about an OS,
and CI runs `cargo test` natively on the mac runner as well as the Windows one. Gating them to `windows`
alone would have quietly halved where they run, to buy nothing.

`COVERAGE` is the exception because nothing tests it: it feeds `scaled` inside the seam, and it is judged
by eye on a Windows desktop. Its narrower cfg is therefore a true statement about test coverage rather
than an inconsistency — and the control run below is what turned that up, because the mac TEST target
fails on `COVERAGE` alone, which the CI log had not got as far as printing.

### Verified without a mac

The gate the README asks a developer to run is entirely on Windows, where `cfg(windows)` is true and none
of this is dead. The local gate is therefore structurally incapable of seeing this class of fault, and a
fix "verified" by running it would have been verified by nothing.

So the predicates in `cursor.rs` were INVERTED on the Windows host — `windows` → `unix`, which is false
there — putting that one file in exactly the configuration a mac build puts it in while the rest of the
crate stayed as it was. Against HEAD it reproduced all nine errors, the eight in the CI log plus
`COVERAGE` on the test target; against the fix, `cargo clippy --all-targets -- -D warnings` was clean for
both the bin and the test target. The file was restored from a copy either way and no inverted line was
committed.

The same trick does NOT work crate-wide, which is worth writing down because it looks as though it should.
Inverting all five files that carry a `windows` cfg produced three failures that are artifacts of the
inversion rather than mac faults: `paths.rs` splits its non-Windows side again by `target_os = "macos"`,
and `ssh/upload.rs` and `ssh/agent.rs` have genuine `cfg(unix)` arms — none of which a Windows host can
turn on, so those files end up in a configuration no real platform is ever in. `cursor.rs` is the only one
split plainly into `windows` and `not(windows)`, which is exactly why the file-local inversion is faithful.

### What it cost

- `src/cursor.rs` — eight cfg attributes and the three paragraphs saying why each is where it is. No
  behaviour changes on any platform: on Windows every predicate is true and the file compiles as before.
- Tests 1143, unchanged. Nothing was added, because there is nothing here a test can hold — the fault is
  a CONFIGURATION the test runner cannot be in.

### Not done

- **The local gate still cannot see a non-Windows fault.** The inversion above is a one-off somebody has
  to think of, not a check anybody runs. The real answer — `cargo check --target x86_64-apple-darwin` on
  the development machine — needs a mac C toolchain for `ring`'s build script and so cannot run there at
  all. CI remains the only honest check for the second target.
- **Nothing pins the gating.** A Windows-only helper added later without a cfg fails on CI's mac job and
  nowhere else, which is precisely the loop that just cost four days.
- **Clippy's own lints on the mac target are still unseen.** `dead_code` is a rustc lint and it aborted
  the build before clippy's late passes ran, so the mac job may yet have something to say once it gets
  past this. That is what the next CI run is for; it is not something this box can answer.
- **CI was red for four days and nothing announced it.** No notification is wired to the workflow, and
  this was found by a person reading a run. A hole in the process rather than in the code, and not closed
  here.
- **The state machine still runs off Windows and does nothing.** Every hover, and every `frame_begin` /
  `drawn` / `frame_end`, writes atomics that nothing on a mac reads. It costs a handful of relaxed stores
  a frame and it keeps `cfg` out of `app.rs`'s message handling, which is the trade taken on purpose
  rather than by oversight.

## §81 — The marks belong to one column (v4.0.0)

§8's tables are read down the Status column: one glyph a row, 165 of them, and the whole point of a
symbol there is that a reader takes it in without reading the line. Sixteen of those rows also carried
marks INSIDE the note — twenty-two of them, across four of the seven tables — because a note that tells
the row's own history has to name what the row used to say: "Read ❌ until §70", "the weakest 🛑 in this
table", "both of these answer 🤷".

Every one of those was TRUE. That is why it survived eight sweeps: nothing here was a wrong claim, and a
sweep that re-derives the marks from the crates has no reason to stop at a sentence that is right. The
fault is not in what the notes said but in where they said it — three marks in a note sit on the same
line as the one in the column, at the same size, in the same shapes, and several of them are the mark the
row NO LONGER carries. A column read at a glance cannot afford a second answer on the same line, and
`? 5 W` was the worst of them: ✅ in the column, and a note reciting ❌ and ✅ in two different jobs.

### What changed

The marks stay in Status; the notes say it in words, and the words are the legend's own — *supported*,
*not supported*, *refused*, *refused with nothing behind it*. Most were a substitution. Three were not,
because the symbol was doing a noun's work and the sentence had to be rewritten around its absence:
"both of these answer 🤷" became "answer the same way, with nothing behind either", "Worth knowing about
the ✅ above" became "the row above", and DECSLRM's "❌ is a sequence that could still land" reads as a
phrase rather than a glyph. Two more said "the ❌" about a gap and now say "the gap", which is the word
the legend gives that mark anyway.

The legend gained the rule, so it does not creep back with the next row.

### What it cost

- `TERMINAL_COMPATIBILITY_PLAN.md` only. No code, no tests, and no row's mark or meaning moved.
- Checked by re-tallying the Status column after the edit rather than by reading the diff: 165 rows,
  ✅ 104 · ❌ 25 · 🛑 30 · 🤷 6, identical to §79's count, and every row still carries exactly one mark.
  That is the check that matters here, because the one way this edit could do damage is by taking a
  symbol out of a Status cell while meaning to take it out of a note.

### Not done

- **§2–§7 still use the marks in running prose, and should.** There is no column there for a symbol to
  be mistaken for, and §6's own heading names two of them. The rule is about the tables.
- **Nothing enforces it.** A row added tomorrow with "Read ❌ until §82" in its note breaks the rule and
  only a reader will notice — the same shape of hole §80 left around the cfg gating, one document over.
- **The four symbols are still what the tables are read by**, so a reader who cannot see them is no
  better served than before. Naming the marks in words inside the notes moves a little way toward a table
  that reads without them; it was not the reason for the change and it does not finish that job.

## §82 — The reason nobody had checked (v4.0.0)

`CSI ? 6 n` is DECXCPR, "where is the cursor?" in DEC's private spelling. It has read ❌ since §67's
sweep found it, with a reason recorded in that section's *Not done*:

> **`CSI ? 6 n` is a real ❌ nobody will miss.** Answering it would mean inventing a page number cmote
> does not have, and the standard spelling already works.

The second half is true. The first half is not, and the way it fails is the one §70 named: a price that
was never being charged. xterm's own ctlseqs, which is the document cmote's `TERM` claims conformance to:

    Ps = 6  =>  Report Cursor Position (DECXCPR).  The response [row;column] is returned as
    CSI ? r ; c R  (assumes the default page, i.e., "1").

**No page is sent.** The reply is the two numbers the ANSI spelling already reports, with a `?` in front
of them. DEC's VT420 manual does define a third parameter, and a terminal with one page has nothing to
put in it — which is precisely why xterm leaves it out, and why the row's note went on charging for an
invention nobody was asking for. Four sweeps re-derived this row's MARK from the crates and none of them
re-read its REASON, because the mark was right: `vte`'s CSI table holds `('n', [])` and no
`('n', [b'?'])`, so the sequence really did reach nothing. §70's lesson, in the one place it had not yet
been applied.

### What shipped

`src/term/dsr.rs`, a scanner of the same shape as `term/tabs.rs`, and `term/mod.rs` answering it inside
the interruption advance. Three decisions carry the section.

**The reply is xterm's, not DEC's.** Two parameters, one-based, no page. Cmote claims to be
`xterm-256color` in `TERM`, in XTGETTCAP's `TN` and in the DA1 it amends; answering a question in a
dialect it does not claim would be the one place it spoke as something else.

**The arithmetic is the engine's, copied.** `cursor_reply(row, col)` adds one to each, which is exactly
what `device_status` does for `CSI 6 n`. That is what makes cmote a second READER of the cursor and never
a second source for it — the property §71 and §73 both refused to give up, and the one that guarantees
these two spellings of one question cannot come to disagree. One test asks both on the same terminal so
the guarantee is pinned rather than argued.

It copies a defect with the arithmetic, and that was the choice rather than an oversight. The engine
reports the cursor's ABSOLUTE row, where DEC defines both spellings as reporting a position relative to
the scrolling region when origin mode is set — the same `goto`-shaped divergence §74 measured on CUU,
CUD and CHA. Correcting it here would make cmote's answer to the DEC spelling disagree with its answer to
the ANSI one for the same cursor, which is worse than one shared divergence a row can name: a program
that asked twice would get two different positions and have no way to tell which was the terminal's.

**It is answered in the interruption advance, not in `term/query.rs`.** Every other query cmote answers itself
is a constant — a version string, a unit id, the sixel decoder's limits — so `query.rs` may collect them
and reply once the chunk has been applied. A cursor is not a constant. A chunk carrying `CSI ? 6 n`
followed by ten more columns of output would, answered that way, report the eleventh column. So `dsr.rs`
reports offsets, `term/mod.rs` reads the cursor with the engine advanced exactly that far, and the reply
goes into the same buffer the engine's own replies land in. That is `rect.rs`'s route for DECRQCRA (§60),
and it is why two questions asked in one write come back in the order they were asked.

### The nine that are refused

`CSI ? Ps n` is a family and xterm answers ten members of it. Cmote answers one, and the other nine are
refused by an allow-list one value wide — the construction `term/iterm.rs` uses for OSC 1337 keys and
`term/pointer.rs` for pointer shapes — rather than left to fall through it.

They are refused because **a reply is an advertisement** (§71), and every one of them advertises
equipment rather than a page: a printer (`15`), a user-defined-key store's lock (`25`), a locator
(`55` / `56`), macro space (`62`), that store's checksum (`63`), a memory self-test (`75`), a
multi-session controller (`85`). None of it exists here, so "ready" or a byte count would each be a claim
about hardware that is not there.

`26` is the one refused on more than that: it reports the **keyboard's nationality**. §36 fixed the rule
it would break when it made DA3 answer a constant unit id rather than the serial number DEC hardware put
there — cmote's identity replies name the program, never the person's machine. A remote must not learn
the layout in front of the user off a query the user never sees.

The refusal is pinned twice, in the scanner and at the boundary, and the boundary test answers DECXCPR
**after** the nine so what it asserts is the allowed value SURVIVING the refusals — §77's ordering, which
a scanner that had simply stopped matching would pass a weaker version of.

### What it cost

- `src/term/dsr.rs`, new — the scanner, the allow-list and the pure reply formatter. Eleven tests.
- `src/term/mod.rs` — one field, one feed, one `Interruption` variant, one method, and five seam tests.
- Tests 1143 → 1159. Matrix: 165 rows → 169, **✅ 104 → 105, ❌ 25 → 24, 🛑 30 → 34**, 🤷 unchanged at 6.

### Not done

- **The origin-mode divergence is inherited, not fixed.** Under DECOM both spellings report the absolute
  row where DEC says relative. Fixing it means either correcting the engine's answer from outside — a
  second source for a field the engine owns, refused in §71 and §73 — or fixing only cmote's spelling and
  letting the two disagree. Neither was taken, and the row says so.
- **`55` and `56` have an honest negative that is not sent.** xterm answers "no locator" (`CSI ? 53 n`)
  and "cannot identify" (`CSI ? 57 ; 0 n`), and those advertise nothing — they are the same shape of
  reply as DECRQM's honest "not recognised" for mode 69, which this document praises. The
  stalled-sender argument that carried DECXCPR applies to them exactly. They are silent because nobody
  has asked for them, which is the honest reason and not a good one.
- **Nothing tells a program its question was refused.** The nine get silence, so a caller waits out its
  timeout — the disclosure §57, §76, §77 and §79 each make for their own rows.
- **No consumer was named.** The decision rested on `query.rs`'s own argument, that an unanswered query
  stalls its sender, and deliberately not on a census: a census that came back empty would not have
  changed the answer, since the cost of answering is a scanner and the cost of not answering falls on
  whoever asks. But it does mean this may be a row of code nothing ever executes.
- **`term/` now holds a ninth chunk-safe CSI state machine**, and this one is a near-copy of
  `term/tabs.rs` down to the buffer bounds. Factoring them would couple modules that today can each be
  read alone, and no one has decided that trade is worth taking; it is noticed here rather than acted on.
- **`query.rs`'s own replies still land at the end of the chunk.** §82 avoided that rather than closing
  it: DECXCPR needed the split and took it, while XTVERSION, DA3, XTGETTCAP and XTSMGRAPHICS go on being
  answered after the advance. For those four it makes no difference, and the ordering question between
  cmote's replies and the engine's within one chunk is still open where it was.

## §83 — The column that had become a document (v4.0.0)

§8's matrix is 169 rows, and its Note column had quietly become the longest prose in the file: 7,876
words, the longest single note 419 of them. A reader who wanted to know what `CSI ? 62 n` *is* had to
read past a refusal, its reason and two section numbers to reach "reports how much room is left in the
terminal's macro store" — the one sentence they came for. The column had been answering a question the
Status column already answers.

So the notes were rewritten to one job: **define the feature, briefly and exactly, then point at what
argues the row.** 7,876 words → 3,640, longest note 62.

### What a note is now

The parameters the sequence takes, the reply it draws, and — where it qualifies the mark — the extent
cmote honours. Then a pointer: the section number, and the module cmote's own code sits in.

    | ? 62 n | Macro space (DECMSR) | 🛑 | DECMSR reports the space left in the terminal's macro
    | store, in units of 16 bytes (§6, §82) |

Nothing else. No history of the row, no test names, no crate arms, no argument for a refusal — all of
which are in the sections the pointer names, which is where they were written first. The notes had
become second copies, and a second copy is a thing that can disagree with the first: §70 and §82 were
both rows whose *note* had gone stale while the section it summarised was fine.

### The pointer is the whole of the trade

Every note ends in at least one `§`, and that is what makes the deletion checkable rather than merely
smaller. Three sweeps of the tree confirmed the facts a note used to carry survive where it points —
`set_scp`, `set_mouse_cursor_icon`, `report_modify_other_keys`, `set_tabs`, `MAX_PAYLOAD`,
`xtermCheckRect` and the rest are all still named in §2–§7 or under Evidence.

Four were **not**, and they moved rather than dying: `vte`'s complete OSC arm list, the `('t', [])`
parameter set that makes `CSI 16 t` a parser gap, the absence of any `#` intermediate in `csi_dispatch`
(the palette stack), and the APC route that drops kitty graphics without calling a `Perform` method.
They are now one Evidence bullet, which is where the 🤷 legend now sends a reader for *where a sequence
dies*.

### What it cost

- 169 notes rewritten. **Only the fourth cell changed** — Code, Feature and Status were carried through
  byte for byte, and the tally after the rewrite is identical to §82's: 169 rows, **✅ 105 · ❌ 24 ·
  🛑 34 · 🤷 6**, and no mark anywhere in a note (§81's rule, re-checked).
- The legend gained a paragraph and lost two claims that had stopped being true: "a ✅ with an empty note
  is the strong claim" (no note is empty now) and "the row names *where* it dies".

### Not done

- **The definitions are unquoted.** Each says what the sequence does and none cites the page it came
  from — not ctlseqs, not ECMA-48, not the VT420 manual. That is exactly the shape §82 walked into: a
  wrong definition reads like a right one, and nothing in the row invites re-checking. The marks were
  verified against the crates four times over; the definitions have been verified once, by writing them.
- **The strong claim is harder to skim for.** §67's "everything works, nothing withheld" used to be
  visible as an empty cell. It is now the *absence* of a qualifying clause inside a sentence, which is a
  weaker signal than blank space and one no script can count.
- **The Feature column and the note now overlap.** "Macro space (DECMSR)" and "DECMSR reports the space
  left in the terminal's macro store" say much the same thing at two widths. Merging them would cost the
  matrix its fixed shape, so it was not done, but the redundancy is real and it is new.
- **Nothing enforces the rule**, as with §81. The next row someone adds may carry a paragraph, and the
  only thing stopping it is this section.
- **The prose in §2–§7 was not touched.** It is longer than the table it explains, and several of its
  paragraphs are themselves summaries of `PLAN.md` sections — the same duplication one level up, left
  alone because this pass was about the column the user was reading.

## §84 — Reading the definitions back (v4.0.0)

§83 left one thing undone on purpose and said so: *"the definitions are unquoted. Each says what the
sequence does and none cites the page it came from… a wrong definition reads like a right one, and
nothing in the row invites re-checking."* This section re-reads them against **xterm's ctlseqs**, the
document cmote's `TERM` claims conformance to. Thirty-odd of the 170 rows were checked. **Five were
wrong**, and only one of those was written by §83.

### The palette stack was the wrong sequence

`CSI # p` and `CSI # q` are **XTPUSHSGR / XTPOPSGR** — aliases of `CSI # {` and `CSI # }`, and a stack of
**video attributes**. The colour stack is the capitals: `CSI # P` / `CSI # Q`, XTPUSHCOLORS / XTPOPCOLORS.
The matrix has carried the lower-case pair under the colour stack's name since §65, and — worse than the
label — under the colour stack's **justification**: *"downstream of the fixed scheme (§6): a stack over a
palette that is never read has nothing to save or restore."*

That is true of `# P` / `# Q` and **void** for the sequence the row actually named. Video attributes are
not a palette cmote never reads; they are bold, italic, underline, reverse — everything the renderer
draws. A row had been declining a real capability with an argument belonging to a different sequence.

Two rows now. The colour stack keeps its 🤷 and its reason. The SGR stack is **❌**: nothing decided
against it, it was never seen. Both still die in the parser — `csi_dispatch` has no `#` intermediate at
all, the only `b'#'` in `vte`'s table being `esc_dispatch`'s DECALN — which is why the mistake was
invisible from the crate side. Re-deriving the mark from the source would have confirmed the mark every
time.

### The other four

- **`? 75 n` is "data integrity"**, not the memory self-test the row called it.
- **DECMSR's reply is `CSI Pn * {`, and ctlseqs names no unit.** §83's note said "in units of 16 bytes" —
  invented precision, written in the very pass that warned it could not detect invented precision.
- **XTSMGRAPHICS had its parameters in the wrong order**, since §41: the rows read `? Pi;Pa;1 S` and
  `? Pi;Pa;3 S`, as though the action were the third parameter. It is the second — `CSI ? Pi ; Pa ; Pv S`.
  `query.rs`'s `graphics_request` reads item then action, so **the code was right and the row had never
  been read back against it either**. The same row also missed that action `2`, reset, is answered with a
  status 0: an action no row in this document mentioned.
- **Two attributions ctlseqs does not carry.** DECFRA's `Pch` range was credited to xterm ("as xterm
  allows") and DECSACE's stream extent to the terminal's power-on state. Both are true of cmote's code —
  `rect.rs` enforces 32–126 / 160–255 and its `RectExtent` derives `#[default] Stream` — and neither is in
  the document they were credited to. The rows now say whose claim each is.

### What it says about the sweeps

Four of the five predate §83; only DECMSR's units were written in that pass. So they sat in the notes
through §67's sweep, §70's, §82's and every audit between, and none of those looked at them, because
each re-derived the **mark** from the crates and the mark was right every time. §70 found a note whose
price had expired and §82 a note whose reason was never true; §84 is the third of the same shape, and the
first to go looking for it deliberately.

### What it cost

- Matrix 169 → **170 rows: ✅ 105 · ❌ 25 · 🛑 34 · 🤷 6**.
- A new Evidence subsection, `### xterm's ctlseqs`, quoting the document for every claim §84 rests on —
  **including the two claims it does not support**, which is the part a later sweep will want.
- Six notes reworded, one row split in two, two Code cells corrected.

### Not done

- **One source, one pass.** ECMA-48 and the VT420 manual were not read. The rows resting on DEC's
  definitions rather than xterm's — DECSCA, DECSED / DECSEL, the four rectangle operations, DECSACE's
  default, whatever unit DECMSR really uses — are still unquoted, and DEC is where the terminal's own
  vocabulary comes from.
- **Roughly 140 rows were not checked.** The sweep went where the parameters are: the CSI query family,
  the rectangles, the graphics rows, the modes. The SGR table was skimmed and the OSC table barely
  touched — `OSC 104` and `110`–`112` did not appear in the fetched text at all, so their definitions
  stand as written from memory.
- **Nothing records which rows have been read back.** A reader cannot tell a verified definition from an
  unverified one, which is the same complaint §83 made about the marks and now applies one column over.
  A per-row checkmark would say it; a fifth symbol in a document that spent §66 retiring one is not
  obviously the way.
- **XTPUSHSGR is now a named gap and stays one.** It would cost a stack of the pen and a scanner, both
  of which cmote has the shape for, and no program has been named that sends it.

## §85 — The stack that was refused under another name (v4.0.0)

§84 found `CSI # p` / `# q` wearing the colour stack's name and the colour stack's argument. This
section answers the sequence that was actually there.

**XTPUSHSGR / XTPOPSGR** — `CSI Pm # {` and `CSI # }`, with `CSI # p` and `CSI # q` as xterm's aliases
"used to work around language limitations of C#" — push the current **video attributes** onto a stack
and pop them back. `Pm` names which in SGR's own numbering (`1` bold … `21` doubly-underlined, plus `30`
and `31` for the two colours, which have no SGR code of their own); no parameter at all saves them all;
the stack is ten deep.

### Why this was work rather than a refusal

The argument the row had been carrying — *"a stack over a palette that is never read has nothing to save
or restore"* — belongs to XTPUSHCOLORS, which is `CSI # P` / `# Q`. Against this sequence it is void.
The attributes it stacks are bold, faint, italic, underline, reverse, conceal, strikeout and the
foreground and background — every one of which cmote draws.

And the failure mode of ignoring it is not an absent feature, it is a **wrong screen**. A program that
pushes, paints itself red and pops expects the pen it had; a terminal that drops both halves goes on
painting red. That is worse than the usual cost of an unimplemented sequence, which is that nothing
happens.

There was also no ground to refuse on. Nothing here leaves the tab, nothing speaks for the machine,
nothing touches anything of the user's — §6's rule is that a remote may change what its own tab looks
like, and this is exactly that. A 🛑 would have had to rest on price, as DECSLRM's does (§5, §73), and
the price turned out to be a scanner and two functions.

### What shipped

`src/term/sgrstack.rs`, the same chunk-safe CSI machine `tabs.rs` and `dsr.rs` use, matching the `#`
intermediate with no private marker — DECSTR (`! p`), DECRQM (`$ p`) and DECSCUSR (`SP q`) each sit one
intermediate away, so all three of final byte, marker and intermediates are tested (§56's near-miss
rule). `Mask` turns xterm's eleven values into cmote's own bitset once, where it can be tested without a
terminal.

**The pen is read, never written.** A push copies the engine's template cell — the same field DECRQSS
reports (§33) — and a pop feeds the engine that pen spelled in SGR. §72's route for DECSTR and §74's for
DECST8C, and for the same reason: the engine stays the only writer of its own template (§71, §73), so
there is no second source to disagree with it later. Fed bytes go straight to the parser, so cmote's own
scanners never see them and this cannot feed itself.

**Split-fed**, for DECXCPR's reason (§82): the pen a push saves is the pen where the push was *written*.
A chunk carrying `CSI [1m CSI # { CSI [3m CSI # }` must save bold and not bold-italic, and a test asks
exactly that.

Three details are cmote's own:

- **`pen_restore` is not `pen_sgr`.** The DECRQSS reply reports a curly, dotted or dashed underline as a
  plain `4` — honest as an answer, lossy as a restore, since feeding it back would straighten the
  program's underline. So the restore string spells the substyles (`4:3` / `4:4` / `4:5`) and carries the
  underline colour (SGR 58) that DECRQSS never reports at all.
- **The protection bit survives.** cmote borrows a spare bit of the engine's flag word for DECSCA (§56),
  and the `CSI 0 m` that opens a restore assigns that whole word. A stack of *video* attributes must not
  clear a cell-protection setting, so it is read across the restore and put back — the care
  `protect::ProtectRequest::Reassert` takes after an ordinary SGR, on the one path that does not go through the
  scanner.
- **An overflowing push drops its own pop.** xterm's stack is ten deep and an eleventh push is dropped;
  cmote drops it too, and *counts* it, so the pop that matches it is dropped as well. Without the count
  every pop after an overflow is one level out — outer attributes restored at an inner level, with no
  error anywhere. One `usize`, and a test that pushes eleven times and pops eleven times and lands back
  on the pen the first push saw.

**A selective push merges at pop time.** `CSI 30 # {` saves the foreground alone, so its pop has to put
that back while leaving whatever the program has done to everything else. The target pen is computed
first — current, with the named attributes taken from the saved one — and then written once, which
avoids emitting per-attribute "off" codes and the trap in xterm's own `22` (neither bold nor faint),
which takes two attributes out where one was meant.

### What it cost

- `src/term/sgrstack.rs`, new: the scanner, the mask and the request. Fifteen tests.
- `src/term/mod.rs`: one scanner field, two state fields, a `Interruption` variant, `apply_sgr_stack`,
  `merged_pen`, `pen_restore`, `sgr_underline_color`, and ten seam tests.
- Tests 1159 → 1183. Matrix 170 rows, **✅ 105 → 106, ❌ 25 → 24**, 🛑 34 and 🤷 6 unchanged.
- `term/dsr.rs`'s header carried §84's two errors in prose (`75` as a memory self-test, `26` as bare
  "nationality") and was corrected with the matrix.

### Not done

- **`Ps = 5`, blink, names nothing.** The engine has no blink flag at all (§5), so a push that asks for
  blink saves a value that does not exist and its pop restores nothing. Disclosed where it is parsed and
  on the row, not worked around.
- **`4` and `21` name one field between them.** xterm has them as separate parameters; cmote's underline
  is one attribute with variants, so a selective push of either moves all of them and the underline
  colour with them. A program that pushes `21` alone and expects its curly underline untouched gets it
  restored too.
- **The OSC 8 hyperlink does not travel.** It rides in the same cell as the attributes but is not an SGR
  attribute, so a push does not save it and a pop does not restore it. xterm has no hyperlink in its
  stack either, which is the argument for leaving it, not a proof.
- **The stack is never cleared.** Not by RIS, not by DECSTR, not by the alternate-screen swap. A program
  that pushes and dies leaves ten pens held until the tab closes — bounded, so not a hazard, but a
  program that pushed before a full reset and pops after it gets a pen from before the reset. Which of
  those DEC or xterm intends is not written down anywhere read so far.
- **No consumer named.** Same as §82: nothing was found that sends this, and the decision rested on the
  failure mode rather than a census. Unlike §82 there is not even a stalled sender to point at — this
  sequence draws no reply, so a program that sends it into a terminal that ignores it simply renders
  wrong and carries on.
- **`term/` now holds a tenth chunk-safe CSI state machine**, near-identical to the ninth. §82 noticed
  this and did not act; §85 has not either, and the case for factoring them is one row stronger.

## §86 — What a hard reset takes with it (v4.0.0)

§85 shipped the video-attribute stack and listed, under *Not done*: **"the stack is never cleared. Not
by RIS, not by DECSTR, not by the alternate-screen swap… a program that pushed before a full reset and
pops after it gets a pen from before the reset."** This closes the first of those three and states why
the other two stay open.

### RIS empties it

`ESC c` puts the terminal back to power-on, and a power-on terminal has nothing pushed. Leaving the
stack standing across it means a remote's state outliving the one sequence whose entire job is to remove
it — and worse than a stale pen, a pen the program has no way to predict, since what it gets back
depends on how many pushes preceded a reset it may not have sent itself.

The `dropped_pushes` counter goes with it. A reset that emptied the pens and kept the counter would
swallow the first pops of the NEW session to pay for an overflow in the old one.

`term/sgrstack.rs` reads `ESC c` itself rather than taking `term/scp.rs`'s word for it, though that
module already scans the same byte for its own store (§76). Two scanners reading one byte is the house
arrangement — every scanner in `term/` reads the stream itself — and the alternative couples two
features so that one's idea of where a sequence sat becomes the other's.

### DECSTR does not, and the swap does not

**The soft reset** is the split `term/rect.rs` already makes for DECSACE, one row over: RIS resets the
attribute-change extent, DECSTR does not, because DEC's published DECSTR list does not name it and §72
honours that list rather than widening it. XTPUSHSGR is an xterm extension DEC never listed at all, so
widening the list to reach it would be inventing an item for a document that has one.

**The alternate-screen swap** saves and restores the *pen* — that is what mode 1049 does, through DECSC
and DECRC — and a stack of pens is not the pen. A program that pushes on the primary screen, swaps, and
pops is doing something odd, but nothing about the swap says its stack should evaporate.

Both are judgements rather than findings, which is worth saying plainly: **no source read so far states
what xterm does with its own stack at either point.** ctlseqs describes XTPUSHSGR's parameters and its
ten levels and says nothing about its lifetime. What is recorded here is the reasoning, so the next
reader knows it is reasoning.

### What it cost

- `term/sgrstack.rs`: a `Request::Reset`, one arm in the scanner, two tests.
- `term/mod.rs`: one match arm, two seam tests — one that a hard reset throws the stack away, one that a
  soft reset leaves it standing.
- Tests 1183 → 1187. No row's mark moves; the matrix row gains a sentence.

### Not done

- **The two open cases are open on purpose and unverified.** If xterm clears its stack on either, cmote
  now differs from it in a way nothing here would notice.
- **Nothing tests the counter across a reset in isolation** — the seam test asserts the pen, which is
  what a program sees, and the counter only shows through it after an overflow. A test that overflows,
  resets and then pops would pin it directly.

## §87 — The sweep reaches the OSC table, and stops (v4.0.0)

§84 read §83's definitions back against xterm's ctlseqs and left, explicitly: *"roughly 140 rows were
not checked… the OSC table barely touched."* This is the next slice of that, and it produced two
corrections, two new tests and one blocker worth writing down.

### The blocker

**ctlseqs' OSC list cannot be read through the fetch.** Every attempt returns the document truncated
part-way through `Ps = 4` — it ends mid-sentence on "Change Color Number *c* to the color specif" — so
`OSC 8`, `10`–`12`, `22`, `50`, `52`, `104` and `110`–`112` have no primary source behind them yet.
Three did arrive and all three match the matrix as written: `0` changes the icon name *and* the window
title, `1` the icon name, `2` the window title.

What those rows were checked against instead is `vte` 0.15.0 — which is the operative source for what
cmote *does* and not for what the sequence *means*. The distinction is the whole of §84: a row can
describe the crate perfectly and still describe the sequence wrongly, which is exactly how `CSI # p`
spent nineteen sections wearing the colour stack's name.

### Two corrections, both of the same shape

The colour OSCs take **lists**, and the matrix had all three as single requests.

- **`OSC 4`** reads its parameters in `index ; spec` pairs — it refuses an even count outright and then
  walks them two at a time — so `OSC 4 ; 1 ; ? ; 3 ; ?` is two questions and draws two replies.
- **`OSC 10 / 11 / 12`** are one arm over three codes, and a list **walks up** from the code it started
  at: `OSC 10 ; ? ; ?` asks for the foreground and then the background, stopping at the cursor.
- **`OSC 104` bare resets all 256 slots.** The row said "puts one palette slot back". For a row that is
  🛑 the practical difference is nil — cmote's renderer never reads that table either way — but the
  definition was wrong about what was being refused, which is the thing §83 said a note is for.

### Both query forms are now pinned

The rows claim the list behaviour, so two tests exercise it: `OSC 4 ; 1 ; ? ; 3 ; ?` gets red and yellow
back in the order asked, and `OSC 10 ; ? ; ?` gets a `10;` reply followed by an `11;` one. That is
§84's finding turned into a habit — a matrix full of definitions nothing had ever run is how the last
five errors survived four sweeps.

### What it cost

- Three notes reworded, two tests, two Evidence bullets — one for the crate's list handling, one
  recording that the OSC section of ctlseqs is unread and why.
- Tests 1187 → 1189. No mark moves; the matrix stays at 170 rows, ✅ 106 · ❌ 24 · 🛑 34 · 🤷 6.

### Not done

- **The OSC rows still have no primary source.** A different route to the document — the plain-text
  build, or the manual page — would close it, and none was tried beyond the fetch that truncates.
- **The SGR table is still unchecked**, and it is the table most likely to be right by familiarity and
  therefore least likely to be read.
- **The vendor rows have no source at all and cannot have one from xterm**: `OSC 7`, `9`, `9;4`, `9;9`,
  `133`, `777`, kitty's `21` and `99`, and the whole `iTerm 1337` namespace are other terminals'
  extensions, documented — where they are documented — by those terminals. §87 did not go looking.
- **`OSC 50` is a case in point and is left standing.** cmote reads it as a cursor shape because `vte`
  does; whose dialect that is, and what xterm means by `OSC 50`, is precisely what the truncated fetch
  could not say.

## §88 — The font on the cursor's code, and an answer §79 was owed (v4.0.0)

§87 stopped at a blocker: ctlseqs' OSC list is past where a fetch of the HTML build returns. The plain
text build reaches further, and it settled the sweep's most useful question first.

### `OSC 50` is the font

> **Set Font to *Pt***. These controls may be disabled using the `allowFontOps` resource. If *Pt*
> begins with a `#`, index in the font menu, relative (if the next character is a plus or minus sign)
> or absolute.

The matrix has had `OSC 50` down as the cursor shape since §60 found it working. What actually happens
is that `vte`'s `OSC 50` arm tests for a `CursorShape=` prefix, honours that, and drops everything else
to `unhandled` — so cmote reads a **different terminal's convention** on the number xterm uses for the
font, and the row said nothing about it.

Split, on §68's rule. The `CursorShape=` spelling keeps its ✅ and now says whose it is. The font is a
new 🤷 row with §6's own argument: the font is chrome the **user** chose, exactly the ground the fixed
colour scheme stands on — and nothing here performs the refusal, the parser dropping it before cmote
sees a byte.

That is the fifth row this document has found describing one sequence under another's name, and the
second in five sections (§84 was `CSI # p`). Both were found the same way: reading the number back
against the document rather than against the crate that implements it.

### `OSC 22` resets what it does not recognise

> Change pointer cursor shape to *Pt*… If *Pt* is empty, or **does not match any of the standard
> names, xterm uses the resource's default 'xterm' shape**.

cmote's allow-list (§77) does the opposite: a refused name leaves the pointer as it was. That is the
better behaviour here and now says so on the row — a remote that may not set a shape must not be able
to clear one either, which xterm's fallback would hand it for free.

### `OSC 8`'s spec, and the parameter cmote does not read

The hyperlink spec settles three things the row was carrying loosely. The close is `OSC 8 ; ; ST`.
`params` is "an optional list of key=value assignments, separated by the `:` character", of which
exactly one is defined. And on the schemes: "it's up to the terminal emulator to decide what schemes it
supports" — which is the ground `ALLOWED_SCHEMES` stands on, quoted on the row that refuses.

The defined key is `id`, and **cmote does not read it**: "character cells that have the same target URI
and the same nonempty id are always underlined together on mouseover", where `link_run_at`
(`ui/grid.rs`) walks the contiguous run of cells sharing the URI. So two runs of one link with a
matching `id` underline apart, and — the direction that is actually wrong — two *different* links that
happen to share a URI underline together. Disclosed on the row rather than fixed, because fixing it
means the engine storing the id, and `Cell::hyperlink` gives back a URI.

### The answer §79 was owed

§79 noticed `vte`'s OSC payload buffer and left the question open in as many words: *"whether a hostile
stream can make the parser hold an unbounded OSC payload is a question about every OSC cmote sees, not
about this row."* It can.

`MAX_OSC_RAW = 1024` bounds an `ArrayVec` that only exists under `not(feature = "std")`. With `std` —
which is what `alacritty_terminal` pulls in — the buffer is a plain `Vec<u8>` and the fullness check is
compiled out, so a remote that writes `ESC ]` and never terminates the string makes the parser
accumulate all of it. Ordinary text is bounded by `SCROLLBACK` = 10 000 lines. An unterminated OSC is
bounded by nothing.

It is recorded and priced rather than fixed, and the reason is in the state machine: every byte that
ends an OSC in `vte` — BEL, CAN, SUB, ESC — routes through `osc_end`, which **dispatches** what has
accumulated. There is no abort that discards. Feeding a CAN, the mechanism §57 uses for DECSLRM, would
deliver a truncated OSC and then leave the rest of the payload to be printed to the screen as ordinary
text: a megabyte of garbage in place of a megabyte of memory, which is a worse outcome for the person
at the terminal. Discarding the bytes instead means filtering the stream on the way in, which §41
refuses for reasons of its own. What is left is a wrapper around the parser — the same price §5 puts on
the left/right margins, and the same answer for now.

### What it cost

- One row split (`OSC 50`), three notes amended (`OSC 8` both halves, `OSC 22`'s refusal).
- Two Evidence bullets: the ctlseqs text-build quotes with the OSC 8 spec beside them, and the payload
  buffer with its price.
- Matrix 170 → **171 rows: ✅ 106 · ❌ 24 · 🛑 34 · 🤷 7**. No code changed.

### Not done

- **`OSC 8`, `104` and `110`–`112` are still unsourced.** Even the text build is returned short of them.
- **The `id` parameter stays unread**, so the hover underline is wrong in both directions for links that
  share a URI or split a run.
- **The unbounded OSC payload stays open**, with the wrapper priced and not built. If it is ever built
  it should carry DECSLRM (§73) with it, the two being the same purchase.
- **The vendor rows still have no source**: `OSC 7`, `9`, `9;4`, `9;9`, `133`, `777`, kitty's `21` and
  `99`, and the whole `iTerm 1337` namespace are other terminals' extensions, and §88 went to xterm.
- **The SGR table is still unchecked.** Three sweeps have now gone past it.

## §89 — Two thirds of the OSC table is nobody's standard (v4.0.0)

§87 and §88 went to xterm and found that most of the OSC table is not xterm's: `OSC 7`, `9`, `9;4`,
`9;9`, `133`, `777`, kitty's `21` and `99` and the whole `iTerm 1337` namespace are extensions one
terminal invented and others copied. §88 listed them as unsourced. This reads each against its own
vendor's documentation.

### What the vendors said

- **kitty `OSC 21`.** The key names were wrong in three places — `cursor_text` not
  `cursor_text_color`, `visual_bell` not `visual_bell_color`, `transparent_background_color1..7` not
  `1–8` — and the palette keys are bare numbers, not `color0`–`color255`. The reset is a **bare key
  with no `=`**, where the row had `key=`.
- **kitty `OSC 99`.** The metadata keys are `p` `i` `d` `e` `f` `u` `n`, colon-separated; the row said
  "identity, urgency and icon metadata" and can now name them.
- **iTerm `OSC 1337`.** `RequestAttention` takes a value (`yes`, `once`, `no`, `fireworks`);
  `SetBackgroundImageFile` takes base64 and an empty value removes the image; `SetColors` keys are
  `fg` / `bg` / `bold` / `link`; and `ReportCellSize`'s reply carries a **scale** beside the height and
  width, which the row had not mentioned.
- **ConEmu `OSC 9`.** The five progress states are exactly what the row claimed.
- **`OSC 7`** is macOS Terminal's, and its argument is a full `file://HOSTNAME/PATH` URL.

### The finding that matters

**ConEmu's `OSC 9` is multiplexed five ways and cmote's model of it had three.** Besides `9;4`
progress and `9;9` the working directory, ConEmu defines:

    ESC ] 9 ; 1 ; ms    ST      sleep the terminal for ms milliseconds
    ESC ] 9 ; 2 ; "txt" ST      raise a GUI message box
    ESC ] 9 ; 3 ; "txt" ST      set the tab's text

None had a row. All three are refused today — but *by being classified as desktop notifications*, which
is what `term/notify.rs` calls any `OSC 9` payload that is not `9;4;` or `9;9;`. The outcome is right
and the reason is wrong, which is the failure this document has now found five times.

Two of them are worth more than a relabel. **`9;1` is a remote pausing the terminal** — a denial of
service against the person at the keyboard, not against the host. **`9;2` is a remote raising a modal
dialog** on the desktop, which is the notification refusal's own argument only more so: it does not
merely leave the window, it takes the focus. Both must stay refused, and §91 makes cmote's own code
say so rather than leaving them to fall through an arm meant for something else.

The third, `9;3`, is a *feature* — and §90 ships it.

### And one argument that expired

§78 refused kitty's `OSC 21` on four reasons, and the second was that answering the keys cmote lacks
would mean **inventing a colour**, which `palette.rs` opens by forbidding. kitty's protocol has an
answer for exactly that case: a query for a colour the terminal does not have is answered with an
**empty value**. So the invention was never required.

The row does not move. The **dialect** reason is the load-bearing one and it is untouched: `TERM`,
XTVERSION and XTGETTCAP's `TN` all say xterm, and a caller sends `OSC 21` only after concluding kitty.
But this is the fourth argument in this document found to have expired while its conclusion stayed
right (§70, §82, §84, and now this), and the pattern is the same every time — the *reason* was never
re-read because the *mark* kept checking out.

### Two that could not be sourced

- **`OSC 133`.** Its specification is Per Bothner's, on `gitlab.freedesktop.org`, which serves an
  access-control page to this reader. Every terminal that documents OSC 133 points there instead of
  restating it, so the whole chain is behind one door.
- **`OSC 777`.** urxvt's manual page documents **no OSC 777 at all**. The sequence is real and widely
  emitted, but "urxvt's" — which this table and `term/notify.rs` both assert — is an attribution
  neither has a citation for. Recorded as folklore on the row rather than quietly kept.

### Also in this pass

Eleven CSI rows that §84 read against ctlseqs but never marked are stamped `§84`. Counting which rows
have a source behind them is how §89 knew where to start, and the count was wrong by eleven — §84's own
fourth *Not done* ("nothing records which rows have been read back") made visible by trying to use it.

### What it cost

- Ten notes corrected, eleven stamped, one Evidence subsection.
- No mark moves and no row is added yet: **171 rows, ✅ 106 · ❌ 24 · 🛑 34 · 🤷 7**. The three ConEmu
  sub-codes get their rows in §90 and §91, with the code that handles them.

### Not done

- **`OSC 133` and `OSC 777` stay unsourced**, and one of them is a feature cmote ships (§34).
- **`OSC 8`, `104`, `110`–`112`** are still only `vte`'s, from §88.
- **kitty's `OSC 30001` / `30101`** — push and pop the colour stack — have never appeared in this
  table at all. They are the same refusal as `OSC 21`'s set half and `CSI # P`, and they have no row.
- **The SGR table is still unchecked.** Four sweeps have gone past it.

## §90 — ConEmu's OSC 9 is five sequences, and cmote knew two (v4.0.0)

§89 read ConEmu's own page and found `OSC 9` multiplexed five ways where this project had modelled
three of them — two honoured and everything else swept into one refusal:

    ESC ] 9 ; 1 ; <ms>      sleep the terminal
    ESC ] 9 ; 2 ; "<txt>"   raise a GUI message box
    ESC ] 9 ; 3 ; "<txt>"   set the tab's text
    ESC ] 9 ; 4 ; st ; pr   progress            — honoured since §54
    ESC ] 9 ; 9 ; "<cwd>"   working directory   — honoured since §17

The first three had no row. All three were refused, and all three were refused **as desktop
notifications**, because `term/notify.rs` calls any `OSC 9` payload that is not `9;4;` or `9;9;` a
notification. The outcome was right for two of them and wrong for the third, and the *description*
was wrong for all three — which is the failure this document has now found six times, and the first
time it has found it in cmote's own code rather than in a note.

### The two that stay refused, now by name

**`9;1` sleeps the terminal.** It is the one refusal in this project that is not about something
leaving the tab. Everything else here — notifications, the clipboard, window ops, inline images — is
refused because it reaches past the tab into the desktop or the user's own data. This one reaches
into the user's **time**: a remote that can say `ESC ] 9 ; 1 ; 60000` holds the window still for a
minute in front of the person at the keyboard, and a broken or hostile host can do it in a loop with
eleven bytes a go. Nothing in cmote would honour it, and after §90 nothing can start to by accident.

**`9;2` raises a GUI message box.** The notification argument at one further remove. A notification
leaves the window; a modal dialog leaves the window **and takes the focus**, carrying text the remote
chose in a window wearing cmote's identity. §54's line covers it and now says so.

Both are `NotifyRefused::Sleep` and `NotifyRefused::MessageBox`, named in the enum `notify.rs` already used to
say *which* refusal a payload was — the same reasoning §79 gave for that enum existing at all: a
refusal that cannot say what it refused is one no test can audit.

### The one that ships

**`9;3` sets the tab's text**, which is what `OSC 1` already does and what cmote's chip already is.
Two spellings of one field, through one module, which is exactly the test §71 set for a second
spelling: a spelling is refused when it would be a second **source** for a field somebody else owns —
iTerm's `CursorShape`, which would write the engine's cursor from outside — and honoured when it
reaches the same field through the same writer, as `OSC 50` does for DECSCUSR's shape.

It is honoured for `9;9`'s reason (§17): **cmote is a Windows client**, and ConEmu's vocabulary is
what a Windows-side shell reaches for. The value is quoted in ConEmu's documentation exactly as
`9;9`'s path is, and `term/icon.rs` trims the quotes the way `term/cwd.rs` already does for that one.

Nothing about the chip's *rules* moves: the name is capped at 24 characters, sanitised, and appended
**after** the endpoint (§55's anti-spoof rule), and an empty name clears it. A remote gets no more of
the chip through the new door than it had through the old — which is the whole of why adding a second
spelling is safe here and would not be if the chip's label could be replaced rather than extended.

### What it cost

- `term/notify.rs`: two enum variants, three lines of matching, `Spelling` renamed `NotifyRefused` for a
  name that covers what it now carries, and the header's map of the multiplex.
- `term/icon.rs`: one alternative prefix and the quote trim, plus the header's argument for two doors.
- Three new matrix rows, six new tests, two at the seam.
- Tests 1189 → 1197. Matrix 171 → **174 rows: ✅ 107 · ❌ 24 · 🛑 36 · 🤷 7**.

### Not done

- **`9;1` and `9;2` are still silent**, like every other refusal here. A program that asks for a
  dialog and gets nothing cannot tell that from a terminal that has not heard of the sequence.
- **The bare `OSC 9 ; <text>` attribution is still folklore.** ConEmu's page documents `9;1`–`9;4`
  and `9;9` and no bare form, so the notification spelling this table credits to ConEmu may be
  Windows Terminal's alone (§89). Recorded in the module header, not chased.
- **Nothing tells the user a tab was renamed by the remote** rather than by cmote. That was true of
  `OSC 1` before this and is no more or less true now, but a second spelling doubles the ways in.
- **`9;3` is not cleared by RIS**, exactly as `OSC 1` is not (§69's note about the title's own
  survival). Consistent with the old spelling, which is the point, and still nobody's decision.

## §91 — The font is the user's (v4.0.0)

§88 found that xterm's `OSC 50` is **Set Font** and split the row: the `CursorShape=` payload cmote
honours on that number is another terminal's convention, and the font half was left 🤷 — a refusal
this document held and nothing in the program performed.

§90 gave `term/notify.rs` a charter wide enough to hold it: not "the notification spellings" but
"the OSC payloads cmote refuses outright". So the font goes in beside them.

**The argument is §6's, unchanged.** The font is chrome the **user** chose, exactly as the colour
scheme is, and a remote may change what its own tab shows rather than what the application looks
like. xterm agrees in its own way: it gates these operations behind an `allowFontOps` resource whose
default is off, which is a terminal shipping the same policy as a setting rather than as a rule.

**What changes is who says it.** `vte` drops a non-`CursorShape=` payload on `OSC 50` to `unhandled`,
so nothing here would have honoured it either way — the same position `kitty 99` and `OSC 777` were
in before §79, and the same answer: a refusal resting on nobody happening to match is one no test can
see and one an engine bump can undo in silence.

**The exclusion is the careful part.** `OSC 50 ; CursorShape=N` is a shipped feature (§60, §71) on the
number being refused, so it is excluded by name and asserted from both sides — a refusal that
swallowed the cursor shape would break something working in order to tighten a policy that already
held. An **empty** payload (`OSC 50 ;`) is refused: it is still xterm's font namespace, and the
permissive reading would let through a payload cmote does not understand.

### What it cost

- `term/notify.rs`: one enum variant, four lines, one test asserting both directions.
- The matrix row moves **🤷 → 🛑**. 174 rows: ✅ 107 · ❌ 24 · **🛑 37 · 🤷 6**.
- Tests 1197 → 1198.

### Not done

- **It changes no behaviour, and is not claimed to.** Nothing in cmote could set a font if it wanted
  to — there is no font-setting path to guard.
- **Silent, like every refusal here.** xterm answers an `OSC 50 ; ?` font query; cmote does not, and a
  program asking gets the same silence as one asking for a notification.
- **`allowFontOps` is a setting where cmote has a rule.** If a user ever wants a remote to pick the
  font — a plausible thing for one's own machine — this is a policy to revisit, not a law.

## §92 — A link is not its address (v4.0.0)

§88 read OSC 8's specification and found the one parameter it defines:

> `params` is an optional list of key=value assignments, separated by the `:` character… Character
> cells that have the same target URI **and the same nonempty id** are always underlined together on
> mouseover.

cmote read neither. `link_run_at` walked outward from the hovered cell while neighbouring cells
carried the same **URI**, and returned that contiguous span. Two things were wrong with it, in
opposite directions, and §92 replaces the walk rather than widening it.

### Correcting §88 first

§88's row note said the URI comparison meant "two different links with one URI underline together",
and while writing this section that looked wrong: `alacritty_terminal`'s `Hyperlink` derives
`PartialEq` over both its `id` and its `uri`, so comparing whole links would have told them apart.

It is wrong one layer further down, and §88's claim survives: **cmote's own seam drops the id**.
`term/screen.rs` built its cell with `hyperlink: cell.hyperlink().map(|link| link.uri().to_owned())`,
so by the time the renderer compared anything, only the address was left. The engine could tell two
links apart and cmote had thrown away what it needed to.

### The two directions

**One address written twice is two links.** A page that prints the same URL on two adjacent runs — a
listing, a diff, a table of the same host — had both underlined when the pointer was over either.
The engine gives every `ESC ] 8` that carries no `id=` an identifier of its own (`<counter>_alacritty`),
so it always knew they were different; the seam did not.

**And a link split into runs is one link.** This is the case the parameter exists for, and the
specification's own worked example: a program writes part of a URL, something else, then the rest,
tying them with `id=1`. The contiguous walk stopped at the gap and underlined half a link.

### What shipped

`term/screen.rs` carries a `Link { id, uri }` instead of a bare URI. `Cell::hyperlink` still hands
back the URI on its own — that is what opening one needs, and `link.rs`, the context menu and the
pointer path are untouched — and `Cell::link` hands back the pair, for the one question that needs
it: are these two cells the same link?

The id has **no accessor**. It is for comparing links, never for showing or following one, and a
generated `3_alacritty` means nothing to anybody — so the only way to use it is the derived
`PartialEq`, and there is no way to read it out and act on it.

The renderer stops computing a span. `link_at` reads the link under the pointer, `plan_runs` carries
that link rather than a range of row-major indices, and each cell asks whether it holds the very same
one. That is O(1) per cell where the old walk was O(link length) per hover, it needs neither the row
count nor the column count, and it implements the specification exactly — including the
non-contiguous case, which no span could have expressed.

### UX

Nothing about the affordance changes: the same single underline, on the same Ctrl-hover, over the
same cells in every case that behaved correctly before. What changes is the two cases that did not —
one link too many underlined, or one link underlined by half.

### What it cost

- `term/screen.rs`: a `Link` type, one accessor beside the existing one, the id carried across the
  seam.
- `src/ui/grid.rs`: the walk deleted, `hovered_link_run` → `hovered_link`, `plan_runs` re-parameterised,
  the per-cell test rewritten. `std::ops::RangeInclusive` is no longer imported there.
- Three unit tests, one per direction and one for the ordinary case; the hover-underline test now
  builds its argument from a real link rather than a hand-written range.
- Tests 1198 → 1200. **No row's mark moves**: 174 rows, ✅ 107 · ❌ 24 · 🛑 37 · 🤷 6.

### Not done

- **The underline still stops at the visible page.** A link whose other run has scrolled off is
  underlined only where it shows, which is what a renderer that draws the viewport can do, and is
  worth stating rather than discovering.
- **The `id` is not used for anything else**, though the specification's intent is broader: an id is
  what would let a click on either run of a split link open one URI, which already works because both
  runs carry the URI anyway.
- **`link.rs`'s scheme policy is unchanged and unexamined here.** A refused scheme is still drawn and
  never opened (§24), and now underlines whole when hovered — the affordance says "this is one link",
  not "this will open".

## §93 — The two replies that advertise nothing (v4.0.0)

§82 refused all nine of the DEC-private status reports beside DECXCPR, and disclosed two of them
under *Not done*:

> **`55` and `56` have an honest negative that is not sent.** xterm answers "no locator"
> (`CSI ? 53 n`) and "cannot identify" (`CSI ? 57 ; 0 n`), and those advertise nothing… The
> stalled-sender argument that carried DECXCPR applies to them exactly. They are silent because
> nobody has asked, which is the honest reason and not a good one.

They are sent now, and the line they cross is the point of this section.

### Why these two and not the other seven

The family is refused because **a reply is an advertisement** (§71). `CSI ? 15 n` answering "printer
ready" is a claim about hardware that is not there; a macro-space byte count invents a store; and
`CSI ? 26 n` would name the user's keyboard layout, which §36 forbids outright.

The locator pair is different in kind, and the difference is not a matter of degree. xterm's answers
for a terminal without a locator are **statements of absence**: "there is no locator", "the locator
cannot be identified". A terminal lacking the equipment can say those truthfully, which is exactly
what it cannot do for the other seven — there is no honest value for "how much macro space", only a
fiction or a silence.

So the rule the family is refused under does not reach them, and what is left is `query.rs`'s founding
argument: an unanswered query stalls its sender. A program asking whether a locator is there and
hearing nothing cannot tell that from a terminal still deciding, so it waits out its own timeout to
learn what six bytes could have told it.

It is the same shape as DECRQM's honest `0` for mode 69 — "not recognised" — which this document
already prefers to silence, and it is worth noticing that the preference was on record for one row
and not applied to these two for eleven sections.

### What shipped

`term/dsr.rs`'s allow-list widens from one value to three and its `feed` reports **which** question
each offset carried, so `term/mod.rs` answers a cursor position from the live cursor and the two
locator questions from constants. `Interruption::CursorReport` becomes `Interruption::Dsr(dsr::DsrRequest)`.

The two negatives ride the split even though they are constants and could have waited for the end of
the chunk like XTVERSION does. One route through one scanner is easier to keep right than two, and
the ordering falls out for free — a test asks both in one write and gets the answers back in the
order the questions were written.

### What it cost

- `term/dsr.rs`: two constants, a `Request` enum, the allow-list widened, two tests.
- `term/mod.rs`: the split variant carries its request, `report_cursor_position` becomes `answer_dsr`,
  one seam test added and one narrowed from nine parameters to seven.
- The row moves **🛑 → ✅**. 174 rows: **✅ 108 · ❌ 24 · 🛑 36 · 🤷 6**.
- Tests 1200 → 1202.

### Not done

- **The locator protocol itself is still absent** — DECELR, DECSLE, DECRQLP, the locator reports. The
  row that moved is the two *questions*, and answering them honestly is what a terminal without the
  protocol should do. There is no row for the protocol, and nobody has asked for one.
- **The other seven stay silent**, and silence still tells a program nothing. The stalled-sender
  argument applies to them too; what does not apply is an answer they could give truthfully, which is
  why they are refused rather than deferred.
- **`CSI ? 53 n` is sent even to a program that never asked about a mouse.** cmote does report mouse
  events (§10, modes 1000–1006), so "no locator" is true of the DEC protocol and might read to a
  careless caller as "no pointing device at all". The protocols are different and the answer is
  correct; the possible misreading is xterm's too.

## §94 — DEC's own manual, at last (v4.0.0)

§84 read the definitions back against xterm and closed with: *"ECMA-48 and the VT420 manual were not
read. The rows resting on DEC's definitions rather than xterm's — DECSCA, DECSED / DECSEL, the four
rectangle operations, DECSACE's default, whatever unit DECMSR really uses — are still unquoted, and
DEC is where the terminal's own vocabulary comes from."*

`vt100.net` hosts the VT510 programmer reference one page per sequence, which is exactly the shape
this needed. Four rows settled, one row added.

### Two open questions, both closed in cmote's favour

**DECSACE's default is `0`**, the wrapped stream — stated on DEC's own page, where ctlseqs lists the
three values and names no default. §84 had softened the row to "cmote powers up in stream" because it
could not confirm the claim; `rect.rs`'s `#[default] Stream` agrees with DEC, and the row can say so.

**DECFRA's range is DEC's, and so is the behaviour.** "`Pch` can be any value from 32 to 126 or from
160 to 255. If `Pch` is not in this range, then the terminal ignores the DECFRA command." §58 had the
range right and credited it to xterm; §88, finding no range in ctlseqs, rewrote the row to call it
"cmote's own allow-list". Both attributions were wrong and the range was right the whole time —
including the part nobody had checked, that DEC prescribes **ignoring the command** rather than
clamping or substituting, which is what `rect.rs` does.

That is worth naming for what it is: two sections in a row **moved a correct fact between wrong
owners**. §88's rewrite was more honest than §58's claim and still not right, because "the source I
can reach does not say this" is not the same as "this has no source".

### DECSTR's list, quoted rather than paraphrased

Eighteen items. cmote sends the eleven anything in this stack models — DECTCEM, IRM, DECOM, DECAWM,
DECNKM, DECCKM, DECSTBM, the charsets, SGR, DECSCA, DECSC — and the other seven (KAM, DECNRCM,
DECAUPSS, DECSASD, DECKPM, DECRLM, DECPCTERM) name state neither `vte`, nor the engine, nor cmote
has, so nothing is left stale by not sending them.

And the departure §72 took on purpose is now citable rather than described: the list says **"Autowrap
(DECAWM): No autowrap"**, and cmote leaves it on because `xterm-256color` declares `am` and its `rs2`
sends no `\E[?7h` after a soft reset. Two documents, and cmote follows the one whose name it answers
to in `TERM`.

### The row that was missing

**`CSI Ps # y` is XTCHECKSUM** — "the bits of `Ps` modify the calculation of the checksum returned by
DECRQCRA", with bits for negating the result, reporting the VT100 video attributes, omitting blanks,
omitting uninitialised cells, and masking the cell value to 8 bits.

It has been carried on this project's list of noticed-and-unchased things since §60, and it matters
to a row that ships: cmote's DECRQCRA answers with **xterm's algorithm at its DEC-compatible
default**, and this is the sequence that would let a program ask for a different one. A program that
sets it and then reads a checksum gets a number computed under rules it did not choose.

It is a ❌ rather than a refusal — nobody decided against it — and it dies where §88 found the rest of
the `#` intermediates dying: `csi_dispatch` matches no `#` at all, so XTCHECKSUM, XTPUSHSGR and both
colour stacks reach the same nothing.

### What it cost

- Four notes corrected, one row added, one Evidence subsection for DEC's manual.
- Matrix 174 → **175 rows: ✅ 108 · ❌ 25 · 🛑 36 · 🤷 6**. No code changed.

### Not done

- **DECSCA, DECSED and DECSEL were not read**, nor DECERA, DECSERA, DECCRA, DECCARA, DECRARA or
  DECRQCRA itself. The manual has a page for each and this pass took five. The rectangle family's
  behaviour is pinned by tests either way; what is unquoted is the *definitions*.
- **ECMA-48 is still unread**, and SCP (§76) is its sequence, not DEC's — the one row in the CSI table
  whose vocabulary comes from neither source this project has now consulted.
- **XTCHECKSUM is a gap and stays one.** Implementing it means five bits of behaviour on a checksum
  nobody has been observed to request, and the DEC-compatible default is the one a VT-conformant
  program expects. Named now, so the next reader chooses rather than not knowing.
- **DECMSR's unit is still unknown** (§84 asked; ctlseqs gives none; DEC's own DECMSR page was not
  fetched in this pass). The row no longer claims one, which is enough for it to be right, and not
  enough for it to be complete.

## §95 — OSC 133, sourced at one remove (v4.0.0)

§89 closed with two sequences it could not source, and OSC 133 was the one that mattered most,
because unlike OSC 777 it is a **feature cmote ships** — the per-tab status dot, jump-to-prompt and
select-command-output all rest on it (§34). The section's finding was that Per Bothner's
specification lives on `gitlab.freedesktop.org` behind an access-control interstitial, and that every
terminal documenting OSC 133 points there rather than restating it.

That last clause was wrong, and the user found the counter-example: **Contour restates it**
(`contour-terminal.org/vt-extensions/osc-133-shell-integration/`).

### What a restatement is worth, and what it is not

It credits no author beyond "inspired by FinalTerm" and lists no implementers, so it does not stand in
for the spec — it is one vendor's account of it, and the matrix's `[community]` tag for OSC 133 is
unchanged. What it does close is the difference between *unsourced* and *sourced at one remove*, and
§94 had just finished paying for confusing those two in the other direction.

Three of its four facts confirm what cmote already does: the four commands, `ST` as either `ESC \` or
BEL, and `D`'s exit code written `[ ; <ExitCode> ]` — **optional in the syntax**, which is what
`Mark::CommandEnd(Option<i32>)` has always assumed. Contour does not say what an absent one *means*,
so cmote's reading of it (show "done", never a wrong number) stays cmote's judgement and is now known
to be one.

### The fourth fact was a decision

The page names two optional `key=value` fields neither this matrix nor `term/osc133.rs` had recorded.

**`133 ; A ; click_events=1`** — *"indicates that the terminal should enable mouse click reporting
for the prompt area."* Refused, and on two grounds that agree:

- It is **input reporting switched on by a payload whose declared job is marking where the prompt
  sits**. Mouse reporting has modes that gate it (§10, `?1000`–`?1006`); a program that wants clicks
  can ask for clicks. This is the same shape as every side door this document has closed — a feature
  reachable through a sequence that was documented as being about something else.
- It is **inconsistent by construction**. A click inside the prompt region would go to the remote
  while a click one line above it selects text locally, with no visible boundary between the two.
  That is precisely the ordering the user set for this pass: UX stability and consistency over
  visual features.

**`133 ; C ; cmdline_url=<percent-encoded>`** — the command line being run. A ❌ rather than a
refusal: nothing in cmote names which command a range of output came from, so there is no reader,
and nobody has decided against having one.

### The refusal was already true, and that is the interesting part

`Scanner::parse` splits the payload on `;` and ignores every field past the letter, so
`click_events=1` was already dropped — and `Mark` has four variants, none of which carries a field,
so there is **no path from this scanner to a mouse mode at all**. The refusal is structural and
predates the decision by sixty sections.

Which makes this a 🛑 whose mechanism is a **test**, not a branch. Adding a branch to reject a field
the code cannot act on would be theatre; what was missing was the *statement*, because an incidental
drop and a deliberate one look identical until someone writes down which it is. The next reader who
sees `cmdline_url` in a capture and thinks "we could show that" will find the field beside it already
answered.

### What it cost

- `term/osc133.rs`: the header block gains the two fields and their reasons; one test pins both.
- Matrix: the `133` row now says BEL-or-ST and that trailing fields are ignored; two rows added; the
  §89 evidence bullet corrected from "could not be sourced" to what Contour supplies.
- 175 → **177 rows: ✅ 108 · ❌ 26 · 🛑 37 · 🤷 6**. Tests 1202 → 1203.

### Not done

- **Bothner's specification is still unread.** The interstitial has not moved, and a restatement
  cannot tell me what it leaves out — most obviously whether the `L` and `P` marks other terminals
  emit are in it. Contour documents four commands and no others; kitty and WezTerm are said to speak
  more, and neither was read in this pass.
- **`cmdline_url` stays a gap with an obvious use.** Select-command-output (§34) already knows a
  range of output belongs to one command; the field is that command's name. Nothing shows it because
  nothing has anywhere to show it, and inventing a place is a visual feature — the thing this pass
  was told to rank last.
- **`OSC 777` is still folklore** (§89): the sequence is real and widely emitted, and "urxvt's",
  which this matrix and `term/notify.rs` both assert, has no citation and is contradicted by urxvt's
  own manual page.

## §96 — A second restatement, and the half of a rule nobody had written (v4.0.0)

§95 took Contour's write-up of OSC 133 and closed with the shape of what it lacked. The user then
supplied a second one, `vtdn.dev/docs/osc/osc133/`, which is the better of the two and still not the
spec — it cites only VS Code's shell-integration page and gives no URL for FinalTerm or Bothner. Four
things came out of it, and the fourth is not about OSC 133 at all.

### It settles what §95 had to leave open

§95 recorded that Contour writes `D`'s exit code as optional but never says what an absent one
*means*, so cmote's reading — show "done", never a wrong number — stayed a judgement. vtdn gives the
bare form **its own line in the syntax table**, "Command finished (no exit code)", and a grammar
production to go with it: `"133", ";", "D", [ ";", exitcode ], ( 0x07 | 0x1b, "\\" )`.

So `OSC 133 ; D` is a **documented spelling**, not a malformation cmote is being generous about. The
behaviour does not change; what changes is that the test pinning it now says which of the two it is.

### Two more rows, both gaps

**`133 ; A ; cl=m`** is VS Code's, "to indicate a multi-line prompt". A ❌, and one that costs
nothing to leave: a prompt jump anchors on the `A` mark's own line, which is the prompt's **first**
line with or without the hint. Nothing in cmote needs the prompt's height.

**Phase letters past the four** are real. vtdn's Konsole entry reads "REPL mode tracking for prompt
(A/N/P), input (B), output (C), and completion (D)" — and then gives `N` and `P` no syntax and no
meaning anywhere on the page, which is the same wall §95 hit from the other side. `Scanner::parse`
already answers an unknown letter with `None`, and that is the right answer for a reason worth
writing down: a **wrong** mark moves a prompt jump or mis-bounds a command's output, where **no**
mark leaves both exactly as they were. Guessing `N` into the nearest phase would be a visible
misbehaviour bought with a guess.

### The citation §34 never had

vtdn's support table lists eleven implementers and thirteen non-implementers, and **Alacritty is in
the second list**. §34's founding claim — that the engine ignores OSC 133, so cmote must sniff the
bytes itself — has been asserted from reading the crate since it was written. It is now also
somebody else's published statement. The claim was never in doubt; it simply had one source, which
was this project.

### The half-rule

The same table lists **xterm** as not implementing OSC 133 — and cmote answers `xterm-256color` in
`TERM`, in XTVERSION and in XTGETTCAP's `TN`. That is the ground §78 refuses kitty's `OSC 21` on and
§82 refuses the DEC-private status reports on: *"answering a question in a dialect it does not claim
would be the one place it spoke as something else."*

Read carelessly, this pass just found cmote reading a sequence its own declared dialect does not
have — five of them, counting OSC 7 and iTerm's `SetMark`, `CurrentDir` and `SetUserVar`.

It is not a contradiction, and the reason is in §82's verb. **Speaking** is what the dialect rule
binds. A remote cannot detect that cmote read an OSC 133 mark: there is no reply, and no state it can
query afterwards. What it produces is a dot on a tab and a place for Ctrl+Shift+Up to land — visible
to the **user**, who chose this terminal, and invisible to the sender. Kitty's `OSC 21` is refused
because `key=?` **answers**, and because setting a colour is observable through `OSC 4`/`10`/`11`.

So the rule has two halves and only one was ever written:

- **cmote may read any dialect, silently.** A read that produces no reply and no queryable state
  cannot make cmote claim to be anything.
- **cmote answers only in the dialect it claims.** A reply is an advertisement (§71), and an
  advertisement in someone else's vocabulary is a claim to be them.

Written down now because until this table put "xterm ✗" next to a feature cmote ships, the first half
had never had to be defended.

### What it cost

- `term/osc133.rs`: the header block folds in the second source, the grammar and the letters past the
  four; the parameter test grows a third case; one test added for the unrecognised letters.
- Matrix: two rows, the `133` row's note, and the §89 evidence bullet gains vtdn beside Contour.
- 177 → **179 rows: ✅ 108 · ❌ 28 · 🛑 37 · 🤷 6**. Tests 1203 → 1204.

### Not done

- **`N`, `P` and `L` have no syntax from any source reached so far.** Konsole's own documentation was
  not fetched, and it is the one place vtdn points for them. Until then the letters are a named gap
  rather than a decision.
- **Bothner's specification is still unread**, and two restatements agreeing with each other is not
  the same as either being right. Both were written by people reading the same wall this project hit.
- **The half-rule is stated here and nowhere structural.** It is a description of what §17, §34, §55,
  §78 and §82 already do, not a check anything enforces; the next silent read of a foreign dialect
  will be judged the same way this one was — by hand.

## §97 — The field that was a bug, and the two that were a rule (v4.0.0)

§95 and §96 left three OSC 133 rows sitting at ❌, and the user asked for each to be settled: support
it or refuse it. Two settled together on a rule this project already had. The third turned out not to
be a gap at all — it was a **live misbehaviour**, and finding it took one more source.

### The source that mattered was not a specification

Contour and vtdn both describe the protocol. **kitty's shell-integration page describes the shell
code that emits it**, which is a different and better thing: it shows what a real shell actually
puts on the wire. Two facts came straight out of its zsh half:

```
mark2=$'%{\e]133;A;k=s\a%}'
[[ $PS2 == *$mark2* ]] || PS2=${mark2}${PS2}
```

`PS2` is the **continuation** prompt — the `>` a shell draws for each further line of a command still
being typed — and kitty prepends `133;A;k=s` to it. `PS1` carries no `k=` at all.

### What cmote was doing with that

`parse` ignores every field past the letter, so `133;A;k=s` read as a plain `Mark::PromptStart`, and
`Prompts::apply` does two things with one of those:

```rust
Mark::PromptStart => {
    self.state = CommandState::Prompt;
    self.record(history_size, row);
    self.pending = Some(Pending { prompt: absolute, output: None });
}
```

So typing a three-line `for` loop under zsh + kitty integration produced **three prompt anchors**:
three gutter ticks, three stops for Ctrl+Shift+Up — and the second half is worse than the cosmetic
half. Each `PromptStart` restarts `pending`, so the command finally filed at `D` was anchored to its
**last continuation line** instead of its prompt. Click that prompt's tick and the output it resolves
is not the one below it.

This was never a missing feature. It was a wrong answer that nobody had a shell configured to
produce.

**Fixed by reading the field in order to drop the mark.** An `A` carrying exactly `k=s` yields
`None`: cmote's model has four phases and a continuation prompt is none of them, and the stream is
already in the prompt phase when it arrives, so producing nothing leaves the state exactly right —
no new variant, no new state, one branch.

The match is on the exact value, and that asymmetry is deliberate: an unknown `k=` keeps the old
behaviour, because mistaking a real prompt for a continuation **loses** a jump anchor while the
reverse only adds one. Between two guesses, take the recoverable one.

### The two that were a rule

**`133 ; C ; cmdline=` / `cmdline_url=`** carries the command line — kitty's zsh half shell-quotes
it, its fish half percent-encodes it. **`133 ; A ; cl=m`** is VS Code's hint that the prompt spans
several lines. Both refused, on one ground they share:

> The command line is already on the grid, in the rows between `B` and `C`. The prompt's extent is
> already on the grid, in the rows between `A` and `B`.

Both fields are the shell's **assertion** about something cmote **observes**, which is §71's
second-source rule exactly — the rule that refused a fourth spelling of the cursor shape and kept
cmote a second *reader* of the engine's state rather than a second *writer* of it. An assertion
beside an observation is two sources for one fact, and when they disagree the remote wins.

There is a second reason for the command line and it is the user's own ordering: showing it needs a
surface that does not exist, and a new visual surface is the thing this pass ranks last. `cl=m` does
not even have that much — honouring it would change nothing, since a jump anchors on the `A` line,
which is the prompt's first line with or without the hint.

Worth naming the contrast, because all three fields are hints about the prompt and they did not go
the same way: **`k=s` was read because ignoring it made an existing feature wrong.** The other two
were refused because honouring them would have needed a new one.

### The third row stays ❌, and that is the answer

`N`, `P` and `L` cannot be settled, and the reason is not that they are undocumented — it is that the
reachable accounts **disagree about what `P` is**:

- vtdn has Konsole tracking "REPL mode … for prompt (A/N/P)".
- A zsh write-up uses `133;P;k=i` for `PS1` and `133;P;k=s` for `PS2` — `P` as the prompt mark
  itself, with `A` alongside.
- A Ghostty fork uses `133;P` for a prompt **redraw**, explicitly one that must *not* open a new
  semantic block.

In two of those, ignoring `P` is right. In the third, ignoring it means seeing **no prompts at all**.
A letter cannot be supported or refused until it means one thing, so the honest mark is the gap, and
`_ => None` remains the safe answer for the reason the `k=` asymmetry has: a wrong mark moves a jump
and mis-bounds a command; no mark leaves both alone.

### What it cost

- `term/osc133.rs`: one branch in `parse`, the header block rewritten around five named fields, two
  tests added (the field itself, and a whole multi-line entry yielding one prompt start).
- Matrix: `k=s` added as ✅, the command line and `cl=m` moved **❌ → 🛑**, the letters row rewritten
  around the disagreement; kitty's page added to the Evidence.
- 179 → **180 rows: ✅ 109 · ❌ 26 · 🛑 39 · 🤷 6**. Tests 1204 → 1206.

### Not done

- **No test drives the bug through `Prompts::apply`.** The two added tests pin the scanner, which is
  where the fix is; the tick-and-anchor consequence is argued in this section and in the row, not
  demonstrated. A test that fed a multi-line entry to a whole `Terminal` and counted `visible_rows`
  would have shown the old behaviour failing, and it was not written.
- **`special_key=1` has no row.** fish emits it on an ordinary prompt start, cmote correctly reads
  that as a prompt start, and the main row's "trailing fields are ignored" already covers it. Named
  in the module and in one test so the next reader does not have to re-derive that it is not a kind.
- **Nobody has run cmote against zsh with kitty's integration installed.** The fix is reasoned from
  kitty's published shell code and pinned by tests against that exact byte string; it has not been
  watched working.
- **`L` was never sourced at all** — it appears in no page read across §95, §96 or §97, only in this
  project's own earlier guess that kitty and WezTerm emit it. That guess is now recorded as
  unsupported: kitty's page has no `L`, and WezTerm's documents no marker letters whatsoever.

## §98 — The catalogues nobody had read (v4.0.0)

§8 has been built against **one** catalogue since it existed, `vtdn.dev`, and every sweep since has
re-derived its marks from the crates. Three more were read end to end for this one: contour's sequence
index, four of its extension pages, and otty's OSC and CSI trees.

**Not one mark moved for being wrong.** Thirty-four rows appeared that had never existed.

That is a different failure from any this document has recorded. §70's row was right about the crates
and wrong about a price that had been paid somewhere else; §88's was right that xterm does not document
a range DEC does; §97's was a field read as the wrong thing. All three were rows being *re-read*. A
catalogue you have not opened produces no row at all — and a table of correct rows looks exactly like a
complete one, which is why "verified against the real sources" has never been the same claim as
"finished".

The 🤷 column went from six rows to twenty-six on one reading. Six was never how many sequences cmote
declines by never having been offered them; six was how many had been noticed.

### What the missing rows turned out to be

- **Colours xterm itself defines and this table never listed**: `OSC 5` / `105` / `106`, the "special"
  colours that tint an SGR *attribute* rather than a palette slot — bold, underline, blink, reverse,
  italic; `13` / `14` the mouse pointer's; `15` / `16` / `18` the Tektronix window's; `17` / `19` the
  selection's; and the resets `113`–`119` that pair with them. The fixed-scheme policy (§6) covered
  every one of them already and had never been asked to.
- **The consequences of being a one-page terminal**: the page family (NP, PP, PPA, PPR, PPB), the
  status display (DECSASD, DECSSDT), and the three DCS sequences that leave something *behind* in a
  terminal — a macro, a downloaded character set, a redefined function key.
- **One absent piece of machinery wearing six names**: SL / SR, DECBI / DECFI and DECIC / DECDC are all
  horizontal scrolling and column insertion, and all ❌ for the same missing thing. Worth seeing
  together; separately each reads like an oversight.
- **Two bulk screen readbacks**, which are where §60's line finally gets tested from the other side.
- **`OSC 88`**, which is a category §6 did not have.

### Three things shipped, and the first changes a rule

**`CSI ? 996 n` is answered — "is your scheme dark or light?" — with `CSI ? 997 ; 1 n`, dark.**
Contour's sequence, adopted by ghostty, kitty and GNOME's vte; asked by neovim, helix, zellij and tmux.
It is a **constant**: cmote's scheme is fixed (§6) and `palette::DEFAULT_BG` is `#1e1e1e`. A test
asserts the background is still dark, so the constant cannot outlive its own premise.

It is also the first reply cmote sends in a sequence xterm does not define, which is exactly what §96's
half-rule forbade two sections ago — *cmote may read any dialect, and answers only in the one it
claims*. That rule is **narrowed** rather than excepted: what a reply must not do is name the **program**
or the **machine** (§36's line, the one DA3's constant unit id keeps), and it must not become a second
source for something cmote can observe (§71). "My background is dark" does neither, and it is not even
new disclosure — `OSC 11 ; ?` is xterm's own spelling of the same fact and cmote already answers it.
One writer, two doors.

The check that the narrowing is not just a licence: kitty's `OSC 21` was refused in §78 on **four**
grounds, of which the dialect was the first. Narrowing that one leaves the row exactly where it was,
because the other three are untouched — answering the keys cmote lacks would invent colours, the
reply's length is the requester's to set, and for the keys it could answer it carries nothing `OSC 4` /
`10` / `11` / `12` do not. A rule that could be narrowed without moving the row it was written for is a
rule that was carrying more weight than it needed to.

And the reason to bother: silence is not neutral. A program that cannot learn the background paints for
the one it guessed, and a light guess over `#1e1e1e` is a screen the user cannot read. That is the
UX-stability harm this project ranks above every visual feature, caused by cmote declining to say
something true about itself.

**`OSC 30` is a third door to the tab chip.** contour's `SETTABNAME`, beside `OSC 1` and ConEmu's
`OSC 9;3`, read through `term/icon.rs` so there is one writer and three doors — §71's test for a second
spelling, and §90's argument verbatim: capped at 24 characters, sanitised, appended after the endpoint
and never in place of it, so a remote gains no more of the chip than it already had.

It is **the thinnest-sourced thing in this codebase** and that is written into the module rather than
smoothed over: one line of contour's index, and the detail page behind it does not resolve. What makes
it safe to act on anyway is the size of being wrong — an unrelated `OSC 30` payload would appear,
sanitised and capped, after the endpoint on its own tab. The same misreading on a colour, a font or a
clipboard would not have been worth it, and none of those is what this module writes.

**Two refusals are now stated by name** in `term/notify.rs`, the module that exists for exactly this.
`OSC 60` is contour's `SETFONTALL` — every face, style and size at once — and it is `OSC 50`'s refusal
at a larger size, with no `CursorShape=` exception to carve out because this number carries one
meaning. `OSC 88` is the Terminal Resume Protocol, and it is the only sequence found in this sweep
whose intended effect is a **local process**: `arm ; cmd=<base64> ; args=<base64> ; cwd=<path>` hands
the terminal a command line to run if it ever restarts. The proposal is reasonable where it was
written, with the program and the terminal on one machine answering to one person. cmote is an SSH
client and the two ends are not the same person. Its `query` operation is refused with the rest,
because "supported" is the advertisement that brings the arm.

### The two readbacks, and the line they cross

`CSI > Pl ; Pr t` (contour's buffer capture) returns the screen's text in a run of `PM 314 ; … ST`
strings. `CSI > Ps ; Pn b` (its semantic-block query, armed by DEC mode 2034) returns the **command
lines, prompts, output and exit codes** as JSON.

§60 allowed DECRQCRA to read the page, and the argument was that every byte on it came from the pty the
reply goes back down — with two properties enforced rather than assumed: the rectangle clamps to the
**visible page**, and what comes back is a 16-bit checksum rather than the text. Both of these break
the first property and neither has the second. A capture's whole purpose is to reach into the
scrollback, which in an SSH client can hold the output of a session that ended before this one began,
on a different host, under a different account.

The block query is worse in kind rather than in degree, and it is the cleanest illustration of §96's
half-rule running the other way: cmote **reads** OSC 133, a dialect it does not claim, and that is
allowed because a read produces no reply. This is the same data flowing outward, and outward is the
direction the rule binds. That contour gates its own query behind a four-word token the terminal mints
is the vendor agreeing about the danger, not disposing of it — the token travels the same wire the
answer does.

Both are 🤷: `vte` has no arm for either final byte under a `>` marker, so nothing here performs the
refusal and §6 is the whole of it.

### What it cost

- `term/dsr.rs`: the allow-list widens from three values to four, one `Request` variant, one reply
  constant, three tests — one of them on `palette::DEFAULT_BG` rather than on this module at all.
- `term/icon.rs`: one more arm in `parse` and the quote-trimming kept to the one spelling whose source
  quotes; two tests, including the two neighbours a prefix match would have swallowed (`OSC 3`,
  kitty's `OSC 30001`).
- `term/notify.rs`: `Refused::Font` grows a second number, `Refused::Resume` is new, two tests.
- `term/mod.rs`: one arm in `answer_dsr`, three boundary tests.
- Matrix 180 → **214 rows: ✅ 111 · ❌ 36 · 🛑 41 · 🤷 26**. Tests 1206 → 1214.

### Not done

- **Nothing was implemented for the ten new ❌ rows**, and two of them are worth a second look before
  the next sweep calls this section thorough. **XTSAVE / XTRESTORE** (`CSI ? Pm s` / `r`) is the one
  with a user-visible failure behind it: a program that saves `? 25`, hides the cursor and restores
  gets no restore, so the cursor stays hidden after it exits — the stuck-state shape §72 exists to
  prevent. **DECIC / DECDC** is the cheapest, `term/rect.rs` already moving whole cells with their
  colour, link and protection attached.
- **ECMA-48 is still unread**, and it is now load-bearing in two rows rather than one: SCP (§76) and
  the page family, whose intermediates are taken from a catalogue that demonstrably drops them
  elsewhere (contour writes SL as `CSI 0..1 @`, which is ICH's spelling).
- **contour's line-reflow extension has no row** — its page does not resolve and no mode number was
  found anywhere else, so nothing was invented for it.
- **`OSC 30` rests on one unverifiable line**, and no other terminal was checked for a conflicting
  meaning on that number.
- **The readback rows are 🤷 and stay 🤷.** Nothing in cmote refuses `CSI > … t` or `CSI > … b`; §6 is
  the whole refusal, and §79's lesson — that 🤷 sometimes means "upstream never looked" rather than
  "upstream refused" — applies to both. A scanner that named them would make them 🛑, and would be the
  first refusal in this codebase written for a sequence no reachable program sends.
- **The SGR table still has not been checked**, eight sweeps running.
- **`OSC 777` is still folklore**, and `OSC 888` was taken from contour's index with no detail page —
  the same evidential footing as `OSC 30`, and it only reaches a 🤷 rather than shipped behaviour.

## §99 — One calculation, and the bit that could not be honoured (v4.0.0)

§94 added the XTCHECKSUM row (`CSI Ps # y` — "the bits of `Ps` modify the calculation of the checksum
returned by DECRQCRA") and closed by leaving it open: *"Named now, so the next reader chooses rather
than not knowing."* This is that choice, and it goes the way it does for a reason that only appears
once the five bits are counted against what cmote can do.

Four of them are mechanical — do not negate the result, do not weigh the video attributes, do not drop
blank cells, do not mask the cell value to eight bits. The fifth is **omit the checksum for cells the
program never initialised**, and cmote cannot perform it at all: the engine's grid starts full of
blanks that read identically to written ones, which is the divergence §60 already discloses as the one
place cmote's number differs from xterm's.

So the choice was never "support it or not". It was **support four fifths of it or none**, and four
fifths is the worse of the two: a program that sets the bit cmote cannot honour would receive a number
computed under rules it did not choose — which is precisely the harm §94 wrote the row to name. A
checksum's whole value is comparability (§60: *"a number nobody else computes the same way is worth
less than no number at all"*), and a request that silently changes some of the calculation and not the
rest destroys exactly that.

cmote therefore answers **one** calculation, always: xterm's `xtermCheckRect` at its DEC-compatible
default, the mode tuned against screenshots from a real VT520 and the one a VT-conformant program
expects when it has asked for nothing else.

The mark is **🤷** and not 🛑, and the distinction is the one §57 is about. Nothing in cmote refuses
this sequence — `csi_dispatch` matches no `#` intermediate at all, so XTCHECKSUM dies in the parser
beside XTPUSHSGR's spelling and both colour stacks. The decision is this section's and §6's; no code
performs it. What the code now does carry is the *consequence*, stated as behaviour rather than as a
note: a boundary test asks for every bit at once and then re-checksums an unchanged rectangle, and the
number is the same on both sides of the request.

### What it cost

- One test in `term/mod.rs`. No production code, because there is none to write for a decision to
  keep doing what it already does.
- The row moves **❌ → 🤷**. 214 rows: **✅ 111 · ❌ 35 · 🛑 41 · 🤷 27**. Tests 1214 → 1215.

### Not done

- **A 🛑 was available and not taken.** `term/rect.rs` already scans this final byte's neighbours
  (`* y` is DECRQCRA), so an arm that recognised `# y` and deliberately produced nothing would move
  the mark and make the refusal cmote's own — §95's "mechanism-is-a-test" pattern. It is not written,
  because it would add a parse for a sequence no program has been observed to send, and 🤷 says
  truthfully what is there today.
- **The other eight ❌ rows in the CSI table are unbuilt features, not open questions** — SL / SR,
  UNSCROLL, DECIC / DECDC, the locator trio, XTSAVE / XTRESTORE, SETMARK, DECRQDE and DECRQPSR. Each
  would be accepted if it were written; none is refused. **XTSAVE / XTRESTORE is the one whose cost is
  understated by that sentence**: restoring an arbitrary private mode means holding a copy of the
  engine's mode state, which makes cmote a second source for it (§71) — the rule that has decided more
  rows in this document than any other, and the reason this is not the cheap row it looks like.
  **Four of the eight have since been built** — SL / SR in §100, UNSCROLL in §101, DECIC / DECDC in
  §102, and SETMARK with DECBI / DECFI in §112 (DECBI / DECFI were not on this list; they were the
  ESC table's own entry in the same family). What is left is the locator trio, XTSAVE / XTRESTORE and
  the two reports, and only the first of those is still "unbuilt" in this entry's sense: the two
  reports are blocked on a reply format nobody has read, which §112's Not done states, and
  XTSAVE / XTRESTORE on the §71 rule above.
- **Bit 5 was not checked.** Newer xterm documents a sixth bit ("do not ignore double-width cell
  values"); the reading this row rests on lists five. It changes nothing — the answer is the same for
  any number of bits — but the row should not be read as a count.

## §100 — The page moves sideways (v4.0.0)

§98 filed SL and SR (`CSI Ps SP @` and `CSI Ps SP A`) as a **gap**, and grouped them with DECBI /
DECFI and DECIC / DECDC as *"one piece of absent machinery wearing six names"*. The machinery is
built. Two of the six now ship, and it cost forty lines — because §56 had already paid for the hard
half, writing cells directly into the engine's grid.

### What they are

ECMA-48's horizontal twins of SU and SD. Every row of the visible page moves sideways by `Ps`
columns; the edge the content moved away from goes blank. xterm's own wording is the whole of what
any reachable source says about them: *"Shift left `Ps` column(s) (default = 1) (SL), ECMA-48."*

Note the verb — **shift**, not scroll — and note what is absent, because it decided the shape of this
section: ctlseqs says **nothing about the margins** for SL, SR, DECIC or DECDC. ECMA-48, where the
definition lives, is still unread here.

### Four rules, and where each came from

**Whole cells move.** Colours, attributes, the OSC 8 link and DECSCA protection travel with the
glyph — DECCRA's rule (§58), for the same reason: protection makes a cell unerasable, not immovable.
The blanks that arrive carry the **pen's background**, which is what the erases write, so a shift
across a coloured screen leaves a strip in that colour instead of a hole in the default one.

**Each row is read out whole before it is written.** Source and destination overlap by definition
here, so the copy goes through a buffer rather than through a walk direction chosen to make the
arithmetic come out — `copy_rect`'s argument, which is DECCRA's own.

**The cursor does not move.** SL and SR shift the data *under* it. That is ECMA-48's behaviour, and it
is also why this is written as a direct grid write instead of as a translation into per-row DCH and
ICH (§72's fifth route): the translation would have had to move the cursor to each row and put it
back, and the one saved-cursor slot it would have wanted belongs to the program (§57's whole subject).

**A wide glyph cut in half is blanked.** A shift can push exactly one cell of a two-cell glyph off the
page, leaving a lead with no continuation at the right edge or a continuation with no lead at the
left. Neither is a state the renderer expects to meet and neither is a character anybody asked for.
Only the two edge columns can be in it — every other pair moves together — so the fix is two `if`s and
not a scan.

### The scrolling region, and a guard that is honest about being partial

A shift ought to stop at the scrolling region's edges. cmote cannot see them: `alacritty_terminal`
keeps `scroll_region` private with no accessor, which is the same wall §58 hit and disclosed.

So the refusal is aimed at the evidence rather than at the fact. **Origin mode refuses the shift** —
not because SL and SR name coordinates that could be misplaced, they name none, but because DECOM only
means anything once DECSTBM has cut a region. Where the one signal in reach says a region is probably
there, cmote does nothing rather than shift rows the program walled off (§57: doing nothing is a
correct refusal where acting on a guess is a wrong action).

It is a **partial guard and the code says so**: a region set *without* origin mode is invisible from
here, and a shift would then move rows outside it. That is written into `apply_rectangle`, into the
§8 row, and into *Not done* below rather than left for a reader to discover.

### Where the code went

Into `term/rect.rs`, whose name is now one section out of date. What a module shares with its
neighbours is not its grammar but its **mechanism**: SL and SR name no corners and are ECMA-48's
rather than DEC's, but they move whole cells across the page with no engine arm behind them, against
the same background, under the same origin-mode refusal, clamped to the same visible page. A second
module doing cell-moving by its own rules is the duplication worth avoiding; two more arms in this
scanner are not.

The two arms are also the most dangerous near-miss in that scanner, and the tests say so: `CSI Ps @`
with no intermediate is **ICH** and `CSI Ps A` is **CUU** — two sequences the engine implements and
every program on earth uses, one byte away. The SPACE intermediate is the entire difference, and it is
matched alongside the final byte and the absence of a private marker, which is §56's near-miss rule.

### What it cost

- `term/rect.rs`: a `Direction` enum, a `Request::Shift`, two arms in `classify`, three tests.
- `term/mod.rs`: `shift_columns`, one arm in `apply_rectangle`, seven tests.
- The row moves **❌ → ✅**. 214 rows: **✅ 112 · ❌ 34 · 🛑 41 · 🤷 27**. Tests 1215 → 1225.

### Not done

- **A scrolling region set without origin mode is not honoured**, and a shift will move rows outside
  it. Closing it means cmote tracking DECSTBM itself, which makes it a second source for state the
  engine owns (§71) — the trade §58 turned down and this section does not reopen.
- **Whether a real xterm bounds SL and SR by the margins was not established.** ctlseqs does not say,
  ECMA-48 is unread, and no implementation was read. cmote's page-wide reading is the only one its
  sources describe, which is not the same as knowing it matches.
- **DECIC / DECDC and DECBI / DECFI are still ❌**, and are now gaps with a working precedent rather
  than gaps with an argument: DECIC and DECDC are this shift bounded to the columns from the cursor
  rightward, and DECBI and DECFI are it triggered by a cursor sitting at a margin.
- **Nothing was tested against a program that emits these.** They are pinned by cmote's own tests
  against cmote's own reading, which is where §97 also stopped — the same disclosure, one section
  after it was made.
- **The scrollback is untouched by a shift**, deliberately and without a test saying so. The visible
  page is what every operation in this family acts on (§60 clamps the checksum to it for a reason),
  but no test would fail if a later hand widened this one.

## §101 — Giving the scrollback back (v4.0.0)

`CSI Ps + T` — UNSCROLL. Scroll the page down, and fill the top from the **scrollback** instead of
with blanks.

§98 filed it as contour's. It is **kitty's**, and contour's own definition says so in as many words:
`"Scroll Down with Scrollback Fill (kitty unscroll)"`, tagged `VTExtension::Unknown`. That is the
first thing this section fixed and the lesson worth keeping: a catalogue lists what a terminal
*implements*, and §98 read one as a list of what a terminal *invented*.

kitty's own page has what an index line cannot: the lines are **moved**, not copied; the rows pushed
off the bottom "are removed from display"; where there is no scrollback "the newly inserted lines
must be empty"; and the motivation — "many modern shells will show completions in a block of lines
under the cursor, this causes some of the on-screen text to be lost even after the completion is
completed. This escape code allows that text to be restored."

That motivation is the whole argument for implementing it properly rather than approximately. Plain
SD would scroll the page down and fill with blanks — **erasing exactly the text the sequence exists to
restore**. And a copy rather than a move would leave the same lines in the scrollback and on the page,
once per tab press, for the life of the session.

### The wall, and the way round it

Rows can be read and written anywhere, history included: `Line(-1)` is the newest scrollback row.
What has no accessor is **shortening the history at the end nearest the page**. `Grid::update_history`
is the only public trim and it drops the OLDEST rows — `Storage::shrink_lines` lowers a length, and
the ring's `compute_index` puts the oldest row at the far end of it.

So the rows nearest the page are not dropped, they are **overwritten**:

1. the page slides down by `lines`, bottom-up, and its last `lines` rows fall off;
2. the newest `N` scrollback rows come in above what is left, newest lowest;
3. whatever the scrollback could not fill is blanked in the pen's background;
4. the rest of the history walks up over the consumed rows, which leaves the spare rows at the
   **oldest** end — the end that *can* be dropped;
5. `update_history(history - N)` drops exactly those, and a second call puts the retention limit back,
   since that method sets the cap as well as trimming.

Every row is **moved**, through `mem::replace` around a single row cloned up front as the placeholder
supply. Deep-copying rows would be a megabyte of cells on a full scrollback, on a sequence a shell may
send on every tab press; moving a `Row` moves a `Vec` header.

The **alternate screen** needs no special case and gets none: that page keeps no history, so `N` is
zero, every inserted line is blank, and what happens is exactly SD — which is what kitty's
specification requires there. The restriction and the implementation meet without a branch.

### The half that is not a grid operation

Every position cmote remembers about a session is an **absolute line index**, `history_size + row` at
the moment it was taken: prompt marks and bookmarks (§34, §55), a finished command's output span
(§34), a picture's anchor (§40), a line's right-to-left flag (§76). Unscrolling moves the boundary
between the scrollback and the page, so it is the only operation in this terminal that can change what
those numbers mean.

The arithmetic is worth writing out, because the ordinary case looks like nothing happens — which is a
very easy thing to get wrong by not thinking about it at all. Writing the document as
`[history 0..H][page 0..R]`:

```
before   [ history 0..H-N ][ history H-N..H ]              [ page 0..R-lines ][ discarded ]
after    [ history 0..H-N ][ blanks × (lines-N) ][ the same H-N..H ][ the same 0..R-lines ]
```

A line above the consumed history keeps its number. Everything from there down moves by the number of
**blanks** — not by `lines`. And when the scrollback held everything asked for, which is the ordinary
case because a program unscrolls what it just scrolled, the blank count is zero and **not one number
changes**; the document simply gets shorter at the end.

The discarded lines are not merely left alone. Their content is gone and the document will grow back
over those indices with new output, so an anchor left there would reappear one day on text it never
described. `rect::Unscrolled::map` answers both questions at once — a new number, or `None` — and each
store renumbers through it: `Prompts` (marks, bookmarks, command spans, the half-built one),
`Graphics` (the primary page's anchors; the alternate page's are rows and cannot be reached by this),
and `Paths` (rebuilt rather than edited, since a shift can move a line onto a number the set already
holds).

A command is dropped whole if any of its three lines is, which is conservative and right: a span with
one end renumbered and the other not would select from a prompt to a line that no longer follows it.

### What it cost

- `term/rect.rs`: `Request::Unscroll`, one arm in `classify`, and `Unscrolled` — the line arithmetic,
  testable without a terminal, which is what this module is for. Three tests.
- `term/mod.rs`: `unscroll`, the whole surgery, and one arm. Eight tests.
- `term/osc133.rs`, `term/graphics.rs`, `term/scp.rs`: one `renumber` each.
- The row moves **❌ → ✅** and its attribution is corrected. 214 rows: **✅ 113 · ❌ 33 · 🛑 41 ·
  🤷 27**. Tests 1225 → 1236.

### Not done

- **The user's text SELECTION is not renumbered.** It lives in `app.rs` beside the find bar, outside
  the terminal, and holds absolute lines like everything else. It is left alone deliberately rather
  than cleared: in the case that matters — the scrollback filling the request — no number moves and
  the selection stays exactly right, so clearing it would destroy a correct selection to fix a rare
  wrong one. The find bar needs nothing: its matches are already invalidated by any output arriving
  (§44). A selection made across an unscroll that had to insert blanks will point one screenful off
  until the next click.
- **This depends on which end `update_history` trims from**, which alacritty documents nowhere — only
  its arithmetic says so. A version bump could reverse it and nothing would fail to compile. What
  would catch it is behavioural and is written: a document read end to end after an unscroll, in
  order, with nothing repeated and nothing missing.
- **Nobody has run this against kitty's shell integration.** It is pinned against cmote's reading of
  kitty's page, which is the same disclosure §97 and §100 both made, three sections running.
- **`Ps` is clamped to the page height** and kitty's "maximum is implementation-defined but must
  support at least one full screen" is satisfied exactly, not generously. A program asking for two
  screens gets one.
- **The cursor does not move**, matching SD, and kitty's page does not say what it should do. A shell
  that expects its prompt to still be under the cursor after an unscroll will be repositioning it
  itself; nothing here checks that assumption against a real one.

## §102 — The margins, and a place to stand

`s (DECSLRM) | Left / right margins` moves **🛑 → ✅**, and takes three more rows with it: DECLRMM
(mode 69) ❌ → ✅, DECIC / DECDC ❌ → ✅, and SL / SR keep their ✅ while losing the refusal they had
carried since §100.

**The verdict this reverses was refused twice, and on one sentence.** TERMINAL_COMPATIBILITY_PLAN part 5
costed the build in detail and turned it down both times, always at the same paragraph: every
`Handler` method has a default empty body, so a method left unforwarded — or one a future
`alacritty_terminal` adds — compiles cleanly and silently drops a sequence, and *"§57's could be
caught at build time with a `const` assertion and this one cannot: a trait growing a defaulted method
breaks nothing."*

It can. `#[deny(clippy::missing_trait_methods)]` on the `impl` block reports every method left to its
default, so a missing one is a build error under the gate's `-D warnings`; and if a future clippy
drops the lint, `unknown_lints` fails the build instead. Verified by deleting `bell()` from the
forwarding list and watching the build fail, rather than assumed — a guard nobody has seen fire is a
guess about a guard.

The lesson is not about clippy. **This document has re-derived its FACTS every sweep since §66 and has
been carrying its PRICES forward unexamined.** §98 found the same failure in the row marks — a table
of correct rows looks exactly like a complete one — and this is its twin in the prose: a price quoted
three times is no better evidence than a row nobody re-read.

### The sixth way in

Five routes were named before this: accept the engine's limit, scan the sequence out and act beside
the grid, borrow a bit of the engine's own state (§56), refuse the sequence properly (§57), translate
it into sequences the engine already takes (§72). All five share a property — cmote never stands in
the engine's way — and that is exactly what margins need.

`term/gate.rs` implements `Handler` itself, holds `&mut Term`, and is passed to `Processor::advance` in
the engine's place. Two things need it and nothing else can give them:

- **Reading back what the engine decided.** `Term::scroll_region` is private, with no accessor and no
  reply arm. A scanner can watch DECSTBM go past on the wire — but not the RESETS, which happen inside
  the engine on RIS and on resize.
- **Pre-empting a decision.** Margins change what printing does, and there is no repairing that
  afterwards: by the time the glyph is on the grid it is at the wrong columns.

What keeps it inside §71's rule is that a gate is not a second author. It pre-empts a decision and
delegates; the engine still writes every cell it wrote before, except inside a narrowed band where the
engine has no opinion at all.

The forwards are macro-generated, and that is a correctness argument rather than a saving of typing.
Sixty-odd hand-written bodies would be sixty-odd chances to pass `count` where `mode` was meant; with
the macro the only thing that can be wrong is a signature, and a wrong signature does not compile.

### The mirror that cannot drift

`term/region.rs` holds the vertical scrolling region. It is exact by construction rather than by care:
the engine assigns that field in exactly four places — `Term::new`, `Term::resize`, `reset_state` and
`set_scrolling_region` — and the last two are `Handler` methods the gate sees while the first two are
calls cmote itself makes. The arithmetic is a transcription of the engine's, down to the case where a
zero top leaves the region starting *above* the first row, so the two can be compared by hand if they
ever have to be.

**Cashed in immediately for §100.** SL and SR were refused whenever origin mode was set, with DECOM
standing in as evidence that a region existed that a shift ought to stop at. The proxy was both too
much — a program with origin mode and no region got nothing — and too little, since a region set
*without* origin mode was shifted straight through. The bound is now the real region.

### The mode is the whole rule

`CSI s` has two meanings on one final byte, and a real terminal tells them apart by DECLRMM. §57 could
not: the engine refuses mode 69, so the only evidence left in the bytes was the parameter count and
`term/cancel.rs` cancelled every parametrised `s` on it. cmote holds the mode now:

- **mode 69 set** — DECSLRM. Margins applied, and the byte still cancelled, since the engine's arm for
  it reads no parameters and would save the cursor on the way past.
- **mode 69 reset** — SCOSC. Let through, and the engine saves the cursor, which is what a real xterm
  does with it, parameters and all.

That is not a loosening of §57. The proof was sitting in the terminfo §73 had already read: all four
of `xterm-256color`'s margin capabilities set mode 69 **first**
(`smglr=\E[?69h\E[%i%p1%d;%p2%ds`). A program that means margins says so before it asks.

### What the margins actually do

Everything keys off **narrowed**, not **enabled**. With the band at the page edges the engine keeps
every operation, so an ordinary session runs on exactly the code it ran on before — reproducing the
engine's behaviour is not the same as having it, and the difference would show first in which rows
reach the scrollback.

Once a column is excluded: a line breaks at the right margin and goes on at the left one; CR goes to
the left margin, and to column 1 from left of the band; CUF, CUB, HT and BS stop at the margins; ICH
and DCH push and pull within the band; SU, SD, IL, DL, IND, RI and NEL scroll only the band's columns;
and under origin mode the columns a program names are counted from the left margin.

**The deferred wrap had to become cmote's.** A terminal does not wrap when the last usable column is
filled — it leaves the cursor there with the wrap owed, so a program that fills the line and then
moves never wraps at all. The engine has that flag and fires it at the SCREEN edge, which for a band
short of the edge never happens, and for a band at the edge wraps to column 0 instead of to the left
margin.

**A row pushed out of a narrowed band is discarded, never scrollbacked.** The history holds whole
lines and this row is a slice of one; the columns outside the band are not leaving. xterm agrees, and
it is also the only answer that leaves the history readable, since half-lines interleaved with whole
ones would make every search, selection and copy downstream of it wrong.

**A margin wrap sets no `WRAPLINE` flag**, and §5 predicted the opposite. The flag belongs to the
whole row, and inside a narrow band the rest of that row is another column of the page — joining on it
would splice unrelated text into every copy taken across a wrap. It costs a wrapped word being copied
in two pieces and keeps the copy honest about what was on the screen.

**Margins are not per-screen**, and §5 predicted that they would have to be. The reason is one the
gate made visible: the ENGINE's vertical region is not per-screen either — `swap_alt` does not touch
`scroll_region` — so making the horizontal axis behave differently from the vertical one would have
been cmote inventing an asymmetry DEC did not write.

### One rule written wrong, and corrected before it shipped

The first version let text OUTSIDE the band keep the whole page and wrap at the screen edge, on the
argument that a full-width status line should not be chopped into the band. It reads better and it is
nobody's behaviour: xterm's `ScrnRightMargin` reads the mode and never the cursor, so with DECLRMM set
a line breaks at the right margin wherever it started. §57's rule pointed the other way — where a
reference implementation has decided, matching it beats improving on it. The test that pins it states
the surprising half out loud rather than hiding it.

### The traps only a gate can show you

The engine implements `newline` by calling its **own** `linefeed` and `carriage_return`, and `goto_col`
by calling its own `goto`. Forwarding either would run the margin-blind version of a method the gate
had just replaced. Nothing warns about this; it is only visible from inside.

The same reading turned up the engine defect §74 recorded against CHA and HPA: `goto` adds the
scrolling region's top to the line it is given, so a pure column move through `goto_col` drags the
cursor downward under origin mode. The gate's own column writes sidestep it, so the defect is gone on
the margin path and untouched elsewhere — fixing it generally is a different change with its own row.

### What it cost

- `term/gate.rs`, new: the `Handler` impl, the lint guard, the forwarding macro, the margin-aware
  methods and the band surgery.
- `term/region.rs`, new: the mirror. Eleven tests.
- `term/margins.rs`, new: the state and the arithmetic. Sixteen tests.
- `term/cancel.rs`: carries the two numbers now, and no longer decides on its own. Six more tests.
- `term/rect.rs`: DECIC / DECDC, in the same grammar as SL, SR and UNSCROLL.
- `term/mod.rs`: the gate wiring, `shift_band_columns`, the margins on the soft-reset string, and
  every advance routed through one place so a synthesised sequence passes the gate too.
- `Cargo.toml`: `unicode-width`, for the one width question that has to be answered *before* the
  engine answers it. Same crate at the same version `alacritty_terminal` already uses for exactly this
  call.
- Four rows move: **🛑 → ✅** (DECSLRM) and **❌ → ✅** (DECLRMM, DECIC / DECDC). 214 rows:
  **✅ 116 · ❌ 31 · 🛑 40 · 🤷 27**. Tests 1236 → 1295.

### Not done

- **Nobody has run this against a real program that uses margins.** It is pinned against cmote's
  reading of xterm's source and DEC's manual — the same disclosure §97, §100 and §101 all made, four
  sections running. That is now the standing state of this work rather than a note on one section.
- **ECMA-48 is still unread**, and it is load-bearing in three places: SCP (§76), the page family, and
  SL / SR's new region bound. §100 refused where it was unsure; §102 acts where it is unsure and says
  so, which is a different kind of claim.
- **Erases are not margin-bounded.** ECH and EL run to the end of the line as they always did, on the
  rule that operations which SHIFT cells are bounded and operations that erase in place are not. That
  rule is cmote's reading, not a quotation; a program clearing to end-of-line inside a band will wipe
  the column beside it.
- **The `? 69` DECRQM answer is the only new thing cmote says.** It reports cmote's own state and names
  neither the program nor the machine (§36, §96), so it clears the rule — but it is another reply
  added to a list §33 called closed, and the list is now long enough that "closed" should probably
  stop being said about it.
- **A fifth writer of `scroll_region`** arriving in a version bump would break the mirror silently.
  Four writers were counted by hand and nothing checks the count.
- **The alternate screen shares the margins with the primary page**, which follows the engine's own
  handling of the vertical region and is not what DEC describes. A full-screen program that sets
  margins and exits leaves them set for the shell.
- **`goto_line` is forwarded, so VPA keeps whatever the engine does with it.** Only the column paths
  were taken over; a row path under margins has nothing to decide, which is true today and is an
  assumption rather than a checked fact.

---

## §103 — Local

The home screen grows a **Local bar**: a row of buttons above the target list, one per shell this
machine can start — PowerShell 7, Windows PowerShell, Command Prompt, Git Bash. Pressing one opens a
session that looks like every other session cmote opens: the shell in the grid, the folder tree and
the files pane beside it showing **this** machine, the editor and the picture preview a double-click
away. No form, no host key, no credential.

### The observation the whole thing rests on

**The GUI never talks to SSH.** It talks to `bridge`, in `SshCommand`s out and `SshEvent`s in, and it
is `ssh::client::run` that turns those into a connection. Look at what that loop actually does with
thirty of its thirty-two arms: it forwards the command to a `SessionMsg` channel and looks at nothing.

So a session task that consumes the same `SessionMsg`s and answers in the same `SshEvent`s is, to
everything above it, a connection. That is not a metaphor — it is the literal type. The cost of a
local session was therefore:

- one new command, `SshCommand::ConnectLocal(Shell)`;
- one arm in `run` to start a local link instead of an SSH one;
- one constructor, `SessionLink::start_local`, differing from `start` by having no host-key one-shot.

Nothing else between the GUI and the shell changed. The terminal, the keymap, the query answering, the
scrollback, the find bar, the tree, the pane, the details popup, the transfer queue, the editor and the
preview all run over a local session without knowing one exists.

That is the same seam §46 found from the other side. §46 put three file backends behind one value
(`Browse` / `Files`) because the question "as which account?" had three answers; this puts two whole
SESSIONS behind one channel because the question "on which machine?" now has two. The lesson repeated
is that the honest seam is rarely where the feature is — it is wherever the code already stopped
caring.

### What could not be borrowed

Four things, and each one is a decision rather than a translation.

**The path dialect.** The panes speak POSIX, and not by choice: SFTP puts `/`-separated paths on the
wire whatever the server runs, so `explorer` was written against one dialect and every path in the
tree, the pane, the editor and the queue is a `/`-rooted string. Windows is not that. `local::path`
maps between them, keeping the panes' single root and still showing every drive:

```
/                 the VIRTUAL root — no native path; listing it lists the drives
/C:               a drive's root       ->  C:\
/C:/Users/cme     a folder on it       ->  C:\Users\cme
```

A drive is a directory named `C:` inside `/`, which costs nothing anywhere else — `explorer::join`
composes it like any other name and the tree's arithmetic is untouched. Rooting the panes at one drive
was the alternative, and there is no honest way for a machine with four drives to show one.

`to_native` is the local file layer's **security boundary**, and it is one-directional: everything
downstream of it takes a real `PathBuf` and never a string, so a path it refuses cannot reach the
filesystem at all. It refuses a `..` or `.` component, a `:` anywhere but the drive (on NTFS that names
an alternate data stream, so `notes.txt` and `notes.txt:hidden` are two files and only one is the row
on screen), a `\` inside a component, and a first component that is not a drive.

**Which shells exist.** The bar is built from a search, not a list: a machine without PowerShell 7 must
not offer a button that fails. Two rules matter. The programs are found in the locations Windows and
the installers use, or by walking `PATH` for a known name — **nothing the user types reaches this**, so
there is no place a crafted string could name a program to run. And `System32\bash.exe` is excluded on
purpose: that name is WSL's launcher, not Git Bash, and running it starts a Linux distribution in a VM
while the file panes beside it describe this machine's drives. A `bash.exe` is accepted only through a
Git installation, which is why the `PATH` half of that search looks for `git.exe` and never for `bash`.
Git Bash also gets `--login -i`, because that is what its own shortcut passes and without it the MSYS2
profile never runs — a bash that cannot find `ls`.

**The `cd` the panes type.** Sync, the tree's and the pane's "Open in terminal" and the tree's Enter key
all move the shell by typing a line at it. `cd '<pane path>'` is right for a remote and wrong here
twice over: the path is not a path on this platform, and the four shells disagree about the command as
well as the path.

| Shell | What it is sent | Why |
|---|---|---|
| Command Prompt | `cd /d "C:\Users\cme"` | without `/d`, a `cd` to another drive moves the directory *on* that drive and leaves the prompt where it was — a no-op that reads as a bug |
| PowerShell (both) | `Set-Location -LiteralPath 'C:\Users\cme'` | `cd` there routes through `-Path`, which treats `[`, `]` and `?` as wildcards; a folder with a bracket in its name is not exotic, and a wildcard matching nothing is an error rather than a move |
| Git Bash | `cd '/c/Users/cme'` | MSYS spelling: the drive letter becomes a top-level directory |
| zsh / bash | `cd '/Users/cme'` | the dialects already agree |

A path that will not translate — the virtual root has no directory to move to — types **nothing**,
rather than putting a failing command in the user's own shell history.

**Where the panes open.** A remote session opens at `/` because that is the top of a machine cmote has
just met. A local one opens at the user's own folder, because the shell is already standing there from
its first prompt and the drive list would put two clicks in front of the first folder anyone wants.

### What the transfers cost: nothing

The user asked for the file layer whole, trees included. It came to one module, and mostly because of
a naming accident that turned out to be a fact: **`ssh::transfer` is not about SSH.** It is about
copying — where a resume picks up, when a progress event is worth sending, what the six-way collision
answers mean and which of them stick, and the difference between a failure that can be resumed and one
that cannot. All of it is direction-agnostic and network-agnostic, and `local::copy` is its third and
fourth caller. The local tree walk is shared the same way: `ssh::upload::walk_local` reads this
machine's disk and describes what it found, symlink cycle rules and all, so it became `pub(crate)` and
gained a caller rather than a copy.

What is left in `local::copy` is one copy engine and one tree walk. "Upload" and "download" are two
directions because a remote session has two machines; here they are the same operation, and the
direction decides only which pair of terminal events the outcome is reported in — because the transfer
queue's state machine listens for the pair belonging to the direction it started, and reporting the
wrong one would leave its slot occupied for the rest of the session.

One refusal is new and has no network equivalent: **copying a file onto itself.** Over a network it
cannot happen; here it can, and the naive answer truncates the user's file to nothing before reading a
byte of it. Both paths are resolved with `canonicalize` first, so a symlink, a `.` on the way or a
different case on a case-insensitive volume are all caught.

### Three things ConPTY taught us, none of them by reading

Every one of these was found by a test that ran a real child on a real pty, and every one of them was
wrong in the first version of the code.

**A shell exiting does not close the pty.** The first design ended a session on EOF — the reader thread
reaching the end of the stream. On Windows the ConPTY object owns the output pipe and holds it open
until `ClosePseudoConsole`, which happens when the master is dropped, which happens when the session
decides it is over. Ending on EOF is waiting for a consequence of the decision to be made before making
it, and the test that asked for it sat there for twenty seconds. A third thread now waits on the child,
and its exit is the event; the reader is unblocked afterwards, by the teardown.

**A ConPTY asks the terminal a question and holds everything until it is answered.** `portable-pty`
creates it with `PSUEDOCONSOLE_INHERIT_CURSOR` — hard-coded, not a choice it offers — and a ConPTY made
that way sends `CSI 6 n`, "where is the cursor?", before a byte of the child's output arrives. cmote
answers it as a matter of course: the engine replies to DSR (§23) and `app` sends whatever
`Terminal::process` hands back straight down the input path. Nothing had to be added. But it is
load-bearing — a version of the test that only collected bytes hung for twenty seconds and saw exactly
four of them, `\x1b[6n` — so the exchange is asserted rather than left to be rediscovered.

**`tokio::select!` cancels the branches that lose.** The exit signal started as a `oneshot::Receiver`
inside `Pty`, behind an `async fn exit(&mut self)` that took it out of an `Option`. The very first chunk
of output cancelled that future — *after* it had taken the receiver, which was then dropped with it — so
every later call parked forever and the shell could exit unnoticed. It looked exactly like
`child.wait()` not working. Both receivers are now `tokio::sync::mpsc`, whose `recv` is documented
cancel-safe, and they are handed to the session loop as a struct beside the pty rather than reached
through it — which also stops a `&mut pty` borrow from locking out the write in the branch next door.

The loop is `biased` as well, so bytes the shell has already produced are delivered before the fact
that it has stopped producing them.

### What is refused, and why each is a refusal

Three features have no meaning without a remote. Each is refused **with its reason** by the session
task, and each has its button removed from the GUI — a control whose only possible answer is "not here"
teaches nothing.

- **Another account (§45).** Elevation is a program run on an existing connection. Becoming another
  user on Windows means UAC, which is a separate process at a different integrity level and not another
  shell on this one; there is nothing here for `sudo -u` to be.
- **Port forwarding (§27).** A tunnel carries a connection through the remote's network. There is no
  remote.
- **Shell integration (§17).** It writes a cwd announcer into the shell's config file. Here that file is
  the user's own everyday profile on this machine, which is a much larger promise than "open a
  terminal" and not one a context-menu item should make.

The consequence of that last one is felt: none of the four shells announces its working directory by
default, so cmote does not know where the local shell is. **Reveal** stays dim, exactly as it does on a
silent remote `bash` (§17), and the tree does not follow a `cd` typed by hand. **Sync** works — it types,
it does not listen.

### What it cost

- `local/`, new, six modules: `shells` (the catalogue and the per-shell `cd`), `pty` (the pseudo-
  terminal), `path` (the dialect), `fs` (the panes' answers), `copy` (the transfers), `session` (the
  `SessionMsg` loop, twin of `ssh::client::stream`). Thirty-eight tests, three of which drive a real child
  on a real pty and a real session end to end.
- `bridge.rs`: one command, `ConnectLocal`.
- `ssh/client.rs`: one arm, one constructor. `ssh/upload.rs`: `walk_local` gained a second caller and
  the visibility to match.
- `app.rs`: `HomeLocalPressed`, `dial_local`, a `local: Option<Kind>` on `Tab`, `forget_connection` so
  the three session endings cannot clear half of it, `default_files_root`, and the per-shell `cd`.
- `ui/home.rs`: the Local bar, and the menu placement told about its height.
- `ui/terminal.rs`: `endpoint: &str` became a `Session { endpoint, local }` pair — `view` was exactly on
  clippy's argument limit, and the flag gates the Tunnels button and the shell-integration item.
- `Cargo.toml`: `portable-pty`, plus `Win32_System_Time` (the machine's timezone, for the pane's mtimes)
  and `Win32_Storage_FileSystem` (`GetLogicalDrives`, so the virtual root does not spin up a floppy
  drive asking whether `A:\` is a directory).
- Tests 1295 → 1333.

### Not done

- ~~**Nobody has clicked a Local button.**~~ Retired the same day: §104 exists because the user opened
  local `pwsh`, `powershell` and `cmd` tabs and pressed a key in them. The path from the button to the
  grid works, and the first thing a hand found on it was a keyboard gesture that could not work — which
  is the argument for the disclosure, not against it.
- **The local shell's working directory is unknown**, for the reason above. That is the one place a
  local session feels thinner than a remote one, and it is the price of not editing the user's profile.
  A `cd` cmote itself typed could be tracked without any announcement at all, which is the cheap half of
  the fix and is not done.
- **A local copy does not carry the source's modification time**, where an SFTP upload does. `std` can
  read a file time and not write one, and setting one means `SetFileTime` on Windows and `futimens` on
  macOS — two more platform calls for a cosmetic property. A copy lands stamped "now", like Explorer's
  copy-paste or `cp` without `-p`.
- **On Windows the details popup shows no owner, group or permission word.** Those are unix facts; the
  owner is behind a security descriptor and the other two do not exist. Left empty rather than invented,
  which is what the pane already does for a server that volunteers no attributes (§20).
- **macOS is written and unrun.** The Local bar offers zsh, bash and `pwsh` there; the paths, the copy
  engine and the session loop are the same code, but the zone is left at UTC because
  `GetTimeZoneInformation` has no counterpart in the bindings cmote already carries — so a local pane's
  times would be right about the instant and wrong about the wall clock.
- **UNC paths have no place in the `/C:` scheme.** `\\server\share` has no drive letter to be the first
  component, so it is refused rather than half-translated.
- **A local session is never a saved target**, so nothing about it is remembered between runs: no
  per-shell folder, no pane layout, no sort. §22's remembering is keyed on an endpoint, and a local
  session deliberately has nothing shaped like one.
- **Two local tabs on the same shell are indistinguishable** — same label, same chip. Fine while the bar
  is one row of four; not fine the moment anyone opens two `pwsh` tabs and looks for the right one.
- **The README has no Local section.** Its tour describes every other screen and the keyboard table now
  mentions the local Ctrl+D (§104), but the bar itself — what it offers, that the offer depends on what is
  installed, that the panes then show this machine — is described here and nowhere a user reads.
- **The catalogue is searched once per run.** A shell installed while cmote is open needs a restart to
  appear. The alternative is a dozen filesystem probes per frame, since the home screen redraws
  continuously.
- **`LOCAL_BAR_HEIGHT` is another eyeballed constant** in the home screen's context-menu placement,
  beside the two `ponytail:` ones already there. It is right for the current layout and nothing checks
  it.

## §104 — The key three shells drop

The first thing a hand found on §103's local sessions: **Ctrl+D does nothing** in a `pwsh`, `powershell`
or `cmd` tab, while the same key in a Git Bash tab logs out.

That is not a local-session bug. It is §30's gesture meeting a shell it was never designed against.
§30 pairs two presses into one motion — Ctrl+D at the shell is EOF, the shell logs out, the tab lands on
the home screen, and a second Ctrl+D closes the tab, exactly a terminal's own "Ctrl+D twice". The pairing
has an assumption in its first half: that the shell answers EOF. Every remote cmote can reach is a POSIX
shell and does. Three of the four local shells do not.

### What EOF actually does to each of them

Measured, not reasoned about — a throwaway probe drove a real ConPTY child of each catalogue entry through
the real `local::pty`, answered the cursor query the way the GUI does, waited for the prompt, wrote one
`0x04`, and recorded both the child handle and every byte that came back:

| Shell | `0x04` at the prompt | first byte back |
|---|---|---|
| Git Bash (MSYS `bash`), and zsh/bash on macOS | prints `logout` and **exits** — the session ends by itself | 1 ms |
| `pwsh` | echoes `ESC[93m^D` onto its input line, keeps running | 10 ms |
| `powershell` | the same 19 bytes, the same colour, keeps running | 17 ms |
| `cmd` | echoes the two characters `^D` bare, keeps running | 0 ms |

The three interpreters have an EOF and it is **Ctrl+Z**, but even that only ever means "end of stream" to a
program *reading* one; there is no byte that tells the interpreter itself to stop. `exit` is a command, not a
key. So the key had no useful meaning on three of the four shells cmote offers, and the second half of §30's
gesture was unreachable on them: you could never get back to the home screen without the mouse.

The echo column is the one that turned out to matter, and the first version of this section was **wrong**
about it — it recorded "nothing. No exit, no output", because the first probe watched the child handle and
never looked at the bytes. There is a signal here, and it was measured away by asking a narrower question
than the design needed.

### The rule: send it, then read the answer

Ctrl+D is **not taken**. It goes to the shell exactly as it would in any terminal, and what comes back
decides.

The first attempt did take it: on a local shell that ignores EOF, the key ran the teardown directly. That
was wrong in the case that matters most, and the report came back within the hour — with `node` running at
that prompt, Ctrl+D belongs to node, and node quits on it. Taking the key threw away a program's own EOF
handling in favour of something cruder, and a `pwsh` tab could not be used to run a REPL any more.

What makes the honest version possible is the echo. A Windows interpreter handed a control byte it has no
meaning for puts it in its input line and draws it as `^D`. So there are exactly two answers to read:

- the answer contains **`^D`** → nothing consumed the byte, so this is the shell's own EOF being ignored, and
  cmote runs the shell's own `exit` for it (`judge_eof` → `exit_the_local_shell`);
- **anything else** — a fresh prompt after node exited, a pager scrolling, a program's output → the byte did
  its job and the session is not cmote's business.

The cost of being right is one output round trip: 10-17 ms, measured.

### What the echo is answered with: `exit`, not a teardown

cmote tears nothing down on this path. It sends an interrupt to clear the input line — which is carrying the
`^D` the shell just echoed onto it — and types `exit`. Then the shell does what `exit` does: runs its exit
path, leaves, and the session ends because its shell ended, arriving through the same `Disconnected` route as
a shell the user quit by hand.

That indirection is the point, and it buys three things a teardown could not:

- **What ends is what echoed.** Measured, because it is the argument for the whole design: a `pwsh` started
  inside the tab's `pwsh` echoes `^D` too, and the sequence ends THAT one — the output comes back at the
  outer `PS C:\Users\cme>` prompt and the session's own shell is still running. A version that ran the
  session's teardown would have closed the tab out from under a nested shell.
- **Nothing is ever killed here.** No `Disconnect`, so no 800 ms window and no `TerminateProcess` fallback.
- **"Nothing happened" is a real outcome.** A shell that refuses the word leaves the session exactly where it
  was, which is the direction this rule fails in everywhere else too.

The interrupt is not politeness; it is what makes the word land. Same shell, same sequence, on an empty line
and on a half-typed one, with only the prefix changed:

| prefix | what happened |
|---|---|
| `0x03` (Ctrl+C) | the shell left, in **all six** cases |
| `0x08` (backspace) | fine on an empty line; on `Get-Chi` `pwsh` ran `Get-` with a PSReadLine suggestion attached and `powershell` ran `Get-exit` |
| nothing | `^Dexit` — refused everywhere |

So `shells::quit_sequence` is `0x03` + `exit` + CR, and it is what the Disconnect button's tidy teardown types
too (§104's "Asking before killing"): the half-typed line it used to garble was the same bug measured here.

`judge_eof` accumulates the answer across reads until the echo appears or `EOF_ANSWER_CAP` (64) bytes have
gone by. Every wrong answer it can give reads as "Ctrl+D did nothing" and never as "the session ended by
itself".

That accumulation is the fix for a bug this shipped with, and the bug is worth keeping because of where it
came from. The first version settled on the FIRST chunk, on the reasoning that a probe outliving its keypress
would weigh unrelated output — with one exception for a chunk ending in `^`, in case a read boundary fell
inside the needle. Both PowerShells then failed in the user's hands: `^D` appeared on screen and nothing else
happened. The echo arrives in **two** reads —

| chunk | bytes |
|---|---|
| 1 | `ESC[?25l` — six bytes, hiding the cursor. No echo, and not a partial one either |
| 2 | `ESC[93m^D…` — the echo, in PSReadLine's colour |

— so the rule decided "a program answered" on chunk one and disarmed. `cmd` answers in a single two-byte
read and worked; one shell of three passing is what a wrong boundary assumption looks like from outside.

Three probes had already been run over this exact exchange and none could have caught it: each one
concatenated every chunk into a `String` before printing, because what was being asked was "what does the
shell say", not "how does it arrive". The bytes were right in all three. The boundaries were invisible.

Two presses are not listened to at all:

- **Not while the alternate screen is up.** The echo test nearly covers this on its own — a pager scrolling
  answers with a screenful, not with `^D` — so what the guard really covers is a pager showing a *file* that
  contains the characters `^D`. And a full-screen program asked for the whole screen, so the key is its own.
- **Ctrl+Shift+D is unwatched**, though it encodes to the same `0x04`. That makes it the escape hatch out of
  this whole rule: the way to hand a bare EOF to a shell that would echo it, with the session left alone.

Matched on the **logical character**, unlike the copy/paste bindings a few lines above it, which match the
physical key so they hold on any layout. This one accompanies a byte the encoder derives from the character
itself (`control_byte`), so the key that sends EOT is the key that is watched — and it is the same match
§30's home-screen Ctrl+D uses, which keeps the two halves of the gesture one key rather than two that happen
to agree on QWERTY.

### A held key is one press

The other half of the same report: "the tab just closed instantly". It had, and a single press cannot do
that — ending a session lands on the home screen and leaves the tab alone. Two key events can, and a held
Ctrl+D produces them: the first ended the shell, the **auto-repeat** arrived on the home screen a few tens
of milliseconds later, and §30's second half closed the tab. The screen the gesture is meant to land on was
never seen.

`is_close_tab` now refuses a repeat, which is what §30 meant by two presses all along. Extracted as a
predicate beside `is_typing` and `is_paste` for the ordinary reason: what a key MEANS is the testable half
of a handler that otherwise only returns an opaque `iced::Task`.

### What "ends the session" actually is

Worth spelling out, because "cmote ends it" read gentler than what the first version did. As written, a
local teardown was **not** a shell being asked to leave:

1. The **GUI went first, synchronously**: the session is remembered (§22), transfers are abandoned,
   `Disconnect` is *queued*, the emulator is dropped and the tab is on the home screen — before anything
   on the session side has happened. So the navigation does not wait for a clean anything.
2. The session task then took `Disconnect` and called `Pty::close`, which is `TerminateProcess` on the
   shell. No profile exit hook, no `exit`, nothing flushed on the way out.
3. Dropping the master closes the pseudoconsole, and whatever the shell had started dies with it.

Step three was measured with the case that prompted the question: `node` at a local `pwsh` prompt, node
v24.18.0, a real ConPTY. The REPL **stays on the main screen** (`is_alternate()` is `false`), which is why
the pager guard could never have saved it — and why the rule above reads the shell's answer instead. After a
teardown the node process is **gone**, so nothing is orphaned; it was never asked to exit either. The grid
goes with the emulator, so what was on screen is not there to read afterwards.

A remote Disconnect is cleaner than that by nature: it closes an SSH channel and the far side's shell gets
a hangup it can act on. A local session has no protocol to be clean *in*, which is why step two now has a
shell being asked first.

### Asking before killing

`Tab::end_session` replaces the plain `Disconnect` at **every** teardown — the button, Ctrl+D, a tab
closing, cmote quitting — because the difference was never in why the session ends. For a local session it
types the shell's own `exit` (one word for all six shells, `shells::QUIT_COMMAND`) and then asks for the
teardown; the session task waits up to `GOODBYE` (800 ms) for the child to actually go before the kill.
The kill is now the fallback rather than the mechanism, so a shell gets its own exit path: PSReadLine
flushing history, a `~/.bash_logout`, an exit trap the user wrote.

Measured through the real session task with a real `pwsh`, from `Disconnect` to `Disconnected`:

| | time | what happened |
|---|---|---|
| asked (the ordinary case) | **115 ms** | the shell exited on its own; no kill, no log line |
| unasked (a full-screen program was up) | **801 ms** | the window elapses and the kill ends it |

Two things about that, both deliberate:

- **The word is not typed at a full-screen program.** `exit` is not a message, it is four keystrokes: at a
  `vim` in normal mode `x` deletes the character under the cursor and `i` starts inserting, so the tidier
  teardown would edit the user's file on the way out. The GUI holds this decision because only the GUI can
  see the grid, and `end_session` runs before the emulator is dropped for exactly that reason.
- **The wait is spent either way**, because the session task cannot see the grid and so cannot know whether
  anything was typed. That is the 801 ms row: invisible everywhere except a quit, and `GOODBYE` is checked
  at COMPILE TIME to be well inside `QUIT_DRAIN_TIMEOUT` (§30) so a quit can never end up waiting for the
  drain's own timeout instead. Two constants in two modules, related by an assertion rather than by a
  comment.

### The prefix that is not cosmetic

Worth keeping the intermediate result, because it is why the prefix is an interrupt and not an erase. A
backspace was tried first, on the reasoning that the echo is exactly one character: with it, `pwsh` showed
`exit` in PSReadLine's green and left in **99 ms**; without it, `^Dexit` in red, `exit: The term 'exit' is not
recognized…`, still running at 2.5 s. Both true, and both measured on an EMPTY line — which is what made the
erase look sufficient. It is not: a line the user had started typing on needs the whole line gone, not one
character, and the table above is what that costs when you get it wrong.

### What it cost

- `local/shells.rs`: `Kind::quits_on_eof`, `QUIT_COMMAND` and `quit_sequence` (with the prefix table in its
  doc), plus the test that writes the EOF split down per kind.
- `app.rs`: the listening block in `on_key`, `judge_eof`, `eof_probe` on the tab, `EOF_ECHO` /
  `EOF_ANSWER_CAP`, `exit_the_local_shell`, `on_alternate_screen`, `is_close_tab` extracted with its missing
  `repeat` term, `end_session` at the three teardown sites that had a plain `Disconnect`, and
  `QUIT_DRAIN_TIMEOUT` made `pub(crate)` so the goodbye window can be checked against it. Seven tests: the
  byte reaching the shell first, the echo answered with `exit` and the session ending only when the shell
  does (the echo fed as two chunks, split inside the needle), the two answers that keep it, the shells that
  are never listened to, the two presses excluded, what a teardown types at which shell, and the held key.
- `local/session.rs`: `GOODBYE`, the compile-time tie to the quit budget, and `farewell` — which drains the
  output while it waits, because the shell's last words must not fill a bounded channel and block the exit
  they are on the way to.
- `README.md`: the keyboard table, the tour's quit paragraph, and the manual-test step, which now names
  the pager case and the history file.
- `docs/ctrl-d-on-windows-consoles.md`, the first file in a new `docs/` folder: the whole rule in one
  place — every guard with its reason, every measured table, the two-read answer, the tests and what each
  pins, and a "where to change what" map — with four hand-written SVGs under `docs/img/` (the decision
  flow, the two-chunk answer beside the buffer it accumulates into, the probe's two states, and the
  teardown times against the two budgets). Written because this section is a *log* — it records how the
  design was arrived at, including the two wrong turns — and someone changing the code needs the result
  without reading the history. The stale half of `quits_on_eof`'s own doc comment (the first probe's "no
  output", and a pointer to a `Tab::end_local_shell` that no longer exists) was corrected on the way.
- Tests 1333 → 1343, one of which drives a real shell end to end.

### Not done

- **The rule reads two characters of the shell's own output.** `^D` is how all three interpreters render an
  ignored control byte, so the needle is theirs and not cmote's invention — but it is still a string match on
  a stream cmote does not control. It is only ever looked for in the answer to a Ctrl+D cmote itself sent one
  round trip earlier, at a local shell, on the main screen, which is as narrow as the window gets without a
  protocol nobody offers here. A program that answers a Ctrl+D by printing `^D` within that window gets an
  interrupt and the word `exit` typed at it, and nothing worse: no teardown rides on this any more.
- **A half-typed command is discarded rather than kept.** The interrupt clears the line, so a Ctrl+D pressed
  after typing `Get-Chi` throws that away and exits — where `bash` would have done nothing at all, since its
  Ctrl+D only means EOF on an empty line. cmote cannot tell an empty prompt from a full one (the echo says
  where the cursor is, not what is left of it), and of the two ways to be wrong, exiting is the one the key
  was pressed for.
- **The needle has to arrive within 64 bytes of the answer.** A shell that clears half the screen before
  echoing would spend the budget first and be read as having consumed the byte. Six bytes is what the
  measured shells say beforehand, so there is an order of magnitude of room, and the failure is one more
  press.
- **Ctrl+D inside a pager still does nothing to the session**, which is right, and nothing tells the user
  which of the two states they are in. A terminal has never had to say.
- **The Disconnect BUTTON still types at whatever is in front of the shell.** Ctrl+D no longer does — it only
  types where the echo proved there is a prompt — but the button, a tab close and a quit have no such proof
  and type anyway. With `node` running that is an interrupt plus a word node answers with an error, and the
  800 ms kill a moment later. Telling a running program from a prompt needs an announcement §103 refuses to
  install; the echo trick cannot help, because there is no key press to answer.
- **The probes were thrown away rather than kept — six of them**, and one test was kept instead. What EOF does
  to each shell, what comes back from it, whether an erase makes the `exit` land, which prefix lands it on a
  half-typed line, what the sequence ends when a shell is nested, and finally how the answer is CHUNKED. The
  fixture-fed unit tests could not have found that last one, so `a_real_local_shell_answers_ctrl_d_by_leaving`
  now drives a live `pwsh` through the session task, the tab's own handlers and the translation `ssh::client`
  does between them, and asserts the shell leaves. It was run against the broken `judge_eof` first, to check
  that it fails rather than passes — and it exposed a second sharp edge in doing so: awaiting a session task
  whose shell never left hangs the test instead of failing it, so the cleanup now disconnects first and gives
  up after five seconds. A test that hangs is not a test.

## §105 — The Git that was there all along

Found while measuring §104: this machine has Git for Windows, and cmote's Local bar has never offered
Git Bash on it. The shell whose Ctrl+D the whole of §104 was written *against* could not be opened.

§103's search looked in two places, and Git for Windows fits in neither:

- **The Program Files folders**, plus `%LOCALAPPDATA%\Programs\Git` for a per-user install. This Git is
  in `C:\git`, because the installer accepts any directory and people use that.
- **`PATH`**, by naming `git.exe` and walking two levels up. The installer's PATH question has a "use Git
  from Git Bash only" answer that adds nothing to `PATH` at all — and neither the machine `PATH` nor the
  user `PATH` here contains anything of Git's.

So both searches were sound and both missed, silently, which is the worst shape a search can have: the
bar's contract is that a button which appears can be pressed, and it says nothing about a shell it failed
to find.

### The third place to look

The installer writes its own root to `HKLM\SOFTWARE\GitForWindows\InstallPath` (`HKCU` for a per-user
install). `recorded_git_roots` reads it with one `RegGetValueW` and hands the roots to the same test every
other candidate passes — `<root>\bin\bash.exe` has to exist before a button is offered.

This is still a **known location** in the sense the module note means, and that distinction is the whole
security story of this module: the key is fixed, its value is written by an installer, and nothing the
user types at cmote reaches any part of it. What comes out is a path that must resolve to a real
`bash.exe` inside a Git installation — which is also what keeps `System32\bash.exe`, WSL's launcher, out
(§103).

### What it cost

- `local/shells.rs`: `recorded_git_roots`, `registry_string`, `wide` — about seventy lines, most of it the
  `SAFETY` note and the reason the key counts as a known location. One test, which asserts that the search
  agrees with the installer's record and skips honestly on a machine with no Git.
- `Cargo.toml`: `Win32_System_Registry`, one feature for one call.
- Tests 1337 → 1338.

### Not done

- **Nothing reads `CurrentVersion` beside it.** cmote takes the root and looks for `bin\bash.exe`; a key
  left behind by an uninstall names a root with no bash in it and is discarded by the same check that
  discards a bad guess.
- **A machine with two Git installations offers one button.** The per-machine install wins, being read
  first, and the per-user one is not offered as a second Git Bash — two buttons with the same label would
  be worse than one (§103 has the same complaint about two `pwsh` tabs).
- **The 32-bit registry view is not searched.** A 32-bit Git on a 64-bit Windows writes under
  `WOW6432Node`, and cmote's process reads the 64-bit view. `RRF_SUBKEY_WOW6464KEY`'s sibling flag would
  add it; a 32-bit Git for Windows on a machine new enough to run cmote is not worth the second call until
  someone has one.
- **macOS has no equivalent and needs none** — `zsh` and `bash` are at `/bin`, and the search there is
  `PATH` plus that one folder.

## §106 — A limit the engine does not share

The architecture review that followed §105 found, as its top recommendation, that the eleven CSI scanners in `term/` each carry their own copy of one grammar, with `const ESC`
spelled thirteen times and `MAX_PARAMS` spelled eight times at **three different values** — 16, 32 and
64. The duplication was the finding. The values were the defect.

A scanner beside the stream and the engine inside it read the same bytes. Wherever the two disagree about
whether a sequence is well formed, one of them acts and the other does not — and that, not the copying,
is what the numbers were quietly costing. **Four** disagreements turned out to be reachable, each verified
against the vendored `vte` and each fixed here with a failing test first. Three were found by hand. The
fourth was found by a harness built *because* finding them by hand had stopped being funny.

### What the engine actually does

Read out of `vte-0.15.0`, because every claim below had been assumed rather than looked up:

| fact | value | source |
|---|---|---|
| parameter limit | `MAX_PARAMS = 32`, counting **PARAMETERS** | `params.rs:5`, full test `:49-51` |
| exceeding it | `ignoring = true`, then the whole sequence is dropped | `lib.rs:454-517`, `ansi.rs:1545-1548` |
| digits per parameter | **no limit** — folded in with `saturating_mul` | `lib.rs:514-515` |
| intermediates | `MAX_INTERMEDIATES = 2`, and a private marker counts against it | `lib.rs:44`, `:207-210` |
| sub-parameters | share the parameter budget | `params.rs:16-19` |

Two of cmote's own numbers were counting the wrong thing entirely: `MAX_PARAMS` was a cap on parameter
**bytes**, which is neither of the two things the engine bounds.

### The three defects

- **§57 — a padded DECSLRM reached the engine as a save-cursor.** `cancel.rs` gave up past 32 parameter
  bytes, so `CSI 000…0001;80s` was never judged; the engine counts two parameters, dispatches
  `('s', [])`, and saves the cursor. With mode 69 set that is the margins silently not applied **and** a
  cursor the program never asked to save overwritten — the exact harm §57 exists to prevent, reached
  through the limit that was supposed to protect it.
- **§56 — protection was silently lost across a long SGR.** `protect.rs` gave up past 32 parameter bytes
  too. `CSI 0;1;2;3;4;5;7;9;21;30;31;38;5;196m` is 33 of them, which true colour reaches with room to
  spare: the scanner dropped it, the engine applied it, and the `Attr::Reset` inside it assigned the flag
  word whole and took the borrowed protection bit with it. Nothing reported the SGR, so nothing put the
  bit back. This is the worst of the three — the feature quietly stops working on the very sequence the
  scanner was written to watch for.
- **§41 — pictures outlived the text they sat beside.** Two ways at once. `graphics.rs` compared the
  erase's parameter BYTES to `b"2"`, so `CSI 002 J` and `CSI 2;5 J` — both an erase-the-screen to the
  engine, which reads `Ps` through `next_param_or(0)` — said nothing; and its 16-byte cap dropped a
  padded one for the same reason as the two above.

### One cap, one clamp

`term/csi.rs` is new, and holds the engine's numbers once with their `path:line` beside them. It is also
where the shared framer lands, so the module exists early rather than being invented twice.

The rule it settles is that the two bounds are **not the same kind of bound**:

- **Too many parameters ends the sequence.** Giving up is right here, because the engine gives up too:
  both sides ignoring the same bytes is agreement.
- **A long digit run is CLAMPED, not capped.** Copying the engine literally would mean buffering
  unbounded remote input, which §12 refuses; abandoning is what caused all three defects. So digits past
  five SIGNIFICANT ones are dropped and the sequence lives — exactly the engine's answer, because five
  digits already reach past `u16::MAX`, so any value a sixth could produce is one the engine has
  saturated too. What a hostile stream can make cmote hold is then under 200 bytes.

`cancel.rs` needed neither: it buffers nothing at all — two `u16`s and two counters — so §12 has nothing
to bite on there and the run is simply uncapped. Worth writing down, because "no limit" reads like an
oversight and here it is the only correct answer.

**Leading zeros cost nothing**, and that was the second attempt rather than the first. Clamping the first
five digits of `000000000000000002` keeps `00000` and reads 2 as **0** — the same defect wearing the fix's
clothes, and it survived until the test written for the erase failed on it. Zeros are not significant, the
engine's fold over them is the identity, so they are dropped and counted against nothing.

`Params::started` exists for a consequence of that: `bytes.is_empty()` no longer answers "has a parameter
arrived", since a dropped leading zero leaves it true. A caller reads emptiness as "a private marker is
still legal here", so without the flag `CSI 0?J` — which the engine drops outright — would have
classified as a selective erase. A fix's own side effects are where the next defect lives.

### Asking the engine instead of reading it

Three defects found by reading a crate and noticing a constant counted the wrong thing. That does not
scale, and it does not survive a version bump: the next `vte` could change any of the five facts in the
table above and every one of cmote's scanners would go on agreeing with a note about the old ones.

So `term/differential.rs` asks the question directly. `alacritty_terminal` re-exports `vte`, which means a
test can drive the **actual parser the engine is built on**, record what it dispatched, and compare that
with what cmote's scanner made of the same bytes. No new dependency, and no crate-reading in the loop:
the answers come from the parser rather than from a paragraph about the parser.

**It found the fourth defect on its first run**, which is the whole argument for it. The parser runs a C0
control that arrives mid-sequence and CARRIES ON with the sequence around it (`lib.rs:190`, `:230`,
`:241`), ignores DEL (`:222`, `:251`), and hands a byte past `0x7f` to `anywhere`, which does nothing with
it (`:438-449`). Only CAN (`0x18`) and SUB (`0x1a`) abandon a sequence — the ANSI state machine's own
definition, and the reason cmote feeds CAN in place of a final byte it refuses — and ESC restarts one. All
three scanners gave up on **every** one of those bytes, so:

| the stream sends | the engine does | cmote did | what it cost |
|---|---|---|---|
| `CSI 5;` LF `70 s` | runs the line feed, dispatches save-cursor | gave up at the LF | §57 again: margins not applied, saved cursor overwritten |
| `CSI 0;` LF `1 m` while DECSCA is armed | runs the line feed, applies the SGR | gave up | §56 again: `Attr::Reset` takes the protection bit, nothing puts it back |
| `CSI 0` LF `2 J` | runs the line feed, erases the screen | gave up | §41 again: the pictures outlive the text |

`csi::passes_through` names the rule once, with the parser's `path:line` per arm, and the three scanners
defer to it. Three of their tests were rewritten, because each was asserting the disagreement — and each
now asserts the rule *and its converse*, since "keep reading" must not quietly become "keep reading for
ever": only CAN and SUB cancel, and DEL is not one of them.

The remaining divergence gets a test that asserts it **as it behaves today**, not as it should behave: a
parameter byte after an intermediate, which the parser refuses outright (`lib.rs:232`) and which cmote's
scanners classify anyway. Harmless for now — the sequence it lands on is cmote's own feature, so there is
no engine action to contradict — and written down as code so the framer has to flip it on purpose rather
than silently. A divergence with a test around it is an inventory item. A divergence with a comment around
it is what the last four defects were.

### The sweep that could not fail

The shape sweep is the fifth defect's story and a lesson about tests, in that order.

A generator over private marker × intermediates × parameter run × final byte, 1920 sequences, asking one
question in one direction: **does any scanner act on bytes the parser threw away?** The converse is not a
defect — the parser frames a great deal cmote deliberately ignores — but this direction means cmote is the
only terminal in the world obeying a spelling, which is exactly the shape §106's ordering fix had been.

It passed. Then it was run against that ordering fix **reverted**, to check it could fail. It passed again.

The generator emitted the parts in the order the grammar defines — marker, then parameters, then
intermediates — so the malformed interleaving the bug needed was one the sweep never produced. 1920 green
cases, and a hole where the thing being tested was. A test that cannot fail is worse than no test, because
it also reports that the area is covered.

So the generator grew an **Order** axis: the well-formed one, a parameter byte after the intermediates
(`lib.rs:232` → `CsiIgnore`), and a private marker after the parameters (`lib.rs:249`, the same). 5760
sequences, and it now fails against either guard reverted — 16 cases for one, 19 for the other.

With the axis in, it found a live defect: **19 sequences, every one of them `protect`, every one a marker
arriving after the parameters.**

| the stream sends | what protect made of it | what the engine did |
|---|---|---|
| `CSI ? 1;2 ? J` | a selective **erase** — cells wiped | dropped the sequence whole |
| `CSI 1 ? m` | a spurious `Reassert` (a no-op, since re-asserting a set bit changes nothing) | dropped it |
| `CSI 1 " ? q` | a spurious DECSCA | dropped it |

The first row is the one that matters: cmote erased cells for a sequence nothing else obeys. The reason it
got that far is worth keeping — `first_param` reads only the FIRST field of the run, so the stray marker sat
in the second where nothing looked at it.

Only `protect` needed the guard, and the other three that buffer a parameter run are clean for a reason
worth writing down rather than leaning on: their classifiers parse the whole run as numbers, so a marker
byte inside it fails the parse and the sequence falls through. `protect`'s does not — two of its five arms
never look at the parameters at all, which is what let a malformed one through to an erase.

Two things this leaves behind. The three clean scanners are clean **by consequence**, not by rule, and the
sweep is what would notice if that consequence ever changed. And the lesson generalises past this section:
a generated corpus is only as good as the axis it varies, and the axis worth adding is the one that makes
the code under test look wrong.

### What it cost

- `term/csi.rs`: `MAX_PARAMS`, `MAX_DIGITS`, `Params`, and five tests including the leading-zero case and
  one asserting that `u16::MAX` fits inside the clamp, which is the whole argument for clamping.
- `term/cancel.rs`: the cap gone, the field's doc explaining why nothing replaces it, and the test that
  asserted the old behaviour rewritten — it was asserting the disagreement.
- `term/protect.rs`: `Params`, `first_param` saturating on overflow while still refusing a non-digit (a
  number past `u16` reads the same on both sides; a field that is not a number is §54's malformed input
  and stays a no-op), and the intermediates cap left local with a note that it is looser than the
  engine's two and cannot be observed.
- `term/graphics.rs`: `Params`, and a `first_param` spelling `next_param_or(0)`.
- `term/csi.rs` again: `passes_through`, the control-byte rule the fourth defect needed.
- `term/differential.rs`: the harness — a `vte::Perform` that records what the parser dispatched, refused
  and executed, plus a generator for every spelling of a sequence the engine reads identically. Eleven
  tests: six hand-written (five agreements pinned from both sides, one the divergence that remains,
  asserted as it behaves), three sweeps over 86 generated cases, and one that feeds every case a byte at a
  time and requires the same verdict.
- Tests 1343 → 1364. Four commits, one per defect, each red before green.
- `TERMINAL_COMPATIBILITY_PLAN.md`: a `term/csi.rs` entry, the three scanners' entries, the ED row, the
  cancel test count, and a mangled sentence in `protect.rs`'s entry that had been sitting there since §72.

### Not done

- **Nine scanners still carry their own grammar**, minus the two rules that have been hoisted out of it.
  This paid for the limits and the control bytes, not the framer: `dsr`,
  `tabs`, `scp`, `sgrstack`, `rect`, `modkeys` and `query` still declare their own `enum Scan` and their
  own bounds. Their disagreements with the engine are currently **unobservable** — it has no live arm
  behind any of their sequences — which is a fact about today's `vte` and not a property of the code. A
  version bump that fills one of those empty trait bodies makes them observable, and the framer is what
  would make that a non-event.
- ~~The control-byte rule is fixed in three scanners and wrong in five.~~ **Done**, and the reason it was
  parked is worth keeping: the note said a fix with no failing test behind it is a change rather than a fix,
  and that the harness could not write one for a sequence the engine has no arm for. Half right. There is no
  engine ACTION to compare against, but SELF-CONSISTENCY is testable and worth the same — a stray byte the
  engine reads through must not change cmote's verdict, or the two disagree the moment a version bump fills
  one of those empty handler bodies. All five failed that test, which was written first. All eleven scanners
  now defer to `csi::passes_through`.
- ~~A parameter byte after an intermediate is still read differently.~~ **Done**, and it was the one
  divergence leaning the other way: cmote acting ALONE rather than failing to act, on a spelling the engine
  refuses outright (`lib.rs:232` → `CsiIgnore`, then nothing dispatched). No engine behaviour was being
  compensated for, so there was no upside to weigh. The four scanners that buffer intermediates abandon the
  sequence now; `dsr` and `tabs` need no guard, because their classifiers already fall through to no
  request when an intermediate they do not expect is there — a check would be dead code.
- **`MAX_INTERMEDIATES` is still 4 in six places**, against the engine's 2 — and the engine counts the
  private marker against that budget while cmote counts it separately. Reachable only with a sequence
  carrying three intermediates, which no scanner here claims: each matches on the intermediates it expects,
  so a longer run falls through to no request while the engine sets `ignoring` and drops it. Both refuse, by
  different routes, which is agreement of a weaker kind than the rest of §106 settles for.
- **A sub-parameter is counted like a separator, not like the engine's shared budget.** `Params` counts
  `:` against `MAX_PARAMS`, which is the right order of magnitude and not the same arithmetic. The case
  that would tell them apart is an SGR with 33 sub-parameters, where cmote would reassert protection that
  was never cleared: a no-op, disclosed rather than fixed.
- ~~The sweep covers one family, not the grammar.~~ **Done, and it found the fifth defect.** The shape
  generator walks marker × intermediates × params × final byte × ORDER — 5760 sequences — and asks the
  one-directional question: does any scanner act on bytes the parser threw away? The converse is fine (the
  parser frames plenty cmote ignores); this direction means cmote is the only terminal in the world obeying
  a spelling. See "the sweep that could not fail" above.
- **Chunk-safety is swept for three scanners and claimed by ten.** Every variant is fed one byte at a
  time as well, and the verdict has to hold — the property §104's Ctrl+D rule broke, on a two-read answer
  it settled after the first read. The other seven scanners make the same claim in their own docs with
  only their own hand-written boundary test behind it.
- ~~It compares the PARSER, not the engine.~~ **Answered in §107**, which compares the handler by running a
  second engine with no gate in front of it. The gap as it stood: `vte::Parser` says what would be
  dispatched; it does not say
  what `alacritty_terminal`'s handler then did with it, and `ansi.rs` has arms that discard a sequence the
  parser was happy with. So "the engine dispatched it" is a lower bound on agreement, and a scanner could
  still act on something the handler ignores.
- ~~§102's region mirror wants the same treatment.~~ **Looked at in §107, and aimed elsewhere.** Its
  arithmetic is a transcription of the engine's and its eleven tests all check the transcription against
  itself, which is what made it look like the risk. It is not: the gate hands the SAME two parameters to
  the mirror and to the engine on consecutive lines, so there is no second derivation to diverge. What the
  mirror FEEDS — the gate's own re-implementations of a dozen engine methods — is where the differential
  test was owed, and §107 is it.

## §107 — The other side of the gate

§106 closed with two admissions. Its harness compared the **parser** and not the handler, so "the engine
would have dispatched this" was only a lower bound on agreement. And §102's region mirror — the one piece
of cmote that copies engine state rather than reading the stream beside it — had no differential test at
all, eleven tests that all checked a transcription against itself.

This section is those two, and the second one turned out to be aimed at the wrong module.

### The mirror was not the risk

`term/region.rs` looks like the dangerous thing: cmote holding a copy of a private engine field. Reading
`gate.rs:457` says otherwise.

```rust
fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
    self.region.set(top, bottom, self.term.screen_lines());
    self.term.set_scrolling_region(top, bottom);
}
```

**The same two parameters go to both.** Not a re-derivation from the bytes, not a second parse — one
`Option<usize>` handed to the mirror and to the engine in consecutive lines, with the page height read off
the engine itself. The arithmetic that follows is a transcription of `Term::set_scrolling_region`
(`term/mod.rs:2155`) line for line: the same `bottom.unwrap_or(screen_lines)`, the same `top >= bottom`
refusal, the same two `min`s against the page. For the two to disagree, one of those five lines would have
to be wrong — and a test written from the same reading of the engine that produced them would agree with
whichever way it was wrong. That is the tautology: an expected value derived the way the code derives it
cannot disagree with the code.

So the mirror got no new tests. What it FEEDS got them.

### What the mirror feeds

`term/gate.rs` re-implements about a dozen of the engine's own `Handler` methods so that a left or right
margin can bound them — LF, NEL, RI, SU, SD, IL, DL, ICH, DCH, CUF, CUB, CR, HT, and the glyph path
itself. It is the most dangerous file in `term/` for a reason that has nothing to do with mirroring: every
other module there acts *beside* the engine and cannot break what the engine does, while this one stands
**in its place**. And it had **no tests of its own** — zero — being exercised only through the 194 that go
in at `Terminal::process`.

Every one of those arms opens the same way:

```rust
if !self.narrowed() { self.term.<the engine's own>(…); return }
```

Which is a property worth stating out loud: **with no margins, cmote's arithmetic must not run at all.**
Bytes in, the engine's own answer out. That is what an ordinary session — every session, since no shell
sets margins — actually depends on, and nothing checked it.

### An oracle that is measured rather than read

`term/gatediff.rs` builds a **second engine**: same `engine_config()`, same page, and no gate in front of
it. Both are fed the same stream and the whole document is compared — every cell's character, attribute
bits and both colours, the cursor, and the scrollback depth.

The point is where the expected values come from. §106's harness had the same virtue one layer down: not
"here is what I believe `vte` does", but `vte` itself, running. Here it is not "here is what I believe the
engine's `linefeed` does" but the engine's `linefeed`, run on the same bytes. **Measured, not
transcribed.** That is the whole difference between this and the region tests it replaced the idea of.

Fifteen margin-free streams, chosen to reach the arms rather than to be pretty: a line feed past the last
row (which has to reach the scrollback), a line feed on the last row of a region, SU and SD, IL and DL both
inside a region and with the cursor outside it, RI at a region's first row, a region whose top is zero (the
one the engine stores *above* the page), a region as tall as the page, CNL and CPL, an autowrap over the
page edge, the deferred wrap held on the last column, cursor motion that stops at the edges, a tab, a
region set backwards, and NEL — which the PARSER expands into `linefeed()` then `carriage_return()`
(`vte-0.15.0/src/ansi.rs:1802-1805`), so the stream exercises the two methods the gate has replaced while
`Handler::newline` is never dispatched at all. That last part was found by the code review below, and it
means `Gate::newline` is unreachable — it exists because `#[deny(clippy::missing_trait_methods)]` requires
every method to be written out, which is the deny doing its job rather than a defect.

### Six properties, and the proof each could fail

Green on the first run, every one of them. A pass-through is *supposed* to be green, which is exactly the
situation §106 spent a subsection on: a test that cannot fail also reports the area as covered. So each was
broken on purpose before being kept.

| property | broken how | what the sweep then said |
|---|---|---|
| with no margins the gate is the engine | `linefeed`'s `!narrowed()` disabled | 6 of 15 streams, first line `scrollback depth: gated 0, engine alone 2` |
| margins as wide as the page are not margins | `narrowed`'s `right + 1 < cols` widened to `<=` | 5 of 15, **two of them only in the `WRAPLINE` flag** |
| a band operation leaves everything outside the band alone | `scroll_band` given the whole width | 238 disturbed cells |
| a band scroll files nothing in the scrollback | the gate made never to narrow | depth 0 → 2 on SU, 0 → 1 on a line feed |
| a band operation leaves the cursor inside the band | (arithmetic, not a mutation — see below) | — |
| an operation refused outside the band changes nothing at all | both `cursor_in_band()` guards deleted | 2 refusals acted, each naming a cell **inside** the band |

Two of those deserve reading twice.

**The scrollback line.** `scroll_band` discards the row it pushes out of a band, on purpose — the history
holds whole lines and a band row is a slice of one (`margins.rs`'s header argues it, xterm agrees). Correct
for a band, catastrophic for a page: a gate that performed a band scroll where it should have forwarded
would look *perfect on screen* and silently drop every line that scrolled off. Nothing but a scrollback
comparison sees that, which is why the seam was agreed to include one.

**The `WRAPLINE` flag.** Two of the five divergences differ in no character at all — only in bit 4 of the
attribute word. That is the flag search, selection and copy read to join a wrapped line back into one
logical line (§35, §40), and the band wrap deliberately does not set it (`gate.rs:246`). Had the sweep
compared characters only, a change that quietly unstitched every wrapped line in the find bar would have
passed it.

### The margins-on half is a reading, and says so

The user asked for the margins-ON rows too, and they cannot be oracle-backed: the engine does not implement
a left margin, so there is no second implementation to measure against. Those two properties come from
xterm's own definition — an operation bounded by the margins moves the columns between them and nothing
else, and a row leaving the band is discarded — and the module labels them as such where they are
declared, beside the streams that are measurements. Ten operations that act and four that are refused, each
run on a page filled row by row
and then photographed before and after, so while the RULE is a reading, the expected value for every cell
outside the band is the cell that was already there.

What they do NOT say is that the band moved *correctly*. That is `mod.rs`'s fifteen margins tests, which
came with §102. These say only that the operation stayed where it belongs.

### A property a refusal satisfies for free

The review of this section found the same shape of hole §106 had found in itself, one layer along, and it
is worth writing down twice because it arrived from a different direction both times.

Two of the twelve band operations were **refusals** — IL and DL with the cursor outside the band, which the
gate declines outright. They sat in the same list as the ten that act, and both sweeps over that list
asserted only two things: that the columns OUTSIDE the band were untouched, and that the scrollback depth
had not moved.

A refusal satisfies both of those by doing nothing. So does an operation that wrongly went ahead **inside**
the band. The two assertions could not tell those apart, and the consequence was measurable: with both
`cursor_in_band()` guards deleted, every test still passed.

The fix is a sweep of its own, and the one place in this module where the WHOLE document has to come back
identical — cells, cursor, history, inside the band and out, because a refusal is the one case with nothing
legitimate to change. Four operations, since `ICH` and `DCH` are refused too, by `shift_cells`'s own cursor
test rather than by `cursor_in_band`. With the two guards removed it now reports both refusals, each naming
a cell *inside* the band that changed — a `'C'` that became a blank, and a `'C'` that became an `'E'`.

Removing `shift_cells`'s test instead produced something better than a failed assertion: a panic on
`right - cursor + 1`, because `cursor` is past `right` in exactly the case the test refuses. That guard is
holding up the arithmetic as well as the meaning, which nothing had said out loud.

**The lesson, stated generally.** A test whose assertion is "nothing outside X changed" cannot test a rule
whose subject is "nothing changed", and a corpus that mixes acting cases with no-op cases will report the
no-ops as covered. §106's version of this was a generator that never emitted the malformed order its guard
was written for. This one was a list that mixed two kinds of entry. Both times the tell was the same, and
both times it was only visible by asking the question the other way round: not "does this pass?", but
"what would have to break for this to fail?"

### What it cost

Three commits, one new test-only module of 604 lines, and the only production code touched is one
constructor extracted so both engines are built by the same function — the five
breakages were each reverted by hand, and `git diff --numstat` was checked against `HEAD` before every
commit, because §106 lost an uncommitted fix to a careless `git checkout --` and that lesson was expensive
enough once.

`cargo test`: 1370 → 1376.

### Not done

- ~~**The corpus is hand-picked, not generated.**~~ It was 15 streams and 12 operations, chosen by
  reading the gate for what it forwards, against §106's shape generator one layer down. **Generated in
  §111**: every hand-written gate arm × eight arrivals × four scrolling regions, 832 streams, run
  against the second engine both plainly and behind a full-width band. It paid at once — deleting
  `insert_blank`'s `!narrowed()` guard leaves the hand-picked corpus entirely green and fails 24 of the
  generated streams, because ICH was never in the fifteen — and the shape sweep's lesson held twice
  over: the two axes that matter are the ones a mutation got past the sweep without, which were **how
  the cursor arrives** (parked by CUP, which clears the pending wrap, versus written to the end of a
  row, which does not) and **a glyph after the operation**, since state an operation leaves behind is
  not in the document until something has to be drawn.
- ~~**The `!narrowed()` guard is checked, not enforced.**~~ **Enforced in §111**, as far as it can be
  without a second parser: the sweep's coverage list is compared against `gate.rs`'s own source —
  the slice from `impl Handler` to the `forward!` invocation, so the generated arms are excluded — in
  both directions. An arm added to the gate without a stream fails a test, and so does a listed arm
  that no longer exists, which would otherwise leave the sweep quietly testing nothing.
- ~~**`Terminal::process` is compared, so cmote's synthesised sequences are out of scope.**~~ True, and
  §111 found what they CAN be held to: not the oracle, but cmote against cmote — once with margins
  never mentioned, once behind a band as wide as the page. Both runs make the same scanners do the same
  work, so the scanners cancel out and what is left of any difference is the gate's own `narrowed()`
  decision. Eight streams: selective erase, rectangular erase, fill and attribute change, a picture, a
  soft reset, a tab-stop rebuild, an SGR push and pop, and a hard reset.
- ~~**The framer is still the review's top recommendation and still unbuilt.**~~ §106 hoisted two rules
  out of the ten scanners' duplicated grammar and left the machine itself in each of them. **Built in
  §111**, and all ten are migrated — which is where the defects that duplication was hiding came out.

## §108 — One place a word is defined

This project's paper trail had a gap nobody had named: 106 numbered sections explaining every
decision, 95 file headers explaining every module, and **nowhere that simply said what a word
means**. Ask "what is a band?" and the honest answer was "read §102". Ask "is a profile a
target?" and there was no answer at all — the type was `Target`, the file `profiles.rs`, the
README said *profiles*, and the disk `targets.json`.

The starting proposal was to rename `PLAN.md` to `CONTEXT.md` and be done. That fails on a
number: the glossary's whole value is being read at the START of a session, and 11,536 lines of
prose is on the order of an entire useful context window. A rename would also break 14 by-name
references across four files, three of them pure CRLF. So the two documents stay two documents,
with different jobs.

### What each document owns

| file | owns |
|---|---|
| `CONTEXT.md` | the WORD. 35 terms, 141 lines, 4.8 KB — small on purpose, because it is read every session |
| `PLAN.md` | the DECISION. Its `§NN` sections **are** this project's ADRs; there is no `docs/adr/` and there will not be one |
| `TERMINAL_COMPATIBILITY_PLAN.md` | the terminal's coverage and the evidence for each claim |
| `README.md` | the user |
| `AGENTS.md` | how to work here — with `CLAUDE.md` a one-line pointer at it |

A `CONTEXT.md` entry may cite a `§` for depth. It may never cite one for a definition: the
definition is the entry. That rule is what stops the two files from drifting, because there is
only ever one place to change.

### The words that meant more than one thing

Every clash in the crate turned out to run along the same fault line — the terminal's vocabulary
against the window's:

| was | is | why |
|---|---|---|
| `ui::selection::Cell` (a screen coordinate) + `Spot` (a document coordinate) | `ScreenSpot` + `DocSpot` | two coordinate spaces, and only one of them was called a spot. `term::screen::Cell` keeps *cell*: content is what a cell IS |
| `term::rect::Area` | `Rect` | the module was already called `rect`; *area* belongs to the window (§48), where a user can point at one |
| `term::mod::Split` | `Interruption` | its own doc says it is "one thing `process` has to do part-way through a chunk". Nothing to do with splitting a window |
| `ui::terminal::Panes<'a>` | `PanesView` | **pane** is the noun; *pane* survived only in prose, and both docs swapped the two words mid-sentence |
| `files::Zone` | `TimeZone` | it is a time zone. A reader assumes a screen region |
| `ui::terminal::Session<'a>` | `UiTerminalSession` | it is an argument pair, not a session |
| *profile* | **target** | 454 uses to 24, and the file on disk already said `targets.json`. `profiles.rs` becomes `targets.rs`; README and UI follow |
| *files strip* / *browser strip* / *tab strip* | **browser strip**, **tab strip** | three names for two things, used interchangeably inside one file (`ui/terminal.rs:364` and `:369`) |

Two words looked like synonyms and are not. `asuser.rs` says its accounts are "one entry per
identity in §45's sense", but an **account** is a login on the remote and an **identity** is one
shell running as it — `Accounts` caches SFTP capability per account, `Identity` is per shell and
carries `LOGIN_IDENTITY`. Both are real, so both are defined, and the doc line that equated them
is the thing that was wrong.

### The rule, so the next name does not need a debate

**One name means one thing crate-wide** — types, `pub` free functions, module-private free
functions, tests included. Not methods, not enum variants, not trait-impl methods: iced requires
`update` and `view`, and those names are not ours.

When that rule fires, which of two names moves:

> **A wrong name is corrected. A merely-duplicated name is prefixed with its module.**

`Interruption` is what `term::mod::Split` IS, so it earns a new word. `Cell` in `gatediff` is not
wrong, only duplicated, so it becomes `GatediffCell` and nobody has to have an opinion.

The inventory is **43 duplicate definitions across 24 names** (267 type definitions, 224
distinct). The two biggest clusters are both inside `term/`: **11 private `enum Scan`** and **6
`pub Request`**, one of each per scanner. That is the duplicated grammar the framer was always
meant to absorb — and they are being renamed now rather than after it, which means the framer may
later delete a name this cleanup just chose. That cost was weighed and accepted: a rename that
lands today is worth more than a name held hostage to a refactor that has been pending since
§106.

### The rules that lived nowhere

`AGENTS.md` is the more useful half of this section. Until now the green gate, the CRLF files,
`ponytail:`, "commit but never push", the build rules and — most importantly — **the security
refusals** existed only in a session prompt. A fresh agent had none of them, which means the
first thing it would do with `Osc52::Disabled` or `ALLOWED_SCHEMES` is help by completing them.
Those are now written down as decisions, with the reason attached to each, because a refusal with
no reason beside it reads exactly like an oversight.

### The gate, in the repo instead of in a habit

`Cargo.toml` now carries what CI has always run:

```toml
[lints.rust]
warnings = "deny"

[lints.clippy]
all = "deny"
```

Which was proved rather than assumed: an `unused_thing` variable added to `main.rs` turns plain
`cargo check` from a warning into `error: unused variable`, exit 101. A warning can no longer wait
until commit time to matter.

`clippy::pedantic` is NOT in that table yet, and the reason is the schedule rather than the
principle. It reports **528 warnings** across 35 lints today:

| lint family | count |
|---|---|
| the five `cast_*` lints — u16 ↔ usize ↔ f32 grid arithmetic | 231 |
| `doc_markdown` | 59 |
| `unreadable_literal` | 52 |
| `float_cmp` | 46 |
| 31 others (`unused_async`, `too_many_lines`, `match_same_arms`, …) | 140 |

Turning it on before fixing them would leave every commit in between failing its own gate, which
is how a team learns to ignore a gate. So it goes on in the same commit that clears the last
warning. All 528 get **fixed**, not allowed: an `allow` hides a question instead of answering it,
and the 231 casts are a real question about boundaries. A `deny` is the opposite and stays
welcome — `#[deny(clippy::missing_trait_methods)]` in `gate.rs` is what makes a missing `Handler`
method a build error, and §107 rests on it.

### The line endings nobody had declared

Four documents were "pure CRLF", and the rule that came with them — edit them through Python,
because any ordinary tool silently converts them — was the most frequently tripped hazard in the
project. Looking at where that invariant actually lived turned up something worse than an
inconvenience.

It lived in `core.autocrlf=true`, **a per-machine git setting the repo never mentioned.** The index
told the real story: `PLAN.md` was the only file stored with CRLF. `CONTEXT.md`, `AGENTS.md`, every
one of the 95 `.rs` files, `Cargo.toml` and `bundle-macos.sh` were all stored LF, and the CRLF in
the working copy was git converting on checkout. Twelve `.rs` files showed CRLF locally and 83
showed LF, for no reason except which ones a tool had last rewritten.

So the invariant was never a property of the repository — it was a property of one machine. A clone
elsewhere would look different, and nothing would say so.

`.gitattributes` now says it, once, for everything:

```
* text=auto eol=lf
*.ttf binary
*.png binary
```

LF in the index and in the working copy. `rustfmt.toml` gains `newline_style = "Unix"`, because
rustfmt's default is `Auto` — "whatever the file already has", and the platform's native ending for
a file it creates, so on Windows a new module would arrive with CRLF and nothing would object.

The whole CRLF rule is therefore **deleted** rather than documented: the four files were CRLF by
accident of history, nothing ever needed them to be, and `PLAN.md` can now be edited by ordinary
tools. One commit shows all 11,680 of its lines changed, which is the honest cost of storing a file
one way and reading it another for a hundred sections.

The conversion also turned up a defect it would never have found otherwise: line 11247 of `PLAN.md`
was a blank line containing `\r\r`, left by one of §106's own Python round-trips — a lone CR in the
text, doubled by a normalise-and-restore pass. It had been in the file, invisible, since then. A
rule enforced by hand collects that sort of thing; a rule enforced by `.gitattributes` cannot.

### What follows, and where it will be recorded

- **§109** — the renames, one commit per term, the gate green each time, the `CONTEXT.md` entry
  landing in the same commit as the code it governs.
- **§110** — the two JSON stores get a version. `targets.json` grows an envelope
  (`{"version": "2", "targets": [ … ]}`, the version a STRING), the loader sniffs `[` for the
  unversioned shape and calls it `"1"`, a file newer than the binary is refused for BOTH load and
  save rather than silently overwritten, `<file>.bak` is written once before the first migrating
  save, and the temp-then-rename write that `vault.rs` already does is factored out and used by
  all three writers.
- **§111** — the pedantic pass, ending with the line that turns it on.

The documentation sweep runs after the renames, in one pass to the final names, and it is
total — prose, code blocks, README, `docs/`, and the four hand-built SVGs, whose `<text>` labels
carry the vocabulary at hand-placed coordinates and will need looking at. **One carve-out**:
verbatim quotes of somebody else's words — alacritty's source, xterm's `ctlseqs`, DEC's manual —
keep their spelling. A renamed quote is no longer evidence for the claim it supports, and the
compat plan's Evidence section is how §106 and §107 justify themselves.

### Not done

- **The glossary is 35 terms, chosen by reading `term/` and the type list.** A word that only
  ever appears in `ssh/` or `ui/` prose may be missing, and the way that surfaces is somebody
  looking a word up and not finding it.
- ~~**The `§` numbering is still two styles**~~ — `## 44. Title` for the older sections, `## §102 —
  Title` for the newer. **Done**, and every one of PLAN's 110 headings is `## §NN — Title` now. §111
  went looking for the leftovers and found something worse next door: the compatibility plan numbered
  its own nine chapters `## 6. Title` and cited them as `§6`, in the same range as PLAN's own §1–§8 —
  so a reader following `§6` from that document landed on the connection-and-authentication flow
  rather than on the refusals it meant, and one sentence there used `§1` for itself and `§25` for
  PLAN. Its chapters are **parts** now, 154 citations rewritten, with the convention stated where the
  document opens. PLAN's side already spelled the file name out when it crossed over.
- **`AGENTS.md` is prose, so nothing enforces it.** The gate and the lint table are enforced; "one
  file header per module" and "no `allow`" are not, and a reader who skips the file is not stopped
  by anything.

## §109 — One name, one thing, and the four shapes of rename damage

§108 wrote the rule down: **one name means one thing crate-wide** — types, `pub` and private free
functions, tests included. This is the section that applied it, over 31 commits with the gate green
at each, and it is recorded here mostly for what went WRONG, because the renames themselves were
easy and the damage they did was not.

### Two rules, because types and functions disambiguate differently

A duplicated TYPE takes its module's prefix: `sixel::Command` becomes `SixelCommand`. A duplicated
FUNCTION takes a better object instead, because a prefix stutters — `link::open_link` reads worse
than `link::open_uri`, which also says the thing that matters (it gates the **scheme** before handing
a URI to the OS).

In both cases only the **less-canonical side moves**. Where a module owns a word, it keeps it:
`term::progress::Progress` is the OSC 9;4 progress a remote reports, `settings::Settings` is the
app's settings, `term::screen::Screen` is the grid — so `transfer::Progress`, `syntax::Settings` and
`app::Screen` are the ones that took a prefix. `files::Entry` and `files::Files` keep their names
because a listing entry and a pane of them are what those words mean here; `asuser`'s pair became
`AsuserEntry` / `AsuserFiles`.

The bulk was mechanical and batched — nine generic names in `term/` and `local/`, the four that meant
a match or a row or a status (113 sites), the files cluster (110 sites), `Kind` / `Progress` /
`Settings` / `Screen` (271 sites, of which `Kind` alone was 72 in `files.rs`). Eleven private `enum
Scan`s became `CancelScan`, `DsrScan` and nine more; six `Request`s became `CancelRequest`,
`RectRequest` and four more. Same ROLE in every case, six unrelated sets of variants.

### The renames that were a judgment, not a prefix

- **`term::rect::Area` → `Rect`.** Two unrelated things wore one word: a resolved rectangle of cells,
  and a place on screen a tab can be sent to. `CONTEXT.md` gives *area* to the window, where a user
  can point at one, so the terminal side took the name its own module already had.
- **`term::mod::Split` → `Interruption`.** It had nothing to do with splitting a window. Its own doc
  says what it is: one thing `process` has to do part-way through a chunk — a prompt mark, a picture,
  a rectangular operation — each carrying the byte offset it sits at. *Split* belongs to §48's window
  cut.
- **`files::Zone` → `TimeZone`.** The zone an mtime is rendered in, which is the server's own because
  the files being listed are its. `CONTEXT.md` already listed *zone* under **Area**'s `_Avoid_` for
  exactly the reason a reader would assume otherwise.
- **`ui::selection::Cell` → `ScreenSpot`, `Spot` → `DocSpot`.** `Cell` meant three things, two of
  them opposites: content in `term::screen`, a viewport position here, a document position beside it.
  `ui/grid.rs` is the proof it had to change — it imported both, so bare `Cell` there meant the
  coordinate while the content had to be aliased to `ScreenCell`. **The file that draws cells could
  not call a cell a cell.**
- **`Shell` was the most crowded name in the crate** — five things, three of them ours.
  `integration::Shell` → `IntegrationShell`, `shells::Shell` → `LocalShell`, `ssh::shell::Shell` →
  `SshShell`, with `iced::advanced::Shell` (imported bare in `ui/grid.rs`) and a `windows_sys` module
  making up the five.
- **`differential::Engine` → `EngineTrace`**, because it records what the engine's parser did and is
  not an engine — a prefix would have preserved a wrong word. `term::region::Region` → `ScrollRegion`
  and `app::Claim` → `KeyboardClaim` on the same reasoning.

### Two that were deletions rather than renames

Applying the rule turned up code written twice, which is how one rule comes to be applied two ways.

`native` was byte-for-byte identical in `local/fs.rs` and `local/copy.rs`. It now lives once in
`local/path.rs`, beside the translation it wraps — and that matters more than tidiness, because
`local::path::to_native` is the local file layer's one-directional security boundary (§103). Two
copies of the check that guards it is two places for the check and its message to drift apart. Nine
call sites now go through one function.

`ui::terminal::human_bytes` divided by 1024 and said `KiB`; `ssh::edit::human_size` did the identical
arithmetic and said `KB`. So the files pane offered "1.5 KiB" while the message refusing to open that
file said "1.5 KB". The duplicate was structural rather than careless — the correct one lived in
`ui::terminal`, and `ssh::edit` will not depend on the UI layer to format a string — so it moved to a
module neither layer owns, `src/human.rs`, "how a byte count is spelled for a person".

### The two vocabulary sweeps

**`profile` → `target`** (§14's remembered way to reach a machine): the file (`git mv`, so history
follows), the module, 29 references, and the prose — 26 in `src`, 15 in `PLAN.md`, 5 in `README.md`,
including a string a user reads in the delete dialog.

**`panel` → `pane`** turned up something Q28 had not anticipated: **panel has three legitimate
meanings here and only one of them is wrong.** One of the two browser panes (wrong); a floating card
of items (`ui::menu::panel`, the eight dialog builders, `PANEL_BG` as their shared chrome); and, in
`cursor.rs`, a laptop display panel. A blanket sweep would have broken correct English in two senses
to fix one, so the files where every use is sense 1 went as a batch, and the three view files that
used the word BOTH ways — sometimes in one function, `pub fn panel` building the pane while `let
panel = menu::panel(..)` two hundred lines below was a menu — were read line by line: 26 lines swept
and 6 kept in `ui/explorer.rs`, 13 swept and 14 kept in `ui/files.rs`.

`CONTEXT.md` gained a **Panel** entry as a result, because the sweep PROVED it is a real concept and
not merely the wrong word. **Pane**'s `_Avoid_` no longer says "panel" flatly; it points at **Panel**
as a different thing.

### The four shapes of rename damage

This is the part worth carrying forward, and every one of them was found the hard way.

**1. Identifiers.** The compiler checks these. They are not the problem.

**2. Prose, which nothing checks.** A batch of thirteen function renames used a bare word-boundary
pattern where a later batch correctly required a `(`. A function name only ever appears before a
paren; the same WORD in a sentence does not. So 88 doc comments were rewritten into the identifier —
"the remote file `entry_grid` under the terminal", "A `entry_grid` tall enough for five whole rows".
Two widget ids went with them.

Then the same shape again, in a way my own check was built to miss. The eleven scanners each document
their entry point as "**Scan** a chunk of shell output", and the `Scan` → `<Module>Scan` rename ran
through those sentences: "DsrScan a chunk", "TabsScan a whole chunk". My check afterwards looked at
the bare `Scan` occurrences that SURVIVED — in files with no `Scan` type — saw the verb intact, and
concluded the verb was safe. **It was safe in the files the rename never touched.** The eleven it did
touch were the ones to look at. The later audit missed them too, because the list I checked for prose
damage was the duplicate-TYPE list, and by then `Scan` and `Request` were no longer duplicates.

**3. String literals, which users read.** Worse than the prose damage, and invisible to everything:
the type renames ran through quoted spans.

```
ui/explorer.rs    "Rename…"  ->  "ExplorerRename…"    context menu item
ui/files.rs       "Rename…"  ->  "FilesRename…"       context menu item
local/session.rs  "Shell integration writes…"  ->  "LocalShell integration writes…"
ssh/asuser.rs     "…Files cannot be read…"     ->  "…AsuserFiles cannot be read…"
```

The app had been showing `ExplorerRename…` in its right-click menu for several commits. Nothing
failed — 1390 tests, clippy clean — because **a literal is not an identifier, so the compiler had
nothing to say either.**

**4. Docs in other files.** `Split` → `Interruption` left twenty-three places calling the mechanism
"the split advance", "the split loop", "the split point" — nine in `PLAN.md` and the compat plan,
fourteen in `src/term/` doc comments. Found only because `TERMINAL_COMPATIBILITY_PLAN.md` happened to
be opened. The four hand-built SVGs under `docs/img/` were the last of this shape: their `<text>`
labels carry the vocabulary at hand-placed coordinates where nothing checks them, and one had gone
stale for a different reason — §45 gave `SshEvent::Output` an `identity` field, so the arm the
diagram named no longer existed.

**The checks that find shapes 2, 3 and 4.** After a rename, grep the **new** name inside comments and
inside quoted spans — the new name, not the old, because what you are looking for is where the
substitution went somewhere it should not have. And the systematic version, which is what finally
caught the stale doc names: **extract every backticked identifier from the documents and test each
one against `src/`.** A name in backticks is a claim that `grep` will find it, so a name that does
not resolve is a defect whether or not it was ever renamed.

### The carve-out

**Verbatim quotes of somebody else's words keep their spelling** — alacritty's source, xterm's
`ctlseqs`, DEC's manuals. A renamed quote is no longer evidence for the claim it supports, and the
compat plan's Evidence section is how §106 and §107 justify themselves. The same line applies to
ordinary English: a rename follows the **type**, not the word, so a window split, a table's split and
the split between two parameters were all left alone.

## §110 — A file that says what it is

Two of the four on-disk files were written by a program that assumed it was the only program that
would ever read them. `targets.json` was a bare JSON array; `settings.json` was a bare object. Neither
said which format it was, and both were written with a plain `std::fs::write`.

That is two separate defects wearing one section.

### The write that could lose everything

`std::fs::write` truncates the file and then fills it. A crash, a full disk or a killed process
between those two steps leaves a truncated file — and a truncated JSON file does not parse.

Which matters here more than it would elsewhere, because of a rule §14 got right: **a corrupt store
must never stop the user from connecting.** `Targets::load_from` therefore treats an unparseable file
as EMPTY. Put the two together and the failure is silent and total: crash mid-save, and the next run
reads no targets, shows an empty home screen, and the save after that writes that emptiness back over
what was left. No error anywhere in the chain.

The vault never had this problem. It has always sealed its blob into a temp file beside the real one
and renamed it over the top, because for `secrets.age` a truncated write means losing every stored
secret at once, and that was obvious enough to handle when it was written (§16).

So the vault's own pattern is now `src/store.rs`, and every writer uses it. Two details are worth
stating because both are easy to get wrong:

- **The temp file goes in the target's own directory.** A rename is only atomic within one
  filesystem, so a temp in `%TEMP%` could be on another volume, where `rename` degrades to
  copy-then-delete and the window comes straight back.
- **`<name>.tmp` appends to the whole file name.** `with_extension` REPLACES it, so `secrets.age`
  would become `secrets.tmp` and `targets.json` would become `targets.tmp` — two stores, one temp
  path. There is a test whose only job is to pin that.

### The fourth writer, found afterwards

The first pass counted three writers. There are four: `hostkey::remove_line` reads `known_hosts`,
drops one entry and writes the rest back — the same read-all-then-`std::fs::write` shape, in the file
that holds the whole MITM defence (§8). It was missed because it lives in `ssh/`, away from the other
stores, and because it is not a serialiser: nothing about it looks like saving settings.

It is also the worst of the four, for a reason the other three do not share. A truncated
`targets.json` at least reads as empty and shows an empty list. A truncated `known_hosts` reads
*fine* — it is a line-per-entry text file, and a short one is a valid short one. The hosts whose lines
went missing simply verify as `Unknown`, which is the **first-contact prompt** rather than the refusal
their pinned key had earned. And the host most likely to lose its line is the one the rewrite was in
the middle of replacing: precisely the host that just presented a changed key. A crash there converts
"this key changed, refuse" into "new host, trust it?" — asked of a user who is already reaching for
yes.

That is the general lesson worth keeping: **a store that fails loudly when truncated is safer than one
that stays readable.** The three JSON files announce their own damage. `known_hosts` cannot, so it
depends entirely on never being damaged.

The editor's own save (`local::fs::save_atomically`) keeps a SEPARATE implementation on purpose, and a
separate name (§109). It writes a file the user is looking at, in a directory the user browses, so it
hides its temp behind a dotted name, deletes it if the rename fails, conjures up no parent directory
beside their file, and fails with a sentence for a dialog rather than an `anyhow` chain. Same
technique, different obligations.

### The version that was not there

An older cmote reading a newer file deserializes what it recognises, ignores the rest, and then
writes its own shape back. Whatever the newer build stored is gone. For a **portable** app this is the
expected case rather than a hypothesis: the whole point of `cmote-data/` beside the binary (§11) is
that the store travels on a stick between machines, which is exactly how it meets a different version
of itself.

`targets.json` grows an envelope; `settings.json` takes the field inline, since it is already an
object:

```json
{ "version": "2", "targets": [ … ] }
{ "version": "2", "window": [1000.0, 700.0] }
```

The version is a **string**, because both files are meant to be read and edited by hand (§22) and
`"2"` reads as one of the other quoted values. **Version 1 is the unversioned shape** — the bare array,
the bare object — so "version N" and "the Nth format" stay the same number for good.

Which shape a `targets.json` is in is decided by its first non-whitespace byte: `[` is the array, `{`
is the envelope. That is the point of the change, stated as plainly as it can be: **a format that says
what it is can be recognised without guessing, forever.**

### Three answers, and the difference between two of them

| what is on disk | what happens |
|---|---|
| version 1 (unversioned) | loaded as before, and marked. The first save writes the envelope and preserves the original as `<file>.bak` |
| version 2 | loaded |
| anything else | **refused, both ways.** Nothing is read — not even the fields this build understands — and the save side returns an error rather than writing, so the file is byte-identical afterwards |
| neither shape | read as empty, keeping §14's rule |

The last two rows look alike and must not be treated alike. A **refusal** means the user's data is
intact on disk and this build is too old to read it, so it must never be overwritten. A **corruption**
means there is nothing to protect, so it must stay overwritable — otherwise one bad byte would lock
the user out of saving anything ever again.

The backup is written **once**. A backup refreshed on every save would be overwritten by the migrated
file on the second run, leaving no copy of the original — which is the only thing this backup is for.

### Telling the user, and where

A refused `targets.json` is **shown**, in danger styling, where "No saved targets yet" would otherwise
be. That is the one place someone looking for their targets is already looking, and the alternative is
worse than silence: an empty home screen invites the user to make a new connection, which writes a new
file over the one that still holds everything.

Q23's answer said "snackbar", and it turned out not to be available — snackbars are per-tab and
`Targets::load()` runs before any tab exists. The home screen's own empty state is the equivalent
surface, and it already distinguished "nothing saved" from "nothing matching", so it was a third case
rather than a new mechanism.

`settings.json` gets no surface, deliberately. It holds a window size and the editor themes: a refusal
costs a remembered window, while overwriting would cost whatever a newer build keeps there. The
protection worth having is the one on the write.

### What it cost

Three commits, one new module of 93 lines, and 12 tests. `cargo test`: 1381 → 1390.

Two of those tests are older ones that changed, and they were right to fail:
`a_missing_file_reads_as_defaults` and `a_first_run_serializes_to_an_empty_object` both encoded the
pre-§110 contract. `{}` was the tidiest possible first-run file, and that is precisely the problem this
section is about.

The `[lints]` table from §108 shaped two commits rather than merely passing them. `back_up_once` was
written in the first slice and removed again, because it had no caller until the second and
`warnings = "deny"` makes that a build error rather than a warning. And `Targets::refusal()` could not
be landed without its reader, which is why the user-facing half of the refusal is in the same commit
as the refusal itself instead of being left for later.

### Not done

- **`known_hosts` and `secrets.age` are unversioned, on purpose.** `known_hosts` is OpenSSH's format
  and not ours to version. The vault's would have to sit outside the seal to be readable, which
  weakens it — if it is ever needed it goes INSIDE the sealed payload, and not before there is a
  second format to tell apart.
- **Nothing reads the `.bak`.** It is a copy for a human with a text editor, not a recovery path. An
  automatic fallback would have to decide when the live file is "bad enough", and §14's rule already
  says a bad file reads as empty rather than as an error.
- **A refusal is not offered a way out.** The message says to use a newer cmote or move the file
  aside; there is no "open it anyway" or "start fresh here" button, because both are one click from
  destroying data this build cannot see.
- **The migration is one step wide.** There is one older format and one current one, so "migrate"
  means "read the array". A third format would want the chain Q17 asked about, and this is not it yet.

## §111 — Pedantic, without a single `allow`

`clippy::pedantic` was 561 warnings when this section opened. Not one of them gets an `allow`.

> Read with §113. Those 561 were the warnings for ONE target — the host — and turning `pedantic` on
> raised five more on `x86_64-apple-darwin` that nothing local could see. They are `#[expect]`, not
> `allow`, so the rule below still holds; the scope of "not one of them" does not.

That was Q42's decision and it is worth restating as a rule, because it is the whole shape of the
section: **an `allow` is a lint switched off for an item; a fix is a lint that keeps working.** The two
look equally green in CI and they are not the same. A file with `#[allow(clippy::cast_possible_truncation)]`
at the top is a file where the NEXT cast — the one nobody thought about — is also silent. `deny` is
fine, because denying is enforcement rather than suppression, and `clippy.toml` is fine, because
configuring what a lint should consider correct is what its authors built it for.

### What the warnings actually were

They arrive as one number and they are not one problem. Sorted by what fixing them costs:

| family | count | what it wants |
|---|---|---|
| the five cast lints | ~224 | `as` replaced by a conversion that cannot silently lie |
| `doc_markdown` | 59 | identifiers in doc comments wrapped in backticks |
| `float_cmp` | 41 | `==` on `f32`/`f64` |
| `unused_async` | 14 | `async fn` with nothing to await |
| `too_many_lines` | 11 | functions over 100 lines |
| `match_same_arms` | 10 | arms with identical bodies |
| `trivially_copy_pass_by_ref` | 9 | `&u16` where `u16` is smaller than the pointer |
| `needless_pass_by_value` | 8 | a value taken and only read |
| `items_after_statements` | 8 | a `fn` or `const` declared mid-body |
| `struct_excessive_bools` | 7 | three or more `bool` fields |
| sixteen singles | 16 | one site each |

Those are WARNINGS, not sites, and the difference matters for the first row: one `as` can raise
truncation, sign loss and precision loss at once, so the 224 cast warnings are about a hundred
casts. The table also leaves out the 59 a machine could apply unaided and the 52
`unreadable_literal`, which are counted where they are fixed below.

### The sixteen singles, and why they are not all the same fix

Small counts, but three of them changed what the code SAYS rather than how it says it, and those are
the interesting ones.

**`option_option` — three sites, and one of them was a security property.** `Option<Option<T>>` is a
tri-state written in a way that reads wrong as easily as right: `None` and `Some(None)` sit one
keystroke apart and mean opposite things. Both places using it — `SessionState`'s remembered sort (§22)
and `iterm`'s user variable (§55) — had paid for that in prose, eight lines of doc comment each
explaining which nesting level was which. In `iterm` the distinction is not a nicety: `Keep` is "we
ignored this payload" and `Clear` is "the shell said the value is empty", and a remote that could
turn the first into the second could CLEAR a real reading by sending rubbish — the same rule §54
applies to progress.

So the three answers get names, in `src/change.rs`:

```rust
pub enum Change<T> { Keep, Clear, Set(T) }
```

`Keep` is the `Default`, so a snapshot built field-by-field says nothing until told to — the safe
direction, since the other default would erase stored values nobody asked to erase. `fold_into` applies
one to a stored `Option<T>` and returns whether anything actually changed, which is exactly what
`set_session` needs to decide whether to write the file at all. Two call sites, not one, which is what
makes it a type rather than speculative generality.

It also made `panes::restore` strictly better by accident. It used to require BOTH halves of the sort
to be present (`if let (Some(sort), Some(dir)) = …`), so a half-filled snapshot applied neither; now
each half folds onto what the pane already holds. A real capture always reports both, so the result is
the same by a route that cannot be wrong-footed.

Naming it `Change` needed `term::rect::Change` — "what one DECCARA or DECRARA does to a cell's
attributes" — to become `AttributeChange`, which is what it always was. §109's rule, applied to a name
that was merely vague until something more general wanted it.

**`struct_field_names` — two sites where a field was named after its own struct.** `Shells.shells`
became `Shells.by_identity`, which says what the `HashMap` is keyed by and reads better at every use
(`by_identity.get(&self.selected)`). `Highlighter.highlighter` — our type holding `syntect`'s — became
`Highlighter.colours`, named for its job rather than its type.

**`similar_names` — three pairs one letter apart.** `writer`/`writes` in the pty (the pty's own write
half versus the channel the GUI hands bytes to, now `to_pty`), `underlined`/`underline` in the SGR
stack (a bool about the MASK versus the value it selects, now `underline_masked`), and
`match_fg`/`match_bg` in the editor gutter (now `match_text`/`match_wash`). All three were typos
waiting to happen, and clippy is right that the fix is a better name rather than more care.

The rest were mechanical: `manual_let_else` ×2, `assigning_clones` ×3, `format_push_string` ×2
(`write!` straight into the `String` instead of `push_str(&format!(…))`), and one
`match_wildcard_for_single_variants` — that last one now names `iced::keyboard::Event::ModifiersChanged`
explicitly, so a keyboard event iced adds later is a compile error here instead of a silently dropped
keystroke.

### `doc_markdown`: the lint is right about code and wrong about names

Fifty-nine doc-comment words shaped like identifiers, splitting three ways.

**Genuinely identifiers** — backticked: `known_hosts` (eleven sites; it is OpenSSH's file name),
`WM_SETCURSOR`, `GatewayPorts`, `TERMINAL_COMPATIBILITY_PLAN`, two `FormStop` variants, and the
Material Icons names in `ui::files::glyph`. That last one got all nine backticked rather than the two
clippy noticed: it is one list of font names, and half-marked reads as if the others were English.

**Not identifiers at all** — `clippy.toml`'s `doc-valid-idents`: `ConEmu`, `PuTTY`, `PSReadLine`,
`ConPTY`, `ReGIS`, `BusyBox`, `TextMate`, `HiDPI`, `PowerShells`. Backticking them would claim they are
names this crate declares, which reads as a promise that `grep` will find them.

**Keycap labels** — same file, separate reason: `NumLock`, `PageUp`, `PageDown`, `AltGr`. The crate
writes keys bare everywhere — Ctrl+D, Ctrl+W, Ctrl+Shift+F — and only these four happen to be
CamelCase. Backticking exactly those would make a key's spelling depend on whether its name has a
capital in the middle.

The configuration was PROVED rather than assumed: a throwaway `/// A ProveItToken` doc comment was
added and clippy flagged it, so `doc-valid-idents` narrows the lint instead of switching it off. `".."`
comes first in the list to KEEP clippy's built-in names rather than replace them.

One word was neither: `UNguarded`, which was emphasis of mine and is now `**unguarded**`.

### `unreadable_literal`: the grouping the lint wants is not the grouping the number has

Fifty-two literals, and clippy's own suggestion would have made every one of them harder to read,
because its grouping is four digits and these are not four-digit things:

```
unix mode bits   0o100644  ->  0o100_644     type | permissions
hex colours      0x00ddff  ->  0x00_dd_ff    r | g | b
```

A mode is the file TYPE in the high digits and the permissions in the low three, so the separator
goes where the halves meet — `0o040_755` is "directory, 755", which is how it is read aloud, where
clippy's four would give `0o04_0755` and cut across both. A colour is three bytes, and grouping in
fours puts green across two groups.

Both groupings were checked against `inconsistent_digit_grouping` as well, because a non-uniform
grouping trades one warning for another; two comments record why the separators sit where they do,
so the next reader does not "fix" them back. Fifty-two warnings closed with the code MORE readable,
which is the answer to the worry behind Q36: strictness only costs something when the lint is right
and the code is wrong.

### The casts: one boundary at a time, because the right answer differs at each

Two hundred and twenty-four warnings, and the temptation is a single sweep of `try_from` with
`unwrap_or_default`. That would have been wrong at most sites, because the question a cast answers
is "what should happen if this does not fit", and the honest answer changes with the boundary. So
they were done as four commits, one per boundary.

**The engine's line numbers (32 sites), which cross in two directions.** cmote counts rows in
`usize` because that is what `screen_lines()`, `history_size()` and `display_offset()` return, while
the engine INDEXES rows with a signed `Line` where 0 is the top of the page and -1 is the newest
scrolled-off row. Every walk over the grid crosses that, and every crossing was a bare cast.

The two directions want opposite answers, so they get two helpers. INTO the engine,
`term::as_line_number` **panics**: a page is at most `u16::MAX` rows because the pty size is a `u16`
pair (§9) and the history is capped at `SCROLLBACK`, so the largest reachable value is about 75,000
against an `i32` holding two billion — a failure would mean the engine had reported a geometry it
cannot hold, and clamping that would only move the panic into the grid's own indexing with a worse
message. OUT of the engine, `screen::as_dimension` **clamps**: everything it feeds is a measurement
a person looks at, and a page of more than 65,535 rows can be neither drawn nor aimed at.

`history_line` earns its own name rather than being a subtraction at each site. It is the one row
index in the file that counts the opposite way from all the others — `history_line(0)` is `Line(-1)`
— and `-(1 + back as i32)` written out twenty times is one typo from reading the wrong row.

**The absolute document line (§40).** A position in the scrollback is numbered in `u64` from the
oldest retained line so that it survives the scrolling which makes every engine-relative row number
stale. Turning one into a viewport row means subtracting a number that can be LARGER, because a mark
above the top of the window is exactly what the projection has to detect — and that subtraction was
happening on `usize` operands cast to `i64` one at a time. `as_document_line` and `as_signed_line`
saturate, with the bound argued rather than assumed: `i64::MAX` lines is nearly three centuries of
output at one line per nanosecond, and a line pinned at that end sorts as "above everything", which
is what the arithmetic would have concluded anyway.

The way back down is the part worth stating. `osc133::project` and `search::visible` both turn a
signed row into a `u16` viewport row and both already had the range check that proves it fits. The
conversion now happens AFTER that check rather than beside it, so the check is what licenses it —
where before, a bound that stopped holding would have drawn a highlight on a row wrapped round into
the visible page instead of dropping it.

**The remaining 41 int-to-int narrowings**, where the right fallback differs at nearly every site,
which is the whole argument for doing this rather than sweeping it:

- **Refuse**, where the function already answers "no": a sixel picture bigger than the caps
  `canvas_size` enforces, a braille pattern outside its 256-point block, a search highlight past the
  row range checked one line above.
- **Clamp**, where the number is a measurement: an SFTP timestamp past 2106 pins at the newest
  instant the protocol's 32-bit field can carry, a cursor colour channel at 255.
- **Zero**, where a malformed value must not be half-read: russh reports a forwarded port as `u32`
  because that is the wire field's width, so a value above `u16` is a malformed message and "no
  port" is the honest reading — not the low half of a number the server never meant.
- **Expect**, where the surrounding code is the proof: a match arm's own range pattern (`0..=255`),
  a mask applied before the narrowing, a `size_of_val` of a 512-element stack buffer. Each message
  names the reason rather than saying "cannot fail".

And `cast_signed` / `cast_unsigned` where the reinterpretation IS the intent — the selection walks
that step signed and clamp back, the picture row that is negative when scrolled past the top, the
Win32 handle that doubles as a pointer-wide unsigned `WPARAM`. Same bits either way; the methods say
so where `as` only implied it.

### The float boundary, where the advice cannot be taken

The last 31 casts are all integers becoming pixels or pixels becoming integers, and none of them is
fixable the way the other 41 were. That was **checked rather than assumed**: std has no
`TryFrom<f32> for usize` and no `From<u32> for f32`, and clippy flags even a provably bounded
`px.clamp(0.0, 4096.0) as usize` for BOTH truncation and sign loss. The one exact spelling available
is `f32::from(u16)`, and a `u16` ceiling is wrong here — an editor buffer can hold more than 65,535
lines and clamping the gutter there would break a file this program can open.

A lint whose advice cannot be taken anywhere in a crate is the case an `allow` exists for, and that
is precisely why it is worth refusing. An `allow` would silence the boundary AND every future cast
that has nothing to do with it. So instead the boundary gets **a place**: seven functions in `ui`
(`pixels`, `signed_pixels`, `cells`, `cells_covering`, `cell_index`, `fraction`, `lines_scrolled`),
one in `sixel` for a colour channel, and one `#[expect]` on `human::bytes`. Each carries the lint it
answers and a reason. The lint stays enabled crate-wide, so a bare float cast written anywhere else
is still a build error.

The functions earn their keep beyond the conversion — the floor, the clamp and the negative case
were repeated at thirty-one call sites and are now written once each — and **a test proved it**:

```rust
cells(f32::INFINITY, pitch)  ->  usize::MAX
```

Infinity FLOORS to infinity rather than to NaN, so the first version of the guard tested `is_nan()`
and let it through, and `as usize` then saturated. An unmeasured `Length::Fill` would have handed a
caller four billion billion rows to walk. Both `cells` and `cells_covering` test `is_finite` now.

The remaining inexactness is stated where it bites rather than tolerated quietly: `f32` represents
integers exactly up to 2^24 (16,777,216), and above that a row index rounds and its pixel top is
wrong by less than the row is tall. The editor's own 8 MiB ceiling is about 8.4 million lines of one
byte, so the bound is unreachable rather than merely large.

### `float_cmp`: thirty-nine tests and two lines of production code, wanting opposite fixes

The production pair was `App`'s cursor-follow asking `if offset == editor.scroll()` to decide whether
a scroll was needed: `keep_visible` returned the caller's own `offset` back when the band was already
visible, so the caller re-derived by equality what the callee already knew. It returns `Option<f32>`
now — `None` for "do not move".

**That change was wrong on the first attempt and the test caught it**, which is the part worth
recording. `keep_visible` has a SECOND path to "no change": a band the window cannot contain — a cell
in a pane dragged shorter than one row — where the clamp *computes* an offset that turns out to be
the one already in force. So the old `==` was not merely comparing a value to itself, and dropping it
lost a case. One comparison lives inside `keep_visible` now, on `to_bits`, because the question is
not "are these numbers close" but "will the widget be handed a different `f32` than it holds" — a
scroll offset is stored and rendered verbatim, so a bit-level test is the honest one, and integers
say that where a bare `==` on floats invites doubt.

The thirty-nine test sites are all screen coordinates, and several compared a measurement against the
arithmetic that produced it (`1000.0 * MAX_PANE_FRACTION`). Those passed as `assert_eq!` because both
sides are the same operations in the same order — luck, not a property. Reorder the multiplication and
an equality that never had a reason to hold starts failing over a difference no screen can show. So
there is one `assert_px!` at the crate root with `PIXEL_TOLERANCE` at a tenth of a pixel: four orders
of magnitude above the arithmetic's noise and four below anything a user could see, declared before
the `mod` list so all seven test modules see it by textual scope.

Proved rather than assumed (§106): a site was temporarily made wrong by half a pixel and the macro
failed with `8 px is not 8.5 px (to within 0.1 px)`. A tolerance macro that quietly passed everything
would look exactly like a clean suite.

### The mechanical middle, where four of forty-three were saying something

`items_after_statements` (8), `match_same_arms` (10), `trivially_copy_pass_by_ref` (9),
`needless_pass_by_value` (8), `unused_async` (14). Mostly compiler-checked moves. The exceptions:

**Two serde predicates could not simply flip.** `targets::is_false(&bool)` and `forward::is_zero(&u16)`
take a reference because `skip_serializing_if` expands to `predicate(&self.field)`, so clippy's advice
would have broken serialization outright. They were also the same question asked of two types, which
§109 says to merge — so both became one generic `store::is_default<T: Default + PartialEq>(&T)`. The
reference is still serde's; a generic parameter has no size clippy can judge, so the lint does not
fire and nothing is suppressed. Two functions deleted, one added.

**One identical pair that reads as coincidence and is a decision.** `AcceptHostKey` and
`ReplaceHostKey` both send `HostKeyChoice::Pin`. That is the design (§8) — `Pin` means "write this key
to `known_hosts`", and whether it LEARNS a first-contact key or REPLACES a stale line is decided by the
verdict the SSH side already holds. The GUI does not get to choose which. The merged arm says so; the
two identical arms did not.

**Fourteen `async fn`s awaited nothing**, all with the same body: clone the sender, `tokio::spawn`,
return. The `async` was not load-bearing and it MISLED, because a reader seeing
`fs::make_dir(&events, path).await` reasonably assumes the folder exists by the next line. It does
not — the task is detached and the answer arrives later as an event.

That one was checked rather than swept, because §103 makes `local/session.rs` and `ssh/client.rs` two
dispatch tables deliberately written to read the same, and a lint that made one ragged would be worth
refusing. The raggedness turned out to be real information: the SSH side awaits because most of its
operations have a `Browse::Denied` arm — a refusal reported inline, before anything is spawned — and
the local side has no permission layer to refuse it. That is now a stated rule: **an arm that awaits
is an arm that can answer before the loop comes round again.**

Three functions were nearly caught by the sweep and put back — `fs::list`, `fs::list_all` and
`fs::report_zone` do await, and `cargo check` said so immediately. Recorded because the mechanical
version of that commit was wrong and only the build caught it.

### `struct_excessive_bools`: right once in seven

The lint's premise is that three or more `bool` fields are usually one state written as several, and
telling which was the work. It held for exactly one.

`editor::Editor` carried `saving` and `close_after_save`, a pair that could express a state with no
meaning — "close when the save lands" while no save is in flight — and that had to be cleared by hand
on the failure path purely so a failed "Save & close" would not close the tab later. That is a state
machine, so it is one now:

```rust
enum SaveFlight { Idle, Saving, SavingToClose }
```

The intent rides OUT of `mark_saved`, which returns whether the tab should close, because the
two-call version had an order the caller could get wrong. `save_failed` drops the pending close along
with the flight, and `take_close_after_save` is gone.

The other six are attribute sets, where the advice would make the code worse. SGR attributes are
independent **by definition** — bold italic strikeout text is one cell wearing three of them — and so
are kitty's protocol bits and the file pane's view toggles. A bitset would replace named fields with
positions, which is the opposite of what this codebase is for. Each takes an `#[expect]` whose reason
says which of the two cases it is, so a future reader can disagree with the classification rather
than with the silence.

### `too_many_lines`: ten variant counts and one real duplication

Eleven functions over 100 lines. Ten of them are dispatch — `update`'s 109 arms, `on_key`,
`on_ssh_event`, the terminal view — where the length IS a variant count, and a cut through an
alphabet is not a refactor. Those take `#[expect]`.

The eleventh was genuine. `ssh::client::run` was 299 lines because twenty-odd arms were the same five
lines of forwarding boilerplate. It is a `SshCommand` → `SessionMsg` table now, one line per command,
with the single place that knows what "no session" means factored out:

```rust
async fn forward(session: Option<&SessionLink>, message: SessionMsg) {
	if let Some(link) = session {
		let _ = link.to_session.send(message).await;
	}
}
```

299 lines to 149, verified variant by variant that the same 32 commands produce the same messages. It
keeps an `#[expect]` even so, because a 32-variant table is still over the limit — but now for the
reason the other ten have.

### Turning it on

```toml
[lints.clippy]
all = "deny"
pedantic = "deny"
```

That line landed in the same commit that cleared the last of the 561, which is the only order that
works: turning it on first would have left every commit in between failing its own gate, and a gate
that is expected to fail is a gate nobody reads.

**Proved rather than assumed.** A throwaway `src/probe.rs` was added containing one `count as f32`
and one unbackticked doc token, and a plain `cargo clippy` — not the gate's `-D warnings`, the
ordinary build — rejected both. Then it was deleted. A lint configuration that silently did nothing
would look exactly like a clean crate.

Twenty-seven `#[expect]`s survive, and they are **three answers rather than twenty-seven**: eleven
`too_many_lines` on dispatch tables, seven `struct_excessive_bools` on independent composable
attributes, nine at the float boundary. The count is stated in `Cargo.toml` above the lint table
because twenty-seven suppressions is a number a reader might reasonably object to, and the objection
should be to the real number. (I first wrote "nine" there, having counted only the float ones, and
corrected it before the commit.)

### The last six `allow`s, which predated this section

Seven suppressions were in the crate before §111 opened; `cursor.rs`'s was already an `#[expect]` and
gained a reason, leaving six `#[allow]`s that AGENTS.md's rule applies to just as much as to the 561.
**Four turned out to be avoidable outright, and the other two were narrowed and re-spelled.** There is
now no `#[allow]` anywhere in `src/`.

**Three `too_many_arguments` were one data clump wearing three hats.** `local::copy::stream`,
`ssh::download::receive_file` and `ssh::upload::send_file` each took their own two ends and then the
SAME six-argument tail — `resume, size, events, ticker, total, cancel` — written out verbatim in
three files. Six arguments repeated identically three times is a type asking to exist, and the
argument limit was the lint noticing. They now take a `transfer::CopyRun`, which holds the five that
are constant for a whole run; the file's own `size` stays an argument because it is the one thing that
changes per call. This is the second half of the change [`Ticker`] already describes — the two `&mut
u64` counters became a ticker, and now the tail around it becomes a run — so the fix was one the file
had already argued for.

**`async_fn_in_trait` was avoidable, and taking it seriously improved the trait.** The lint's point is
that an `async fn` in a trait leaves the returned future's auto traits unnameable, so a caller cannot
require `Send` — which matters for `asuser::Exec`, whose futures are awaited inside spawned work. The
answer is to write what the sugar hides:

```rust
fn stdout(&self, snippet: &str) -> impl Future<Output = Result<String>> + Send;
```

That states the contract instead of suppressing the warning about its absence, and it is **checked**:
an implementation whose future is not `Send` no longer compiles. The test double proved it
immediately — `Script` recorded its commands in a `RefCell`, and `&RefCell` is not `Send`, so it had
to become a `Mutex`. That is not a cost of the change, it is the bound doing its job on the first
thing it touched. (The guard is taken in its own small `record` method so it cannot be held across an
`.await`, which is the other way a future loses `Send`.)

**`field_reassign_with_default` genuinely cannot be fixed, so it was narrowed.**
`syntect::Theme` is `#[non_exhaustive]`, which means no struct literal is available to another crate
— not even `..Default::default()` — so default-then-assign is the only shape the language leaves.
The `allow` sat on the whole of `cme_theme`, most of which is a sixty-line table of scope colours it
had nothing to say about. The four assignments moved into their own `assembled` function, so the
`#[expect]` now covers four lines instead of sixty.

**`dead_code` turned out not to accept an `#[expect]` at all, which is worth recording.**
`elevate::valid_user` is a security check with its own tests, kept rather than deleted because
whatever replaces the withdrawn elevate dialog will need exactly it before composing a command (§47).
The obvious conversion fails:

```
error: this lint expectation is unfulfilled
    = note: `-D unfulfilled-lint-expectations` implied by `-D warnings`
```

The tests DO call it, so under `cargo test` the item is used, `dead_code` never fires, and the
expectation goes unfulfilled — which `-D warnings` turns into a build error. Tried rather than
assumed, and the shape that works is `#[cfg_attr(not(test), expect(dead_code, reason = …))]`: the
escape is scoped to the configuration that actually needs it. Both `cargo clippy` and `cargo clippy
--all-targets` are clean, and being an `expect` it still becomes a build error the day a real caller
appears — which is exactly when the note should be deleted.

### The flake, which was not a flake but a coin toss

The suite had a test that failed sometimes and nobody had pinned down. It is
`app::tests::a_real_local_shell_answers_ctrl_d_by_leaving` — §104's one test against a REAL `pwsh` on
a real pty — and the way it was found is worth more than the fix.

Thirty runs of the whole suite on an idle machine were **all green**, which is what makes this kind of
test so easy to leave alone. Repeating the suite is the wrong instrument: the failure is not random,
it is *load-dependent*. Running that one test with eight busy cores beside it failed **17 times out of
17** — not a flake at all, but a coin toss whose odds change with the machine.

The test had to guess when the shell was at a prompt, and its guess was **2.5 seconds of silence**.
That is not a prompt signal. A shell running its profile goes quiet mid-startup, and on a loaded
machine those gaps stretch past the threshold — so the press landed before the shell was reading
input and the byte was simply lost. Every failure was `pressed` true and `typed` false: cmote pressed,
nothing came back.

**The first fix was wrong, and the load loop said so.** Requiring output before counting the silence
("quiet with nothing printed yet is a slow start") is a real improvement and it fixed nothing: 6 of 6
still failed, because a gap *between* two bursts of profile output looks exactly like a prompt too.
Worth recording because the hypothesis was confirmed by the symptom and still wrong about the cause.

What works is to stop betting on the guess: press on **every** settle rather than once. An unanswered
Ctrl+D is precisely the case §104 is built to survive, so re-pressing costs nothing, and the claim
under test was never "the first guess at where the prompt is happens to be right" — it is that a press
at a prompt is echoed and answered. 0 failures in 6 under the load that gave 17 out of 17.

Then the check that mattered, because a retry loop is exactly the shape that can make a test
unfalsifiable: `EOF_ECHO` was changed to `b"^X"` so the rule could never fire, and the test failed
(§106). It still tests what it says it does.

`local::pty`'s real-child test was stressed the same way and is fine — 5 of 5. It waits for the child
to EXIT rather than for a prompt, so it makes no timing guess at all, which is the distinction: a test
may wait for an event it will certainly get, and must not wait for one it is only hoping for.

### The CSI framer, all ten scanners in

`csi::Framer` is §106's top recommendation, built here. **Ten** modules under `term/` each scanned
CSI sequences beside the engine, and each carried its own copy of the grammar — 62 to 162 lines apiece
of "find where a CSI starts and ends in a stream that arrives in arbitrary chunks", with the
module-specific part of it a handful of lines. All ten are migrated. `feed` loops: `tabs` 62→9,
`dsr` 62→8, `scp` 74→19, `protect` 82→27, `sgrstack` 76→21, `modkeys` 62→24, `rect` 74→25,
`cancel` 80→9, `graphics` and `query` 162→11 for the CSI half, both keeping their DCS machine.

**The count was got wrong twice in this section's own prose, in both directions**, and it is worth the
paragraph because both errors were written from memory rather than measured. It read *nine* while the
migration was running, missing `query` — the one scanner that reads CSI *and* DCS from a single
machine, so it did not look like a member of the family until its CSI half came out. Then the
correction overshot to *eleven*, counting a module that is not a scanner at all: `differential` holds a
framer because it is the test harness that drives one. (The same off-by-one is in that harness's own
sweep, where the scanner table has eleven rows for ten scanners — `protect` is listed twice because its
verdict depends on whether the pen is armed.) The check that settles it is one line: `framer:
super::csi::Framer` appears once per scanner, and nothing else in `term/` holds one. Ten.

**The payout ran upward, not downward.** The worry with sharing a grammar between ten readers is that
the shared code settles on the laxest behaviour of its callers. The opposite happened, because each
migration was one commit and the strictest existing rule won every time:

- `scp` alone refused a parameter byte after an intermediate. `protect` alone refused that AND a
  private marker after the parameters, each with a note naming the defect that found it — the worst
  being `CSI ? 1;2 ? J` classified as a selective erase, because `first_param` read only the first
  field and the stray marker hid in the second. Both rules are the framer's now, so the other eight
  scanners obey what one or two of them used to.
- `modkeys` had no state for an intermediate byte and none for a byte the engine reads straight
  through. It was the last of the ten still carrying §106's divergence, and the reason the
  differential sweep's read-through list had five entries: a test asserting what a module does wrong
  is not a test.
- `first_param` lost its "unreadable" case entirely. With a marker refused mid-parameters, a
  parameter run is digits and separators only, so it returns `u16` rather than `Option<u16>`.

**Defects were found BY migrating**, which is the argument for one scanner per commit rather than one
sweep. Three came out of the eight straightforward ones, and the two that also read DCS produced four
more (below):

1. `Params` dropped leading zeros so thoroughly that an all-zero field rendered as nothing, making
   `CSI # 1 ; 0 {` identical to `CSI # 1 ; {`. Invisible to the four scanners migrated first (each
   reads an omitted parameter as 0) and breaking for `sgrstack`, which treats an empty field as
   malformed on purpose. `Params::finish` writes the zero back.
2. **Four** bounds named `MAX_PARAMS` counted parameter BYTES, not parameters — 64 in `sgrstack` and
   `rect`, 16 in `modkeys` and `query` — and all four ABANDONED the sequence over a long digit run,
   where the engine saturates the number and acts. That is §106's defect shape four more times, and
   not one of the four was the engine's number in the engine's unit.
3. `sgrstack` and `modkeys` both crossed onto the framer reading a `:` as another `;`. The
   hand-rolled walks they replaced had dropped those sequences by ACCIDENT — the colon made a field
   unreadable as a number — and taking the accident for the whole of the reason was a widening
   nobody asked for. Caught before `rect` followed it, where `CSI 2 : 3 ; 5 ; 7 $ z` would have
   erased a rectangle the program never named. `Csi::sub_parameters` reports the fact; each scanner
   applies its own policy, and the framer does not abandon these, because sub-parameters are legal in
   the engine's grammar and refusing one there would be cmote's policy inside the module whose only
   job is to agree.

**The migration order did work that no ordering document could have.** `cancel` was written down as
the hardest of the ten and turned out to need nothing invented for it: `Params::finish` (from
`sgrstack`) gave it `Some(0)` versus `None`, `Csi::sub_parameters` (from `rect`) gave it the colon
refusal, and the final byte's own offset is `offset - 1`. Its `feed` went 80 lines to 9, its struct
five fields to one, and **not one of its 32 tests changed** — the strongest evidence so far that the
framer reproduces the family's behaviour rather than approximating it. Had `cancel` gone first, all
three would have had to be invented at once.

`rect` needed the one structural departure. The other three RIS-reading scanners collect `ESC c` in a
second pass and merge by offset, because for them a reset is another request in the list. For `rect` a
reset CHANGES WHAT THE OTHER SEQUENCES READ — DECSACE's extent is stamped onto every attribute request
after it — so a second pass would read the whole chunk against whichever extent the chunk ended with.
`feed` cuts the chunk into runs at each RIS and feeds them in turn, rebasing each run's offsets.

### The two that scan CSI *and* DCS, and the blocker that was not one

`query` and `graphics` were the last two, and the awkward ones: each reads CSI and DCS from a single
machine, so only the CSI half could move. Both now do, and both keep their own DCS machine — the line
`osc.rs` drew when `graphics` kept its own OSC framing.

`query` was also the one scanner missing from the differential sweep, and those two facts were
related: it disagreed with the parser over shapes the sweep walks by the hundred, so it could not join
until the framer fixed that. Measured before, rather than assumed:

| fed to `Queries::feed` | answered, before | after |
|---|---|---|
| `` CSI > 0 q `` | `Version` | `Version` |
| `` CSI > `` + 16 zeros + `` q `` | `Version` | `Version` |
| `` CSI > `` + 17 zeros + `` q `` | nothing | `Version` |
| `` CSI > 0 `` LF `` q `` | nothing | `Version` |

Both empty rows were sequences `vte` frames and dispatches. The harm was small and one-directional —
cmote failed to ANSWER a query rather than acting on one alone, so a program asking XTVERSION with a
stray control byte in it waited out its timeout — but it was the same class, and its `MAX_PARAMS = 16`
was the fourth byte-counting bound.

**`query` needed one thing no other scanner did: the ORDER of its results.** Its answers become reply
bytes sent back to the remote, and a program that asks two questions matches the answers by position.
Two unordered passes put every CSI answer after every DCS one, so `DCS $ q m ST` before `CSI > q`
would have come back reversed. Each half collects with the offset it completed at and the two merge on
it; the offsets are then dropped, since no caller wants them. `graphics` needed the same merge for a
different reason — a picture is reported PAST its terminator and an erase BEFORE its first byte, so
`img2sixel; clear` gives both events the same offset, and a stable sort with the strings scanned first
is what keeps the draw before the erase.

**The design question that was blocking both does not exist.** The paragraph here used to say that
splitting `query`'s CSI half onto a DCS-unaware framer would let a payload containing `ESC [ > c`
frame as a real query, and that gating the callback on "not currently inside a DCS" was the question
to settle. That was wrong, and measuring it is what said so.

**ESC does two jobs at once.** In the ANSI state machine it ENDS whatever control string is open and
it OPENS the next sequence, and `vte` does both — a DCS interrupted by an ESC unhooks, and the
sequence that ESC introduced is dispatched normally. So there is no such thing as a payload the
engine reads as data and a framer reads as a sequence: the only way into a CSI is `ESC [`, and that
ESC has already ended the string for the engine too. `a_framer_cannot_be_fooled_by_a_control_string`
pins it over five shapes of control string, and the framer's claims equal the engine's dispatches in
every one.

Which stands the worry on its head. The framer's ESC handling is UNCONDITIONAL, and being
unconditional is exactly what the fused machine got wrong — so the framer is stricter than what it
replaces, not laxer.

Two live defects fell out of asking, and both are fixed:

- **`query` did only the first of ESC's two jobs**, in two of its states: a stray ESC dropped it back
  to ordinary text, so the sequence that ESC had opened was never seen. Fed five shapes of control
  string, the engine dispatched an XTVERSION in every one and cmote answered none. The quietest kind
  of divergence there is — no screen corruption, no acting alone, just a program waiting out its
  timeout — which is why it survived from §33, where the scanner was written, to §111.
- **`graphics` had the same gap, where it costs more**, because the sequence dropped can be RIS: a
  hard reset arriving mid-payload left every picture standing on a screen it had just wiped.

And one that was **duplicated-grammar drift in its plainest form**: single-byte ST (0x9c) ends a
control string, `graphics` has had a `const ST` for its sixel payloads all along, and `query` never
learned it — so a DECRQSS ended that way went unanswered while a picture ended that way drew fine.
One of a pair of scanners knowing a rule its twin does not is the whole argument for the framer,
restated in the half of the family the framer does not cover.

And one found by migrating `graphics`, which is **the same byte answered the opposite way**. `CSI 2:3 J`
erases the screen: the engine HAS an arm for ED, and `next_param_or(0)` reads the first sub-parameter
of the first parameter. `graphics`' hand-rolled loop abandoned the sequence on the colon, so the text
went and every picture stayed on a screen that had just been wiped. Which makes its policy on a
sub-parameter the reverse of `rect`'s, and both right: `rect` reads DECERA, which the engine frames and
drops, so cmote is the only actor and refusing an undefined spelling costs nothing — while a rectangle
built from a misread corner erases cells nobody named. That is exactly why `Csi::sub_parameters`
reports the FACT and leaves the policy to each scanner, and
`a_sub_parameter_is_read_or_refused_by_who_the_engine_leaves_it_to` pins both halves side by side.

Three measurements changed my mind rather than confirming it, which is the reason to measure at all:

1. `DCS $ q m ESC ESC \` — queued as a third fix. The engine keeps the payload and dispatches
   nothing, which is what the code already did. Left alone.
2. **BEL is not a DCS terminator for the engine.** It reads the byte into the payload and carries on.
   cmote accepts BEL because real emitters send it; the note on the constant now says that is
   leniency rather than agreement, and that nothing on the screen turns on it.
3. `graphics` clears its payload when a sixel starts, so an abandoned payload cannot leak into the
   next picture. Checked before "fixing" it.

What deliberately did NOT change: an abandoned control string still goes unanswered. The engine cannot
tell a clean terminator from an interrupted one — `unhook` fires either way — so it would answer.
cmote treats a string that named no terminator as malformed and replies nothing, which is §54's rule
and §60's: an invented answer is worse than a missing one.

With that settled, both migrations were mechanical, and the differential sweep now covers all ten
scanners over 6720 shapes — `c` and `S` joined the shape space with `query`, being DA3 and
XTSMGRAPHICS, since a sweep that never spelled them could not have caught it acting on one alone.

### The second door, and the rule that was wrong in three framers at once

The entry above this one used to say the framer frames CSI only, that `query` and `graphics` would keep
their own DCS machines, and that a `dcs::Framer` was the obvious next move. It is made now, and the
reason to record it here rather than start a new section is that it found the same defect a third time
in the module §111 had been holding up as the template.

**`dcs::Framer` is the other door: `ESC`, and everything through it that is not a CSI.** A DCS control
string, and the two-part escape sequences — of which RIS is the only one anything in cmote acts on. The
grammar was spelled SIX times: two full DCS machines (`query`, `graphics`) and four copies of

```rust
if self.after_escape && byte == b'c' { … }      // protect, scp, sgrstack, rect
self.after_escape = byte == ESC;
```

The introducer turned out to be the CSI grammar over again — marker, parameters, intermediates, one
final byte, and the same two refusals — so `csi::Params`, `csi::MAX_INTERMEDIATES`,
`csi::passes_through` and `csi::Span` are shared rather than restated. Reading `vte`'s state table
instead of remembering it produced four rules nothing in cmote obeyed:

1. **CAN and SUB end a control string** (`lib.rs:320-324`). Both DCS machines read those bytes into the
   payload and went on waiting, so a later ST completed a string the engine had thrown away hundreds of
   bytes back — `query` answering a cancelled question, `graphics` decoding a payload with a foreign
   sixel spliced into it.
2. **DEL and the high bytes are DISCARDED from a payload, not kept** (`:330`, `:335`). A scanner
   comparing a payload against a known selector has to see what the handler saw.
3. **A byte the engine reads through in the INTRODUCER keeps the string** — §106's rule in the third of
   the three states that has it.
4. **`ESC` then a C0, a DEL or a high byte stays in the escape state** (`:341`, `:381-383`), so
   `ESC` LF `c` is a hard reset. This is the one that mattered most, and it is where the section stopped
   being about DCS: **`csi::Framer` had it wrong too**, dropping to ordinary text on that line feed and
   then reading the `[` as a printable character — losing `CSI 2 J` for all ten scanners at once, on a
   sequence the engine really performs. And all four hand-rolled RIS watchers had it wrong, each keeping
   state a reset had thrown away: an armed pen, a store, a stack, a DECSACE extent stamped onto every
   request after the reset. Three bytes from a remote and cmote's idea of the terminal parts company
   with the engine's.

**Then the same rule again, in `osc.rs`.** Going to fix the DCS door meant reading the OSC one beside
it, and it had the same two ESC defects plus two of its own: a byte read through between the `ESC` and
the `]` (so `ESC` LF `] 7 ; … BEL` was an OSC cmote never saw), an ESC that ends one string while
opening the next (a second OSC starting inside the first was lost entirely, so the cwd never arrived),
CAN and SUB read into the payload rather than ending the string (`ESC ] 7 ; /a CAN /b BEL` became the
path `/a/b`, a directory nobody named), and the C0s the engine drops kept in the payload. Four
scanners sit on that framer.

Two design findings, both the opposite of what the entry above had predicted:

- **An abandoned string needs no state at all.** Both DCS machines had a "follow this one silently to
  its terminator" mode, and `query`'s doc explained it as stopping a payload masquerading as a query.
  It cannot masquerade: an ESC is the only byte that can interrupt a control string, and an ESC ends it
  for the engine too. Hunting for the next ESC is everything that mode did. The same argument retires
  `osc.rs`'s stated reason for keeping `graphics` out — "a different policy, not a different number" —
  because following an overlong payload to its end and abandoning it on the spot are the same machine.
  What was genuinely different was the cap, and a cap is a const parameter.
- **Framing the escape sequences is not scope creep, it is the only way to be right.** A watcher that
  tests "was the previous byte an ESC" cannot tell `ESC` LF `c` (a reset) from `ESC ( c` (a charset
  designation) in either direction. `Control::Escape` reports the intermediates and the final byte, and
  every caller tests both.

`query` came out of it with no state machine at all — two framers and two tables — and `graphics` lost
seven states, its parameter buffer, its payload, its overflow flag and its sequence-start field. All
their tests passed unchanged. `rect` is the one that is not a merge: RIS resets DECSACE and that extent
is stamped onto the requests that FOLLOW, so it collects the reset offsets first and cuts the chunk at
each of them.

Proved rather than asserted, which is the part that took the longest: the harness grew `hook`, `put`,
`unhook`, `esc_dispatch` and `osc_dispatch`, a 180-shape sweep over the DCS introducer, a payload
compared byte-for-byte against what the engine was given, and a RIS test driving all five readers over
each of the six read-through bytes. Disabling the framer's read-through arm fails three tests; the OSC
pair fails against the old framer. The one deliberate divergence — cmote abandoning a cancelled OSC
where the engine dispatches it — is pinned from both sides rather than left implied.

### Not done
- **`pedantic` is not `nursery` or `restriction`.** Nothing here argues those should follow; this
  section is evidence about one lint group and should not be read as a policy about all of them.
- **The ESC door is still spelled three times.** `csi::Framer`, `osc::Framer` and `dcs::Framer` each
  open on an ESC and each must obey the same rules about what may sit between it and the introducer,
  and about an ESC that ends a string. They do now, and `differential.rs` holds all three to the
  engine's parser — but nothing STRUCTURALLY stops the fourth one from drifting again, which is exactly
  the position the DCS grammar was in when this section started. Merging them would mean one machine
  that dispatches by family, which is a small `vte` and would want its own section to justify; the
  three-framer split earns its keep while what differs is the introducer and the terminator. The cheap
  half of the fix is already there: the four things that are genuinely common (`Params`,
  `MAX_INTERMEDIATES`, `passes_through`, `Span`) live in one place.
- **`osc::Framer` is stricter than the engine on a cancelled string, and `dcs::Framer` on an
  interrupted one.** Both are the safe direction and both are tested as divergences, but they are
  divergences: the engine dispatches what it had and cmote drops it. Safe only because the engine has
  no handler behind any OSC or DCS cmote reads, so no second actor can fall out of step — an engine
  version that grew one would make this a defect, and the tests naming it are how that gets noticed.
- **The load stress is manual, not in CI**, and that is a decision rather than an omission. It lives
  in `AGENTS.md` beside the prove-it rule, in §13's category: a check that depends on the machine.
  GitHub's runners are two to four cores and already noisy, so a stress job would be the most likely
  thing in CI to fail for the weather — and a job that fails randomly is one people learn to ignore,
  which costs more than it catches. The price is that the next test betting on a timing window gets
  found the way this one was: by somebody noticing.

## §112 — Two rows the framers made cheap

§99 listed eight ❌ rows in the CSI table and called them "unbuilt features, not open questions". Four
had since been built by the sections that grew the machinery they needed (§100, §101, §102). This
section builds two more, and the reason to record it is not the sequences — they are small — but WHY
they were small: both had been costed as "needs a scanner", and §111 had just made a scanner one line.

That is the whole shape of this section. Neither row was reconsidered on its merits. What changed was
the price, and the price changed as a side effect of work that was about something else.

### DECBI / DECFI — the last two names of a six-name family

`ESC 6` and `ESC 9` are the horizontal twins of RI and IND: one column back or forward, and AT the
margin the band slides sideways under the cursor instead. DEC's own words for the first — "moves the
cursor backward one column. If the cursor is at the left margin, all screen data within the margins
moves one column to the right" — and DECFI is the mirror at the right margin.

§98 named this family as **one piece of absent machinery wearing six names**: SL / SR, DECBI / DECFI,
DECIC / DECDC. §100 built the sideways scroll and moved two. §102 built the margins and moved two more.
That left DECBI / DECFI as a gap whose definition was already entirely written as code — and it stayed
a gap for two more sections, because what was missing was neither half of the definition. It was a way
to READ two escape sequences. `vte` dispatches `ESC 6` and `ESC 9` to nothing at all (`ansi.rs`, they
fall to its `unhandled!` arm), so there was no `Handler` arm to implement: it needed a scanner, and a
scanner for two bytes meant a state machine of its own with a chunk boundary to get right.

§111's `dcs::Framer` reports escape sequences as well as control strings, and `rect.rs` was already
using one — for the RIS that resets DECSACE. So the cost here was **two match arms** in a callback that
already existed:

```rust
b'6' => indexes.push((span.past(), RectRequest::Index { forward: false })),
b'9' => indexes.push((span.past(), RectRequest::Index { forward: true })),
```

The applier is where the two behaviours are chosen, because the applier is what knows the margins. Two
details are worth stating:

- **The cursor half is asked of the ENGINE**, in its own spelling — CUB and CUF, fed through the gate,
  which already bounds them by the margins. cmote writing the cursor directly for the sake of two
  bytes would make it a second writer of the one piece of state §71 is most careful about, and the
  engine's own sequence does the same job with the same bounds.
- **DECFI's scroll is a delete at the LEFT margin**, not at the cursor. The content moves left, so the
  hole opens at the right and the loss is on the left — while the cursor sits on the right margin. That
  is one column argument away from what DECIC and DECDC do, so `shift_band_columns` gained an `_at`
  form and the band arithmetic stayed written once.

Six tests, and the one that earns its place is the near miss: **`ESC 7` is DECSC**, the save-cursor the
engine implements, one byte from `ESC 6` — and `ESC ( 6` designates a character set. The intermediates
test is what keeps the second one out, which is §56's rule in a family §56 never touched.

### SETMARK — a second door to one instruction

`CSI > Ps M` is contour's spelling of what `OSC 1337 ; SetMark` does, and cmote has shipped that since
§55. So this row was never about a feature; it was about a spelling. The compat plan had it as "a gap
and a cheap one — one writer, one more door — left open because a scanner is real work and nothing but
contour's own integration emits it".

A scanner stopped being real work in §111. It lands in `term/iterm.rs`, beside the meaning rather than
in a module of its own: `Report::Mark` and its one consumer are already there, and a second module for
one final byte would put one concept in two places — which is what §108 is about. The cost is a file
named for iTerm2 holding a sequence contour defined, and the header says so.

**The near miss here is dangerous rather than merely possible.** `CSI Ps M` with no private marker is
**DL**, delete lines — a sequence the engine implements and every full-screen program uses. Reading one
as a bookmark would leave a tick in cmote's gutter every time a program deleted a line, which is not a
subtle failure and would not have been caught by a test of the sequence itself. All three parts are
matched together (§56), and `a_delete_lines_is_not_a_mark` pins it from the other side.

`Ps` is contour's mark KIND and is not read. cmote has one kind — a tick in its own gutter — so a
parameter naming another would be answered with the only one there is, which is exactly the reading
`SetMark` already gets by carrying no parameter at all.

### Files

- `src/term/rect.rs` — `RectRequest::Index`, two arms in the escape pass, and the sort that merges the
  two families by offset.
- `src/term/mod.rs` — `index_column`, and `shift_band_columns_at` split out of `shift_band_columns`.
  Six tests.
- `src/term/iterm.rs` — a `csi::Framer`, `is_set_mark`, the merge, and three tests.
- `TERMINAL_COMPATIBILITY_PLAN.md` — two rows to ✅, and the horizontal-family paragraph closed.
- 1,494 tests green.

### Not done

- **DECRQDE and DECRQPSR stay gaps, and the blocker is a document, not the state.** Both describe state
  cmote holds — the displayed extent is arithmetic on numbers `CSI 18 t` already reports, DECCIR is the
  cursor DECXCPR reads, DECTABSR is `term/tabs.rs`'s own table. What is missing is the exact shape of
  the replies: DECRPDE's parameter list and whether DECRQPSR's `DCS … $ u ST` envelope has an "I do not
  report that" form the way DECRQSS's does. Writing a reply in a format nobody has read is the thing
  §60 refused for the checksum and §54 for the progress: **an invented answer is worse than a missing
  one**, because a program acts on it. They are cheap the day DEC's manual is read for them.
- **XTSAVE / XTRESTORE is not a scanner problem** and this section changes nothing about it. Restoring
  an arbitrary private mode means holding a copy of the engine's mode state, which makes cmote a second
  source for it — §71's rule, and the reason the row's own note calls the cost understated.
- **The DEC locator trio is still a gap nothing asks for.** cmote reports mouse events in xterm's
  spelling (modes 1000–1006), which is what programs emit; the two locator *status* questions are
  already answered with the honest negative (§82). What is absent is DEC's own protocol, and a
  protocol nothing sends is the one kind of gap that stays cheap by staying open.
- **Neither sequence has a differential test.** `ESC 6` / `ESC 9` and `CSI > Ps M` are cmote's alone —
  the engine dispatches all three to nothing — so there is no second verdict to compare against, which
  is the same position the other nine act-alone scanners are in (§106). The self-consistency sweeps
  cover the CSI one by construction, since it goes through the shared grammar.


## §113 — The half of the tree the gate never reads

> Read with §114. "One target wide" is one of the gate's two narrow axes; the other is that it runs
> whatever compiler was last installed here, while CI follows stable.

The macOS CI job was red. Not newly red: red since §103, for **118 commits and several pushes**, while
the local gate ran green every time and every commit went out believing it.

That is the section. The nine compile errors underneath it are small and mechanical, and they are not
what is worth recording. What is worth recording is that **the gate is one target wide**, so half the
`cfg` in this tree has exactly one reader — CI — and that reader had been saying so, in the open, for
four days.

### Two classes, two different ages

The nine errors are two unrelated breakages that had piled up on each other:

- **Four unused imports**, since §103 (`82f3af9`, 2026-08-17) — the commit that introduced
  `local/path.rs` and `local/shells.rs` with their `cfg` pairs. `Component` is used only by the
  Windows `to_posix`; `Path` only by the Windows `git_bash_path`; the test module's `to_posix`, `Path`
  and `PathBuf` only by `#[cfg(windows)]` tests. Each is a plain `unused_imports`, an error under
  `warnings = "deny"` — and one that can only be seen from the other target.
- **Five `clippy::unnecessary_wraps`**, since §111 (`0396af9`) turned `pedantic` on. `grab_interaction`,
  `mode_of`, `owner_of`, `group_of`, `source_mode` — each the macOS arm of a `cfg` pair whose Windows
  twin answers `None`.

The ages matter because they say the same thing twice: neither was introduced carelessly, and neither
was catchable by anything a developer runs. §103 and §111 both ended on a green gate.

### The `Option` belongs to the signature, not to the arm

The five pedantic errors are clippy being right about what it can see and wrong about the code. On
macOS `mode_of` really does always answer `Some` — but the `Option` is the shape the two arms SHARE,
and a Windows row genuinely has no mode, owner or group to show. Clippy lints one `cfg` at a time, so
it never sees the twin returning `None`.

There is no restructuring that removes the wrapper honestly. Folding the pair into one function with a
`cfg` inside the body does not help — the lint reads the compiled body, which is still always `Some`.
Turning the lint off in `Cargo.toml` would trade a real lint everywhere for five sites. So these are
the first suppressions in the tree, and they are `#[expect]` rather than `#[allow]` for the reason §111
gave when it deleted the last one: an `expect` that stops being needed becomes an error. The day one of
these arms gains a failing `Metadata` call, clippy says the expectation went unfulfilled.

§111's title — "Pedantic, without a single `allow`" — survives literally, and it should be read with
this section beside it: pedantic was turned on, and verified on one of the two shipped targets.

### One thing measured before writing it

An `#[expect(clippy::…)]` is compiled by plain `rustc` too, and the macOS job's SECOND step is
`cargo test`, which does not run clippy. If rustc reported the expectation as unfulfilled, every
`#[expect]` added here would break the tests it was meant to leave alone. Measured on a spot where the
lint cannot fire — the always-`None` Windows `mode_of`:

| | unfulfilled `clippy::` expectation |
|---|---|
| `cargo check` / `cargo test` (rustc) | silent, exit 0 — `warnings = "deny"` never sees it |
| `cargo clippy` | `error: this lint expectation is unfulfilled` |

So the attribute is inert where clippy is not running and enforced where it is, which is what makes it
safe on the native test step. This is the sort of thing that reads as obvious after the fact and
decides the design before it.

### The gate's blind spot, written down where the gate is

`AGENTS.md` claimed "a commit that fails the gate locally fails it there too" — true, and the wrong
direction. The implication runs one way only, and the converse is what everyone actually relies on.
It now says what the gate cannot see, and the two rules that follow: **read the macOS job**, and treat
any change touching a `cfg` pair as unverified until it has.

A local cross-check was attempted first and is not available: `rustup target add x86_64-apple-darwin`
installs a `std`, and then `ring`'s build script runs `cc` for the target and there is no darwin C
toolchain on this machine — no `clang`, no `zig`, nothing that ships an SDK. So the nine fixes here are
argued from the compiler's own diagnostics and from reading each sibling arm. **They are not verified
by a run.** That is the honest status of this commit, and CI is what closes it.

### Files

- `src/local/path.rs` — `Component` moved into the Windows `to_posix`; the test module's Windows-only
  imports gated.
- `src/local/shells.rs` — `Path` dropped from the module imports and spelled out at its one use.
- `src/local/fs.rs` — three `#[expect]`s and the note above the trio that says why once.
- `src/cursor.rs`, `src/ssh/upload.rs` — one `#[expect]` each.
- `AGENTS.md` — the green gate's blind spot, and the two rules.

### Not done

- **The macOS half of `local::path` has almost no test of its own.** Every `/C:` test is
  `#[cfg(windows)]`, which is right — drive letters are not paths over there — but it leaves
  `to_posix`'s macOS arm untested and `to_native`'s covered by two assertions borrowed from shared
  tests. This is the local file layer's one-directional security boundary, so that is a real gap and
  not a tidy one. It is not closed here because a test written for a platform that cannot be run
  locally is a guess, and this section is already shipping nine of those.
- **Nothing prevents the next `cfg`-pair drift.** The blind spot is documented, not removed. Removing
  it needs either a darwin toolchain that can run `cc` (an SDK on this machine) or a CI job that fails
  loudly enough to be read — and the second one already existed.
- **The `cfg` pairs were not audited beyond the nine.** Nine is what the compiler named; the tree has
  around sixty `cfg` attributes, and the ones whose macOS arm happens to lint clean today are clean by
  luck, not by check.

## §114 — And one toolchain behind

§113 ended on "the gate is one target wide", and one day later the **Windows** job went red — the job
that runs the same `cargo clippy --all-targets -- -D warnings` the gate runs, on the same target, with
no cross-compile anywhere near it. Seven errors, on two lints that do not exist in the clippy on this
machine.

| | version |
|---|---|
| local | clippy 0.1.97 / rustc 1.97.1 (2026-07-14) |
| CI (`dtolnay/rust-toolchain@stable`) | rustc 1.98.0 (2026-08-18, channel 2026-08-20) |

So the gate is narrow along a **second** axis, and this one is worse in a way, because nothing about
it is visible: `@stable` in CI floats and follows the release train, while the local toolchain is
pinned to whenever somebody last typed `rustup update`. The two drift apart silently, the gap widens
until it is bridged by hand, and a lint that shipped in stable **the day before** turns a green gate
into a red push.

Note what this is not. §113 was rot — code wrong since §103 and reported for four days. This is the
opposite: every one of these seven sites was clean when it was written, and stable moved under them.
No commit here did anything wrong, which is exactly why no amount of care at commit time would have
caught it.

### The fix is a fifth gate step, not a pin

The obvious answer is a `rust-toolchain.toml` pinning both sides to one version. It is the wrong one
for this project. Pinning makes the two compilers identical, but it converts "a new lint arrived" from
an event into a chore, and this is a tree that runs `clippy::pedantic` on purpose (§111) and wants the
new lints on the day they ship. Freezing the compiler to keep CI quiet would be buying green by
turning the alarm off.

The other direction gets both. `rustup update stable` joins the gate as its first step:

```
rustup update stable      # ← §114
cargo check --all-targets
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

It is a no-op when already current, it costs seconds when it is not, and it closes the gap by
construction rather than by remembering. CI keeps floating, so a new lint still arrives immediately —
it just arrives *locally first*.

**The residual risk, named:** a stable release landing between the local gate run and the CI run. The
window is a six-week cadence against a run that takes minutes, and the failure mode is a red CI job,
which §113 already established as a thing that gets read. That is a small enough hole to leave open
knowingly; it is not a hole to leave undescribed.

### Both lints turned out to be right

Worth saying, because appeasing a linter and being taught something by one look identical in a diff:

- **`chunks_exact_to_as_chunks`**, five sites. `as_chunks::<N>()` hands back `&[[u8; N]]` instead of
  `&[u8]`, so the indexes that follow are bounds-checked once, at the split, rather than on every
  access the optimiser cannot see through. At `editor.rs`'s UTF-16 decode it deleted code outright: a
  `&[u8; 2]` is exactly what `u16::from_le_bytes` wants, so the array that was being rebuilt by hand
  from two indexes is gone. Each site gained a line saying why the discarded remainder is empty —
  every one of them already had the length guard that makes it so, and at `hex_decode` that guard is
  load-bearing, because silently dropping a trailing half-byte would decode `54E` as `54`.
- **`unused_async_trait_impl`**, twice, on `Script` — the `Exec` test double (§46). Both methods were
  `async fn` with nothing to await, and saying so in the signature is more honest than hiding it. It
  also surfaced a distinction worth knowing: an `async fn` body does not start until the first poll,
  so `record` used to run at the `.await`; written as `-> impl Future` returning `future::ready`, the
  body runs at the **call** and only the answer is deferred. Every test here awaits immediately so the
  recorded order is unchanged, but the two are not the same function and now the signature says which.

One correction the lint's own suggested diff got wrong: it kept `bail!`, which expands to a `return`
and cannot appear where the signature promises a future. `Err(anyhow!(…))` is the same value without
the control flow.

And a footnote that is really §113 repeating itself inside one commit: importing `anyhow!` at module
scope failed immediately, because its only use is inside a `#[cfg(test)]` impl. Moved to the one
function that uses it, exactly as `Component` was in §113.

### Files

- `src/cursor.rs`, `src/editor.rs`, `src/term/query.rs` — five `as_chunks::<N>()`.
- `src/ssh/shellfs.rs` — `Script`'s two `Exec` methods, and the note on eager body vs deferred answer.
- `AGENTS.md` — the version axis, and `rustup update stable` as the gate's first step.

### Not done

- **CI is not pinned, on purpose** — see above. If a floating `@stable` ever breaks a release build
  rather than a lint run, that trade is worth reopening; a lint break is cheap and a build break is
  not.
- **The gate still cannot see the other target.** §114 fixes the version axis only. `rustup update`
  does nothing about `x86_64-apple-darwin`, which still has exactly one reader.
- **Nothing checks that the gate's first step was actually run.** It is a line in `AGENTS.md`, the same
  standing as the other four.

## §115 — What the red step was hiding

§113 fixed the macOS clippy step, so for the first time the macOS job got as far as its second step
and ran `cargo test`. One test failed:

```
test local::pty::tests::a_real_child_runs_on_a_real_pty_and_its_exit_is_observed ... FAILED
```

`git log -S` puts that test in **`82f3af9`, §103** — the *same commit* whose unused imports broke the
clippy step. Steps stop at the first failure, so the job exited 101 at clippy and never reached the
tests. **This test has therefore never passed on macOS, not once, and nothing could have said so**:
the one runner that would have told us was stopped one step short of finding out, by a fault in the
same commit.

That is the shape worth keeping. §113 and §114 were both a *compiler* unable to see something.
This is different and worse: fixing a red step does not turn a job green, it moves the job forward to
whatever the red step was standing in front of. A first green run after a long red one is not a
verdict, it is the first look.

### Third axis: the gate runs the host's tests

The gate is one target wide (§113) and one toolchain version narrow (§114), and this is the third:
**`cargo test` runs the host's tests.** 1494 of them here, 1482 on the mac runner — the difference
being the `#[cfg(windows)]` ones. Those 1482 have exactly one reader, and lints and tests are not the
same kind of evidence. A lint is an opinion about source; a failing test is the program behaving
differently. Everything §113 said about reading the macOS job applies here with more force.

### A ConPTY fact asserted as a pty fact

The bug itself is small and its shape is the familiar one. The test asserted:

```rust
assert!(answered, "the ConPTY asked where the cursor is and cmote answered — ...");
```

`answered` is true when the emulator had something to reply to. On Windows that is not optional and
the module note says why: `portable-pty` builds the ConPTY with `PSUEDOCONSOLE_INHERIT_CURSOR`, so it
sends `CSI 6 n` and withholds every byte the child prints until it is answered — a version of this
test that only read bytes hung for twenty seconds and saw four bytes. But that is a **ConPTY**
behaviour, not a pty one. A Unix pty asks nothing, `/bin/sh -c 'echo …'` prints and exits, the
emulator has nothing to reply to, and an assertion whose own failure message names a Windows object
fails on a Mac.

The fix asserts the platform fact instead of one platform's half of it:

```rust
assert_eq!(answered, cfg!(windows), "…");
```

Both directions on purpose. Expecting `false` off Windows is not a weaker assertion than skipping the
check — it pins that a Unix pty puts no query, so a pty that ever *started* putting one is caught here
rather than quietly changing what this test covers. `cfg!` rather than `#[cfg]` for the reason
`local::path`'s traversal test uses it: both arms compile on both platforms, so neither can rot
unseen, and no import or variable is left unused (§113).

Probed per the prove-it rule, by inverting the expectation on Windows: `left: true, right: false`. So
`answered` really is `true` here, the assertion is live rather than vacuous, and the macOS failure is
its exact mirror.

### The masking, fixed where it happened

A red step hiding every step behind it is a property of the workflow, not of this bug, and it will do
it again. Both `cargo test` steps now carry `if: ${{ !cancelled() }}`, so clippy and the tests report
on every run and a lint error can never again stand in front of a behaviour failure. Not `always()`:
this workflow sets `cancel-in-progress`, and a superseded run should stop rather than spend minutes
testing a commit nobody is waiting on.

### Files

- `src/local/pty.rs` — the assertion, and the module note's fourth bullet, which promised the exchange
  was asserted unconditionally.
- `.github/workflows/ci.yml` — `if: ${{ !cancelled() }}` on both `cargo test` steps.

### Not done

- **This fix is not verified on the platform that failed.** Same status §113 shipped with, and for the
  same reason: no darwin toolchain here. The Windows arm is verified and unchanged, the macOS arm is
  argued from what a Unix pty does.
- **It is not known whether this was the only macOS test failure.** The CI log available was truncated
  mid-run, and that run was the first `cargo test` this target has had in over 118 commits. One failure
  is what could be seen; it is not established to be one failure.
- **`echoing_shell` covers `windows` and `target_os = "macos"` and nothing else.** Correct for what
  cmote ships, and a third unix would find the fixture has no arm at all rather than a poor one.

## §116 — The scrollbar becomes a control

§23 shipped a scroll *indicator*: a thin thumb in the grid's right padding gutter that reported where
the view sat and how deep the history was, moved by the wheel and the keys, and drew nothing at all at
the live bottom. This makes it grabbable — press and drag the thumb to move the view, press the bare
track to jump there and carry on dragging from it, release to stop.

Two things about the old design had to change, and both were right before and wrong now.

### An auto-hiding control cannot be grabbed

`scrollbar_thumb` returned `None` at `offset == 0`, and §23 called that "auto-hiding without an
animation timer". As an indicator that was exactly right: at the live tail there is nothing to report.
As a control it is the whole problem — a bar that appears only after you have scrolled by some other
means cannot be the thing you scroll WITH, so the gesture would have been "wheel a little, then you
may drag". The only `None` left is a session with no scrollback at all, where there is genuinely
nowhere to go.

That is a deliberate reversal of a §23 decision, not an oversight in it, and the paragraph in §23 that
still describes the auto-hide now says so.

### The mapping had to become invertible, and was not

This is the part worth reading. A drag needs the *inverse* of the thumb geometry: the pointer says
where the thumb should be, and the code has to answer with the offset that means. The forward mapping
scaled the position by the whole track height and then CLAMPED the result to the track's bottom:

```rust
let position = (history - offset) / (history + rows);   // fraction of the DOCUMENT
let thumb_top = (track_top + position * track_height).min(max_top);
```

For a shallow history the clamp is an equality and the picture is right, which is why it survived
review as an indicator. For a deep one it is not: past the clamp a whole RANGE of offsets all draw the
same bottom-most thumb, so the mapping stops being injective and no inverse exists. Left alone, a drag
down a long history would move the view while the thumb sat pinned at the bottom, not following the
pointer — the exact thing that makes a scrollbar feel broken.

So the forward mapping now scales onto the span the thumb can actually occupy, which is the track less
the thumb's own height, and takes its fraction of the *scrollable range* rather than of the document:

```rust
let position = (history - offset) / history;            // fraction of the RANGE
let thumb_top = track_top + position * (track_height - thumb_height);
```

The thumb's height still carries the document's depth — that is what it is for — and now
`scrollbar_offset` is an exact inverse rather than an approximate one. The two are only correct
together, so the test asserts the ROUND TRIP over a table of depths and offsets rather than either
one's arithmetic. Probed by restoring the old forward mapping: at history 5000, offset 2500 came back
as 2391, 109 lines adrift, which is the drift a user would have felt as lag.

### Where the press is answered, and why there

Inside the `Grid` widget, before both of the other things a press can mean. Above the widget a
`mouse_area` starts a text selection; inside it, a mouse-aware program gets a report. A press on the
bar is neither — it is chrome in the padding gutter rather than a cell, and it is cmote's own view
control rather than anything the remote should hear about — so the widget claims it and captures it,
and neither path sees it. This is the same shape the wheel branch above it already had.

A full-screen program needs no special case, which is the nice part. The alternate screen retains no
history, so `history_size()` is 0, so there is no thumb, so nothing is claimed: a click in `vim`'s
right-hand column reaches `vim` exactly as before.

Three smaller decisions:

* **The grab zone is the whole 6px gutter, not the 4px paint.** The thumb is thin so it reads as an
  indicator; a 4-pixel target is a target you miss. This is the rule the left gutter's prompt ticks
  already use — a 3px tick, and the press tests the gutter (§34).
* **A drag is not bounds-tested on the move.** Wander off the bar sideways or past either end and it
  keeps scrolling, pinning at the end it ran out at. Dropping a drag because the pointer strayed a few
  pixels is the other thing that makes a scrollbar feel broken.
* **The grip is stored, not the starting offset.** The thumb stays under the same part of itself for
  the whole drag. Treating the pointer's y as the thumb's top would jump the view by up to a thumb's
  height on the first pixel of movement.

### `ScrollMotion::To`, the first absolute motion

Every other motion in `ScrollMotion` is relative because every other caller is: a wheel notch, a page,
a key. A drag is not — what it knows is where the view should BE. The engine has no absolute scroll, so
`To` reads the current offset and issues the delta that reaches the target, clamped to the retained
history. Doing that inside `term/` rather than in the UI is the point: the alternative is the widget
keeping its own copy of the offset, which would drift from the engine's clamping the first time a drag
ran off an end. The subtraction is in the line-number domain for the reason `jump_prompt` already
gives — both are `usize` offsets, and a scroll DOWN would wrap.

### No hand cursor, deliberately

`cursor.rs` says every grabbable surface should wear the §51 hand, "the point of a shared affordance is
that the user learns it once", and lists two — the tab chip and the dialog header. The scrollbar is now
a third draggable thing and does NOT wear it, because the rule is about affordance and not about
mousedown: those two are objects you pick up and put somewhere else, and CSS's `grab` means exactly
that. A scrollbar is a slider — it goes nowhere, and no browser or terminal shows a hand over one. A
hand here would promise the wrong gesture.

### Files

- `src/ui/grid.rs` — the invertible geometry (`scrollbar_thumb`, `scrollbar_offset`,
  `scrollbar_track`, `scrollbar_thumb_height`, `document_lines`), `GridState::scroll_grip`, and
  `scroll_drag` wired ahead of the report path.
- `src/term/mod.rs` — `ScrollMotion::To`.
- `src/app.rs` — `Message::TerminalScrollTo` and `on_terminal_scroll_to`.
- `PLAN.md` §23 and the two overviews, `TERMINAL_COMPATIBILITY_PLAN.md` — five places called the bar
  read-only or said it vanishes at the bottom.

### Not done

- **The widget plumbing is not verified by a run.** The geometry, the clamping and the handler's
  contract are all unit-tested, and the round trip and the no-side-effects assertions were both probed
  by breaking them. What no test here covers is iced's own event ordering — that capturing the press
  really does keep the `mouse_area` above from starting a selection. That needs a window, a local shell
  and a hand on a mouse, which is §13's manual category.
- **No wheel-over-the-bar special case.** A wheel while the pointer is on the bar scrolls as it does
  anywhere else on the grid, which is what every terminal does; it is only worth noting because a drag
  and a wheel now share a surface.
- **No page-jump on a track press.** A press on the bare track jumps straight to that position, which
  is what was asked for. The other convention — page up or down by one screen per click, holding to
  repeat — needs a timer, and cmote runs no animation timer (§23).
