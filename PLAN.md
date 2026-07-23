# cmote — Design Plan

A **native, portable Windows 11 SSH client** written in Rust. A single window: fill
in host / port / user, choose an auth method (password and/or a private key — PEM or
PuTTY `.ppk`), connect. On success the SSH server gives us a shell and we render a
**full VT terminal** inside the window — a working interactive prompt.

This is a **learning project**: the code is meant to be read as much as run, so this
plan is didactic. It explains *why* each choice was made (idiomatic Rust, async,
security) and marks every deliberate shortcut with a `ponytail:` note so "simple"
reads as intent, not ignorance.

Status: **design only — nothing built yet**. The Rust toolchain is verified working
on this machine (`rustc`/`cargo` 1.91.0, `x86_64-pc-windows-msvc`, VS 2019 BuildTools
VC x64 tools present; a hello-world compiled, linked, and ran). This document is the
reference to build against.

---

## 1. Locked decisions

| Area | Decision |
|---|---|
| Language | Rust, stable channel (1.91.0 verified) |
| Target | `x86_64-pc-windows-msvc` (native Win11, MSVC linker) |
| Distribution | **Portable**: one self-contained `.exe`, no installer, no registry writes, no external runtime |
| GUI | **iced 0.14** — pure-Rust, Elm architecture (state / `Message` / `update` / `view`) |
| SSH | **russh 0.62** — pure-Rust async SSH client (no C deps → clean static build) |
| Async runtime | **tokio** (multi-thread) on a background thread; bridged to the GUI by channels |
| Terminal | **Full VT emulator** — `vt100` maintains the screen grid; iced renders the cells |
| Key formats | OpenSSH / PEM native via `russh::keys`; **PuTTY `.ppk` converted** with `puttykeys` |
| Host key | **TOFU** (trust-on-first-use) against a portable `known_hosts`; explicit user accept; mismatch = hard stop |
| Credentials | **Session-only** — held in memory, `zeroize`d on drop, never written to disk |
| Auth order | Offer `publickey` first (if a key is given), then `password`; driven by what the server accepts |
| File picker | `rfd` — native Windows open-file dialog for the key file |
| Errors | `anyhow` at the app boundary; typed `thiserror` enums deferred until a real API needs them |
| Config location | `known_hosts` beside the exe (`./cmote-data/`), falling back to `%LOCALAPPDATA%\cmote` if that dir is read-only |

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
  no external SSH library. See §3 / Cargo.toml.)*
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
| `russh` | 0.62.4 | async SSH client | tokio-based; `client::Handler` trait. **`default-features = false` + `ring`** backend (not the default `aws-lc-rs`, which needs NASM) |
| `russh::keys` | (with russh) | key loading + `known_hosts` | `load_secret_key`, `decode_secret_key`, `check_known_hosts_path` |
| `tokio` | 1.53 | async runtime | features: `rt-multi-thread`, `net`, `io-util`, `sync`, `macros`, `time` |
| `vt100` | 0.16.2 | VT/ANSI screen parser | feeds bytes → `Screen` grid of cells (0.16, not 0.15 — latest on crates.io) |
| *(ppk crate)* | **TBD** | convert `.ppk` → OpenSSH | **`puttykeys` from §7 is NOT published on crates.io.** Decision pending — see §7 |
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
└── src/
    ├── main.rs           entry; #![windows_subsystem = "windows"]; spawns runtime + iced::run
    ├── app.rs            iced App: State, Message, update(), view(), subscription()
    ├── ui/
    │   ├── mod.rs
    │   ├── connect.rs     the connection form (host/port/user/auth/key/passphrase)
    │   └── terminal.rs    render the vt100 Screen grid as iced widgets
    ├── ssh/
    │   ├── mod.rs
    │   ├── client.rs      russh Handler impl; connect → auth → shell; the tokio task loop
    │   ├── auth.rs        method selection + attempts (publickey, password)
    │   ├── hostkey.rs     TOFU: check_known_hosts_path, fingerprint, accept/learn
    │   ├── keyfile.rs     load PEM/OpenSSH; passphrase handling; zeroize; dispatch .ppk
    │   └── ppk.rs         in-house PuTTY .ppk parser: header, MAC, KDF, decrypt (§7)
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
- **PuTTY `.ppk` (in-house parser — DECIDED)** — **no SSH library reads `.ppk`
  natively** and **no usable crate exists** (the `puttykeys` name originally planned
  is not published on crates.io; the only PuTTY crates wrap the C `ssh2` lib or are
  OpenSSH-only). Decision: **we parse `.ppk` ourselves** in `ssh/keyfile/ppk.rs`.
  This is the most didactic path (binary-format parsing, MAC verification, KDF) and
  keeps the build pure-Rust and portable. Flow:
  1. Read the text container: header line (`PuTTY-User-Key-File-2` or `-3`), key
     algorithm, `Public-Lines` (base64), `Private-Lines` (base64), `Private-MAC`.
  2. **Verify the MAC** (HMAC-SHA-256 for v3; HMAC-SHA-1 for v2) before trusting any
     bytes — reject a tampered file.
  3. If encrypted (`Encryption: aes256-cbc`), derive the key — **Argon2** for v3
     (params are in the header), a SHA-1 construction for v2 — from a `Zeroizing`
     passphrase, then AES-256-CBC decrypt the private blob.
  4. Re-encode the inner key into an OpenSSH private key string and hand it to
     `russh::keys::decode_secret_key(openssh_str, None)` for the actual crypto.
  - **Scope for v1:** **RSA and Ed25519** inner keys (the common cases). ECDSA `.ppk`
    is detected and reported with a clear "unsupported key type — export as OpenSSH
    in PuTTYgen" message rather than failing cryptically. Broader types → §15.
  - Passphrase, derived key, decrypted bytes, and the re-encoded OpenSSH string are
    all secret → held in `Zeroizing`, never logged, never written to disk.
  - Pure-Rust deps for this (RustCrypto): `base64`, `hmac`, `sha1`, `sha2`, `aes`,
    `cbc`, `argon2`. Added when `ppk.rs` is implemented.

`ponytail:` v1 targets the two common inner-key types (RSA, Ed25519) and the current
PPK v2/v3 containers; anything exotic gets a clear error, not a silent failure.
Broader key-type support noted in §15.

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
- **Render** (`ui/terminal.rs`): draw the `Screen` in iced using a **monospace** font,
  one styled span per run of same-attribute cells. `ponytail:` a straightforward
  cell/row render first; only reach for a custom canvas or GPU glyph atlas if scrolling
  large output actually lags.
- **Input**: iced keyboard events → the bytes a terminal sends (printable chars
  direct; Enter → `\r`; Ctrl-C → `0x03`; arrows/Home/End/F-keys → their escape
  sequences). Sent as `SshCommand::Input`.
- **Resize**: when the terminal view's cell dimensions change, resize the `vt100`
  parser *and* send `SshCommand::Resize{cols,rows}` so the server reflows (`window_change`).
- **Scrollback**: `vt100` keeps a bounded scrollback; expose it read-only in v1.
- **Security note**: rendering untrusted server bytes is safe here — the vt100 parser
  *interprets* escapes into grid state; it never executes anything. We deliberately do
  **not** honor dangerous sequences (e.g. clipboard-write OSC 52) in v1.

---

## 10. UI (iced)

A small state machine drives the single window.

```
enum Screen { Connect, Connecting, Terminal, Error(String) }
```

- **Connect form** (`Screen::Connect`): text inputs for host, port, user; a radio /
  segmented control for auth method (Password / Key / Both); a "Browse…" button
  (`rfd`) for the key file; a passphrase field (shown when a key is chosen or when the
  backend requests it); a Connect button. Live validation disables Connect until
  inputs are sane (§6.0).
- **Connecting** (`Screen::Connecting`): a status line reflecting the flow steps —
  *connecting → verifying host key → authenticating*. If a host key needs accepting,
  an inline panel shows the fingerprint with Accept / Reject.
- **Terminal** (`Screen::Terminal`): the vt100 grid fills the window; keyboard focus
  goes here; a thin status bar shows user@host and a Disconnect button.
- **Error** (`Screen::Error`): a generic, non-leaking message plus a "Back" button to
  the form. Detail is logged, not shown (§12).

All state is owned in the iced `State` struct; every transition is a `Message` handled
in `update`. No mutable global state, no `unsafe`.

---

## 11. Portability / config / build

"Portable" is a hard requirement: copy one `.exe`, run it anywhere, leave no trace in
the registry.

- **No console window**: `#![windows_subsystem = "windows"]` in `main.rs` so launching
  the exe doesn't pop a black cmd window (we render our own terminal).
- **Config path resolution** (in this order):
  1. `./cmote-data/` next to the executable (`std::env::current_exe()`), if writable —
     true portable mode (USB stick, any folder).
  2. else `%LOCALAPPDATA%\cmote\` — fallback when the exe sits in a read-only location
     (e.g. `Program Files`).
  `ponytail:` plain `std` for path + a write-probe; no `directories` crate needed for
  two paths.
- **Only file written**: `known_hosts`. No secrets on disk in v1 (§1, §12).
- **Release profile** (`Cargo.toml`): `opt-level = "z"` or `3`, `lto = true`,
  `codegen-units = 1`, `strip = true`, `panic = "abort"` — smaller, faster, single
  self-contained exe (the MSVC CRT links statically enough for portability on Win11).
- **Build/run**: `cargo run` (dev), `cargo build --release` → `target/release/cmote.exe`.

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
  - **No C/C++ is compiled during our build** — `cc`, `cmake`, `bindgen`, `nasm`,
    `pkg-config` are all absent from the build-dependency tree. The build uses only
    `cargo` + `rustc`; no C toolchain is invoked.
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
  `ponytail:` no CI SSH server in v1; document the manual smoke test in the README.

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
  with **Windows DPAPI** (user-bound, portable-friendly) or an OS keyring; adds a real
  secret-at-rest threat model. v1 is session-only.
- **Multiple sessions / tabs** — the channel-per-session design (§4) already allows it;
  v1 ships one session for simplicity.
- **Broader auth** — `keyboard-interactive` (2FA / OTP prompts), SSH agent / Pageant
  support, certificate auth.
- **More key types for `.ppk`** — the in-house parser (§7) covers RSA + Ed25519 in
  v1; ECDSA support is a follow-up (add the curve handling to `ppk.rs`).
- **SFTP / file transfer, port forwarding (local/remote/dynamic)** — russh supports the
  channels; each is a feature, not a v1 need.
- **Richer terminal** — swap `vt100` for `alacritty_terminal` if we need wide-char /
  advanced modes / higher throughput; GPU-accelerated glyph rendering if scrolling lags.
- **Bracketed paste + clipboard integration** — with the safety review that OSC 52
  and paste injection require.
- **Host-key mismatch override UI** — a guarded "the key changed, here's the old vs new
  fingerprint" flow, if ever needed (kept out of v1 on purpose).
- **Code signing + auto-update** — sign the exe (Authenticode) so Win11 SmartScreen
  trusts it; add a signed update channel.
- **GNU toolchain build** — only if a fully MSVC-CRT-free static exe is ever required.
