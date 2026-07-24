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

Status: **design only — nothing built yet**. Both targets are supported first-class,
and each has a verified toolchain on its host (a hello-world compiled, linked, and ran):

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
| Terminal | **Full VT emulator** — `vt100` maintains the screen grid; iced renders the cells |
| Key formats | OpenSSH / PEM native via `russh::keys`; **PuTTY `.ppk` via `ssh-key`'s `from_ppk`** (already in the russh tree, `ppk` feature) |
| Host key | **TOFU** (trust-on-first-use) against a portable `known_hosts`; explicit user accept; mismatch = hard stop |
| Credentials | **Session-only** — held in memory, `zeroize`d on drop, never written to disk |
| Auth order | Offer `publickey` first (if a key is given), then `password`; driven by what the server accepts |
| File picker | `rfd` — native open-file dialog for the key file (Win32 on Windows, `NSOpenPanel` on macOS) |
| Errors | `anyhow` at the app boundary; typed `thiserror` enums deferred until a real API needs them |
| Config location | `known_hosts` beside the exe (`./cmote-data/`), falling back to `%LOCALAPPDATA%\cmote` (Windows) or `~/Library/Application Support/cmote` (macOS) if that dir is read-only |

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
- **vt100 over alacritty_terminal** — a real terminal must interpret ANSI escape
  sequences (colors, cursor moves, clears). `vt100` parses a byte stream into a simple
  `Screen` grid of cells we can render directly in iced — small, readable, enough for
  v1. `alacritty_terminal` is more complete but heavier and its API tracks Alacritty's
  needs, not ours. `ponytail:` start with vt100; upgrade path noted in §15.
- **Session-only credentials** — the safest secret is the one never persisted. v1
  holds passwords / decrypted keys only for the session and wipes them with `zeroize`.
  Saved profiles (encrypted at rest) are a deliberate later feature (§15), not a v1
  gap.

---

## 3. Tech stack + versions (mid-2026)

| Crate | Version | Purpose | Notes |
|---|---|---|---|
| `iced` | 0.14.0 | GUI (Elm architecture, `Task`, `Subscription`) | pure Rust; wgpu/tiny-skia renderer, no web runtime |
| `russh` | 0.62.4 | async SSH client | tokio-based; `client::Handler` trait. **`default-features = false` + `ring`** backend (not the default `aws-lc-rs`, which needs NASM; `ring` builds on both targets — prebuilt asm on Windows, via Xcode CLT `clang` on macOS) |
| `russh::keys` | (with russh) | key loading + `known_hosts` | `load_secret_key`, `decode_secret_key`, `check_known_hosts_path` |
| `tokio` | 1.53 | async runtime | features: `rt-multi-thread`, `net`, `io-util`, `sync`, `macros`, `time` |
| `vt100` | 0.16.2 | VT/ANSI screen parser | feeds bytes → `Screen` grid of cells (0.16, not 0.15 — latest on crates.io) |
| `.ppk` support | (in `ssh-key`) | read PuTTY `.ppk` → `PrivateKey` | **No separate crate.** `ssh-key 0.7.0-rc.11` (pinned by russh, `ppk` feature on) provides `PrivateKey::from_ppk` — see §7 |
| `zeroize` | 1.9 | wipe secrets from memory on drop | `Zeroizing<String>` for passwords/passphrases |
| `rfd` | 0.17.2 | native file-open dialog | portable; used to pick the key file (0.17, not 0.15) |
| `anyhow` | 1.0 | app-level error handling (`Result<_, anyhow::Error>`) | context-rich errors, `?` everywhere |
| `thiserror` | 1.x | *(deferred)* typed error enums for module boundaries | add when a module becomes a real API |

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
│   ├── FiraMono-Medium.ttf   monospace font (normal weight) embedded in the exe (§9, §11)
│   ├── FiraMono-Bold.ttf     its bold weight, for bold cells (§11)
│   └── FiraMono-LICENSE.txt  the family's OFL 1.1 license (required for redistribution)
└── src/
    ├── main.rs           entry; #![windows_subsystem = "windows"] (inert on macOS); spawns runtime + iced::run
    ├── app.rs            iced App: State, Message, update(), view(), subscription()
    ├── ui/
    │   ├── mod.rs         view helpers; host-key / passphrase / error dialogs (§8, §7, §6)
    │   ├── connect.rs     the connection form (host/port/user/auth/key)
    │   ├── dialog.rs      shared modal-dialog chrome: header (title + ✕) / body / footer (§10)
    │   ├── selection.rs   stream text selection over the grid; text extraction (§10)
    │   └── terminal.rs    render the vt100 Screen grid; pixel→cell resize math (§9)
    ├── ssh/
    │   ├── mod.rs
    │   ├── client.rs      russh Handler impl; connect → auth → shell; the tokio task loop
    │   ├── auth.rs        method selection + attempts (publickey, password)
    │   ├── hostkey.rs     TOFU: check_known_hosts_path, fingerprint, accept/learn
    │   ├── keyfile.rs     load PEM/OpenSSH + PuTTY .ppk (via ssh-key from_ppk); passphrases; zeroize (§7)
    │   └── fixtures/      real .ppk test vectors (Ed25519, plain + encrypted)
    ├── term/
    │   └── mod.rs         vt100::Parser wrapper: feed bytes, expose Screen, handle resize
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
   - known + **mismatch** → **abort** the connection, surface a loud warning (possible
     MITM). No override in v1.
4. **Authenticate (§7)** — attempt in order, stopping on first success:
   - if a key was supplied → `authenticate_publickey`.
   - else / on failure, if a password was supplied → `authenticate_password`.
   - respect the server's advertised methods; report `Authenticating`, then either
     `Connected` or a generic `Error` (no oracle about which field was wrong).
5. **Shell**: `channel_open_session()` → `request_pty(term = "xterm-256color", cols,
   rows, …)` → `request_shell()`. The pty size comes from the current terminal-view
   dimensions.
6. **Stream**: loop — server data arrives on the channel → `SshEvent::Output(bytes)` →
   GUI feeds it to the vt100 parser (§9); user keystrokes arrive as
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
- **Mismatch**: a stored key that no longer matches → treat as hostile (key rotation
  *or* MITM). v1 **refuses to connect** and tells the user to remove the stale entry
  by hand if the change is legitimate. No silent override, no "connect anyway" button
  in v1 (that button is how people get MITM'd).
- **Why not skip it** — accepting any host key (the "just make it work" shortcut) turns
  every connection into a spoofing target. Non-negotiable; never simplified away.

---

## 9. Terminal emulator

Turning a raw byte stream into a screen.

- **Parser**: `vt100::Parser` fed every `SshEvent::Output` chunk. It interprets ANSI
  escapes and maintains a `Screen`: a grid of cells, each with a glyph, fg/bg color,
  and attributes (bold, underline, inverse), plus cursor position.
- **Render** (`ui/terminal.rs`): draw the `Screen` in iced using a **bundled**
  monospace font (**Fira Mono**, embedded in the exe — OFL 1.1). Each row is a `row`
  of **fixed-width boxes**: consecutive same-attribute *narrow* cells coalesce into one
  box (width `n × cell`), and a *wide* cell (CJK/emoji) gets its own box **two** cells
  across. Pinning every box to an exact multiple of the cell width keeps columns aligned
  even when a wide glyph falls back to a system font whose advance we don't control
  (free-flowing text would shift the rest of the line). Bundling the font (rather than
  `Font::MONOSPACE`) makes the grid look identical on every machine and gives an
  **exact** cell advance (600/1000 em = 0.6), which the resize math depends on. Both the
  **Medium (500)** and **Bold (700)** weights are embedded (same Fira Mono release, same
  OFL licence), so a bold cell resolves to a real heavier face; every weight shares the
  0.6 advance, so bold does not disturb the cell metric. `ponytail:` a *narrow* glyph the
  bundled font lacks can still drift within its coalesced box, but the drift is clipped
  at the box edge and resets at the next box, so it never desyncs the whole line — a full
  canvas / GPU atlas stays the escape hatch only if this ever matters.
- **Input**: iced keyboard events → the bytes a terminal sends (printable chars
  direct; Enter → `\r`; Ctrl-C → `0x03`; arrows/Home/End/F-keys → their escape
  sequences). Sent as `SshCommand::Input`. The cursor and Home/End keys honour
  **application cursor mode** (DECCKM — read from `Screen::application_cursor()`):
  when a full-screen app such as vim/less/nano sets it (`ESC[?1h`), `term::keymap`
  emits the **SS3** form (`ESC O A`) instead of the default **CSI** form (`ESC [ A`),
  which is what those apps bind their arrow keys to — without it the arrows are
  ignored and the cursor cannot move (fixed in v1.1.1). PageUp/Down/Insert/Delete
  are `~` sequences DECCKM does not alter, so they are the same in both modes.
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
  clamped ≥ 1×1). On a *change*, `App` resizes the `vt100` parser **and** sends
  `SshCommand::Resize{cols,rows}` so the server reflows (`window_change`). A fresh shell
  fits immediately by fetching the current size once (`window::latest` → `window::size`)
  instead of waiting for the first resize event.
- **Scrollback**: `vt100` keeps a bounded scrollback; expose it read-only in v1.
- **Security note**: rendering untrusted server bytes is safe here — the vt100 parser
  *interprets* escapes into grid state; it never executes anything. We deliberately do
  **not** honor dangerous sequences (e.g. clipboard-write OSC 52) in v1.

---

## 10. UI (iced)

A small state machine drives the single window.

```
enum Screen { Connect, Connecting, ConfirmHostKey, NeedPassphrase, Terminal, Error(String) }
```

- **Connect form** (`Screen::Connect`): text inputs for host, port, user; a radio for
  the auth method (Password **or** Key — a sum type, never both, §7); a "Browse…"
  button (`rfd`) for the key file; a password field for password auth. There is **no**
  passphrase field: a key's passphrase is asked for on its own screen, and only if the
  key turns out to be encrypted (see below). A Connect button; validation fails fast to
  the Error screen (§6.0).
- **Connecting** (`Screen::Connecting`): a status line reflecting the flow steps —
  *connecting → verifying host key → authenticating*.
- **Confirm host key** (`Screen::ConfirmHostKey`): first-contact fingerprint with
  Accept / Reject (§8), in the shared dialog chrome (below). Closing (✕) rejects — the
  safe default, so dismissing never trusts an unverified host.
- **Need passphrase** (`Screen::NeedPassphrase`): shown only when the chosen private
  key is encrypted (§7). A masked field with Unlock / Cancel; the field is auto-focused
  when the screen opens (a `text_input::focus` task keyed to a shared id, refocused on
  every re-ask) so the user can type at once. A wrong passphrase re-shows the prompt
  (the session re-asks, bounded) with an "incorrect" hint — the app tracks whether an
  attempt was already made this connection, since the bridge emits the same
  `NeedPassphrase` for a first ask and a re-ask. The typed text is moved into a `Secret`
  and cleared on submit. This is a local key-file passphrase, not remote auth, so the
  hint is not a credential oracle (§12). The prompt uses the shared dialog chrome (below).
- **Dialogs** (`ui::dialog`, done): the disconnect confirmation, the host-key prompt, the
  passphrase prompt, and the error notice all wear one chrome — a **header bar** with the
  question as a title on the left and a **close ✕** on the right (wired to the safe action:
  cancel / reject / cancel / back, so dismissing is never the destructive choice), a
  **body** explaining what confirming will do, and a **footer** of evenly-spaced buttons.
  A single builder — `dialog(title, on_close, body, footer)` — centres the card in the
  window, so the frame changes in one place and every prompt stays consistent.
- **Terminal** (`Screen::Terminal`, done): a fixed-height status bar in three
  equal-width zones — **Copy / Paste** on the left, the live session's `user@host:port`
  centered, **Disconnect** on the right; the vt100 grid fills the rest, and keyboard
  focus goes there. Disconnect opens a
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
    Paste wrapping/injection safety lives in `term::keymap::encode_paste` (§9).
- **Error** (`Screen::Error`): a generic, non-leaking message plus a "Back" button to
  the form. Detail is logged, not shown (§12).

All state is owned in the iced `State` struct; every transition is a `Message` handled
in `update`. No mutable global state, no `unsafe`.

---

## 11. Portability / config / build

"Portable" is a hard requirement: copy one `.exe`, run it anywhere, leave no trace in
the registry.

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
  on Windows, `target/release/cmote` on macOS.

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
  run `cargo audit` (RustSec advisory DB) + `cargo deny` (licenses + duplicate/banned
  deps) in CI. This is where a Rust app's real risk lives — the dependency tree.
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
    justification recorded here. `cargo deny` bans re-introducing `aws-lc-sys`.
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
- **Terminal** (`term/`): feed byte fixtures (plain text, color SGR, cursor moves,
  clear-screen) → assert the resulting `Screen` grid. Deterministic, no network.
- **Input mapping**: key events → correct byte sequences (Enter, Ctrl-C, arrows).
- **Deferred / manual**: end-to-end connect against a local `sshd` (or a container).
  `ponytail:` no CI SSH server in v1; the manual smoke test is documented in the
  README (password + key + `.ppk` auth, TOFU first-contact, terminal I/O and resize,
  disconnect, and the host-key-mismatch hard stop).

Tests use Rust's built-in `#[test]` / `#[cfg(test)]` — no framework dependency.

---

## 14. Coding conventions — DECIDED: idiomatic Rust

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

## 15. Deferred (with upgrade paths)

- **Saved connection profiles + credential persistence** — encrypt secrets at rest
  with **Windows DPAPI** / the **macOS Keychain** (both user-bound) or an OS keyring;
  adds a real secret-at-rest threat model. v1 is session-only.
- **Multiple sessions / tabs** — the channel-per-session design (§4) already allows it;
  v1 ships one session for simplicity.
- **Broader auth** — `keyboard-interactive` (2FA / OTP prompts), SSH agent / Pageant
  support, certificate auth.
- **More key types for `.ppk`** — the in-house parser (§7) covers RSA + Ed25519 in
  v1; ECDSA support is a follow-up (add the curve handling to `ppk.rs`).
- **SFTP / file transfer, port forwarding (local/remote/dynamic)** — russh supports the
  channels; each is a feature, not a v1 need.
- **Richer terminal** — swap `vt100` for `alacritty_terminal` if we need advanced modes
  / higher throughput; GPU-accelerated glyph rendering if scrolling lags. (Wide-char
  cells now lay out correctly on the `vt100` grid — see the render note in §11.)
- **Clipboard: mouse selection + copy + bracketed paste** — *done (v1.1)*: stream
  selection with copy, and bracketed paste with the injection-terminator scrub (§9-§10).
  Still deferred: honoring remote **OSC 52** clipboard-write requests (kept out on
  purpose — we only touch the clipboard on explicit local action), keyboard shortcuts for
  copy/paste (v1.1 is button- and menu-driven), and rectangular/block selection.
- **Host-key mismatch override UI** — a guarded "the key changed, here's the old vs new
  fingerprint" flow, if ever needed (kept out of v1 on purpose).
- **Code signing + auto-update** — sign the exe (Authenticode) so Win11 SmartScreen
  trusts it, and `codesign` + notarize the macOS binary/`.app` so Gatekeeper allows it;
  add a signed update channel.
- **GNU toolchain build** — only if a fully MSVC-CRT-free static exe is ever required.
- **Apple Silicon (`aarch64-apple-darwin`) build** — the whole stack is
  architecture-agnostic; add the target (and a universal binary via `lipo`) when an ARM
  Mac needs it. v1 targets Intel Sequoia as asked.
