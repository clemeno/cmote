# Changelog

What changed between releases, for someone deciding whether to download one. It is a summary and
not the record: every change here was designed and argued in a numbered section of
[PLAN.md](PLAN.md), which is where the reasoning, the measurements and the refusals live. Section
numbers are given so a line here can be read back to its section.

Versions are bare `MAJOR.MINOR.PATCH`, matching the tags. Pushing a tag is what builds the
binaries and opens a draft release; the date below is filled in when that happens.

## 4.0.0 — unreleased

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
