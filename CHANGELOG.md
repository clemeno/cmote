# Changelog

What changed between releases, for someone deciding whether to download one. It is a summary and
not the record: every change here was designed and argued in a numbered section of
[PLAN.md](PLAN.md), which is where the reasoning, the measurements and the refusals live. Section
numbers are given so a line here can be read back to its section.

Versions are bare `MAJOR.MINOR.PATCH`, matching the tags. Pushing a tag is what builds the
binaries and opens a draft release, so the date on each heading is written in the release commit
itself — the one the tag will point at — rather than added afterwards. 4.0.0 is why: it said
"unreleased" in the tree its own tag names, because filling the date in later is a step that
happens after the only moment anyone is looking.

## 4.0.2 — 2026-09-11

### Fixed

- **Deleting a remote folder works (§173).** It did not, on any conformant server: choosing
  **Delete…** on a folder failed with "no such file" and removed nothing. A server lists `.` and
  `..` inside every directory, the delete walk took both for real children, and `..` is the parent —
  so the walk climbed out of the folder it had been asked to empty and kept going until a path grew
  too long for the server to answer.

  **Worth knowing if you used it:** the failure was also what protected you. Nothing is removed
  until the walk finishes, and that walk could not finish, so no delete ever got as far as removing
  the wrong thing. But it had already gathered names from the folders *above* the one you picked,
  and those are the names the removal step would have unlinked. On a server where the climb had
  ended instead of erroring, a delete could have taken a sibling folder with it. Nothing of the sort
  has been reported, and this closes the path to it.

### Under the hood

- A drop onto the files pane now records itself in a debug build's console, with how many files and
  folders it held and what was decided — the other half of the timeline the 4.0.1 session-ending
  diagnostics started. Nothing visible in a release build, which has no console, and the
  drag-and-drop disconnect it was added for is still open and still unreproduced.

## 4.0.1 — 2026-09-11

Eight commits on from 4.0.0, and a point release rather than a feature one: a dependency sweep
(§172) that asked all 21 direct dependencies for their newest version, plus the diagnostics behind
one open bug report. Two of the three things here are speed, and both are latency rather than
throughput — so the further away the machine you are working on, the more of it you get.

### Speed

- **Typing echoes sooner (§172).** Nagle's algorithm had been on for every session since 1.0: it
  holds a small write back until the previous one is acknowledged, so as to coalesce it with
  whatever comes next. On a terminal the small writes *are* the keystrokes, and they have nothing
  to coalesce with — so each one waited for a round-trip that bought nothing. `TCP_NODELAY` is set
  now, which is what OpenSSH and PuTTY both do for an interactive session.
- **Downloads and remote file opens stop waiting between chunks (§172).** Reading a remote file
  used to be one 32 KiB request, a full round-trip of waiting, then the next — so a download could
  never go faster than **32 KiB per round-trip**, no matter how much bandwidth was there. Two things
  changed with russh-sftp 3.0: the request size is no longer whatever buffer cmote happened to pass
  (32 KiB) but what the connection allows (about 256 KiB), and sixteen of them are in flight at
  once. The ceiling becomes roughly **4 MiB per round-trip** — on a 50 ms link, about 0.65 MB/s
  before and about 84 MB/s now, which is to say the download is finally limited by the connection
  instead of by the waiting. Uploads gain less: they were already pipelined, eight deep, now sixteen.

Where the two land differs, so it is worth being exact rather than calling both of them "faster".

The download ceilings above are **arithmetic, not a benchmark** — request size times depth, divided
by the round-trip. A real transfer also pays for the disk, the cipher and the server, so treat them
as the limit that was removed rather than a speed promised. Nothing here was timed against a real
host. But the removal is large enough that it applies on a fast network too: even at 1 ms, the old
32 KiB-per-round-trip ceiling was about 33 MB/s, already under gigabit.

The typing change is smaller than it sounds, and in the other direction. Nagle only holds a write
back while an earlier one is still unacknowledged, so an isolated keystroke after a pause was never
delayed — what was delayed is typing *faster* than the round-trip, which then reached the far end in
clumps instead of one character at a time. So this is about echo becoming even rather than becoming
quicker, it grows with distance, and on a LAN there is nothing there to win.

### Fixed

- **A tab that ends its own session now says why (§171 follow-up).** A drag-and-drop upload has been
  reported as sending a tab back to the target list, which only happens when the session genuinely
  ends — and all five ways that can happen arrived at the screen as the same silent event, including
  the two that mean opposite things ("you asked to disconnect" and "the command channel closed under
  you"). Each now names itself. **This changes nothing you can see:** the release binary has no
  console, so the reasons are readable only in a debug build, and the bug itself is still open.

### Under the hood

- 43 dependency updates, no requirement changed. russh-sftp moved a major version and needed no
  code change at all; `age` and `base64` were each re-tested and deliberately held, with the reasons
  rewritten because both had gone stale (§172).
- Nothing about your files changed. The vault stayed on age 0.11, so `secrets.age` keeps its exact
  format — saved credentials open with the same master passphrase, with no migration and nothing to
  re-enter. `known_hosts`, `targets.json` and the settings file are untouched.

## 4.0.0 — 2026-09-09

140 sections and 375 commits on from 3.0.0. The headline is that cmote stopped being only an SSH
client — it opens a **local** shell too — and that the terminal itself went from "good enough" to
audited line by line against the specifications.

### A shell without a connection

- **Local sessions (§103).** A shell on this machine in a cmote tab, with the same grid, the same
  find bar, the same file panes and the same transfers — no SSH, no server. The shells offered are
  discovered from known install locations, `PATH` and the Git installer's registry key (§104, §105),
  never from typed text.

### More than one account on one connection

- **Elevation (§45, §46, §47).** A session can act as another account — the file panes follow the
  account switched to, and getting back in is its own guarded path. The rules are security
  decisions, written down: a secret goes only to a channel that has just asked for one, a sudo
  password only after sudo has been *observed* to want it, and an account that took more than one
  factor is denied file access up front.

### The window

- **Split it (§48, §52).** Two areas side by side or stacked, and a tab can be sent to the other.
- **The strip is yours (§38).** Drag to rearrange; a file tab sits beside the session it came from.
- **Scrollbars became controls (§116, §118, §125)** — one per window, draggable, with the find
  matches washed onto them.
- **Closing a tab returns you where you were (§37)**, the window size is remembered app-wide (§31),
  and the keyboard follows whatever you last acted on (§50).
- **A hand over everything you can pick up (§51, §119, §120)** — Windows ships no hand cursor, so
  cmote draws its own.
- **Filtering the target list (§49)**, and opening a file no longer holds the window (§121).

### Files

- **A remote text editor in a tab (§32)** — open, edit, save back over SFTP.
- **A picture opens as a picture (§53)** — PNG, JPEG, GIF, BMP and WebP, with the format read from
  the file's own leading bytes and never from the name a remote chose.
- **Transfers cancel and resume (§16, §17)**, keep the source's timestamps, and resume *across a
  reconnect*. A recursive walk follows symlinks but never into a cycle.
- **Delete takes a folder with its whole subtree (§18)**, behind a confirmation that names what
  goes.
- **The panes open where the shell is standing (§160)** rather than at `/`.
- **Crowded folders arrived (§166, §167, §168).** A folder of 237,173 entries used to build 1.9
  million widgets to show sixty; the grid is virtualised now. A tree listing that took **334
  seconds** takes **8.7** — four separate causes, each measured before it was fixed.

### The terminal

- **Shell integration (§17, §34, §95, §96, §164).** cmote can install the `OSC 7` + `OSC 133` block
  into a remote shell's own config, from a dialog that shows the exact text first and never types
  anything at the prompt. With it: the directory in the title, **Sync** and **Reveal**, a tick in
  the gutter beside every prompt, jump-to-prompt, select-a-command's-output, and a per-tab
  status dot.
- **Find in the scrollback (§35, §39, §44, §138)** — every match on screen washed at once, keeping
  up with live output, and invertible.
- **Selection that means text (§40, §42)** — document lines rather than grid rows, so it survives
  scrolling; double click takes a word, triple takes the whole logical line across a wrap.
- **Inline images (§41)** — sixel pictures composited into the scrollback.
- **A remote command reports its progress (§54)** to the tab and to the Windows taskbar.
- **Hyperlinks (§92)**, the font is the user's (§91), the tab has a name a program cannot overwrite
  (§69), and a script can bookmark a line (§55).
- **The compatibility programme (§56–§99, §140–§165).** Every CSI, OSC, DCS and mode row read
  against DEC's own manuals, xterm's `ctlseqs` and each proposal's source, and answered: supported,
  refused by cmote's own code, or refused in the same spirit as the engine. Rectangular editing,
  character sets, 7/8-bit control replies, double-height lines, reverse wrap, horizontal scrolling,
  synchronized output, the locator protocol, XTSAVE/XTRESTORE and the mouse are all in it. The
  audit is [TERMINAL_COMPATIBILITY_PLAN.md](TERMINAL_COMPATIBILITY_PLAN.md), and the refusals are
  as carefully argued as the features.

### Authentication

- **Certificate auth (§7, §16), file-based.** An OpenSSH user certificate presented beside a key,
  auto-filled from the `<key>-cert.pub` sibling, remembered as public metadata and never in the
  secret vault.
- **A vault bug worth naming (§16).** A failed authentication used to leave a captured password
  behind, and a later connect with "Remember" *off* could store the earlier host's password under
  the earlier endpoint. Fixed at both ends, and locked by tests on every path that ends an attempt.
- **Host certificates are refused (§142)** — trust is TOFU on the host key, and a CA-signed host
  key is not honoured.

### Platform and build

- **macOS ships (§127)** as a universal `cmote.app` — one bundle, both slices, Finder-launchable.
- **A release pipeline (§16).** A version tag builds both targets, checksums them into
  `SHA256SUMS`, and opens a *draft* release. The artifacts are deliberately **not** code-signed or
  notarized, and will not be — so `SHA256SUMS` is the integrity check, not a stand-in for one.
- **`clippy::pedantic`, without a single `allow` (§111).** Every escape is an `#[expect]` with a
  reason, so a suppression that stops being needed becomes an error.
- **The 14,800-line file is gone (§126, §128–§134)**, split by who owns the state rather than by
  what the code looks like.
- **`cargo doc` joined the green gate (§171)** after being found broken — eighteen errors nothing
  else was running.

### Earlier releases

1.x through 3.0.0 predate this file. Their sections are in [PLAN.md](PLAN.md) — the numbered record
runs from §1, and each feature's section says which version it shipped in.
