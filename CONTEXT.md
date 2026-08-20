# cmote

A portable SSH client for Windows 11 and macOS: one window, a full VT terminal, and a file
browser beside it. This file is the glossary — the one place a word is defined. `PLAN.md`'s
numbered sections (`§NN`) are the decision records; cite one here only for depth, never for a
definition.

## Language

### Product

**Target**:
A remembered way to reach one machine — endpoint, auth kind, key path, and where the last
session left off. Never a secret (§14, §22).
_Avoid_: profile, connection profile, saved host

**Endpoint**:
The `user@host:port` string that identifies a target for its whole life. Renaming a target
never changes it.

**Session**:
One SSH connection and every shell running on it (§45).
_Avoid_: channel — a session used to be one channel, and stopped being one when elevation
arrived

**Local session**:
A session against a shell on this machine instead of over SSH (§103). Same shape, no network.

**Account**:
A login on the remote machine: the one the session authenticated as, plus each one elevated
into (§45, §46).

**Identity**:
One shell running as an account, known to the SSH task by number. An account can have several;
each is a channel of its own (§45).

**Elevation**:
Becoming another account on a session that is already logged in — a program (`sudo`, `su`) run on
the connection, not a second authentication (§45). Also what a target REMEMBERS about doing it since
§47: the account, the program, whether to do it on connect, and whether its password is in the vault.
_Avoid_: privilege escalation — the account elevated into need not have more privilege than the one
that logged in

**Shell**:
The remote program a session talks to on one channel.

**Grid**:
The matrix of cells the terminal shows (§9).

**Tab**:
One session, editor or files view, opened in the tab strip (§26).

**Tab strip**:
The row of tabs above the grid.
_Avoid_: strip, on its own

**Browser strip**:
The band under the grid holding the folder tree and the files pane (§18, §19).
_Avoid_: files strip

**Pane**:
One half of the browser strip — the folder tree, or the files pane (§18, §19).
_Avoid_: panel, which is a different thing here — see **Panel**

**Panel**:
A floating surface drawn over everything else: a context menu's card of items, or a dialog's.
Never one of the browser panes, and never the terminal grid. `PANEL_BG` is its dark fill.

**Area**:
A place on screen a tab can be sent to: Main, Right, or Bottom — what a user can point at, which
a `pane_grid` index cannot be. One cut is all there is, so the vocabulary is closed (§48).
_Avoid_: zone, quadrant

**Region**:
One split region of the window: its own tab strip, whichever tab is on screen, and the state
they share (§48). What a user points at is an **Area**; the region is what that resolves to.

**Vault**:
The age-sealed `secrets.age`, and the sole place any secret is stored (§12, §16).

**Forward**:
A tunnel a target carries and re-establishes on reconnect (§27).

**Time zone**:
The zone an mtime is rendered in — the server's own, because the files being listed are its.

**Copy run**:
One copy from its first file to its last, whichever direction it goes (§16, §17, §19). The state
that is the same for every file in it — the resume answer, the total the bar is measured against,
the event channel, the ticker, the cancel flag — is `transfer::CopyRun`; a file's own size is not,
so it stays an argument.
_Avoid_: *transfer* for this, which is the whole feature rather than one run of it.

### Terminal internals

**Engine**:
`alacritty_terminal`'s `Term`, specialised to cmote's reply-collecting listener (§23). The one
name that would change under another engine.

**Screen**:
cmote's read-only view of the engine's grid, so nothing outside `term/` touches the engine
itself (§9).

**Gate**:
The one place that sits between the parser and the engine, standing in the engine's place so
that margins can bound an operation (§102, §107).

**Margins**:
The left and right columns a program walls off with DECSLRM (§102).

**Band**:
The columns between the margins — what a bounded operation may move, and all it may move
(§102).

**Narrowed**:
Margins enabled *and* not sitting at the page edges. A band spanning the whole page is not a
band, so the gate steps aside and the engine keeps the operation (§102).

**Scroll region**:
The rows a program walls off with DECSTBM — DEC's own name for them. cmote mirrors the engine's,
because the engine does not expose it (§102).
_Avoid_: region, unqualified — that is the window's (see **Region**)

**Scanner**:
A reader that watches the byte stream beside the engine for one family of sequences and
reports what it found, without standing in the engine's way (§34, §41).

**Framer**:
The shared machine a scanner uses to find a sequence's payload and its end (§106).

**Interruption**:
One thing `process` must do part-way through a chunk, ordered by the byte offset it sits at —
so a mark and an image land in the order the stream put them (§34, §41, §55).

**Screen spot**:
A position in viewport coordinates: row 0 is the top visible line.

**Doc spot**:
A position in the scrollback document, which survives scrolling (§40).

**Oracle**:
A second engine, built the same way and with no gate, fed the same bytes. Truth that is
measured rather than transcribed (§107).

### How we work

**§**:
A numbered `PLAN.md` section. These are the project's ADRs — there is no `docs/adr/`.

**`ponytail:`**:
A marker on a deliberate shortcut, so that "simple" reads as intent rather than ignorance.

**Green gate**:
`cargo check --all-targets`, then `cargo test`, then
`cargo clippy --all-targets -- -D warnings`, then `cargo fmt --check`. All four, before any
commit.

**Prove-it**:
Breaking the code on purpose to show that a passing test *can* fail, recorded beside the test.
A test that cannot fail reports its area as covered.

**Differential test**:
A test whose expected values come from running the real thing — `vte`, or a bare engine —
rather than from re-deriving them the way the code derives them (§106, §107).
