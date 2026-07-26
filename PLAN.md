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

Status: **shipping — v2.0.0** (v1 feature set complete; v1.3 adds saved connection
targets on a home screen — profiles only, no secrets — plus an optional
key-passphrase field, §14; v1.3.1 fixes numpad number keys sending navigation
instead of their digits, §9; v1.3.2 makes the home screen follow the system
light/dark theme so the target list stays readable, §14; v1.4.0 tracks the remote
working directory and uploads a local file into it over SFTP, §17; **v2.0.0** puts a
2D folder tree of the remote filesystem beside the terminal — browse, jump, rename,
copy paths, §18). The icon grid of files under the terminal — every entry in one
directory, streamed in batches, with rename and download (§19) — is built and unreleased;
it ships under whatever version comes next. Both targets are supported
first-class, and each has a verified toolchain on its host:

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
| Credentials | Secrets **session-only** — held in memory, `zeroize`d on drop, never written to disk (§12). Connection *profiles* (no secret) are saved so the home screen can list targets (§14) |
| Auth order | Offer `publickey` first (if a key is given), then `password`; driven by what the server accepts |
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
- **vt100 over alacritty_terminal** — a real terminal must interpret ANSI escape
  sequences (colors, cursor moves, clears). `vt100` parses a byte stream into a simple
  `Screen` grid of cells we can render directly in iced — small, readable, enough for
  v1. `alacritty_terminal` is more complete but heavier and its API tracks Alacritty's
  needs, not ours. `ponytail:` start with vt100; upgrade path noted in §16.
- **Session-only *secrets*** — the safest secret is the one never persisted. Passwords
  and decrypted keys live only for the session and are wiped with `zeroize`. As of v1.3
  connection *profiles* (host / port / user / auth kind / key path — no secret) ARE
  saved so the home screen can list targets (§14); persisting the secrets themselves,
  encrypted at rest, stays a deliberate later feature (§16), not a v1 gap.

---

## 3. Tech stack + versions (mid-2026)

| Crate | Version | Purpose | Notes |
|---|---|---|---|
| `iced` | 0.14.0 | GUI (Elm architecture, `Task`, `Subscription`) | pure Rust; wgpu/tiny-skia renderer, no web runtime |
| `russh` | 0.62.4 | async SSH client | tokio-based; `client::Handler` trait. **`default-features = false` + `ring`** backend (not the default `aws-lc-rs`, which needs NASM; `ring` builds on both targets — prebuilt asm on Windows, via Xcode CLT `clang` on macOS) |
| `russh::keys` | (with russh) | key loading + `known_hosts` | `load_secret_key`, `decode_secret_key`, `check_known_hosts_path` |
| `russh-sftp` | 2.3.0 | the sftp subsystem, for file upload (§17) | rides russh's `ChannelStream` — a protocol on the existing SSH stack, not a second one. Pure Rust, no C |
| `tokio` | 1.53 | async runtime | features: `rt-multi-thread`, `net`, `io-util`, `fs` (streaming an upload off disk, §17), `sync`, `macros`, `time` |
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
│   ├── FiraMono-LICENSE.txt  the family's OFL 1.1 license (required for redistribution)
│   ├── MaterialIcons-Regular.ttf  the file-type icons the files pane draws with (§19)
│   └── MaterialIcons-LICENSE.txt  its Apache-2.0 license (required for redistribution)
└── src/
    ├── main.rs           entry; #![windows_subsystem = "windows"] (inert on macOS); spawns runtime + iced::run
    ├── app.rs            iced App: State, Message, update(), view(), subscription()
    ├── explorer.rs       the remote folder tree's model: nodes, expansion, path arithmetic (§18)
    ├── files.rs          the files pane's model: one directory, batched listings, icon categories (§19)
    ├── ui/
    │   ├── mod.rs         view helpers; host-key / passphrase / error dialogs (§8, §7, §6)
    │   ├── connect.rs     the connection form (host/port/user/auth/key)
    │   ├── dialog.rs      shared modal-dialog chrome: header (title + ✕) / body / footer (§10)
    │   ├── explorer.rs    the folder-tree panel, its splitter and its context menu (§18)
    │   ├── files.rs       the file icon grid, its splitter and its context menu (§19)
    │   ├── menu.rs        shared right-click menu chrome: panel / items / dismiss layer (§10)
    │   ├── selection.rs   stream text selection over the grid; text extraction (§10)
    │   └── terminal.rs    render the vt100 Screen grid; pixel→cell resize math (§9)
    ├── ssh/
    │   ├── mod.rs         module tree + `open_sftp`, shared by upload, download and browse (§17-§19)
    │   ├── client.rs      russh Handler impl; connect → auth → shell; the tokio task loop
    │   ├── auth.rs        method selection + attempts (publickey, password)
    │   ├── browse.rs      list + rename remote folders and files over sftp, falling back to `ls`/`mv` (§18, §19)
    │   ├── download.rs    file download over an sftp channel: stream, progress (§19)
    │   ├── hostkey.rs     TOFU: check_known_hosts_path, fingerprint, accept/learn
    │   ├── keyfile.rs     load PEM/OpenSSH + PuTTY .ppk (via ssh-key from_ppk); passphrases; zeroize (§7)
    │   ├── upload.rs      file upload over an sftp channel: exists-check, stream, progress (§17)
    │   └── fixtures/      real .ppk test vectors (Ed25519, plain + encrypted)
    ├── term/
    │   ├── mod.rs         vt100::Parser wrapper: feed bytes, expose Screen, handle resize
    │   └── cwd.rs         scan OSC 7 / OSC 9;9 out of the output stream: the remote cwd (§17)
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
  are `~` sequences DECCKM does not alter, so they are the same in both modes. The
  **numpad number keys** (0-9 and the decimal) get special handling (fixed in
  v1.3.1): they mean a digit with NumLock on but navigation with it off, and winit
  reports the *navigation* logical key either way while filling `text` only when a
  digit was typed. iced does not surface NumLock, so `term::keymap::encode` keys off
  the **physical** code plus the presence of `text` — a numpad number key that
  produced text sends that character; otherwise it falls through to the navigation
  mapping. Without this, typing e.g. `pm2` on the numpad emitted arrow keys.
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
  the Error screen (§6.0). **Full keyboard navigation** (§10): iced can only focus text
  inputs, so a bespoke focus ring (`ui::connect::FormStop`) also covers the radios and
  the Connect button. `App::form_focus` tracks the current stop; a Connect-screen
  `keyboard::listen` subscription feeds `FormKey`, where **Tab / Shift+Tab** move the
  stop (`next`/`previous`), **Enter / Space** activate a radio/button stop
  (`activation`), and a text stop takes native focus (`focus(id)`) while a
  radio/button stop unfocuses all (`focus(NO_FOCUS_ID)`) and gets a highlight ring in
  the view. On a text stop, Enter/Space are left to the field.
- **Connecting** (`Screen::Connecting`): a status line reflecting the flow steps —
  *connecting → verifying host key → authenticating*.
- **Confirm host key** (`Screen::ConfirmHostKey`): first-contact fingerprint with
  Accept / Reject (§8), in the shared dialog chrome floating over the dimmed connect form
  (below). Closing (✕) or a backdrop click rejects — the safe default, so dismissing never
  trusts an unverified host.
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
  - **Folder tree beside the grid** (done, v2.0): the right of the screen holds the remote
    folder explorer, with a draggable splitter between it and the grid and a status-bar
    button that hides it. Its width comes out of the grid's own width, so the same
    `grid_size` call reflows the pty for a panel resize and a window resize alike (§18).
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
- **Terminal** (`term/`): feed byte fixtures (plain text, color SGR, cursor moves,
  clear-screen) → assert the resulting `Screen` grid. Deterministic, no network.
- **Input mapping**: key events → correct byte sequences (Enter, Ctrl-C, arrows).
- **Deferred / manual**: end-to-end connect against a local `sshd` (or a container).
  `ponytail:` no CI SSH server in v1; the manual smoke test is documented in the
  README (password + key + `.ppk` auth, TOFU first-contact, terminal I/O and resize,
  disconnect, and the host-key-mismatch hard stop).

Tests use Rust's built-in `#[test]` / `#[cfg(test)]` — no framework dependency.

**CI (done, v1.2 — `.github/workflows/ci.yml`).** Every push and pull request to
`main` runs the same gates the README asks for locally, so `main` stays green on both
targets: `cargo fmt --check` (once, platform-independent), `cargo clippy -D warnings`
+ `cargo test` on **Windows** (native `x86_64-pc-windows-msvc`) and on **macOS**
(clippy cross-compiled against the shipped `x86_64-apple-darwin` target — proving it
builds, ring included — with the tests run natively on the aarch64 runner, valid
because the logic under test is architecture-agnostic, §16), plus the supply-chain
audit (§12). Only the live-SSH end-to-end path stays manual — there is still no CI SSH
server. CI builds no release artifact; publishing the portable binaries stays a manual
step (§16, code signing / release automation).

---

## 14. Saved connection targets (v1.3)

The home screen (`ui/home.rs`) is the landing screen: a list of previously used
connection **targets**, so reconnecting is a click instead of re-typing the form.

- **What persists — profiles only, never secrets (§12).** A target records `name`,
  `host`, `port`, `user`, `auth_kind`, (for key auth) `key_path`, and the panels'
  `show_hidden` preference. No password and no
  key passphrase is ever written. This keeps the §12 "the safest secret is the one never
  persisted" guarantee **and** keeps the store fully portable — a `targets.json` copied
  to another machine leaks nothing. The user still enters the secret on the form each
  time. *(Opt-in, encrypted-at-rest secret persistence — Windows DPAPI / macOS Keychain
  — is deliberately deferred to a later investigation; see §16.)*
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
- **Per-target display preference.** The `.*` toggle shared by the folder tree and the
  files pane (§18, §19) is remembered with the target: whether a server's dotfiles are
  the point or the noise is a property of that server, not of the app. It is applied on
  `Connected` — before the first listing, so nothing flashes — and written back only when
  the toggle actually moves. A `targets.json` written before the field existed defaults
  to *shown*, which is what those installs already did.
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

- **Credential persistence (secrets at rest)** — saved connection *profiles* shipped in
  v1.3 (§14), but only the non-secret metadata. Persisting the password / passphrase
  themselves — encrypted with **Windows DPAPI** / the **macOS Keychain** (both
  user-bound) or an OS keyring, as an opt-in per target — is the remaining piece. It
  adds a real secret-at-rest threat model and, being machine-bound, trades against the
  portable store, so it is a deliberate later investigation.
- **Multiple sessions / tabs** — the channel-per-session design (§4) already allows it;
  v1 ships one session for simplicity.
- **Broader auth** — `keyboard-interactive` (2FA / OTP prompts), SSH agent / Pageant
  support, certificate auth.
- **More key types for `.ppk`** — the in-house parser (§7) covers RSA + Ed25519 in
  v1; ECDSA support is a follow-up (add the curve handling to `ppk.rs`).
- **SFTP / file transfer** — *partly done (v1.4, v2.0, unreleased)*: **upload** of a chosen
  local file into the shell's current directory (§17), a **folder tree** of the remote
  filesystem that browses and renames (§18), and a **files pane** that lists one whole
  directory and **downloads** a file from it (§19). Still deferred: creating and deleting
  remote entries, directory (recursive) transfers, cancelling a transfer in flight,
  resuming an interrupted one, drag-and-drop onto a folder, more than one transfer at a
  time, and preserving file modes/timestamps in either direction.
- **Port forwarding (local/remote/dynamic)** — russh supports the channels; a feature,
  not a v1 need.
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

---

## 17. Remote working directory + file upload (v1.4)

Two features that only make sense together: the status bar gained a **file picker** and
an **Upload** button, and the upload's default destination is *the directory the shell is
currently in*. SSH does not offer that directory, so cmote has to learn it.

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
  `vt100`, which ignores OSC codes it does not know — so nothing is stripped and the two
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
  `cmote — user@host:port — /current/dir` while connected, dropping the directory when the
  shell never announces one. The title costs no grid space, which the status bar would.

### Uploading (`ssh/upload.rs`)

- **Its own channel, not the pty.** The shell channel is for keystrokes: everything sent
  is echoed, binary needs encoding, and the terminal would have to render the transfer.
  The upload therefore opens a **second channel running the sftp subsystem** (`russh-sftp`
  over russh's `ChannelStream`), which is real file transfer — the shell keeps running
  untouched beside it. Opening happens inline in the session loop (a borrow of the session
  handle); the transfer itself is **spawned**, so a large file never stalls the shell pump.
- **Ask before sending, ask before replacing.** Upload opens a confirmation showing the
  local file and an **editable destination path**, prefilled with the tracked cwd joined to
  the file's name — and with the bare name when the cwd is unknown, which the server
  resolves against the login directory. The destination is then checked **before** the file
  is created (SFTP's create truncates, so a late failure would already have destroyed the
  old contents): an occupied path stops the transfer and raises the overwrite prompt
  instead. Both prompts use the shared dialog chrome (§10), and every dismissal route
  cancels.
- **Progress.** The copy loop streams 32 KiB chunks and emits `UploadProgress` every
  256 KiB — enough for a smooth bar, far below the flood that per-chunk events would be.
  The status bar's centre zone becomes a progress bar plus a byte readout while a transfer
  runs, then shows the outcome until the next upload.
- **Failures stay in the bar.** A failed upload shows its reason in the status bar and
  keeps the file selected for a retry; it must not route to the error *screen*, which
  would tear down a perfectly healthy shell over a file that never left. Unlike an auth
  failure (§12), the detail here is the user's own path — showing it is what makes the
  error actionable.
- **Success deselects.** A completed upload clears the picked file, which disables the
  Upload button again, so a stray second click cannot re-send what just landed. The
  reported destination is the server's `canonicalize` of the path, so the user sees where
  the bytes actually went rather than what they typed.
- **Keyboard.** While an upload dialog is open the terminal's key listener swallows keys
  (as it already does for the Disconnect modal), so typing goes to the path field and not
  to the remote shell; `Esc` cancels a confirmation and a running transfer ignores it —
  there is no cancel to give it (deferred, §16).

---

## 18. Remote folder explorer (v2.0)

The headline of v2: a **2D tree of the remote filesystem** to the right of the terminal,
so the far side can be navigated with the mouse instead of `cd` and `ls`. It is split
three ways — a pure model (`explorer.rs`), a pure view (`ui/explorer.rs`), and the
network calls (`ssh/browse.rs`) — which is what keeps the interesting rules (relative
paths, what collapsing does, which folders a `cd` reveals) unit-testable with no server.

### The model (`explorer.rs`)

- **Folders only, POSIX paths.** The tree lists directories, not files: files have no
  action attached to them yet (download is still deferred, §16), and leaving them out
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
  re-opening shows one clean level again — which matches the menu's Expand, which opens
  exactly one level. Nothing is discarded, so re-expanding costs no round trip.
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

- **Layout.** A fixed-width column to the right of the grid with a draggable splitter
  between them, and a status-bar button that hides the whole thing. The panel's width is
  taken out of the grid's, so `grid_size` subtracts `Explorer::reserved` and a splitter
  drag reflows the remote pty exactly as a window resize does — one code path, and a
  round-trip test locks `window_size`/`grid_size` together with the panel included. The
  drag is clamped to 60% of the window, because a splitter with no ceiling can leave the
  terminal one column wide and the user dragging their way back out.
- **Right-click menu**, on the folder under the pointer: *Open in terminal*, *Rename…*,
  *Copy name*, *Copy relative path*, *Copy full path*, *Expand (refresh)*, *Collapse*.
  "Copy relative path" is disabled when the shell has never announced a cwd — there is
  nothing to be relative to. "Expand" force-refetches, which is also the refresh for a
  directory changed from the shell (a `mkdir` typed at the prompt).
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

## 19. Remote files pane (unreleased)

An **icon grid of every entry in one directory**, full width under
the terminal and the folder tree. The tree (§18) answers "where am I in the filesystem";
this answers "what is actually in here". Same three-way split — a pure model
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
  size and the relayout, not the fetch: russh-sftp's `read_dir` runs the whole readdir
  loop before returning. It costs no extra round trips (SFTP sends a name's attributes
  with the name, so there is no per-file stat either way), but it does hold the listing in
  memory once. Upgrade path: drive `RawSftpSession::opendir`/`readdir` and emit a batch
  per protocol packet.
- **Every batch carries its request number.** Leaving a directory bumps the number, so
  batches still in flight for the folder just left are dropped instead of being mixed into
  the new one — the bug this design exists to prevent. Failures carry it too.
- **Sorted by the server task, appended by the model.** Directories first, then everything
  else, each case-insensitively. Sorting once, before the cut into batches, is what lets
  the model simply append and still hold a stable order across batches; sorting per batch
  in the model would either break the order or re-sort the whole listing on every one.
- **Symlinks keep their own kind.** Resolving one costs a round trip *per link*, and a
  crowded directory is exactly where that adds up — so a link gets the link icon and is
  not followed. (The tree does resolve them, §18: it sees far fewer entries and needs to
  know whether the link is expandable.)
- **No sizes, no timestamps.** The grid shows neither, so it asks for neither.

### Two sources for "which directory", last one wins

- The shell's working directory (OSC 7, §17) drives it: `cd` in the terminal and the pane
  follows, exactly as the tree's auto-reveal does.
- A click on a tree row points the pane at that folder **without moving the shell**, which
  is what makes it usable to look inside a folder you are not in.
- The catch those two create: the shell re-announces its directory at *every prompt*, so a
  naive "follow the cwd" would drag the pane back from the tree click on the next
  keystroke. `Files::follow` therefore acts only when the announced directory differs from
  the last one followed — a repeat is not a move — while `Files::show` (the tree click,
  entering a folder) is unconditional.
- Double-clicking a directory in the grid **moves the shell** (`cd`, quoted as §18 does)
  and retargets the pane immediately rather than waiting for the prompt. That keeps one
  "where am I" in the window instead of three.

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
  only), **Download…** (files only), **Rename…**, **Copy name / relative path / full
  path**, **Refresh**. Each inapplicable item is *disabled*, not hidden, so the menu keeps
  one shape.
- **Rename** reuses the tree's rules and the same `RenameDir` command — SFTP's rename does
  not care whether it is moving a directory — with the same guards: no blank name, no `/`
  (that would be a move, not a rename), and a destination that already exists is refused
  rather than replaced. Both panels react to `RenameDone`.
- **Download** is the mirror of the upload (§17): its own sftp channel, its own spawned
  task, progress in the status bar through the shared `TransferProgress`. The destination
  comes from the native **save dialog**, which is also what asks about replacing a local
  file — a second prompt in our own chrome would only be a second chance to answer it
  wrong. One transfer at a time: starting a download while one runs is refused with a
  notice rather than fighting over the one progress bar.
- **The menu opens upwards.** Same frozen-anchor construction as the tree's (§18), but
  bottom-aligned: this pane is at the bottom of the window, so a menu dropping downwards
  would fall off it. `pane height − pointer.y` puts the menu's bottom under the cursor.
  `ponytail:` no clamping at the right edge — the pane is full width, so a menu opened in
  the last ~180 px can run past it. Upgrade path: pass the window width into the view.
- **The `.*` toggle is the tree's.** One flag (`Explorer::show_hidden`) filters both
  panels, and each header carries a checkbox that shows and flips it — so hiding dot-files
  hides them everywhere, and the pane still has the control when the tree is collapsed.
