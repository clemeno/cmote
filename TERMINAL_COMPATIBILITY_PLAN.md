# cmote — Terminal Compatibility Plan

A reference inventory of **what cmote's terminal still lacks to render or drive any
documented remote-terminal-application UX**, and what each remaining gap would cost. Every
entry is a sequence specified in an official or widely-adopted source — nothing here is
speculative.

**Rewritten 2026-07-29, after the §23 engine swap.** The baseline is now
**`alacritty_terminal` 0.26** — a full VT implementation — replacing the old **`vt100`
0.16.2** subset. The previous edition of this document was written against `vt100` and framed
every gap as a `[bolt-on]` / `[engine]` split, because that crate was a deliberately small VT
subset that could neither parse nor represent most of the spec. **That ceiling is gone.** What
remains is a much smaller, concrete set of genuine gaps, grouped below by where the work lives.

**State as of §42.** Every *engine-independent* item this document ever listed is now either shipped
or refused with its reason recorded: input (§2), query→reply (§3) and the rendering/attribute layer
(§4) are closed. §6 holds what cmote refuses on purpose. §36 also *corrected* this document: blink was
listed as cmote's policy choice when in fact the engine drops the attribute entirely. **§39 touched this
surface without moving a row** — the find bar's match washes are a local highlight, not a sequence
answered; see the note in §4. **§40 likewise moves no row**: it changed the *coordinate space* the
selection and the copy path work in (viewport rows → absolute document lines), which is a cmote-side
refactor of reads the engine already served; see the note in §4 and the `term/screen.rs` evidence.

**§41 is the first change in a long while that DOES move rows — and it corrects this document's biggest
standing claim.** §5 and §7 said the last item of real UX value, graphics, needed *both* an engine fork
and a renderer compositor. The compositor half was true. The fork half was not: the engine's DCS hooks
are no-op debug logs, so a sixel payload is already followed to its terminator and dropped, and the
sequence can be scanned out of the same byte stream that reaches the engine — the tactic §17, §9, §33 and
§34 all use. So **cmote now draws sixel images** (`term/sixel.rs`, `term/graphics.rs`, PLAN §41), answers
**XTSMGRAPHICS**, and amends the engine's own **DA1** reply to advertise attribute 4. The rows that moved
are listed in §5 and §8; kitty graphics, iTerm2 OSC 1337 and ReGIS stay ❌, and the reason has changed
from "the engine cannot" to "their payloads are PNG/JPEG, which is a decoder dependency and an attack
surface" (§5).

**§42 moves no row back the other way, but it does fix a defect on this surface.** Word and line
selection (PLAN §42) is local UX, yet it needed the engine's line-wrap flag — and reading it exposed
that cmote's **copy across a wrapped line was inserting a newline** where every other terminal unwraps.
See the note at the end of §4 and the `term/screen.rs` evidence for `line_wrapped`.

**Update trigger.** This document tracks only the *terminal* surface — `src/term/` and
`src/ui/grid.rs`. Update it when, and only when, a change touches those: a query newly answered, a
mode newly honoured, a sequence newly rendered, or an engine bump that moves the ceiling. Editor,
files-pane, and window-chrome work does **not** belong here. When terminal work does land, the edit
is two places: the gap list in §2–§5 (or §6, if the answer is a deliberate refusal — a decision is
also an edit) and the matching row in the §8 matrix.

Sources cited by tag:

- `[ECMA-48]` — ECMA-48 (5th ed.), the ANSI/ISO control-function standard.
- `[DEC]` — the DEC VT100 / VT220 / VT320 / VT420 / VT520 programmer reference manuals.
- `[xterm]` — *XTerm Control Sequences* (Thomas Dickey), the de-facto reference for
  everything past the DEC set.
- `[community]` — a spec adopted across terminals but owned by no vendor (OSC 8 hyperlinks,
  synchronized output `?2026`, OSC 133 shell integration).
- `[vendor]` — documented by one vendor only (kitty keyboard/graphics, iTerm2 OSC 1337).

File:line evidence for the audited claims is collected in the [Evidence](#evidence) appendix.

---

## 0. The new baseline

`alacritty_terminal` 0.26 parses and represents the full VT set cmote cares about, and —
unlike `vt100` — **generates host replies itself** (through `Event::PtyWrite`), so the
"application stalls on a query timeout" class of bug is largely closed by construction. cmote
wires the engine behind a cmote-owned seam (`term::screen`), drains the engine's replies
through a `Replies` listener, and answers itself both the queries that need cmote's own data (its
colour scheme, its cell pixel size) and the four identity queries the engine drops — XTVERSION,
DECRQSS, XTGETTCAP and DA3, sniffed from the stream by `term::query` (§33, §36). The old `term::compat` (cursor-move rewriter) and
`term::answer` (reply synthesizer) modules were **deleted** in the swap — the engine does both.

Effort is now just *where the work lives*, not a hard wall:

- **[keymap]** — cmote's input encoder (`term::keymap`); engine-independent.
- **[reply]** — extend cmote's reply path (the `Replies` listener in `term::mod`, or the
  `term::query` stream scanner) for a query the engine does not answer itself — the route §33 took
  for XTVERSION / DECRQSS / XTGETTCAP and §36 for DA3.
- **[seam+grid]** — surface a getter the engine already has through `term::screen`, then render
  it in the grid.
- **[engine-limit]** — `alacritty_terminal` 0.26 itself does not parse or represent it; it would
  need an engine fork/upgrade or a scanner bolted on beside it. This is the *new, far higher*
  ceiling, and it is short.

---

## 1. Baseline — what already works

So the gaps read against a known floor. As of v3.0 (§23) cmote:

- **Renders** (per cell): bold, **dim / faint**, **italic** (from a bundled IBM Plex Mono
  face, since Fira Mono ships none), inverse, **conceal**, **strikethrough**, every
  **underline style** (single / double / dotted / dashed / curly) and **underline colour**, and
  full colour depth — 16 / 256 / 24-bit truecolor, fg and bg. Draws **braille** (U+2800–28FF)
  and **rounded box corners** (U+256D–2570) from geometry, since no bundled monospace font
  carries them. Cursor is drawn in the **shape a program picks with DECSCUSR** (block /
  underline / bar / hollow), steady — cmote runs no animation timer, so blink is dropped. Since §41 it
  also **composites inline sixel images** over the cells they reserve — decoded in-house, anchored to a
  document line so a picture rides the scrollback, drawn on the alternate screen too (its own page, with
  its own lifetime), and advertised to programs through DA1's attribute 4 and XTSMGRAPHICS.
- **Keeps 10 000 lines of scrollback** with a thin, read-only scroll indicator (§23 Stage 8):
  the wheel and Shift+PageUp/PageDown/Home/End scroll the history, and typing snaps back to the
  live bottom. The alternate screen keeps no history, so scrolling is inert there by design. That
  history is **searchable** (§35): Ctrl+Shift+F floats a find bar over the grid, and each hit is
  revealed (centred when off-screen) and turned into an ordinary selection, so Copy takes it
  (`term::search`). **Every hit on the visible screen is washed** in a second highlight colour (§39),
  the current one keeping the selection's own fill, so the bar shows where else the query is. The
  history is also **selectable and copyable as a document** (§40): a selection's endpoints are absolute
  line indices, so scrolling moves the highlight with its text and a copy — plain or styled HTML — reads
  the retained history rather than only the visible grid.
- **Lets the engine interpret** the whole VT stream, no cmote papering-over: the **DEC
  line-drawing charset** (older programs box-draw with it), **origin mode** (so cursor reports
  are origin-correct), **custom tab stops** (HTS / TBC), the **autowrap toggle** (DECAWM),
  **REP** repeat, the (vertical) scroll region, and **alternate-screen** switching.
- **Answers host queries.** Primary / secondary **DA** (`CSI c`, `CSI >c`), **DSR** status and
  cursor-position (`CSI 5n`, `CSI 6n`), and **DECRQM** request-mode are answered by the engine.
  The **colour queries** (OSC 10 / 11 / 12 and OSC 4 palette) and the **pixel / text-area
  size** reports (`CSI 14t`, `CSI 18t`) are answered by cmote's listener from its own colour
  scheme and cell metrics — so a program probing the background to pick a light-vs-dark theme is
  answered rather than left guessing. The four **identity queries** the engine drops — XTVERSION
  (`CSI > q`), DECRQSS (`DCS $ q … ST`), XTGETTCAP (`DCS + q … ST`) and DA3 (`CSI = c`) — are sniffed
  from the stream and answered by cmote itself (`term::query`, §33, §36), so a program fingerprinting
  the terminal or reading back its SGR no longer stalls on a dropped query.
- **Shows the window title** a program sets with OSC 0 / OSC 2 in the title bar (§23).
- **Tracks and honours modes**: application-cursor DECCKM (arrows → SS3), application-keypad DECKPAM
  (the unambiguous numpad keys → SS3, §36), bracketed paste
  `?2004`, cursor visibility `?25`, mouse `?9 / 1000 / 1002 / 1003` in SGR / UTF-8 / classic
  encodings, alternate screen, and **focus reporting `?1004`** (the shell hears `CSI I` / `CSI
  O` as it gains or loses focus).
- **Encodes input**: printable + layout, Ctrl-\* → C0, Alt-as-meta, Enter/Tab/Backspace/Esc,
  arrows/Home/End (DECCKM-aware), Insert/Delete/PageUp/Down, **F1–F12** and **F13–F24**,
  **modified named keys** (Ctrl/Shift/Alt + arrows / Home / End / navigation / F-keys, in the
  xterm `CSI 1;<mod><final>` / `CSI <n>;<mod>~` form), **modifyOtherKeys** (Ctrl/Alt on the
  main keyboard reported as `CSI 27;<mod>;<code>~` at level 1/2 when a remote editor enables it,
  scanned out of the stream by `term::modkeys`), numpad (NumLock-aware), bracketed paste with
  an injection scrub.
- **Reads the remote cwd** from OSC 7 / OSC 9;9 for the tree, pane, and title (`cwd.rs`).
- **Reads OSC 133 shell-integration marks** (§34) for a per-tab command-status dot, jump-to-prompt
  (Ctrl+Shift+Up/Down) and select-command-output (Ctrl+Shift+O, or clicking a prompt tick), scanned
  out of the stream by `term::osc133` — the same tactic as `cwd`, but with each mark's grid line
  captured by splitting the engine advance at it.
- **Can install the shell hook that emits both** (§17, `integration.rs`), on request, into the
  remote's own rc file. Nothing here changes: the emulator still only ever READS what the stream
  carries, and a shell that was already announcing is unaffected. What changes is how many remotes
  say anything at all — a plain bash emits neither sequence until someone puts the hook there, and
  before this the only someone was the user, by hand.

---

## 2. Input — closed (all `[keymap]`, engine-independent)

`modifyOtherKeys` (§1), the **kitty keyboard protocol** (§25) and now **DECKPAM** (§36) are all
**done**. Nothing in the input class is open.

Kitty shipped by flipping the engine's `kitty_keyboard` config flag on — so the engine tracks the
push/pop/query stack and answers `CSI ? u` itself — and writing the key-press → `CSI u` encoder
(`term/kitty.rs`), the flag set read off the seam (`Screen::kitty_flags`).

**DECKPAM shipped for the keys that can safely take it (§36).** The engine tracks the mode bit, so
this needed only a seam getter (`Screen::application_keypad`) and a branch in `keymap::encode`
reading it out of the grouped `Modes`. Diverted: NumpadEnter `ESC O M`, and the operators
`* + , - / =` → `ESC O j/k/l/m/o/X`. **Not** diverted, deliberately: the numpad **digits** and the
decimal point. cmote mirrors xterm's default `numLock: true` behaviour — with NumLock on the numpad
sends its digit (the `pm2 ls` fix, keyed off the OS producing `text`), with NumLock off it is
navigation following DECCKM — and every ncurses app sets DECKPAM as part of terminfo `smkx`, so
honouring it for the number keys would divert NumLock-on digits to `ESC O p…y` inside vim / less,
re-breaking the exact digit-typing that fix protects. xterm's `numLock` resource makes the same call,
so this is parity, not a shortcut. The navigation role needed nothing: `smkx` sets DECCKM *and*
DECKPAM, so the `ESC O B`-style bytes a program expects are what DECCKM alone already produces.

---

## 3. Query → reply — closed (`[reply]`)

The whole query class is closed. DA1 / DA2 / DSR / DECRQM are answered by the engine; the colour
and pixel-size queries by cmote's listener; and **since §33** (DA3 added in §36, XTSMGRAPHICS in §41)
the identity queries the engine drops are answered by cmote's own stream scanner (`term::query`), the
same out-of-band tactic `cwd` / `modkeys` use for sequences the engine ignores:

- **XTVERSION** (`CSI > q`) → `DCS > | cmote(<ver>) ST` — full, a truthful name and build version.
- **XTGETTCAP** (`DCS + q <hex> ST`) → states only the two caps cmote can give truthfully —
  terminal name `xterm-256color` and 256 colours — and answers every other capability an honest
  unknown (`DCS 0 + r <name> ST`).
- **DECRQSS** (`DCS $ q <sel> ST`) → reports **SGR** from the live pen (the exact attributes the
  grid paints, rebuilt after the chunk advances so a set-then-query in one write is seen), and
  every other setting an honest `ps=0` (`DCS 0 $ r ST`) rather than a lie about state cmote renders
  fixed or cannot read.
- **DA3 tertiary attributes** (`CSI = c`) → `DCS ! | 00434D45 ST` (§36). The engine's
  `identify_terminal` handles the no-intermediate (DA1) and `>` (DA2) forms and drops the `=` one, so
  this fell to the scanner too. The eight hex digits are a **constant** — site `00`, id `434D45`
  (`CME` in ASCII): on DEC hardware they were a serial number, and a per-machine value would hand
  every host a stable fingerprint of the user's computer off a query they never see. The reply
  identifies the program, not the person. The "is this the default parameter form?" test is now shared
  with XTVERSION (`default_params`), so the two arms cannot drift.

- **XTSMGRAPHICS** (`CSI ? Pi ; Pa ; Pv S`) → cmote's graphics limits (§41). The engine's only `S` is
  SU with no intermediate, so the `?` form falls to the scanner. Answered from what the sixel decoder
  actually enforces — 256 colour registers, 4096×4096 and 4 Mpx — so a program sizing a picture is told
  a promise cmote keeps; a *set* (action 3) is honestly refused with the value cmote will keep to, and
  ReGIS (item 3) is answered "unknown item" rather than given a geometry it could never honour.

There is also one reply cmote does not *originate* but **amends**: the engine writes DA1 as `CSI ? 6 c`,
and attribute **4** is how a terminal says it draws sixels — which is what chafa's auto mode, `lsix` and
ranger's previewer read at startup. Since §41 cmote rewrites that reply on its way out
(`query::with_sixel_attribute`) rather than sending a second DA1 (the program would parse one of them as
input) or suppressing the engine's (that would mean cutting bytes out of an inbound stream mid-sequence).

The one remaining reply-class sequence, **answerback (ENQ `0x05`)**, is refused as policy rather than
carried as a gap — see §6.

---

## 4. Rendering / attributes — closed at this layer

**OSC 8 hyperlinks are now done** (§24), **including the Ctrl-hover underline** (v4.0.0) — the seam
surfaces the per-cell URI (`Cell::hyperlink`), Ctrl+click and a context-menu Open/Copy follow it,
`link` gates the scheme to http/https/mailto before opening, and the grid now underlines the whole
run of a link while Ctrl is held over it, so the link reveals itself before the click.

**Correction (§36): blink is not cmote's choice, it is the engine's ceiling.** Earlier editions of
this table listed **blink (SGR 5/6)** as `[policy]` — "the engine stores the bit; cmote draws steady
by choice". That was wrong, and re-checking it was part of §36: `vte` parses SGR 5/6 into
`Attr::BlinkSlow` / `BlinkFast`, but `alacritty_terminal` 0.26's `terminal_attribute` has **no arm
for either**, and its cell `Flags` carry **no blink bit at all** — the attribute is dropped before
cmote can see it. The row moved to §5 as an `[engine-limit]`. (cmote's no-animation-timer policy is
still true and still applies to the *cursor*, whose blink the engine does track.)

**OSC 133 shell-integration is now done** (§34) — the stream scanner this row once anticipated
(`term/osc133.rs`, beside the cwd tracker). It drives a per-tab command-status dot, jump-to-prompt
(Ctrl+Shift+Up/Down), and select-command-output (Ctrl+Shift+O, or clicking a prompt tick, turns a
command's C→D range into an ordinary text selection). Prompt lines and command output ranges are
stored as absolute line indices so they ride the scrollback, captured by splitting the engine
advance at each mark. Capture of an output taller than the screen — the one piece this listed as left
for later — landed in §40, when the selection itself moved to absolute lines; what is still deferred is
walking *older* commands from the keybind (a prompt-tick click already reaches any of them).

**The grid now carries a second highlight layer (§39), and it answers no sequence.** The find bar's
on-screen matches are washed per cell (`ui/grid.rs::match_mask`, a row-major mask built once per frame
and read in `cell_style` between the inverse/cursor swap and the selection fill). It is recorded here
only because it touches this document's surface — `src/term/` and `ui/grid.rs` — and to be explicit
that **no row of §2–§6 or the §8 matrix moves**: no host request produces it, no attribute is newly
honoured, and nothing about what a remote program can ask for changed. A local highlight over cells the
program already painted is cmote's own UX, like the selection it sits under.

**§40 moved the selection into document coordinates, and that answers no sequence either.** A
selection's endpoints are now absolute line indices instead of viewport rows, so the grid resolves the
row it is drawing into the line that row shows (`Marks::top_line` + `Screen::line_at`) and the copy path
reads lines straight out of the retained history (`Screen::line_cell`) rather than only the visible
grid. The engine is read in one more way and driven in none: **no row of §2–§6 or the §8 matrix moves**.
It is recorded here because the mapping now lives on this document's surface (`term/screen.rs`) and is
the one place a viewport row and a document line meet — the same coordinate the OSC 133 marks (§34) and
the search matches (§35, §39) are already stored in.

**§41 adds a THIRD layer to the grid, and this one does answer a sequence.** Inline sixel images
(`term/sixel.rs`, `term/graphics.rs`, `ui/grid.rs`) are the first drawing cmote does on a remote
program's instruction that the engine has no representation for at all — so unlike §39 and §40, rows
move: the sixel DCS, XTSMGRAPHICS, DECSDM and DA1's attribute 4 (§3, §5, §8). Three things make it fit
the layer rather than sit beside it:

- **The picture's cells are real cells.** `term::mod` reserves them by feeding the engine ECH + LF, so
  the grid under an image is ordinary blank scrollback: it scrolls, evicts and reflows as text does, and
  a program that writes over the image's rows erases them the way it erases anything.
- **It is drawn where its own text is**, from an absolute document line (§40) resolved back onto a row
  per frame — the reverse of §39's projection, in the same coordinate.
- **The alternate screen has its own page of pictures** (`ranger`, `mpv --vo=sixel`), which §41 first
  shipped without and this document called the one real gap left. It closed on a coordinate insight
  rather than new machinery: that page keeps no history, so `history_size` is 0 and the absolute
  document line of row `r` is exactly `r` — the same space, not a second one. The renderer takes
  whichever page is up and branches on nothing. What differs is the *lifetime*: a second store, emptied
  by either screen swap and by `CSI 2 J`, replacing a picture whose box a new one overlaps, and retiring
  one the program has drawn a glyph over. The reservation steps its rows with **CUD** there rather than
  LF, since a page with no history must never be scrolled by cmote's own bookkeeping (PLAN §41).

**§42 reads one more engine flag and answers no sequence.** Word (double-click) and line (triple-click)
selection is local UX in the §39/§40 family — nothing a remote can ask for — but it appears here because
it put a new reader on the seam: `Screen::line_wrapped(line)`, the engine's `WRAPLINE` flag, which says
whether a document line is continued by the next. **No row of §2–§6 or the §8 matrix moves.** Worth
recording anyway, because reading that flag corrected a real defect on this surface: a copy across a
wrapped line used to paste a **newline into the middle of a logical line** (a long path, a long command),
where every other terminal unwraps. `Selection::extract` and the HTML copy now join a wrapped row to the
next with nothing, and skip the trailing-blank trim on a wrapped row — a blank in the middle of a
logical line is a space, not the grid's width padding. It also fixed a one-cell blind spot on this
surface: a **one-character find-bar hit** (§35) and a **command whose output is a single character** (§34)
were both being revealed and then not highlighted, because a range one cell wide was indistinguishable
from a click that had not dragged.

---

## 5. The new engine's own ceiling (`[engine-limit]`)

`alacritty_terminal` 0.26 does not parse or represent these, so they would need an engine
fork/upgrade or a scanner bolted on beside it. This is the whole of the remaining hard ceiling —
short, and since §41 nothing left in it is high value:

- **~~Sixel~~ — SHIPPED in §41, with no engine work at all.** This entry used to say graphics needed
  "both engine work *and* a compositor in the renderer". The compositor was real; the engine work was
  not. The crate's `hook`/`put`/`unhook` are no-op debug logs, so a sixel DCS is followed to its
  terminator and dropped — it cannot reach the grid, and it can be scanned out of the same stream beside
  the engine (`term/graphics.rs`) exactly as the cwd, modifyOtherKeys, the identity queries and the OSC
  133 marks are. cmote decodes the payload itself (`term/sixel.rs` — sixel is printable ASCII, so no
  image-format dependency), anchors the picture to an absolute document line (§40), reserves the cells
  it covers by feeding the engine ECH + LF, and composites it in `ui/grid.rs` — on the alternate screen
  too, which needed no new coordinate, only its own store. See PLAN §41.
- **ReGIS / kitty graphics / iTerm2 inline images (OSC 1337)** — still ❌, but the reason has moved out
  of this section's premise. Nothing about the engine blocks them either; kitty and iTerm2 carry
  **PNG/JPEG** payloads, so each needs an image-format decoder — a parser fed bytes straight off the
  wire, i.e. a dependency and a security decision rather than a rendering one — and ReGIS is a vector
  language with no users worth the interpreter. `[DEC]` / `[vendor]`. The placement, reservation,
  compositing and capability machinery §41 built is protocol-agnostic, so kitty would be a decoder plus
  a scanner arm.
- **Blink** (SGR 5/6) — `vte` parses it (`Attr::BlinkSlow` / `BlinkFast`), but the engine's
  `terminal_attribute` has no arm for either and its cell `Flags` hold no blink bit, so the attribute
  never reaches the grid (§36, moved here from §4). Showing it would take a per-cell scanner beside
  the engine (as `modkeys` is) *plus* the repaint timer cmote deliberately does not run. `[ECMA-48]`,
  low value.
- **Double-width / double-height lines** (DECDWL / DECDHL, `ESC#3-6`) — not represented
  (single wide glyphs are; whole-line doubling is not). `[DEC]`.
- **Left / right margins** (DECSLRM, VT420) — the engine's scroll region is vertical only
  (`set_scrolling_region(top, bottom)`). `[DEC]`.
- **DRCS soft fonts, VT320 status line, VT420 rectangular ops** (DECCRA / DECFRA / DECERA, and
  the DECRQCRA checksum query some conformance suites block on) — not represented. `[DEC]`.
- **Synchronized output `?2026`** — the **vte parser batches** the run between `?2026h` and
  `?2026l` (`vte-0.15.0/src/ansi.rs` BSU/ESU), but `alacritty_terminal`'s mode handler is a no-op
  (`SyncUpdate => ()`) and DECRQM reports it reset. cmote already paints atomically from the grid
  each frame, so the visible effect is nil either way. `[community]`, low pri.

---

## 6. Deliberately excluded (policy, not gap)

**OSC 52 clipboard read/write** — the engine surfaces it as `Event::ClipboardLoad` /
`ClipboardStore`; cmote **drops both on purpose** (§9 / §12 / §23): a remote could read or
poison the local clipboard, and cmote touches the clipboard only on an explicit *local* action.
The **bell** is dropped for the same "no remote-driven side effects" reason. Answering an OSC 52 read
query would be an injection vector and stays out.

**Remote colour *set* requests** — `OSC 4;n;<spec>`, `OSC 10 / 11 / 12` with a value, and the resets
`OSC 104 / 110 / 111 / 112`. The theme is chrome the **user** chose and cmote owns, so a remote does not
repaint it. Worth stating precisely what happens, because "ignored" is not quite it: the engine's
`set_color` **records** the value in its own colour table and marks the terminal fully damaged, and
cmote's renderer never reads that table — `ui/grid.rs` paints from `palette` alone. So a set costs a
full-screen repaint and changes nothing. Harmless, and invisible.

That leaves one **asymmetry worth knowing about**, since it is a consequence rather than an oversight:
a *query* is answered from cmote's scheme (`report_color`), so set-then-query does not round-trip. A
program that sets the background to pink and then asks is told the background is cmote's — which is the
honest answer, and the useful one for anything probing whether the set took. A program that sets and
merely *assumes* success will draw text for a background it has not got. Honouring sets is the only fix,
and that is exactly what this policy refuses.

**Remote-triggered desktop notifications** — `OSC 9;<text>`, `OSC 777` (urxvt) and `kitty 99` (rich
notifications) are all the same feature in three spellings, and all three are **refused on purpose**
(§54). A notification *leaves the window*: it lands on the user's desktop, outlives the tab, and on
Windows sits in the Action Center after the session is gone. That hands a remote a channel to the
machine itself, and a compromised or merely chatty host would spam it. cmote's rule throughout is that
a remote may change what its own tab looks like and nothing more.

**This is what makes `OSC 9;4` progress a different question, not an inconsistency** — §54 implements
it. OSC 9 is *multiplexed*: `9;9` is the Windows working-directory announcement cmote has read since
§17, `9;4` is progress, and a bare `9;<text>` is the notification. Progress cannot leave the chip it
belongs to, so the worst a lying remote achieves is a wrong number on its own tab; the number is
clamped and a malformed report changes nothing. The line here is not "which OSC number" but **whether
the effect escapes the tab**.

**Remote-set mouse pointer shape** — `OSC 22` is **refused**, and the reason is not the one that first
suggests itself. "The pointer is ours like the theme is ours" is the weaker half: a colour scheme is
persistent identity the user chose, whereas a pointer shape is transient feedback about what sits under
it, and there the remote program genuinely knows more than cmote does. Refusing it does cost something
real — no I-beam over text in a full-screen editor, no wait cursor through a long operation.

The load-bearing reasons are these three:

- **The pointer is already contested, and the arbitration is hand-rolled unsafe code.** cmote sets four
  shapes of its own — `ResizingHorizontally` / `ResizingVertically` on the panel splitters, `Grab` /
  `Grabbing` on every drag handle — and on Windows it *paints its own hands* through a `WM_SETCURSOR`
  subclass that answers **before** winit, because Windows ships no hand cursor at all (§51). Both §51
  and §52 went into getting that contest right. OSC 22 adds a remote as a fifth voice, and every pair
  then needs a winner: the remote asks for `wait` while the pointer rests on a chip that wants the open
  hand — which one? That question would be answered inside the subclass, the last place in cmote worth
  adding a case to.
- **A pointer shape is window-wide, so it fails the same test §54 applies to progress.** The cursor
  travels over the tab strip, the file panes and the dialogs, none of which belong to the remote. Its
  shape would either leak outside the grid — the effect escaping the tab — or the subclass would have
  to learn the grid's bounds to fence it in, which is more contest in that same unsafe path.
- **`none` is in the vocabulary.** OSC 22 carries cursor *names*, and hiding the pointer is one. A
  remote that makes the local mouse pointer vanish over cmote's window is a real nuisance with no
  obvious undo.

Note this refuses nothing that works today: cmote sets **no** cursor over the terminal grid, so this is
an unrealised nicety being declined, not a behaviour being removed.

**Answerback (ENQ `0x05`)** — refused for the same reason, and this is why xterm ships it empty too
(§36). The trigger is a *single ordinary byte*, so any binary output that happens to contain `0x05`
— a `cat` of a binary, a corrupt download, a stray progress stream — would type the answerback string
into the shell's input as if the user had. That is a remote-driven side effect on the user's keyboard
in exchange for legacy identification nobody asks for; the DA / DECRQM / XTVERSION / DA3 answers cover
every probe a modern program makes.

---

## 7. Recommendation

**There is no A-sized item left.** Input (§2), query→reply (§3) and the rendering/attribute layer
(§4) are all closed; what remains is the engine's own ceiling (§5) and the two sequences cmote refuses
on purpose (§6). §36 closed the last four items — DA3 and DECKPAM by writing them, answerback and
blink by deciding them (and, for blink, by correcting a wrong claim in this document: the engine drops
the attribute, so it was never cmote's choice to make).

**§41 took the last ceiling-raiser, and it turned out not to need the engine at all.** This section
used to name **graphics** as the one remaining item worth planning, "large, needing engine work *and* a
compositor in the renderer". Half of that was a wrong premise: the engine's DCS hooks are no-op debug
logs, so a sixel picture never reaches the grid and can be scanned out beside the engine like every other
sequence it ignores. cmote now decodes sixel in-house, anchors each picture to an absolute document line
(§40), reserves the cells it covers by feeding the engine ECH + LF, and composites it in the grid widget
— plus the two answers that make programs *offer* pictures at all: **XTSMGRAPHICS**, and attribute 4
added to the engine's own **DA1** reply. Details in PLAN §41; the moved rows are in §5 and §8.

What is left in §5 (blink, double-height lines, left/right margins, rectangular ops, synchronized output,
and the PNG/JPEG-carrying kitty and iTerm2 image protocols) is legacy, rare, invisible in practice, or a
decoder dependency — **no item of real UX value remains anywhere in this document.**

The **DECKPAM** subset shipped as a seam getter (`Screen::application_keypad`) plus one guarded branch
in `keymap::encode` — the numpad keys with no NumLock meaning to lose, and explicitly *not* the digits
(§2, §36). **DA3** shipped as a `CSI =` state in the same scanner that answers XTVERSION, with a
constant unit id chosen so the reply cannot fingerprint the machine (§3, §36).
The **kitty keyboard protocol** (was #1) shipped as `term::kitty` + a `keymap::encode` branch,
the inverse of the modifyOtherKeys split: the engine already implements the whole control plane
(push / pop / set / query, stack, alternate-screen swap), gated behind its `kitty_keyboard`
config flag — cmote flips that on, so there is no scanner and no reply path, and reads the active
flags off the seam (`Screen::kitty_flags`) to drive the `CSI u` encoder. Disambiguate, event types
(press / repeat / release, the key-up now forwarded from iced), report-all and associated text are
encoded; alternate keys best-effort (§25). `OSC 8 hyperlinks` (an earlier #1) shipped as a seam
getter (`Cell::hyperlink`) plus the `link` module: **Ctrl+click** or a right-click **Open link /
Copy link** follows it, the scheme gated to http/https/mailto and the URI handed to a launcher
that never builds a shell command line (§24); v4.0.0 added the **Ctrl-hover underline** — the grid
finds the pointer's link run (`link_run_at`) and underlines it while Ctrl is held, driven off the
repaints the app already emits on a hover move or a modifier change, so it needs no new plumbing. `modifyOtherKeys` (an earlier #2) shipped as
`term::modkeys` + a `keymap::encode` branch: the stream is scanned for `CSI > 4 ; p m`, and a
Ctrl/Alt main-keyboard combo is reported as `CSI 27;mod;code~` (level 2 for every combo, level 1
for the gap combos only) — kept for the programs that speak it rather than kitty. **OSC 133
shell-integration** (§4's old low-pri row) shipped as `term::osc133`: the stream is scanned for the
A/B/C/D marks, prompts and command output ranges stored as absolute line indices so they ride the
scrollback, and the result drives a per-tab command-status dot, Ctrl+Shift+Up/Down jump-to-prompt,
and select-command-output (Ctrl+Shift+O or a prompt-tick click turns the C→D range into a text
selection) — the same scanner-beside-the-cwd tactic, but with each mark's grid line captured by
splitting the engine advance at it. **Scrollback search** (§35) then shipped on the same two
foundations, needing nothing of the engine beyond reads: `term::search` walks the whole grid
(history included) for a query, and each hit is revealed by a scroll and handed to the UI as an
ordinary selection — so it added no reply path and no clipboard code, and at first no rendering
either. **§39 then added the one rendering piece**: the hits that fall on the visible screen are
resolved to viewport rows (`Search::visible`) and washed per cell, the current one still keeping the
selection's fill. **§40 then took the last viewport-bound piece the other way**: the selection's own
endpoints became absolute lines, so the grid projects a row onto a line to highlight it and the copy
path reads the history directly — one more kind of read, still nothing of the engine beyond reads.
**§41 is where that run of reads finally writes something back**: to reserve the cells a picture covers,
cmote feeds the engine ECH + LF *as if the remote had sent them* — the reservation then obeys the scroll
region, the autowrap mode and the character set exactly as the program's own output would, and the
picture's cells become ordinary scrollback that scrolls, evicts and reflows like any other.

Every `[engine-limit]` item that carried real UX value is now shipped, and it was shipped **without
touching the engine** — the same beside-the-engine tactic, one more time. What remains there (blink,
double-height lines, left/right margins, rectangular ops, synchronized output, kitty/iTerm2 images) is
legacy, invisible in practice, or a PNG/JPEG decoder dependency. For "support *any* documented app UX",
there is no outstanding ceiling-raiser left; every item this document ever listed is either shipped or
refused with its reason written down.

**§54 then closed the OSC column's last item of real value, and turned four ❌ rows into decisions.**
`OSC 9;4` progress reporting shipped (`term/progress.rs`) — a per-tab bar on the chip and the taskbar
button mirroring the active tab. The same pass wrote down the stance the notification rows had been
missing: `OSC 9;<text>`, `OSC 777` and `kitty 99` are one feature in three spellings and are **refused**
(§6), because a notification escapes the tab and lands on the desktop; `kitty 21` is refused for the
reason 4 / 10 / 11 / 12 already were, a fixed scheme. Those rows now read as choices rather than as
work not yet done, which is the difference between a gap and a policy.

**`OSC 22` was then decided too, which empties the OSC column: every row is now shipped or refused with
its reason written down, and none is merely outstanding.** The mouse-pointer shape looked at first like
the one cheap gap left — tab-local, and cmote already owns a cursor mechanism to hang it off (`cursor.rs`,
§51). Looking properly reversed that. The pointer is *window-wide* chrome that travels over the strip,
the panes and the dialogs, so a remote's shape either escapes the tab or needs fencing; `none` is one of
the names it can carry, so a remote could hide the local pointer; and cmote's cursor is already contested
by four shapes of its own, arbitrated inside a hand-rolled `WM_SETCURSOR` subclass that took §51 and §52
to get right — the last place worth adding a fifth voice. Refused, with the cost admitted: no I-beam over
text, no wait cursor through a long operation. See §6.

---

## 8. Feature support matrix (vs `vtdn.dev`)

A per-sequence audit against the escape-sequence catalogue published at
[vtdn.dev](https://vtdn.dev), so support is legible one line at a time rather than only as the
"still-missing" lens of §2–§6. Every ✅/⚠️/❌ below was verified against the real sources — the
engine crate (`alacritty_terminal-0.26.0`), its parser (`vte-0.15.0`), and cmote's own layer
(`term/`, `ui/grid.rs`) — not from memory.

Legend: **✅** full · **⚠️** partial or a deliberate quirk · **❌** not supported. A ❌ marked
*(policy)* is excluded on purpose (§6), not a gap.

### OSC — Operating System Command

| Code | Feature | Status | Note |
|---|---|---|---|
| 0 | Icon name + window title | ✅ | title shown; icon name dropped (`term/mod.rs`) |
| 2 | Window title | ✅ | control chars stripped (anti-spoof) |
| 4 | Palette entry set / query | ⚠️ | query answered from cmote's scheme; **set** recorded by the engine and never read by the renderer (fixed palette) |
| 7 | Working directory | ✅ | cmote's own scanner (`term/cwd.rs`, §17) |
| 8 | Hyperlinks | ✅ | rendered + Ctrl-click; web/mail only (`link.rs`, §24) |
| 9 | Desktop notification | ❌ | *(policy)* — a notification leaves the window and lands on the desktop (§6, §54) |
| 9;4 | Progress reporting | ✅ | per-tab bar on the chip + the taskbar button mirrors the active tab (`term/progress.rs`, §54); all five states, share clamped |
| 10 / 11 / 12 | Default fg / bg / cursor colour | ⚠️ | query answered (scheme-accurate — `report_color` resolves against `palette`, the same source `ui/grid.rs` paints from; cursor reports the **fg**, since the cursor is drawn by inverting the cell); **set** recorded by the engine and never read — a full repaint for no change |
| 22 | Mouse pointer shape | ❌ | *(policy)* — the pointer is window-wide chrome and already contested by four of cmote's own shapes (§6) |
| 52 (write) | Clipboard write | ❌ | *(policy)* — remote must not poison local clipboard (§6) |
| 52 (read) | Clipboard read | ❌ | *(policy)* — remote must not read local clipboard (§6) |
| 104 | Reset palette entry | ❌ | no effect (fixed palette) |
| 110 / 111 / 112 | Reset fg / bg / cursor colour | ❌ | no effect (fixed scheme) |
| 133 | Shell integration (semantic prompts) | ✅ | scanner (`term/osc133.rs`, §34): per-tab status dot + jump-to-prompt + select-command-output; A/B/C/D tracked, exit code from D |
| Kitty 21 | Colour by semantic name | ❌ | *(policy)* — same fixed scheme as 4 / 10 / 11 / 12: the theme is cmote's, not the remote's |
| Kitty 99 | Rich notifications | ❌ | *(policy)* — a notification, in a third spelling (§6, §54) |
| iTerm 1337 File | Inline images | ❌ | a PNG/JPEG payload, so it needs an image-format decoder — cmote's own images are sixel, which needs none (§5, §41) |
| iTerm 1337 | Marks / vars / profiles | ❌ | |
| 777 | urxvt notification | ❌ | *(policy)* — a notification, in a fourth spelling (§6, §54) |

### CSI — cursor movement & editing

| Feature | Code | Status | Note |
|---|---|---|---|
| Cursor up / down / fwd / back | A / B / C / D | ✅ | |
| Cursor next / prev line | E / F | ✅ | |
| Absolute position | G / H (+ f) | ✅ | HVP `f` too |
| Forward / backward tab | I / Z | ✅ | |
| Vertical / horizontal PA | d / \` | ✅ | |
| Save / restore cursor | s / u | ✅ | ANSI.SYS form |
| Insert / delete / erase char | @ / P / X | ✅ | |
| Insert / delete line | L / M | ✅ | |
| Erase in display | J | ✅ | |
| Erase scrollback | 3 J | ✅ | |
| Erase in line | K | ✅ | |
| Selective erase (protected) | ? J / ? K | ❌ | no protected-region support |
| Repeat character | b (REP) | ✅ | handled in the vte parser (`ansi.rs`) |
| Scroll up / down | S / T | ✅ | |
| Scrolling region (top / bottom) | r (DECSTBM) | ✅ | vertical only |
| Left / right margins | s (DECSLRM) | ❌ | engine scroll region is vertical only (§5) |
| Tab clear | g | ✅ | |
| Rectangular erase / fill / copy | $ z / $ x / $ v | ❌ | not represented (§5) |
| Cursor style | Ps SP q (DECSCUSR) | ✅ | block / underline / bar; blink dropped |
| Device status report | 5n / 6n | ✅ | |
| Primary / secondary DA | c / > c | ✅ | unblocks vim / tmux startup; since §41 cmote amends the engine's DA1 to add attribute **4**, so programs know it draws sixels (`term/query.rs`) |
| Tertiary DA | = c | ✅ | answered by cmote's scanner with a constant unit id (§36) — this row read ❌ until §41 spotted it, having been left behind when §36 shipped it |
| Graphics attributes (XTSMGRAPHICS) | ? Pi;Pa;Pv S | ✅ | colour registers and max image size, from the decoder's real limits (§41) |
| Request mode (DECRQM) | ? Ps $ p | ✅ | engine answers |
| Colour palette stack | # p / # q | ❌ | |

### ESC — single sequences

| Feature | Code | Status | Note |
|---|---|---|---|
| Index / Reverse index | ESC D / ESC M | ✅ | |
| Next line | ESC E | ✅ | |
| Set tab stop | ESC H | ✅ | |
| Save / restore cursor | ESC 7 / ESC 8 | ✅ | |
| Full reset | ESC c (RIS) | ✅ | |
| Keypad app / numeric | ESC = / ESC > | ✅ | tracked, and encoded for the numpad keys with no NumLock meaning (Enter, `* + , - / =`); digits deliberately keep their NumLock behaviour (DECKPAM, §2, §36) |
| Screen alignment test | ESC #8 (DECALN) | ✅ | |
| Designate charset G0 / G1 | ESC ( / ESC ) | ✅ | DEC line-drawing works |
| Single shift G2 / G3 | ESC N / ESC O | ❌ | |
| Locking shifts | LS2 / LS3 / LS1R… | ⚠️ | SO / SI + designation only |
| Double-height / width lines | ESC #3–6 | ❌ | not represented (§5) |
| 7 / 8-bit control output | ESC SP F / G | ❌ | |
| UTF-8 charset | ESC % G | ✅ | engine is always UTF-8 |

### DCS — Device Control String

| Feature | Code | Status | Note |
|---|---|---|---|
| Request status string | DCS $ q (DECRQSS) | ⚠️ | SGR reported from the live pen; other settings honest `ps=0` (`term/query.rs`, §33) |
| Termcap query | DCS + q (XTGETTCAP) | ⚠️ | terminal name + colour count answered; other caps honest unknown (§33) |
| Terminal version | CSI > q (XTVERSION) | ✅ | replies `cmote(<ver>)` (`term/query.rs`, §33) |
| Tertiary device attributes | CSI = c (DA3 → DECRPTUI) | ✅ | replies a **constant** unit id `00434D45`, never a machine-derived one (`term/query.rs`, §36) |
| Sixel graphics | DCS … q | ✅ | decoded in-house and composited over the grid; the picture is anchored to an absolute document line and reserves its cells (`term/sixel.rs`, `term/graphics.rs`, §41). The alternate screen has its own page of them, on the same coordinate with the history at zero — so `ranger` previews and `mpv --vo=sixel` draw |
| tmux passthrough | DCS tmux; … | ❌ | |

### SGR — text styling

| Attribute | Code | Status | Note |
|---|---|---|---|
| Bold | 1 | ✅ | |
| Dim / faint | 2 | ✅ | faded toward bg |
| Italic | 3 | ✅ | bundled IBM Plex Mono face |
| Underline | 4 | ✅ | |
| Slow / rapid blink | 5 / 6 | ❌ | **dropped by the engine** — `vte` parses it, `alacritty_terminal` has no arm and no cell flag, so it never reaches cmote (§5, §36) |
| Reverse video | 7 | ✅ | |
| Hidden / conceal | 8 | ✅ | copy still yields the text |
| Strikethrough | 9 | ✅ | |
| Double underline | 21 / 4:2 | ✅ | |
| Curly / dotted / dashed underline | 4:3 / 4:4 / 4:5 | ✅ | drawn as our own quads |
| Overline | 53 | ❌ | not carried |
| 16 ANSI colours | 30–37 / 40–47 / 90–97 / 100–107 | ✅ | |
| 256-colour indexed | 38;5 / 48;5 | ✅ | |
| Truecolor (`;` and `:`) | 38;2 / 38:2 | ✅ | both spellings |
| Underline colour | 58;5 / 58;2 | ✅ | |

### DECSET / DECRST private modes

| Mode | # | Status | Note |
|---|---|---|---|
| Application cursor keys | 1 | ✅ | arrows send SS3 |
| 132 / 80 column | 3 | ⚠️ | DECCOLM clears screen, no resize (DECRQM: NotSupported) |
| Global reverse video | 5 (DECSCNM) | ❌ | |
| Origin mode | 6 | ✅ | |
| Auto-wrap | 7 | ✅ | |
| Blinking cursor | 12 | ⚠️ | tracked, drawn steady |
| Show / hide cursor | 25 | ✅ | |
| Reverse wrap | 45 | ❌ | |
| Left / right margin | 69 | ❌ | |
| Sixel scrolling (DECSDM) | 80 | ⚠️ | the mode is not tracked; cmote always scrolls — the modern default, and what emitters assume (§41) |
| Alternate screen | 1049 | ✅ | no scrollback there, by design |
| Mouse: normal / btn / any | 1000 / 1002 / 1003 | ✅ | `term/mouse.rs` |
| Focus events | 1004 | ✅ | cmote sends CSI I / CSI O |
| SGR mouse | 1006 | ✅ | |
| Alt-scroll | 1007 | ✅ | |
| SGR-pixel mouse | 1016 | ❌ | |
| Bracketed paste | 2004 | ✅ | with an injection scrub |
| Synchronized output | 2026 | ⚠️ | parser batches; engine mode is a no-op; cmote already atomic |
| Grapheme clustering | 2027 | ❌ | |
| Colour-scheme reporting | 2031 | ❌ | |
| In-band resize | 2048 | ❌ | |
| Insert / replace (IRM) | 4 | ✅ | |
| Newline mode (LNM) | 20 | ✅ | |
| X10 mouse (press-only) | 9 | ❌ | engine never implemented it |

### Graphics, window ops, keyboard, C0

| Feature | Status | Note |
|---|---|---|
| Sixel images | ✅ | decoded and composited by cmote itself, no engine work (§41) |
| Kitty graphics protocol / unicode placeholders / animation | ❌ | its payloads are PNG/RGBA chunks, so it needs an image-format decoder — a dependency and a security decision, not a rendering gap (§5, §41) |
| ReGIS | ❌ | a vector language; no users worth an interpreter (§5) |
| iTerm2 inline images (OSC 1337) | ❌ | same reason as kitty: a PNG/JPEG payload (§5, §41) |
| Graphics capability report | ✅ | XTSMGRAPHICS (`CSI ? Pi;Pa;Pv S`) answered from the decoder's real limits — 256 registers, 4096×4096 / 4 Mpx; a *set* honestly refused (`term/query.rs`, §41) |
| Window iconify / move / resize / raise / maximize / fullscreen (CSI 1–10 t) | ❌ | *(policy)* — cmote owns its tabbed window; remote can't drive it |
| Window / position / state reports (CSI 11 / 13 t) | ❌ | |
| Text area in pixels / chars (CSI 14t / 18t) | ✅ | the two size *queries* are answered |
| Cell size (CSI 16 t) | ❌ | |
| Title stack (CSI 22 / 23 t) | ✅ | `push_title` / `pop_title` |
| **Kitty keyboard protocol** | ✅ | engine tracks the flag stack; cmote encodes CSI-u (`term/kitty.rs`, §25) |
| **xterm modifyOtherKeys** | ✅ | scanned out of the stream by cmote (`term/modkeys.rs`, §9) |
| ENQ answerback | ❌ | **refused on purpose** — a lone `0x05` in binary output would type a string into the shell (§6, §36) |
| BEL | ⚠️ | accepted, **silent** — bell event dropped |
| BS / HT / LF / CR | ✅ | |
| SO / SI | ✅ | charset shift |

**Shape of it.** The whole legacy VT100 / xterm core is ✅ — cursor motion, editing, SGR, full
colour, alternate screen, mouse, bracketed paste, focus, DA1 / DA2 / DSR / DECRQM, DECSCUSR, REP, the
kitty keyboard protocol, the application keypad, and — since §33, completed by §36 — every identity
query the engine dropped (XTVERSION, DECRQSS SGR, XTGETTCAP, DA3). Most of the ❌ column is
**deliberate**: no images, no remote clipboard (OSC 52), no answerback, no remote window control
(CSI t), no blink (the engine drops it), and a fixed colour scheme so dynamic-palette writes are
query-only. The genuine plain gaps left are the newer private modes (2027 / 2031 / 2048),
selective / rectangular editing, and left-right margins — all catalogued with their cost in §5, which
is now the *only* section with anything open in it.

---

## Evidence

Audited file:line anchors behind the claims above, for later re-checking.

### `alacritty_terminal` 0.26.0 (registry crate — `…/alacritty_terminal-0.26.0/src/`)

- **Generates host replies** via `Event::PtyWrite`. `identify_terminal` (`term/mod.rs:1257`)
  answers **primary** DA (`ESC[?6c`) and **secondary** DA (`ESC[>0;<ver>;1c`) — the `=`
  (tertiary) intermediate falls to a debug no-op. `device_status` (DSR, `term/mod.rs:1332`) and
  `report_mode` (DECRQM, `term/mod.rs:2135`) reply likewise.
- **Kitty keyboard**: fully implemented but **guarded on `config.kitty_keyboard`** — every
  handler (`push_keyboard_mode` `term/mod.rs:1288`, `pop_keyboard_modes` `:1308`,
  `report_keyboard_mode` `:1275`, `set_keyboard_mode` `:1029`) early-returns when the flag is off.
  cmote **turns it on** in `Terminal::new`, so the engine tracks the pushed-flags
  `keyboard_mode_stack`, swaps it across the alternate screen, and answers the `CSI ? u` query
  (`report_keyboard_mode` writes `ESC [ ? <flags> u` as an `Event::PtyWrite`). The active flags
  fold into `TermMode` (`DISAMBIGUATE_ESC_CODES` `1<<18` … `REPORT_ASSOCIATED_TEXT` `1<<22`), which
  cmote reads through `Term::mode()` (`term/mod.rs:709`). So the encoding is cmote's only job (§25).
- **DECKPAM**: `set_keypad_application_mode` (`term/mod.rs:2180`) inserts `TermMode::APP_KEYPAD`
  (`term/mod.rs:59`), `unset_keypad_application_mode` (`:2186`) removes it — the engine tracks the
  mode, so cmote only reads it back and encodes (§36).
- **No blink at all**: `vte-0.15.0/src/ansi.rs:1844-1845` parses SGR 5/6 into `Attr::BlinkSlow` /
  `BlinkFast`, but `terminal_attribute` (`term/mod.rs:1885-1926`) has **no arm for either** and
  `term/cell.rs` `Flags` declare **no blink bit**, so the attribute is dropped before the grid. This
  corrects an earlier claim in §4 that the engine stored it (§36).
- **XTWINOPS size reports**: `text_area_size_pixels` (`term/mod.rs:2259`) and
  `text_area_size_chars` (`term/mod.rs:2268`).
- **OSC 8 hyperlinks**: stored per cell — `Cell::set_hyperlink` (`term/cell.rs:202`) and read
  back via `Cell::hyperlink` → `Option<Hyperlink>` with `.uri()` (`term/cell.rs:219`), the
  handler at `term/mod.rs:1874`. cmote surfaces this through the seam (below).
- **Scroll region is vertical only**: `set_scrolling_region(top, bottom)` (`term/mod.rs:2155`) —
  no horizontal (left/right) margins.
- **No graphics, no double-height lines, no left/right margins, no `?2026`** — no `Sixel`,
  `graphics`, `DoubleHeight`/`DECDHL`, `left_right_margin`, or synchronized-update symbols in the
  crate source.
- **Every DCS is a no-op, and that is what let §41 in.** `vte-0.15.0/src/ansi.rs`'s
  `hook` (`:1311`), `put` (`:1319`) and `unhook` (`:1324`) are debug logs with no body, so a sixel
  payload is followed to its terminator and dropped — it cannot reach the grid, and cmote can scan the
  same bytes for it without racing the engine or filtering the stream. `CSI S` is dispatched only as
  `('S', [])` (SU, `ansi.rs:1736`), so the `?`-prefixed XTSMGRAPHICS form falls through to the
  unhandled arm and is cmote's to answer. `('X', [])` (ECH, `:1766`) and LF are what cmote feeds back
  in to reserve a picture's cells.

### cmote (`c:/sources/github_clemeno/cmote/src/`)

- **`term/mod.rs`** — the `Replies` listener answers the events that expect a report and drops
  the rest (`~:228-258`): `Event::PtyWrite` (the engine's DA / DSR / DECRQM / cursor-position,
  accumulated whole), `ColorRequest` (OSC 10 / 11 / 12 / 4, resolved against cmote's scheme via
  `report_color`), `TextAreaSizeRequest` (`CSI 14t`, from the grid + cell pixel size),
  `Title` / `ResetTitle` (OSC 0 / 2, sanitized). **Dropped**: `ClipboardLoad` / `ClipboardStore`
  (OSC 52), the bell, and colour *set* requests. `SCROLLBACK = 10_000`. The seam hides the
  engine types behind `Terminal` + `ScrollMotion`. Since §33 `process` also drains the `term::query`
  scanner: the chunk is scanned for identity queries *before* the engine advances, then each completed
  query becomes a reply — XTVERSION / XTGETTCAP / DA3 from static facts (`VERSION`, `UNIT_ID`),
  `Decrqss(Sgr)` from the live pen via `pen_sgr(self.term.grid().cursor.template)`, built after the
  advance so a set-then-query in one write is seen. **§41 added the inline-image half**: `process` merges
  the prompt marks and the image events of a chunk into one offset-ordered list (`splits`, since the
  engine only advances forwards), `apply_graphics` anchors a picture at `history_size + cursor row` and
  column, and `reserve_cells` feeds the engine `CSI <cols> X` + LF per row and a closing CR, so the
  picture's box is erased, its cells become ordinary scrollback and the cursor lands at the left margin
  below it. `set_cell_pixels` now also feeds the image store (pixels → cells), `resize` clears the
  placements with the prompt marks, and the drained reply buffer goes out through
  `query::with_sixel_attribute`. `Terminal::images` surfaces the placements. **The alternate screen has
  its own page of them**: `history_size` is 0 there, so the anchor is simply the cursor's row;
  `sync_alternate` empties that page on either screen swap, `retire_covered_images` drops a picture the
  program has since written a glyph into, and `reserve_cells` steps its rows with **CUD** instead of LF,
  because a page with no history must never be scrolled by cmote's own bookkeeping.
- **`term/query.rs`** — the identity-query scanner (§33, §36), the same out-of-band tactic as `cwd` /
  `modkeys`: a chunk-safe byte state machine (`Queries::feed`) recognising **XTVERSION** (`CSI > q`),
  **DA3** (`CSI = c`), **DECRQSS** (`DCS $ q <sel> ST`; `m` → `Sgr`, every other selector
  `Unsupported`) and **XTGETTCAP** (`DCS + q <hex>[;…] ST`). Both private-CSI queries answer only in
  their default parameter form — the shared `default_params` predicate (empty or all zeros), since a
  non-zero param on the same final byte is a different private sequence. An unrecognised DCS is
  followed to its terminator (`DcsIgnore`) so sixel data cannot masquerade as a query, and
  `MAX_PARAMS` / `MAX_DATA` bound a hostile stream (§12). Reply builders `version_reply` /
  `da3_reply` / `decrqss_sgr_reply` / `decrqss_unsupported_reply` / `gettcap_reply`;
  `known_capability` states only `TN=xterm-256color` and `Co`/`colors=256`. `term/mod.rs` holds the
  two identity constants — `VERSION` (`cmote(<crate version>)`) and `UNIT_ID` (`00434D45`, a
  **constant** so a DA3 reply cannot fingerprint the machine). Parse-only, no engine types,
  unit-tested per reply shape. **§41 added a fifth query and one amendment**: a `CsiQuestion` state
  reads **XTSMGRAPHICS** (`CSI ? Pi;Pa;Pv S`, every DECSET/DECRST passing through it unread on its own
  final byte) and `graphics_reply` answers item 1 with the register count and item 2 with the max
  geometry, status 3 for a *set* and status 1 for an unknown item — the numbers coming from
  `term::sixel`'s own constants via `term/mod.rs`, so the reply cannot drift from what the decoder
  enforces. `with_sixel_attribute` rewrites a DA1 reply the **engine** wrote (`CSI ? <params> c`) to
  carry attribute `4`, leaving a reply that already names it, and every non-DA1 `CSI ?` reply
  (DECRQM's `$y`, kitty's `u`), untouched.
- **`term/sixel.rs`** — the payload decoder (§41), pure and engine-free. `walk` is the single place the
  command grammar is written (`"` raster, `#` select/define, `!` repeat, `$` CR, `-` next band, and the
  `?`..`~` sixel bytes); `canvas_size` measures through it — preferring the raster attributes as the
  sender's crop, else the extent of *painted* pixels — and `paint` draws through it, so the two passes
  cannot disagree. A colour introducer **selects** the register it defines (every emitter relies on it);
  `#Pc;Pu;…` reads RGB as **percentages** and HLS from **DEC's blue hue origin** (rotated 240° onto the
  standard wheel); an unset pixel stays fully transparent so the grid's own background shows through;
  the 16 VT340 defaults are pre-loaded. Bounds (§12): `MAX_WIDTH`/`MAX_HEIGHT` 4096, `MAX_PIXELS` 4 Mpx
  (an image past them is refused **whole**, never clipped), `COLOR_REGISTERS` 256 (a higher index
  clamps), saturating parameters, and per-pixel bounds checks so a lying raster attribute can only lose
  pixels. Unit-tested per command, per colour space and per cap.
- **`term/graphics.rs`** — the image scanner and store (§41). `Images::feed` is the same chunk-safe byte
  machine as `cwd`/`osc133`, recognising a sixel DCS (`ESC P <params> q … ST`, with BEL and 8-bit ST
  accepted, and any other DCS followed silently so a DECRQSS payload cannot be read as a picture) plus
  `CSI 2 J` / `CSI 3 J` / RIS. Its two event kinds report **opposite** offsets on purpose: a picture past
  its DCS (the cursor is only right once everything before it has been drawn), an erase *before* its
  sequence (`CSI 3 J` drops the engine's history, so which placements it takes must be decided first).
  `Placement` carries the absolute `line`, `col`, the reserved `rows`/`cols`, the pixel `width`/`height`
  and an **iced image handle** — minted once at decode so the renderer's texture cache keys off a stable
  id instead of re-uploading every frame. Caps: `MAX_PAYLOAD` 16 MiB per picture (past it the DCS is
  still followed, nothing decoded), `MAX_IMAGES` 64 and `MAX_TOTAL_BYTES` 64 MiB, evicted oldest-first;
  `clear_screen` / `clear_scrollback` split on the first visible line, `clear` takes everything.
- **`term/screen.rs`** — engine-agnostic view. `Cell` getters: `contents`, `is_wide`,
  `is_wide_continuation`, `fgcolor`, `bgcolor`, `bold`, `dim`, `italic`, `hidden` (conceal),
  `strikeout`, `underline` (`UnderlineStyle`), `underline_color`, `inverse`, `hyperlink` (the
  cell's OSC 8 URI, §24). `Screen` getters: `size`, `cursor_position`, `display_offset`,
  `history_size`, `hide_cursor`, `cursor_shape`, `application_cursor`, `application_keypad`
  (DECKPAM, §36), `bracketed_paste`, `focus_reporting`, `mouse_mode`, `mouse_encoding`, `cell`,
  `kitty_flags` (the five active kitty protocol flags, read off `Term::mode()`, §25) and, since §41,
  `is_alternate` (`TermMode::ALT_SCREEN` — which page the images are placed on and read back from, since
  that screen keeps no history and so carries its own store on its own lifetime). Nothing the
  engine tracks is left unsurfaced now; blink it does not track at all (see above). **§40 added the
  document readers**: `line_at(row)` is the single written-down form of `history_size + row -
  display_offset` (the viewport → document mapping §34's ticks and §39's washes are placed by), and
  `line_cell(line, col)` reads a cell by **absolute line** — mapping the document onto the engine's
  `-history_size ..= screen_lines - 1` grid lines and answering `None` for a line the session no longer
  has. `cell(row, col)` is now `line_cell(line_at(row), col)`, so the viewport and document readers
  cannot drift; the pair is what lets a selection be stored in document coordinates and copied whole.
  **§42 added one more read**: `line_wrapped(line)` reports whether a document line is *continued* by the
  next one — `Flags::WRAPLINE` on the row's last cell, which the engine sets in `Term::wrapline` when
  output ran past the right margin (`term/mod.rs:968`) and rewrites during a reflow (`grid/resize.rs`).
  It is what makes a triple click take a whole *logical* line and a copy across a wrap re-join its halves
  instead of pasting a newline into the middle of a path. `grid_line(line)` is the shared private helper
  both `line_cell` and `line_wrapped` resolve through, so a cell read and a wrap check cannot disagree
  about which line is which.
- **`term/keymap.rs`** — printable + layout, Ctrl → C0, Alt-as-meta, named keys including
  **F1–F24** and the **modified named keys** (`modifier_param` computes the xterm parameter,
  `letter_key` / `tilde_key` shape the two key families), **modifyOtherKeys** (`modify_other_key`
  / `other_key_bytes` emit the `CSI 27;mod;code~` form when the level is on), the numpad NumLock
  heuristic, and the bracketed-paste terminator scrub. It now also carries an input-modes bundle
  (`Modes` — DECCKM, DECKPAM, the modifyOtherKeys level, the kitty flags) and a `KeyEvent` (press /
  repeat / release), and **dispatches to `term/kitty.rs` whenever a kitty flag is active**, superseding
  the legacy path; a legacy release yields nothing and a legacy repeat is a press. **DECKPAM** (§36) is
  `application_keypad_bytes`: SS3 for NumpadEnter `M` and the operators `* + , - / =` →
  `j k l m o X`, taken after the kitty hand-off and only on the unmodified form; the digits and the
  decimal point are deliberately excluded so a NumLock-on numpad still types numbers inside every
  ncurses program (terminfo `smkx` sets DECKPAM, so the mode is on for their whole run) — xterm's
  `numLock` default makes the same call.
- **`term/kitty.rs`** — the kitty keyboard encoder (§25). `KittyFlags` (the five progressive flags)
  and `KeyEvent`; `encode` turns a key event into kitty's `CSI <keycode>[:<shifted>] ;
  <mods>[:<event>] ; <text> u`, keeping the legacy final byte for keys that had one (arrows,
  Home/End, F-keys, the `~` navigation keys), leaving Enter/Tab/Backspace legacy until modified,
  and emitting `CSI 27 u` for Esc. Reports releases/repeats only under `report_events`, plain
  letters as codes only under `report_all`, and the associated text / shifted alternate under
  their flags. No engine dependency — pure input → bytes, unit-tested per flag.
- **`term/modkeys.rs`** — the `modifyOtherKeys` stream scanner (`CSI > 4 ; p m` → `Off` /
  `Level1` / `Level2`), a small state machine mirroring `cwd.rs`. Read by
  `Terminal::modify_other_keys` and threaded into `keymap::encode`.
- **`term/osc.rs`** — the shared OSC framer (§17, §34, §54). One chunk-safe byte machine
  (`Text`/`Escape`/`Payload`/`PayloadEscape`) recognising `ESC ] payload (BEL | ESC \)`, calling back
  once per completed payload with the byte offset **just past its terminator** — the coordinate §34
  needs to line a mark up with the grid, and which §17 and §54 ignore. `Framer<CAP>` takes its payload
  cap as a const parameter, so each scanner keeps deriving `Default` and keeps its own limit named in
  its own module (`cwd` 4096, `osc133` 512, `progress` 128); past the cap the payload is abandoned and
  framing resumes (§12). This replaced three copies of the same machine that had already drifted.
  **`graphics.rs` deliberately keeps its own**: a 16 MB binary payload whose overflow must keep
  scanning to the real terminator while flagging the payload spoiled, which is a different policy, not
  a different number.
- **`term/progress.rs`** — the command-progress scanner (OSC 9;4, §54). `Reports::feed` runs on the
  shared framer and keeps a latest-value `Progress` — `None` / `Indeterminate` / `Working(share)` /
  `Failed(share)` / `Paused(share)` for `st` 0 / 3 / 1 / 2 / 4, the share clamped to 100. Untrusted
  input throughout, so **a malformed report is a no-op**: an unknown `st`, a non-numeric field, an
  `st=1` with no share and an out-of-range number all leave the previous reading alone rather than
  blanking it, and `st=2`/`st=4` with no share stay at the share already reached. A command ending is
  judged **inside** `feed`, payload by payload, via `osc133::ends_command` — because one chunk can
  carry a `D` and then the first report of the *next* command, so clearing after the chunk would wipe
  the new report. There is deliberately **no `clear` on the interface**: unlike prompt marks and
  images, a progress reading has no place on the grid, so `resize` must not drop it. Fed the whole
  chunk by `process` with no split, like the cwd. Surfaced by `Terminal::progress`; drawn as a 3 px bar
  along the bottom of the tab chip (`ui/tabs.rs`) and mirrored onto the Windows taskbar button for the
  **active** tab only (`taskbar.rs`). Parse-only, no engine, no widgets — fully unit-tested.
- **`term/osc133.rs`** — the shell-integration scanner (§34). `Scanner::feed` runs on the shared
  framer and returns *a list* of `(offset, Mark)` — A / B / C / D, with D's exit
  code parsed from its next field. `Prompts` holds the command state (`Idle`/`Prompt`/`Running`), the
  last exit, the prompt lines as **absolute indices** (`history_size + row`), and each finished
  command's output as an absolute half-open range `[output, end)` keyed by its prompt line;
  `visible_rows` / `jump` / `latest_output` / `output_at_prompt` do the arithmetic. `process`
  (`term/mod.rs`) splits the engine advance at each mark's offset to read the cursor line there.
  Parse-and-arithmetic only, no engine types — the scanner, the state machine, and the
  jump/visibility/output math are all unit-tested with no terminal. Surfaced by
  `Terminal::{command_state,last_exit,prompt_rows,jump_prompt,select_output_latest,select_output_at_row}`;
  drawn as a per-tab dot (`ui/tabs.rs`) and a left-gutter tick (`ui/grid.rs::prompt_tick_rect`);
  jumped by Ctrl+Shift+Up/Down (`app.rs::prompt_jump`); its output selected by Ctrl+Shift+O or a
  gutter-tick click, both building an ordinary `ui::selection::Selection` (`app.rs::set_output_selection`)
  from an `OutputSpan` that carries **absolute lines** since §40 — so revealing decides what is on screen
  and the span decides what is selected, and an output taller than the screen is copied whole.
  Marks and command ranges are cleared on resize (reflow invalidates absolute lines).
- **`term/search.rs`** — the scrollback find bar's core (§35). `Row` is one grid line flattened for
  searching — its ASCII-lowered glyphs plus a **byte → column map** grown in lockstep by `push`, so a
  hit found by `str::find` reports grid *columns* (what a selection addresses) and a wide glyph's
  skipped trailing cell cannot shift the columns after it; `trim_end` drops the row's width-padding
  first. `Search` holds the query, every match in document order and which is current: a new query
  lands on the **newest** hit (`set_matches`), a re-scan keeps the current one **by identity**
  (`refresh`), and `step` wraps both ways. Pure — no engine, no widgets, all unit-tested.
  `Terminal::find` walks `-history_size ..= last screen row` (the engine keeps history on the negative
  lines) stamping each hit with the absolute line `history_size + line`, the same coordinate
  `osc133` uses; `Terminal::reveal_line` scrolls a line into view — **centred**, and left in place when
  already visible — and reports only *whether* it could be shown: since §40 `app.rs::reveal_match`
  selects the match's own absolute line and columns, with no conversion at all.
  Opened by Ctrl+Shift+F, which then owns the
  keyboard (the `self.search.is_some()` guard in `app.rs::on_key`, mirroring the inline rename fields);
  drawn as a floating overlay (`ui/terminal.rs::search_bar`) rather than a bar that would reflow the pty.
  **`Search::visible` (§39)** projects the hits onto the screen as it is scrolled — `absolute -
  history_size + display_offset`, the same mapping `osc133::visible_rows` uses — starting the walk at
  the first visible line with a `partition_point`, since the list is in document order and a one-letter
  query has tens of thousands of hits nearly all off screen. `ui/terminal.rs::view` resolves them and
  `ui/grid.rs::match_mask` flattens them into a per-frame row-major mask that `cell_style` reads
  between the inverse/cursor swap and the selection fill, so the current hit keeps the selection's
  colour and the rest wash amber. `ponytail:` per-row matching, so a hit across a wrapped line's fold is
  missed; and the match list is rebuilt only on a query change or a step, so output printed since is
  neither counted nor washed until the next one.
- **`term/mouse.rs`** — modes `?9 / 1000 / 1002 / 1003`; encodings classic / UTF-8 / SGR.
- **`link.rs`** — following an OSC 8 hyperlink (§24): `is_allowed` gates the scheme to
  http/https/mailto (pure, unit-tested), `open` hands an allowed URI to `open::that_detached`
  (PowerShell `Start-Process` with the URI as env-var data, never `cmd /C start`). Wired in
  `app.rs` (Ctrl+click → `follow_link`, `link_at` reads the seam) and `ui/terminal.rs`
  (right-click **Open link / Copy link**, `link_at` resolves the clicked cell).
- **`ui/grid.rs`** — the Ctrl-hover link underline (§24): `link_run_at` walks the contiguous
  same-URI reading-order run under a cell (pure, unit-tested), `hovered_link_run` gates it on Ctrl
  being held and the pointer being over the grid (read from the widget's own `State.modifiers` and
  the `draw` cursor), and `cell_style` gives a plain link cell in that run a single foreground
  underline while it is the hover target. Also the two local highlight layers over the cells: the
  find bar's per-frame match mask (§39, `match_mask`) and — since §40 — the **document-line
  projection of the selection**, `Marks::top_line` (`screen.line_at(0)`) plus the row being drawn,
  so `plan_runs` asks the selection about a *line*, once per row, and a scroll moves the highlight
  with its text instead of leaving it on the rows it was dragged over. **§41 added the image
  compositing**: `image_bounds` is that same projection run backwards — a placement's absolute line
  minus the frame's `top_line`, **signed**, so a picture anchored above the viewport is drawn with its
  top off screen — and `draw` paints each one after the text at its native pixel size, snapped, clipped
  to the intersection of its reserved cell box and the visible grid, and only while `is_alternate` is
  false. The handle is cloned out of the placement (a reference count, not the pixels), so the frame
  costs no upload.
- **Deleted in the swap**: `term/compat.rs` (the cursor-move rewriter) and `term/answer.rs`
  (the reply synthesizer) — the engine parses every spelling and answers every query they used
  to cover.
