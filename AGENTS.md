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

Before **any** commit, all five, in this order:

```
rustup update stable
cargo check --all-targets
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

CI (`.github/workflows/ci.yml`) runs the same three that apply to it, plus `cargo deny` and
`cargo audit` for the dependency tree, and repeats both on the mac — but not on one target.
**Clippy is cross-compiled to `x86_64-apple-darwin` and the tests run natively on the aarch64
runner**, which between them compile both slices of the universal bundle (§127). There is
deliberately no second clippy pass for `aarch64-apple-darwin`: the tree contains no `target_arch`,
`target_pointer_width` or `target_endian` `cfg` at all, so the two darwin targets select exactly
the same source. That sentence is the one that stops being true if an arch-conditional ever
appears, and the step is what to add.

**`rustup update` is the first step for a reason (§114).** CI uses `dtolnay/rust-toolchain@stable`,
which floats with the release train; the toolchain here is pinned to whenever it was last updated by
hand. Let those drift and a lint that shipped in stable yesterday turns a green gate into a red push,
on the host target, with no `cfg` involved — which is how §114 happened. Updating first is a no-op
when current and costs seconds when it is not. **CI is deliberately not pinned to a version**: this
tree runs `clippy::pedantic` on purpose and wants new lints the day they ship, so the answer is to
see them locally first, not to freeze the compiler.

**The gate is one target wide, and that is its blind spot.** It runs for the host, so it lints
only the `cfg` the host selects. Every `#[cfg(target_os = "macos")]` and `#[cfg(not(windows))]`
item in the tree is invisible to it — an import used only by a Windows twin, a cfg-paired
function whose macOS arm always answers `Some`, a `#[expect]` that goes unfulfilled over there.
The macOS `cfg` is linted in exactly one place: CI. So the implication runs one way only — a
commit that fails the gate fails CI, but a commit that PASSES the gate can still be red on
`x86_64-apple-darwin`, and nothing local will say so.

Two consequences, both learned the hard way in §113:

* **Read the macOS job.** It is the ONLY reader of half the `cfg` in this repo, and the only reader
  of the tests that run there, so its result is not a formality — it is the only evidence that
  exists. In §113 it had been red since §103 and stayed red for 118 commits across several pushes,
  because a green local gate was being read as "the commit is fine" and the one job that disagreed
  was never opened. **How many tests those are is the job's to report, not this file's**: the set is
  the host's suite minus the `#[cfg(windows)]` ones plus the macOS ones, so no number written here
  can be checked from here. One was: measured at §113, never revisited, and still being read as
  current when §156 swept the docs.
* **A step that goes green reveals the next step; it does not clear the job.** §113 fixed the macOS
  clippy step, and the `cargo test` step behind it — which had never once run on that target — failed
  immediately, on a test broken in the same commit (§115). Both `cargo test` steps now carry
  `if: ${{ !cancelled() }}` so a lint error cannot hide a behaviour failure again, but the habit is
  the real fix: after a red job goes green, read the whole run, not the step you fixed.
* **A platform fact belongs on both arms.** `assert_eq!(x, cfg!(windows))` beats asserting one
  platform's half and hoping (§115). Both arms compile everywhere, so neither can rot unseen, and
  expecting the absence is a real assertion rather than a skipped one.
* **A shared function citing one platform's hazard is a `cfg` pair nobody wrote (§124).** The
  audit of all 71 arms found every pair consistent and one real bug in code with *no* arms:
  `local::path`'s component whitelist refused `\` and `:` everywhere, on two reasons the module note
  states as Windows' own, and on macOS that made a file the panes had listed refuse to open. Reach
  for `cfg!(windows)` — the macro — rather than a `#[cfg]` pair whenever the difference is a value
  and not an API: both arms then compile and lint everywhere, and the test can assert the difference
  as `== cfg!(windows)` instead of asserting one half.
* **A change that touches a `cfg` pair is not verified locally, and cannot be.** Cross-checking
  is impossible on this machine — `ring`'s build script runs `cc` for the target and there is no
  darwin C toolchain here, so `rustup target add x86_64-apple-darwin` gets you a `std` and then
  fails in `cc-rs`. The only check available is reading the sibling arm, then watching CI.

## Building

`cargo build` — debug, about ten seconds. That is the loop; `--release` is for shipping only
(it turns on LTO and one codegen unit, so it is minutes).

If a `cmote.exe` is running it holds a lock on `target/`. Only then, build with
`CARGO_TARGET_DIR=target-verify cargo build` — never as a habit, because it is a full cold
rebuild.

## Line endings

**LF, everywhere.** Enforced by `.gitattributes` (`* text=auto eol=lf`) and by `rustfmt.toml`'s
`newline_style = "Unix"`, so it no longer depends on anybody's `core.autocrlf`. Edit any file
with any tool; there is nothing to preserve and nothing to verify afterwards.

A carriage return in a text file is now a mistake, not a convention. Fonts (`*.ttf`) and the
cursor bitmaps (`*.png`) are marked `binary` and must never be rewritten.

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

- **`#[expect]` is not `allow`, and it is the one escape.** Where a lint's advice cannot be taken —
  the layout boundary in `ui`, where std offers no exact conversion between an integer and an `f32`
  in either direction (§111) — the answer is an `#[expect]` carrying a `reason`, on the smallest item
  that needs it, and never a lint switched off for a file or a crate. Three things make that different
  from an `allow`: the lint stays enabled everywhere else, so the same mistake written elsewhere is
  still a build error; the `reason` is a sentence a reader can disagree with; and `expect` FAILS the
  build if the lint stops firing there, so the escape cannot outlive its cause. If the same `#[expect]`
  starts appearing in several places, that is the signal to give the boundary one home and route
  everything through it, which is what `ui::pixels` / `ui::cells` are.

- **A lint's configuration is not a suppression either.** `clippy.toml` is where a lint is told what
  this project counts as correct — `doc-valid-idents` for proper nouns that are not identifiers (§111).
  Prefer it to an `#[expect]` when the answer is "the lint's default list is wrong for us" rather than
  "this one site is an exception".

## Tests

AAA structure, descriptive names, 80% target on logic; anything needing a live server is manual
(§13). `cargo test` is the number of record.

**Measure before designing the fix.** "This is slow" names a symptom, not a line. Time the parts, in
`--release`, before choosing what to move or rewrite — a throwaway `#[test]` that prints elapsed
milliseconds is enough, and it is deleted afterwards.

§121 is the cautionary case and every number in it was a surprise. A four-second freeze on opening a
file was 98% one call, and the plan going in — move the pure work to a worker thread — addressed
**21 ms of 4005**. The obvious simplification, one paste instead of many, was 33× *worse* than what it
replaced. A `ponytail:` that had estimated an image decode at "a fraction of a second" was three to
five times low. None of that is visible from reading the code, and all of it changed what got built.

The habit generalises past performance: when a sentence in a comment or in PLAN.md states a cost, and
the work depends on that cost, re-measure it rather than inherit it.

**Prove-it discipline.** A test that passes on first run has not been shown to work — it has
been shown to pass. Break the code on purpose, watch the test fail, record what it said, then
restore. A test that cannot fail still reports its area as covered, which is worse than no test
(§106, §107).

**Break the LINE, not the area.** "It went red when I broke something" is not the claim; the claim is
that it goes red when *this guard* is removed. §121 shipped a test named for a late reply arriving at a
closed tab which passed with its guard deleted — a closed tab is caught one line earlier by having no
viewer at all, so the test was true to its own name and no evidence whatsoever for the line beside it.
Probe each new test against the specific line it is there to protect, one at a time. Two guards on
consecutive lines need two probes.

**Never revert with `git checkout --` while a probe is in flight.** It reverts to HEAD, which is the
whole uncommitted change and not the one line just broken. Either commit before probing or put the
line back the way it was taken out. This cost a full re-application of §121's first commit.

**The load stress, and when to reach for it.** A test that waits for something it only *hopes* will
arrive passes on an idle machine and fails on a busy one, so repeating the suite proves nothing —
thirty green runs and a 100% failure rate can be the same test on the same commit. Put the machine
under load instead:

```
for j in $(seq 8); do ( timeout 120 bash -c 'while :; do :; done' & ) ; done
cargo test <name>
```

Manual on purpose, the same category as §13's live-server tests: it depends on the machine, and a CI
job that fails for the weather is a job people learn to ignore. Reach for it whenever a test infers
a state from a *delay* — "it has been quiet for N ms, so the shell must be at a prompt". That one
inference made `a_real_local_shell_answers_ctrl_d_by_leaving` fail 17 times out of 17 (§111), while
the whole suite ran green thirty times in a row beside it.

The distinction worth keeping: a test may wait for an event it will **certainly** get — `local::pty`'s
real-child test waits for an exit and is fine under the same load — and must not wait for one it is
merely expecting.

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
