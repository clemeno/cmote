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

Status: **shipping — v3.0.0** (v1 feature set complete; v1.3 adds saved connection
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
follows OSC 8 hyperlinks — Ctrl+click or a right-click Open link / Copy link, the scheme gated
to http/https/mailto, §24; and speaks the kitty keyboard protocol — the `CSI u` encoding
for disambiguate, press / repeat / release events, report-all and associated text, superseding
modifyOtherKeys when an editor turns it on, §25). Everything since v2.3.0 lands in the one
**v3.0.0** major release — §23, §24 and §25 are all part of it, with no point increments above
3.0.0. Both targets are supported first-class, and each has a verified
toolchain on its host:

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
| `iced` | 0.14.0 | GUI (Elm architecture, `Task`, `Subscription`) | pure Rust; wgpu/tiny-skia renderer, no web runtime. **`features = ["advanced"]`** since v2.3 — it unlocks the `Widget` trait, which the terminal grid is one of (§9) |
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
    ├── explorer.rs       the remote folder tree's model: nodes, expansion, path arithmetic (§18)
    ├── files.rs          the files pane's model: one directory, batched listings, icon categories (§19)
    ├── forward.rs        the pure port-forward spec: kind (L/R/D) + bind/target, parse / validate / label / serialise (§27)
    ├── link.rs           opening an OSC 8 hyperlink safely: the scheme allow-list + the OS browser launch (§24)
    ├── palette.rs        the terminal colour scheme (default fg/bg + xterm-256), shared by the renderer and the colour-query answerer (§9, §23)
    ├── paths.rs          data-dir resolution: `cmote-data/` beside the exe if writable, else `%LOCALAPPDATA%\cmote` / `~/Library/Application Support/cmote` (§11)
    ├── profiles.rs       load/save `targets.json`: saved connection profiles + the per-target session snapshot; corrupt file → treated as empty (§14, §22)
    ├── secret.rs         the session-secret wrapper (`Secret` over `zeroize`): passwords / passphrases held in memory, wiped on drop, never logged (§12)
    ├── ui/
    │   ├── mod.rs         view helpers, incl. the shared `elide_middle` path/name cut (§22); host-key / passphrase / error dialogs (§8, §7, §6)
    │   ├── connect.rs     the connection form (host/port/user/auth/key)
    │   ├── dialog.rs      shared modal-dialog chrome: header (title + ✕) / body / footer (§10)
    │   ├── explorer.rs    the folder-tree panel, its splitter and its context menu (§18)
    │   ├── files.rs       the file icon grid, its splitter and its context menu (§19)
    │   ├── forward.rs     the port-forwards manager dialog: active-tunnel rows + the add form (§27)
    │   ├── grid.rs        the terminal screen as ONE custom widget: cell-exact quads + text, drawn braille and box corners, mouse reports (§11)
    │   ├── home.rs        the home screen: the saved-target list, select / open / rename / delete, theme-following colours (§14)
    │   ├── menu.rs        shared right-click menu chrome: panel / items / dismiss layer (§10)
    │   ├── selection.rs   stream text selection over the grid; text extraction (§10)
    │   ├── snackbar.rs    the copy-confirmation toast, bottom-centre, self-dismissing (§10)
    │   ├── tabs.rs        the tab strip across the top: one chip per session + "+"; mouse-only select / open / close (§26)
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
    │   ├── keyfile.rs     load PEM/OpenSSH + PuTTY .ppk (via ssh-key from_ppk); passphrases; zeroize (§7)
    │   ├── transfer.rs    the recursive transfer's shared spine: the tree plan + the per-file collision protocol (§17, §19)
    │   ├── upload.rs      file + recursive-folder upload over an sftp channel: batch pre-scan, stream, progress, per-file collisions (§17)
    │   └── fixtures/      real .ppk test vectors (Ed25519, plain + encrypted)
    ├── term/
    │   ├── mod.rs         terminal emulator wrapper: drive the engine, expose the screen view, resize, answer the host's colour/size queries (§9, §16, §23)
    │   ├── cwd.rs         scan OSC 7 / OSC 9;9 out of the output stream: the remote cwd (§17)
    │   ├── keymap.rs      GUI key events → the bytes a terminal sends; legacy or kitty per the active mode (§9, §25)
    │   ├── kitty.rs       encode a key event in the kitty keyboard protocol's CSI u form (§25)
    │   ├── mouse.rs       pointer events → the xterm mouse reports a program that asked for them expects (§9)
    │   ├── modkeys.rs     scan `CSI > 4 ; p m` out of the stream: the remote's modifyOtherKeys level (§9)
    │   └── screen.rs      the engine-agnostic Screen/Cell/Color view the app reads through — incl. a cell's OSC 8 link + the kitty flags (§9, §16, §23, §24, §25)
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
enum Screen { Home, Connect, Connecting, ConfirmHostKey, HostKeyChanged, NeedPassphrase, Interactive, VaultUnlock, Terminal, Error }
```

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
- **Confirm host key** (`Screen::ConfirmHostKey`): first-contact fingerprint with
  Accept / Reject (§8), in the shared dialog chrome floating over the dimmed connect form
  (below). Closing (✕) or a backdrop click rejects — the safe default, so dismissing never
  trusts an unverified host.
- **Host key changed** (`Screen::HostKeyChanged`, §8, §28): the mismatch override dialog, over
  the same dimmed form. Loud by design — a red "possible man-in-the-middle" line and **both**
  SHA-256 fingerprints (stored vs presented, selectable for out-of-band comparison, seeded into
  `App::dialog_body`) — with a three-button footer: **Reject** / **Trust once** / **Replace key**.
  Closing (✕) or a backdrop click rejects, so dismissing never trusts a changed key. A changed
  key is never auto-trusted; each override is one explicit click (§8).
- **Need passphrase** (`Screen::NeedPassphrase`): shown only when the chosen private
  key is encrypted (§7). A masked field with Unlock / Cancel; the field is auto-focused
  when the screen opens (a `text_input::focus` task keyed to a shared id, refocused on
  every re-ask) so the user can type at once. A wrong passphrase re-shows the prompt
  (the session re-asks, bounded) with an "incorrect" hint — the app tracks whether an
  attempt was already made this connection, since the bridge emits the same
  `NeedPassphrase` for a first ask and a re-ask. The typed text is moved into a `Secret`
  and cleared on submit. This is a local key-file passphrase, not remote auth, so the
  hint is not a credential oracle (§12). The prompt uses the shared dialog chrome (below),
  floating over the dimmed connect form.
- **Interactive prompt** (`Screen::Interactive`, §7): the server's keyboard-interactive
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
    the text is selectable yet never mutable. While the disconnect modal is open, `on_key`
    stops forwarding keys to the shell so Ctrl+C copies rather than sending ETX to the
    remote. The `Screen::ConfirmHostKey` / `Screen::Error` variants carry no text anymore —
    the message lives in `dialog_body`, so they are bare markers.
  - **Draggable by the header** (§10): pressing the header background starts a drag
    (`DialogGrabbed`), and while dragging a transparent full-window capture layer reports
    every pointer move (`DialogDragged`) and the release (`DialogReleased`) — so tracking
    survives the pointer leaving the card. `App` moves the card by the pointer delta and
    clamps it (`dialog_pos`, `window_size`): horizontally exact via the fixed width, and
    vertically only far enough to keep the header on screen (`DIALOG_DRAG_MIN_VISIBLE`) —
    iced does not expose the card's real height, so this keeps the dialog draggable to the
    window's bottom (and grabbable back) rather than stopping short of it. The ✕ button
    captures its own press, so closing never starts a drag.
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
  - **Folder tree beside the files pane** (done, v2.0; moved v3.x): the right end of the
    bottom browser strip holds the remote folder explorer, with a draggable splitter between
    it and the files pane and a status-bar button that hides it. Its width comes out of the
    *pane's* now, not the grid's — the terminal keeps the full width above the strip — so a
    tree resize only reshapes the pane, and only the strip's height reflows the pty (§18, §19).
- **Error** (`Screen::Error`): a generic, non-leaking message (selectable/copyable) plus
  a "Back" button, in the shared dialog chrome floating over the dimmed connect form.
  Closing (✕) or a backdrop click goes Back. Detail is logged, not shown (§12).

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

Only the live-SSH end-to-end path stays manual — there is still no CI SSH server. CI
builds no release artifact; publishing the portable binaries stays a manual step (§16,
code signing / release automation).

---

## 14. Saved connection targets (v1.3)

The home screen (`ui/home.rs`) is the landing screen: a list of previously used
connection **targets**, so reconnecting is a click instead of re-typing the form.

- **What persists — profiles only, never secrets in this file (§12).** A target records
  `name`, `host`, `port`, `user`, `auth_kind`, (for key auth) `key_path`, the panels'
  `show_hidden` preference, and a `remember_secret` flag. No password and no key passphrase is
  ever written to `targets.json`. This keeps the §12 "the safest secret is the one never
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
  (named after the endpoint) or refreshes an existing endpoint's auth/key while keeping
  its custom name — so reconnecting never spawns a duplicate and never clobbers a rename.
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
  **Delete**. `Esc` on the form returns to the list.
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
  primary method (§7). Certificate identities are left for the still-deferred certificate path.
  See §7 and `ssh/agent.rs`.
- **Broader auth (still deferred)** — certificate auth (including agent-held certificates).
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
  like the listings. v3.x then added **cancel and resume** (§16 below, wired in §17): the status
  bar's ✕ stops the running transfer — the worker deletes the partial it was writing and drops the
  rest of the batch, since a deliberate cancel is final — and a mid-flight *failure* instead keeps
  its partial and offers a **Resume** that re-sends only the bytes still missing (a byte-offset
  append for a single file; a whole tree re-walked and size-compared so only the gaps and the
  interrupted file's tail cross again). v3.x also began **preserving file metadata** across a copy
  (§17, §19): every finished file is stamped, best-effort, with the source's **modification time**
  — the one attribute meaningful in the everyday Windows case, where the client neither has a Unix
  mode to send nor can apply one — and, when both ends are Unix, its **permission bits** too (so a
  script keeps its `+x`). It never fails a transfer: a server that refuses `setstat` or a filesystem
  that will not take the timestamp is logged and the bytes stand. Still deferred: resuming across a
  *dropped connection* (cancel/resume live inside one live session — a lost session tears down to
  the error screen, so its transfer state is gone), drag-and-drop onto a folder, two transfers at
  once (a batch queues instead, §17, §21), preserving the *access* time as its own attribute
  (SFTP couples it with mtime, so an upload sends the pair but does not treat atime as a goal), and
  following symlinks inside a recursive walk (they are counted and skipped, never followed, so a
  cyclic link cannot loop the transfer).
- **Port forwarding (local/remote/dynamic)** — *done (v3.0.0)*. All three — `-L` local, `-R`
  remote, `-D` dynamic (a SOCKS5 proxy) — run over the live connection, managed from a **Tunnels**
  dialog on the status bar and remembered per target so a reconnect re-establishes them (§27). v3.x
  then added the **server-assigned remote port** (`-R 0`): a remote forward may bind port 0, the
  server picks a free port, and the row shows the port it chose while the saved spec keeps 0 so a
  reconnect asks afresh (§27). Still deferred: bracketed-IPv6 bind addresses and per-tunnel
  connection counts / activity in the dialog.
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
per-cell URI and cmote follows it on Ctrl+click or a right-click **Open link / Copy link**,
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
- **Code signing + auto-update** — sign the exe (Authenticode) so Win11 SmartScreen
  trusts it, and `codesign` + notarize the macOS binary/`.app` so Gatekeeper allows it;
  add a signed update channel.
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
- **Where it runs.** `Terminal::process` feeds the same bytes to the tracker and to
  the engine, which ignores OSC codes it does not know — so nothing is stripped and the two
  never disagree. The path is exposed as `Terminal::cwd`.
- **The shell hook** (`ssh::client::CWD_HOOK`). Passive reading alone leaves a plain
  bash/zsh remote silent, so right after the shell opens cmote sends **one line** that
  defines `cmote_cwd` (a `printf` of OSC 7) and hooks it into `PROMPT_COMMAND` (bash) and
  `precmd_functions` (zsh), then calls it once for the starting directory. It is sent as
  ordinary shell input, so it is echoed once like any typed command; every later
  announcement is invisible. `ponytail:` bash and zsh only — fish and OSC 9;9 shells are
  already covered passively, and any other shell prints one syntax error and leaves the
  cwd unknown. Upgrade path: probe the shell (`echo $0`) and send the matching snippet.
- **Shown in the window title.** `App::title` is a function of the state:
  `cmote — user@host:port — /current/dir` while connected, dropping the third part when the
  shell never announces one. When a program sets its own window title (OSC 0/2, §23) that takes
  the third slot instead of the cwd — the endpoint always stays, so the window is still
  identifiable by host even while a program owns the title. The title costs no grid space,
  which the status bar would.

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
  `UploadDone` pumps the next, exactly as the download queue does (§21). The status bar's
  centre zone is a progress bar per file; the closing notice is `Uploaded N files` for a
  batch, `Uploaded to <path>` for a lone one. A race — a file that appears on the server
  after the pre-scan — is skipped rather than reopening the question mid-batch.
- **Progress.** The copy loop streams 32 KiB chunks and emits `TransferProgress` every
  256 KiB — enough for a smooth bar, far below the flood that per-chunk events would be.
- **Failures stay in the bar.** A failure shows its reason in the status bar, never the error
  *screen* — that would tear down a healthy shell over a file that never left. Unlike an auth
  failure (§12), the detail here is the user's own path, so showing it is what makes the error
  actionable. A failure *before any bytes move* (the sftp channel would not open, the source would
  not read) has nothing to resume, so the queue moves to the next file as it always did; a failure
  *mid-copy* keeps its partial and pauses the batch for a **Resume** instead (§16 cancel/resume,
  below).
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
  batches, §19). The walk is **iterative, not recursive**, so a deep tree costs heap not stack,
  and **symlinks are counted and skipped, never followed** — that is what stops a cyclic link
  looping the transfer. Missing destination directories are created; existing ones are merged into.
- **Per-file collisions, mid-transfer.** A tree cannot be pre-scanned into one list a user would
  read, so each file whose destination is already taken **parks the transfer and asks** — the
  spawned transfer sends `SshEvent::TransferConflict` and awaits a `ResolveConflict` reply on a
  channel `client::run` forwards to it, the same request/await shape auth uses for a passphrase
  (§7); the shell keeps flowing behind the prompt. The six answers are a file manager's: *Overwrite*
  / *Keep both* (a `-1` copy) / *Skip* settle just this file, *Overwrite all* / *Skip all* set a
  sticky policy that settles every later collision without asking again, and *Cancel* stops the
  whole transfer — files already copied stay. This is the per-file mirror of the flat batch's
  up-front question above; the flat paths are untouched.

### Cancel and resume (v3.x)

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
  and reports `SshEvent::TransferInterrupted`; the GUI remembers what it launched (`in_flight` →
  `resumable`) and shows Resume. Resume re-issues the exact command with `resume` set. The task then
  **sizes the destination and sends only the bytes still missing**: `resume_start` (pure, unit-tested
  in `ssh/transfer.rs`) compares the destination's current size to the source's — equal or larger is
  *skip* (already there), smaller is *append from there*, absent is a fresh send. A single file opens
  its destination without truncating and seeks both ends to the offset; a tree re-walks and
  size-compares every file, so only the gaps and the interrupted file's tail cross again — and in
  resume mode an existing destination is the transfer's own earlier work, never a collision, so it
  never re-prompts. This is size-based, trusting the existing prefix to be the file's own (`ponytail:`
  no checksum — the same assumption `curl -C -` and rsync's naive mode make). A batch resumes its
  failed file, then drains the rest as usual.

Both live inside one connection: a transfer whose *session* drops tears down to the error screen, so
its state is gone — resuming across a reconnect is the deferred upgrade path (§16).

### Preserving file metadata (v3.x)

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
- **A second, muted line carries the size, the modified date and the `owner:group`**
  (`4.0 KB · 2026-03-20 11:46 · cme:staff`), reading left to right like a terse `ls -l` line.
  A directory shows no size — a directory entry's own size is not the size of its contents, and
  printing 4096 for every folder would be noise that reads as data — but keeps the date and the
  owner. Any fact the `ls` fallback never learned shows as a dash, the same convention the popup
  uses; the `owner:group` is the popup's own `owner_group` helper (§20), so the two agree.
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
- **The "Sync" button brings the console to the pane.** Since browsing no longer moves the
  shell, the pane and the console drift apart on purpose; Sync is the manual way to close
  that gap, typing a quoted `cd` (via `move_shell_to`) so the shell — and with it the tree
  and the title — comes to the folder on show. It carries no path: `app` reads `Files::path`
  when the press lands, so it can never move the shell somewhere the pane has since left. It
  sits in the left button group after Upload, and is **disabled** whenever there is nothing
  to do — no directory on show, or the pane and the shell's announced cwd already agree
  (an exact string compare, so an un-announced cwd leaves it live and the `cd` is a harmless
  no-op). Dimmed, it doubles as a tell that the two are already in step.

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
- **A focused panel keeps every key**, not just the ones it uses. A panel that swallowed
  only the arrows would leave Tab completing paths at a prompt the user is not looking at.
  **Esc** hands the keyboard back to the shell from either panel.
- The focused panel wears a one-pixel ring (`ui::explorer::focus_border`, shared by both),
  which is the only thing that tells the two panels apart at a glance.

### Walking the panels

- **Files pane:** Left/Right step one cell, Up/Down a whole row, Tab/Shift+Tab
  next/previous, **Enter** enters a folder (through the double-click's own handler, so
  "only a directory can be entered" is decided in one place), **F2** renames. Both ends
  clamp instead of wrapping.
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
  the exact byte count once the two differ) and `owner:group`.
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
- **The pane is pinned while the shell settles.** The shell announces its login directory
  *before* the replayed `cd` runs, so without a guard that announcement would drag the pane
  off a divergent `files_path`. `App::resume_cwd` holds the cwd we are waiting for: until the
  shell reaches it, `SshEvent::Output` does not let the pane follow; once it does, the
  follow-guard is seeded (so the pane stays put now but follows the next real `cd`) and the
  pin lifts. An explicit move by the user lifts it early.

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
  portable app; and its image capability is unusable here without an image compositor we do
  not have. Revisit only if images become a hard requirement *and* it gets published
  (wezterm#6663, open, no ETA).
- **`termwiz`** alone is a parser + screen buffer, **not** a full emulator — using it would
  mean re-implementing the very state machine the swap exists to drop.

**Trade accepted:** no inline images and no text-blink attribute — both marginal or unusable
for us today. The major version bump to **v3.0** marks this core change.

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
  offered, handy when the visible text hides it. `ponytail:` there is no hover affordance in
  v3.0.0 — a link is discoverable by right-clicking or Ctrl+clicking, and a program that styles
  its link (colour/underline) still shows through; a Ctrl-hover underline is a later polish.
  (`ponytail:` hover affordance still open as of v3.0.0.)

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
click on a chip selects, "×" closes, "+" opens a fresh home tab. Closing a **live** tab first
raises a confirmation (like Disconnect, closing is not undoable); an idle tab closes at once.
Closing the last tab never leaves an empty window — a fresh home tab replaces it.

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
itself for a row, and serialises for `targets.json`. `ponytail:` a bare port is allowed on the bind
side (loopback assumed); an unbracketed IPv6 literal is not (the host/port split is on the last
colon).

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

### The bridge, the state, and the dialog

Two new commands (`AddForward { id, spec }` / `RemoveForward(id)`) and two events (`ForwardReady
{ id }` / `ForwardFailed { id, reason }`) extend the protocol (§4); the id is app-assigned, keying a
forward to its outcome and its removal. Each tab keeps a `Vec<ForwardEntry>` (id + spec + status:
Starting → Active / Failed) and a small add-form. The **Tunnels** button (status bar, with the live
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
(`Screen::HostKeyChanged`, `ui::host_key_changed_view`): a red "possible man-in-the-middle" line
above **both** SHA-256 fingerprints — what was trusted vs what the server now sends, seeded into the
selectable `dialog_body` so either can be copied for out-of-band comparison. Three buttons:

- **Reject** — refuse. The safe default: the ✕, a backdrop click, and a GUI that went away all pick
  it.
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

## 29. Drag-and-drop upload (v3.0.0)

Uploading already had four doors (§17) — all of them a picker or a menu. This adds the obvious
fifth: drag a file off the desktop and drop it onto the window, and it uploads into the files
pane's current directory. One file this iteration; folders and multi-file drops are deliberately
out of scope (there is a menu **Upload folder…** for a whole tree, §17).

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
- `FileDropped(path)` → `Message::FileDropped` → `Tab::on_file_dropped`.

The drop events carry **no pointer position** (iced does not report one), so a drop cannot be aimed
at a widget — but it does not need to be: every drop targets the pane's own directory by definition,
which is the whole contract. So position is irrelevant and a drop anywhere on the window uploads
into the pane's folder.

`on_file_dropped` reuses the entire §17 upload pipeline. It seeds a one-file batch
(`upload_files` / `upload_dir` = the pane's directory) and calls `on_upload_confirmed` — the same
entry point the destination-confirm dialog calls — so the destination is **pre-scanned** and, on a
name already taken, the **same Overwrite / Keep both / Skip / Cancel dialog** opens (reused, not a
new one). There is no destination-confirm step: the drop already said where.

**Every upload now re-lists its destination.** When a batch (or a folder-tree upload) lands, the
completion calls `refresh_remote_dir(upload_dir)` — the same helper a create or delete uses — so if
the files pane (or the tree) is showing the folder the file went into, it re-lists in place and the
new file appears without a manual Refresh. It is a no-op when the pane is elsewhere, so an upload to
the shell's cwd while the pane is pointed at another folder costs no round trip. This fixes the
drag-drop's most confusing gap: a file you drop onto the pane you are looking at now shows up in it.
The tree-upload flow keeps no queue, so it stashes its destination in `upload_dir` on start for the
same completion to read.

### The decision is pure

The guard logic is pulled into `drop_outcome(connected, busy, is_dir, pane_dir) -> DropOutcome`,
free of `self` so it is unit-tested like `plan_uploads` and `band_hits`. The order is deliberate:

- **no session** outranks everything — a folder dropped onto a home tab is a silent `Ignore`, not a
  "folders aren't supported" notice about something that could never have uploaded;
- a **busy** transfer (or a batch mid-setup — `upload_files` still non-empty catches the *second*
  file of a multi-file drop, since each file arrives as its own event) is declined, one flow at a
  time behind the single progress bar;
- a **folder** is declined this iteration;
- a plain file with a **real pane directory** becomes an `Upload`; without one (nothing listed yet)
  it is `NoDir`, and the user is told to open a folder rather than the file landing on a guess.

### What is deliberately NOT here

- **No drag-out** (pane → desktop): iced cannot originate an OS drag; see above.
- **No folders, no multi-file** drop yet — single files only this iteration.
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
one tab, their drag cannot live on any single `Tab`; it lives on `App` (`overlay_pos` /
`overlay_dragging` / `overlay_drag_last`, seeded centred on open by `seed_overlay`). While an overlay is
up (`overlay_open`), `App::update` steers the shared `DialogGrabbed` / `DialogDragged` / `DialogReleased`
to itself (`on_overlay_dragged`, clamped by `clamp_overlay_pos` against the full window height, strip
included) instead of delegating them to the active tab, which otherwise drives its own dialogs' drag.
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
