# cmote

[![CI](https://github.com/clemeno/cmote/actions/workflows/ci.yml/badge.svg)](https://github.com/clemeno/cmote/actions/workflows/ci.yml)

A **native, portable SSH client for Windows 11 and macOS** written in Rust. A home
screen lists your saved connection targets; pick one (or start a new connection), fill
in host / port / user, pick an auth method (password or a private key — PEM or PuTTY
`.ppk`), connect. On success the server hands us a shell and cmote renders a **full VT
terminal** inside the window — a working interactive prompt, with the remote working
directory in the title bar and one-click file upload into it.

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
- A full **VT terminal** (`vt100` grid rendered by iced) that reflows to the window
  size, forwarding the new pty size to the remote (§9).
- **Mouse text selection** (drag to select, highlighted in place) with **Copy** and
  **Paste** — from the status-bar buttons or a right-click menu. Paste is
  **bracketed-paste** aware and strips the paste-injection terminator (§9-§10).
- **File upload into the shell's current directory** — pick a local file with **File…**,
  send it with **Upload**, and it goes over **SFTP** (its own channel, so the shell keeps
  running) into the directory the remote shell is sitting in. The destination is shown
  before anything is sent and is editable; an existing file is never replaced without a
  second confirmation. A progress bar with the byte count runs in the status bar, and a
  finished upload deselects the file (§17).
- **The remote working directory in the window title** — cmote reads the `OSC 7` /
  `OSC 9;9` sequences shells emit on each prompt, so the title follows `cd` on POSIX *and*
  Windows remotes. bash and zsh are hooked up automatically when the shell opens; fish and
  Windows Terminal-style prompts already announce it themselves (§17).
- **Consistent dialogs** — the delete-target, disconnect, upload and overwrite
  confirmations, the host-key prompt, the passphrase prompt, and the error notice share
  one chrome: a header bar (question on the left, close ✕
  on the right, wired to the safe action), an explanatory body, and evenly-spaced footer
  buttons. Each **floats over the page it belongs to** (the connect-flow dialogs over the
  connect form, the disconnect modal over the shell) behind a dim backdrop; clicking the
  card never dismisses it (only a click outside does); the body message is **selectable and
  copyable** — drag to select, `Ctrl+C` to copy (handy for the host-key fingerprint or an
  error message); and the dialog is **draggable** by its header, clamped to the window (§10).
- Session-only **secrets** — passwords and key passphrases are held in memory and
  `zeroize`d on drop, never written to disk (§12). Only non-secret connection *profiles*
  are persisted, for the home list (§14).

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
connection profiles — **no secrets**). Both live in the same directory, resolved at
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
mapping (including application-cursor-mode arrow keys, CSI vs SS3), the grid-resize
math, mouse-selection geometry and text extraction (wide
glyphs, trailing-blank trimming, multi-row joins), paste encoding (bracketed-paste
wrapping and the injection-terminator scrub), and the remote-cwd scanner (OSC 7 and
OSC 9;9, split across chunks, percent-escapes, Windows paths, oversized payloads).

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
than the normal one (both weights are bundled — §11). Print wide glyphs over aligned
columns (e.g. `printf '12\n世b\n'`) and confirm the character after a CJK/emoji glyph
stays in its column — a wide glyph reserves two cells (§11). Resize the window and run
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
the title should follow within a prompt. Then:

- Click **File…**, pick a local file — its name appears next to the buttons and **Upload**
  becomes enabled. Click **Upload**: the dialog shows the file and a destination path
  ending in `/tmp/<name>`. Confirm → a progress bar with the byte count runs in the status
  bar, then `Uploaded to /tmp/<name>`, and the file is deselected (Upload disabled again).
  `ls -l /tmp` on the remote should show it, byte-for-byte identical (`sha256sum` both
  ends for a binary file).
- Upload the **same file again** → the **Replace the file on the server?** prompt appears
  and Cancel leaves the remote file untouched (check its `mtime`); Replace overwrites it.
- Edit the destination in the dialog to a directory you cannot write (`/etc/x`) → the
  status bar shows the failure, the shell stays open, and the file stays selected.
- Start a shell that does **not** announce its directory (`docker exec … sh`, or unset the
  hook with `unset PROMPT_COMMAND; unset -f cmote_cwd`) → the title drops the directory
  and the upload dialog offers the bare file name, which lands in the login directory.

**8. Full-screen apps (arrow keys).** Run `vim` (or `less` on a long file). The file
should render, and the **arrow keys** should move the cursor — this exercises
application cursor mode (DECCKM): the app enables it and cmote switches its arrow keys
to the SS3 form so they register. In `vim`, `:q!` to exit.

**Cleanup:**

```sh
docker rm -f cmote-sshd
rm -f smoke_key smoke_key.pub smoke_key_enc smoke_key_enc.pub
```

## License

MIT — see [LICENSE](LICENSE).
