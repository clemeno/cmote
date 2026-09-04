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

**Phase**:
Which of a session's two parts it is in: dialing while the handshake is in flight, live once a shell
is open (§134). Never runs backwards.
_Avoid_: state, for this — what a target remembers about where the last session left off (§22) is a
different thing entirely

**Local session**:
A session against a shell on this machine instead of over SSH (§103). Same shape, no network.

**Account**:
A login on the remote machine: the one the session authenticated as, plus each one elevated
into (§45, §46).

**Login directory**:
Where the login account's shell stands at its first prompt — the account's own home on the remote
(§160). Asked for once, on a first connection, so the file panes open there instead of at `/`; a
target that remembers a directory of its own is never asked (§22).
_Avoid_: *home*, unqualified — that is the **Home screen**, which is a tab content and has nothing
to do with a machine; and *cwd*, which is where the shell is NOW rather than where it began

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

**Workspace**:
One identity's own view of the machine: its grid, its scrollback, its selection, its find bar (§45).
What makes switching accounts a swap — the identity on screen holds the live one, each parked
identity holds its own.

**Grid**:
The matrix of cells the terminal shows (§9).

**Tab**:
One thing open in the tab strip (§26): a session, or a file open for viewing. What it is showing at
any one moment is its **Tab content**.

**Tab content**:
Which of the four things a tab is showing — the home screen, the connect form, a session, or a
viewer — together with whatever that one thing is holding while it shows (§10, §130, §134).
_Avoid_: screen, for this — that is the terminal's (see **Screen**)

**Home screen**:
The tab content that lists the saved targets, and the one a tab starts on (§14). `HomeScreen` is what
it is DOING while it shows: its right-click menu, its inline rename, its delete confirmation.

**Connect form**:
The tab content that asks for host, port, user and auth kind (§7). `ConnectFlow` is what it is holding
while it shows: the focus ring's stop, and the **Prompt** over it, if there is one.

**Viewer**:
A remote file open on a tab of its own — text (§32) or a picture (§53). Not a session: its
load rides the parent session's channel. Covers both, and only the picture half is read-only: the
text half is the **Editor**, and `viewer.rs` is neither of them but the sliver of "how far has it
read" the two share (§121).

**Editor**:
The text half of a viewer, which WRITES (§32): a buffer, a line-number gutter, a find bar, a theme
per file extension, and a save that persists exactly as opened — BOM and charset included — rather
than converting behind the user's back. `editor.rs` is its model, `ui/editor.rs` its view,
`ssh/edit.rs` its network.
_Avoid_: viewer, for this half alone — a picture is a viewer too and cannot be edited

**Prompt**:
A question the connect form asks — the vault's master passphrase, or a failure notice (§12, §16).
Asked by the FORM, so always either before there is a session or after one has gone.
_Avoid_: prompt for the shell's own, which is a **Prompt mark**; or for an elevation's password
question, which is a **Challenge**

**Challenge**:
A question asked by a session that already exists: the four the handshake can stop on (§7, §8) and the
ones `sudo` puts while elevating (§45). Held by the thing waiting on the answer, which is why
answering one never moves a tab off its content (§134).

**Dressed prompt**:
A shell prompt whose last character is not one of `$`, `#`, `%`, `>` — a theme's arrow, or the
bracket a `[\u@\h \W]` prompt closes on when nobody put a `\$` after it (§157). Read as the end of an
elevation's conversation only when it wears the trailing space a prompt conventionally has and names
no credential, since those characters end ordinary prose too.
_Avoid_: custom prompt — what matters is not that somebody wrote it but that its last character is
ambiguous, which a `$` never is

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

**Contrast floor**:
The 4.5:1 every ink slot of the palette clears against the page, and the rule `palette.rs` is
chosen to satisfy rather than a property it happens to have (§159). A test holds it, so the
sixteen hex values are a consequence and may be retuned — downwards past this, not.
_Avoid_: *accessible* / *WCAG AA* as the name — the floor is borrowed from WCAG, but nothing here
claims the conformance that word implies

**Screen**:
cmote's read-only view of the engine's grid, so nothing outside `term/` touches the engine
itself (§9).
_Avoid_: screen for a page of the window — a tab shows **Tab content**, not a screen

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

**Prompt mark**:
Where a shell says its own prompt begins, announced with OSC 133 (§34). The oldest of the three
things this project called a prompt, and the reason the other two are qualified.
_Avoid_: prompt, unqualified — that is the connect form's (see **Prompt**)

**Held update**:
The frame `vte` is buffering while a program has mode 2026 open — written to the terminal but not
yet on screen (§122). It is *held*, never "synchronized": `sync_alternate` is a different thing
entirely.
_Avoid_: sync, for this

**Watermark**:
How far along the reply buffer has already been encoded seven-bit or eight-bit and must not be
rewritten (§145). A chunk may switch the control form part-way through, so `seal` encodes what has
accumulated and moves the mark past it — an answer formed before the switch keeps the form it was
promised, and the final pass touches only what came after.
_Avoid_: sealed, as a noun — the mark is the thing, `seal` is what moves it

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
`rustup update stable`, then `cargo check --all-targets`, then `cargo test`, then
`cargo clippy --all-targets -- -D warnings`, then `cargo fmt --check`. All five, before any
commit. The update is first, not a courtesy: CI floats with the release train, so a toolchain
left behind turns a lint that shipped yesterday into a red push (§114). `AGENTS.md` says why at
length.

**Prove-it**:
Breaking the code on purpose to show that a passing test *can* fail, recorded beside the test.
A test that cannot fail reports its area as covered.

**Differential test**:
A test whose expected values come from running the real thing — `vte`, or a bare engine —
rather than from re-deriving them the way the code derives them (§106, §107).
