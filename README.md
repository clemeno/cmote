# cmote

A **native, portable Windows 11 SSH client** written in Rust. One window: fill in
host / port / user, pick an auth method (password and/or a private key — PEM or
PuTTY `.ppk`), connect. On success the server hands us a shell and cmote renders a
**full VT terminal** inside the window — a working interactive prompt.

This is a **learning project**. The code is meant to be read as much as run, so it is
written didactically: it favours idiomatic Rust, explains *why* each choice was made,
and marks every deliberate shortcut with a `ponytail:` note so "simple" reads as
intent, not oversight. The full design rationale lives in [PLAN.md](PLAN.md); section
references below (§n) point into it.

## Features

- Single-window connection form: host, port, user, and an auth method.
- **Password** auth, or **private-key** auth with a native file picker (`rfd`).
- Key formats: OpenSSH / PEM (via `russh::keys`) and PuTTY **`.ppk`** (via
  `ssh-key`'s `from_ppk`). Encrypted keys prompt for a passphrase on their own screen.
- **Trust-on-first-use** host-key verification against a portable `known_hosts`:
  first contact shows the fingerprint for explicit accept/reject; a later key change
  is a hard stop, not a warning (§8).
- A full **VT terminal** (`vt100` grid rendered by iced) that reflows to the window
  size, forwarding the new pty size to the remote (§9).
- Session-only credentials, held in memory and `zeroize`d on drop — never written to
  disk (§12).

## Requirements

- **Rust** stable (developed against 1.91.0), target `x86_64-pc-windows-msvc`.
- The **MSVC** toolchain — Visual Studio Build Tools with the VC++ x64 tools and the
  Windows SDK (the default MSVC linker). No NASM or C compiler is required: the SSH
  crypto uses the `ring` backend, which ships pre-generated assembly for this target
  (§2).

## Build and run

```sh
# Debug build and run
cargo run

# Optimized, self-contained portable exe
cargo build --release
# → target/release/cmote.exe
```

The release binary is portable: copy `cmote.exe` anywhere (including a USB stick) and
run it — no installer, no registry writes, no external runtime.

## Data and portability

The only file cmote writes is `known_hosts`. It is resolved at runtime (§11,
`ssh::hostkey::known_hosts_path`):

1. **Portable mode (preferred):** `cmote-data/` beside the executable, when that
   directory is writable. This keeps the host-key store travelling with the exe.
2. **Fallback:** `%LOCALAPPDATA%\cmote\` when the exe sits in a read-only location
   (e.g. `Program Files`).

To reset trust for a host, delete the offending line (or the whole file) from
whichever location is in use.

## Testing

Pure logic is unit-tested; anything needing a live server is manual (§13). No test
framework is pulled in — everything uses Rust's built-in `#[test]` / `#[cfg(test)]`.

```sh
cargo test          # run the unit tests
cargo fmt           # format (rustfmt, hard tabs — see rustfmt.toml)
cargo clippy --all-targets -- -D warnings
```

Automated coverage: key parsing (encrypted/unencrypted OpenSSH, RSA and Ed25519
`.ppk`, unsupported-key error path), host-key match/unknown/mismatch decisions and
fingerprint formatting, terminal byte-stream → grid, key-event → byte-sequence
mapping, and the grid-resize math.

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
port `2222`, user `tester`, choose **Password**, type `testpass`, connect. Expect:

- The **Unknown host key** screen appears once, showing a SHA-256 fingerprint.
  Accept → the shell opens; the fingerprint is now pinned in `known_hosts`.
- Reconnecting no longer prompts (the key matches the pinned one).

**3. Terminal behaviour.** In the shell: run `ls`, `echo hi`, an interactive program
(`top`, then `q`), and **Ctrl-C** to interrupt. Resize the window and run `tput cols;
tput lines` (or `stty size`) — the reported size should track the window. Click
**Disconnect** → you return to the form immediately.

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

**Cleanup:**

```sh
docker rm -f cmote-sshd
rm -f smoke_key smoke_key.pub smoke_key_enc smoke_key_enc.pub
```

## License

MIT — see [LICENSE](LICENSE).
