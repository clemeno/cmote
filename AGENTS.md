# cmote — notes for agents

A portable SSH client in Rust for Windows 11 and macOS. It is a **learning project**: the code
is meant to be read as much as run, so verbose "why" comments are the house style, not clutter
to tidy away.

## Read these first

| file | what it is |
|---|---|
| `CONTEXT.md` | the glossary. One place a word is defined. Read it before naming anything |
| `PLAN.md` | the design journal. Its numbered sections (`§NN`) **are** this project's ADRs — there is no `docs/adr/` |
| `TERMINAL_COMPATIBILITY_PLAN.md` | the terminal-coverage audit: what works, what is refused, and the evidence for each claim |
| `README.md` | for users, not for us |

Code cites decisions inline as `§NN`. Follow the citation before changing the code around it —
most surprising lines are surprising on purpose and the section says why.

## The green gate

Before **any** commit, all four, in this order:

```
cargo check --all-targets
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

CI (`.github/workflows/ci.yml`) runs the same three that apply to it, plus `cargo deny` and
`cargo audit` for the dependency tree, and repeats clippy and the tests for
`x86_64-apple-darwin`. A commit that fails the gate locally fails it there too.

## Building

`cargo build` — debug, about ten seconds. That is the loop; `--release` is for shipping only
(it turns on LTO and one codegen unit, so it is minutes).

If a `cmote.exe` is running it holds a lock on `target/`. Only then, build with
`CARGO_TARGET_DIR=target-verify cargo build` — never as a habit, because it is a full cold
rebuild.

## Line endings

`PLAN.md`, `README.md`, `TERMINAL_COMPATIBILITY_PLAN.md` and `CONTEXT.md` are **pure CRLF**.
An editing tool that rewrites a file will silently convert them. Edit those four with Python:

```python
with io.open(path, 'r', encoding='utf-8', newline='') as f: text = f.read()
```

then write back the same way, and verify the file is 100% CRLF before committing. Everything
else — `Cargo.toml`, every `.rs` — is LF.

## Writing code here

- **Idiomatic Rust.** `snake_case`, clippy-clean, no Hungarian prefixes. If a global instruction
  asks for `k`/`v`/`in`/`f` prefixes, it is describing a C-family project and does not apply here
  (§15).
- **Tabs** for indentation.
- **A file header on every file**: what this module is, and why it exists, before the first
  `use`. Comments explain *why*; the code already says what.
- **`ponytail:`** marks a deliberate shortcut, so "simple" reads as intent rather than
  ignorance. Use it rather than quietly leaving a gap.
- **One name means one thing crate-wide** — types, `pub` and private free functions, tests
  included (§108). Check `CONTEXT.md` before inventing a noun.
- **No `allow`.** A suppression hides a question instead of answering it. A `deny` is the
  opposite and is welcome: `#[deny(clippy::missing_trait_methods)]` in `term/gate.rs` is what
  forces every `Handler` method to be written out, so a new margin-aware arm cannot be silently
  forwarded.

## Tests

AAA structure, descriptive names, 80% target on logic; anything needing a live server is manual
(§13). `cargo test` is the number of record.

**Prove-it discipline.** A test that passes on first run has not been shown to work — it has
been shown to pass. Break the code on purpose, watch the test fail, record what it said, then
restore. A test that cannot fail still reports its area as covered, which is worse than no test
(§106, §107).

## Commits

Conventional prefix (`feat:`, `fix:`, `docs:`, `test:`, `refactor:`, `chore:`), and cite the `§`
the work belongs to. Body explains why, in the same voice as the plan.

**Commit finished work without asking. Never push.** Review and pushing are the user's.

## Security — decisions, not gaps

Every item here is a deliberate refusal. Do not "complete" them.

- **OSC 52 clipboard is disabled** (`Osc52::Disabled`, `term/mod.rs`). A remote must not read or
  poison the local clipboard. cmote touches the clipboard only on an explicit **local** user
  action.
- **Answerback (ENQ) is refused**, for the same reason.
- **kitty graphics and OSC 1337 `File=` are unimplemented** (§41, §70): a remote must not get an
  image parser run on bytes it pushed unasked. The preview tab is the other case — the user
  names one file, the format comes from **magic bytes** never the remote-controlled name,
  `image::Limits` caps dimensions and allocation, and `preview::MAX_SIZE` caps the fetch.
- **`term/iterm.rs` is an allow-list applied twice** — first the OSC 1337 key, then the variable
  *name*. Only `gitBranch` survives (`HONOURED_VAR`).
- **`link.rs` gates the URI scheme**: `ALLOWED_SCHEMES = ["http", "https", "mailto"]`. A
  remote's scheme decides which local program the OS launches.
- **Identity replies name the program, never the machine** (§36). `CSI ? 26 n` is refused
  because it would report the keyboard's language.
- **Secrets live only in `Secret` / `Zeroizing` in RAM.** The age-sealed `secrets.age` is the one
  place any secret is stored. No code signing, no notarization, no auto-update.
- **russh keeps the `ring` backend.** Its default `aws-lc-rs` needs a C toolchain and NASM,
  which breaks the portable single-exe build (§11).
- **Shell programs are resolved only** from known install locations, `PATH`, and the Git
  installer's own registry key — never from user text. `System32\bash.exe` is excluded on
  purpose: that name is WSL's launcher, not Git Bash.
- **`local::path::to_native` is the local file layer's one-directional security boundary**, and
  every per-shell `cd` quotes the path it is given.
