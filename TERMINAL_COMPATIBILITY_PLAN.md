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
listed as cmote's policy choice when in fact the engine drops the attribute entirely — and **§60's audit
corrected it again**, reopening §3 by one row (`CSI ? 4 m`, a query nothing answered) and fixing five
more rows in §8 that had drifted from the crates; **§61 then closed that row**, so §3 stands again.
**§62 corrected two more in the same direction** — `CSI 1–10 t` and ENQ answerback were credited to
cmote as refusals nothing in cmote performs — while splitting §8's refusals into **🛑** (cmote's code
refuses it) and **🤷** (nothing does; it dies upstream). **§63 then took the one item that split turned
up as work**: OSC 52's refusal moved from an inherited crate default plus a catch-all to a stated
`osc52: Osc52::Disabled`, with a test on the field. No row changed status; the mechanism behind two of
them did.
**§64 ran the same check over the colour rows** — the last ⚠️ rows in §8's OSC table — and found one
mark covering two opposite answers and one cost that does not exist: `OSC 4` and `OSC 10 / 11 / 12`
answer a query fully and refuse a set, and the note charged the refused set with a full-screen repaint
that never happens, since `mark_fully_damaged` sets a bool nothing in cmote reads. Both rows now name
each of them a query answered in full and a set refused on purpose. Each is now **two rows**, the way
`OSC 52` has been two rows since §62, and the refused half is pinned by a test in each direction.
**§65 then audited every remaining ⚠️ row against the crates**, split seven more the same way, re-marked
`BEL` as the 🛑 it always was, and found one real gap behind a comfortable-looking mark: cmote never
drives `vte`'s synchronized-update timeout, so a remote can hold the visible screen still with eight bytes
(mode 2026, and §7).
**§39 touched this
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

## 3. Query → reply — closed again (`[reply]`)

This section read **closed** from §36 until §60's audit found it was not: `CSI ? 4 m` (XTQMODKEYS,
"what modifyOtherKeys level are you at?") is dispatched by `vte` to `report_modify_other_keys`, left at
the trait's empty default by `alacritty_terminal`, and was covered by neither `term/query.rs` nor
`term/modkeys.rs` — which read the *set* form and nothing else. So a program that asked waited out its
timeout. Nobody ever hit it; it was found by reading the crate's trait impl for methods still sitting at
their default. **§61 closed it**, and closed it in `term/modkeys.rs` rather than beside the other
answerers, because that module holds the level and so is the one place that sees the sets and the
questions in stream order — the answer is the level as it stood where the question sat.

DA1 / DA2 / DSR / DECRQM are answered by the engine; the colour
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

- **XTQMODKEYS** (`CSI ? 4 m`) → `CSI > 4 ; Pv m`, the level `term/modkeys.rs` is holding (§61). The
  answer is deliberately spelled as the SET form, which is xterm's own choice and a good one: what
  comes back is exactly the sequence that would restore the state, so a program can pocket the reply
  and write it back on the way out without parsing a byte of it. **Only resource 4 is answered.**
  XTMODKEYS carries seven, cmote holds one, and the reply format being an XTMODKEYS control means
  there is no spelling of "I do not have that resource" — so an answer for `modifyCursorKeys` would
  be a level asserted for a knob cmote's key encoder does not have. Silence for the other six is the
  honest reading, and the same call §60 made three times.

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
carried as a gap — see §6. XTQMODKEYS was the opposite while it lasted: not refused, just missed.

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
- **~~Selective erase / protected regions~~ — SHIPPED in §56, and not by the usual tactic.** DECSCA
  (`CSI Ps " q`) and the `?` erases (DECSED / DECSEL) have no arm anywhere in `vte`, and protection is
  **per-cell** state, so the scan-it-out-and-keep-it-beside-the-grid move that carried §17 / §33 / §34 /
  §41 / §54 / §55 does not work here: a bitmap beside the grid would have to be re-aligned on every
  scroll, insert, delete and reflow, which is re-implementing the grid. What worked instead was to
  borrow the **one unused bit** in the engine's own per-cell flag word (`Flags` names 15 of 16) and set
  it on `grid.cursor.template`, after which the engine carries protection as if it were bold — for free,
  through scrolling and reflow and the alternate screen. Nothing in the engine reads it, nothing in the
  renderer draws it, and a build-time assertion fails the compile if a future engine version claims the
  bit. cmote then performs the erase itself, cell by cell, because the engine's own `CSI 2 J` scrolls
  the viewport into history rather than blanking it. See PLAN §56 and `term/protect.rs`.
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
  (`set_scrolling_region(top, bottom)`), and horizontal ones reach into what printing, wrapping,
  `IL`/`DL`, `ICH`/`DCH` and every scroll do. `[DEC]`, and **still ❌ — but as a cost, not a
  capability.** An earlier reading of this row called it impossible without re-implementing the grid.
  That was wrong, and the correction is worth writing down because it is the only row in this document
  whose verdict rests on price rather than on a wall:

  - **There is a seam.** `Processor::advance<H: Handler>` is generic and `Term<T>` merely *implements*
    `Handler`, so cmote can pass a wrapper that holds the margin state, overrides the margin-sensitive
    methods and forwards the rest. No fork, no patch.
  - **The state that decides where a line breaks is public.** `Cursor::input_needs_wrap` is a `pub`
    field, so the pending-wrap flag can be read and set from outside — the piece assumed to be sealed
    inside `Term::input`.
  - **§58 already built the hard primitive.** Scrolling a column band by a row IS a rectangular copy
    plus an erase, and `copy_area` / `erase_area` exist.

  What it would take: about twelve of `Handler`'s 71 methods overridden (`input`, `carriage_return`,
  `linefeed`, `insert_blank_lines`, `delete_lines`, `insert_blank`, `delete_chars`, `goto`, `goto_col`,
  `move_forward`, `scroll_up`/`scroll_down`, `set_scrolling_region` plus the resets), the rest
  forwarded, `DECIC`/`DECDC` scanned out as `vte` has no arm for those either, and a `unicode_width`
  dependency so a two-cell glyph wraps at the right column. `Term::input` itself would be *pre-empted*
  rather than reimplemented — wrap at the margin first, then delegate, so the engine's own autowrap
  never reaches the screen edge. Call it 400–600 lines plus tests.

  What blocks it is what it would cost to keep. **Every `Handler` method has a default empty body**, so
  a method left unforwarded — or one a future `alacritty_terminal` adds — compiles cleanly and silently
  drops a sequence. That is the same class of hazard as §57's borrowed flag bit, except §57's could be
  caught at **build time** with a `const` assertion and this one cannot: a trait growing a defaulted
  method breaks nothing. Add the smaller ones — a margin wrap has to set `WRAPLINE` itself or copy,
  search and reflow read the line as two; margins are per-screen and have to ride the alternate-screen
  swap; resize reflows assuming full width, so margins would reset on resize as xterm's do. Against
  that: essentially nothing emits DECSLRM outside a conformance suite. So the answer is still no, on
  price.

  What **§57** changed is the cost of *refusing* it. DECSLRM shares its final byte with save-cursor,
  and `vte`'s arm for that byte ignores its parameters, so the refusal was not free: `CSI 5;70 s`
  *saved the cursor*, overwriting a value the program meant to restore from later. cmote now cancels
  that byte in flight, so the request does nothing at all — the `s` row in §8, PLAN §57, and
  `term/cancel.rs`.
- **~~VT420 rectangular ops~~ — SHIPPED in §58.** DECERA (`$ z`), DECSERA (`$ {`), DECFRA (`$ x`) and
  DECCRA (`$ v`) all read as engine limits until §56 built the hard half of them: writing cells
  straight into the grid, and knowing which of them a program protected. `vte` matches `$` only in the
  two DECRQM spellings, so all four fall through unhandled and are cmote's — a grammar, some clamping
  arithmetic and four small methods (`term/rect.rs`). One limit is disclosed rather than solved:
  **origin mode is refused**, because with DECOM set the corners count from the top of the scrolling
  region and the engine keeps that region private. See PLAN §58.
- **DRCS soft fonts and the VT320 status line** some conformance suites block on. `[DEC]`.
  ~~The DECRQCRA checksum query (`* y`)~~ **SHIPPED in §60**: it was §33's kind of work rather than
  §58's, and it is worth nothing unless the four digits match, so the algorithm was **copied** from
  xterm's `xtermCheckRect` at its DEC-compatible default rather than derived from the shape of the
  sequence. Three divergences are named rather than papered over — blink has no engine flag, a cell
  written through a DEC charset designation reaches the grid already translated, and the engine cannot
  tell a never-written cell from a written blank. ~~The attribute half
  of the rectangular family — DECCARA / DECRARA (`$ r` / `$ t`) and the DECSACE (`* x`) that picks
  their extent~~ **SHIPPED in §59**, on the geometry §58 had already built: a selector list folded to
  three masks at parse time, then applied a named bit at a time so a cell keeps its italics, its
  underline style and cmote's DECSCA protection bit. **Blink is read and dropped** — the engine's
  flag word has no bit for it.
- **Synchronized output `?2026`** — the **vte parser batches** the run between `?2026h` and
  `?2026l` (`vte-0.15.0/src/ansi.rs` BSU/ESU), but `alacritty_terminal`'s mode handler is a no-op
  (`SyncUpdate => ()`) and DECRQM reports it reset. cmote already paints atomically from the grid
  each frame, so the visible effect is nil either way. `[community]`, low pri.

---

## 6. Deliberately excluded (🛑 / 🤷 in §8 — policy, not gap)

**OSC 52 clipboard read/write** — **refused at the engine boundary** since §63: `engine_config` sets
`osc52: Osc52::Disabled`, so `clipboard_store` and `clipboard_load` return before an event exists. A
remote could read or poison the local clipboard, and cmote touches the clipboard only on an explicit
*local* action (§9 / §12 / §23).

Worth recording what this replaced, because the outcome never changed and the *statement* did. Until
§63 the field sat at its `Config::default()` value, `Osc52::OnlyCopy` — which upstream documents as "a
compromise between entirely disabling it (the most secure) and allowing paste", and a compromise is not
a refusal. A remote's write was therefore parsed, base64 and all, and raised as
`Event::ClipboardStore`, and what kept it off the clipboard was the **catch-all arm** of
`Replies::send_event` discarding an event it does not recognise. Correct, and correct for as long as
cmote has existed — but a fall-through says nothing, so nothing failed if a later edit started handling
that event. The catch-all is still there as the second line; the decision is now in the field, and a
test (`the_engine_is_told_to_refuse_the_remote_clipboard`) fails if the field goes.
The **bell** is dropped for the same "no remote-driven side effects" reason. Answering an OSC 52 read
query would be an injection vector and stays out.

That bell is the **🛑** in §8, and §65 had to correct its mark to say so: it had been a ⚠️ reading
"accepted, silent", as though nothing had decided anything. Something did. `vte` dispatches `BEL` and
`alacritty_terminal` implements it — `bell()` is `self.event_proxy.send_event(Event::Bell)` — so the
event genuinely arrives and cmote's catch-all drops it. It is the last refusal in this document standing
on a fall-through alone: OSC 52 got a config field in §63, and the colour sets have a renderer that
structurally cannot read them, but a bell has neither.

**A blinking cursor** is refused on price rather than on principle, and it is cmote's own refusal
(§8's `12 (the blink)`). A remote's `CSI ? 12 h` is tracked by the engine and reported back by DECRQM,
but nothing draws it: `term/screen.rs`'s `CursorShape` deliberately carries no blink and cmote runs no
animation timer, so the cursor is always steady — the same call §4 makes for SGR blink and DECSCUSR's
blinking shapes. The cost is admitted: a program that blinks the cursor to draw the eye gets a steady
one, and DECRQM will tell it the mode is set, which is true of the mode and not of the screen.

**Remote colour *set* requests** — `OSC 4;n;<spec>`, `OSC 10 / 11 / 12` with a value, and the resets
`OSC 104 / 110 / 111 / 112`. The theme is chrome the **user** chose and cmote owns, so a remote does not
repaint it. Worth stating precisely what happens, because "ignored" is not quite it: the engine's
`set_color` **records** the value in its own colour table, and nothing in cmote ever reads that table —
`ui/grid.rs` paints from `palette` alone, through a style resolver that is never handed a terminal to
ask, and `report_color` answers queries from the same const table. So the set is stored where no reader
exists. Harmless, and invisible.

Until §64 this paragraph added that a set "costs a full-screen repaint", which is **not true** and is
worth correcting rather than quietly dropping: `set_color` does call `mark_fully_damaged`, but that is
one bool (`self.damage.full = true`) in the engine's own damage tracking, and cmote calls neither
`damage()` nor `reset_damage()` anywhere — it repaints because bytes arrived, as it would for any
output. The marginal cost of a refused colour set is that bool. A refusal this cheap needs no cost
argument, and inventing one made the policy look like a performance trade it is not.

**Since §64 the refused half is pinned**, in both spellings:
`a_palette_colour_set_does_not_move_the_query_answer` sets slot 3 to red, asserts the engine *did*
record it (`term.colors()[3].is_some()`, so the test cannot pass merely because the set was never
parsed), and then asserts `OSC 4;3;?` still answers the scheme's yellow;
`a_default_colour_set_does_not_move_the_query_answer` does the same through `OSC 11`. The renderer's
half needs no test — the resolver has no route to the engine's table — but the *reply* half was resting
on nobody happening to wire `Term::colors` into `report_color`, which is the §63 lesson one column over.

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

**iTerm2's OSC 1337 namespace, key by key** — this one is not a single decision, and treating it as one
would have been a mistake with teeth. OSC 1337 is a `key=value` grab-bag about twenty keys deep sharing
one OSC number, and **two of those keys are refusals above, in a different costume**: `Copy=<base64>` is
an OSC 52 clipboard write, and `SetProfile=` / `SetColors=` are a theme repaint.
`SetBackgroundImageFile=` is both, plus a remote naming a file for cmote to decode (§41). A generic
"support OSC 1337" would have reopened all three silently.

So `term/iterm.rs` is an **allow-list**: a key not named in it produces nothing, which means a key
iTerm2 adds tomorrow is refused by default rather than by anyone remembering to refuse it. Each
dangerous key is additionally pinned by a test **by name**, so the refusal is checked rather than
intended. `StealFocus` and `RequestAttention=` are refused on §54's line — the effect escapes the tab;
note that `RequestAttention` flashing the taskbar button is an *interrupt demand*, which is why it is
refused while §54's progress on the same button is not. `ClearScrollback` destroys the user's own record
of the session. Honoured: `SetMark` (a bookmark, additive over §34) and `CurrentDir=` (a third cwd
spelling). Details in PLAN §55.

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
an unrealised nicety being declined, not a behaviour being removed. And note who does the declining:
`vte` dispatches OSC 22 to `set_mouse_cursor_icon`, a `Handler` method `alacritty_terminal` leaves at
its empty default, so the sequence never reaches cmote at all. The reasoning above is why cmote would
refuse it; nothing in cmote currently has to. That is the **🤷** in §8.

**Answerback (ENQ `0x05`)** — refused for the same reason, and this is why xterm ships it empty too
(§36). The trigger is a *single ordinary byte*, so any binary output that happens to contain `0x05`
— a `cat` of a binary, a corrupt download, a stray progress stream — would type the answerback string
into the shell's input as if the user had. That is a remote-driven side effect on the user's keyboard
in exchange for legacy identification nobody asks for; the DA / DECRQM / XTVERSION / DA3 answers cover
every probe a modern program makes.

The refusal is a **🤷** rather than a **🛑** in §8, and precisely because it is: `vte`'s `execute`
matches HT / BS / CR / LF / VT / FF / BEL / SUB / SI / SO and drops `0x05` to a `debug!`, and cmote's
own scanner has no arm for it either. Answerback is refused by never having been written, which is a
decision this section stands behind and no code enforces.

---

## 7. Recommendation

**There is no A-sized item left.** Input (§2), query→reply (§3) and the rendering/attribute layer
(§4) are all closed; what remains is the engine's own ceiling (§5) and the two sequences cmote refuses
on purpose (§6). §60's audit briefly reopened §3 — `CSI ? 4 m` went unanswered — and §61 closed it in a
scanner arm and a two-parameter reply, which is what a gap found by reading a trait impl rather than by
hitting it tends to cost. §36 closed the last four items — DA3 and DECKPAM by writing them, answerback and
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

**§65 swept the ⚠️ rows and turned up one item of real work.** Auditing all ten the way §60 taught —
`vte`'s dispatch arms first, then which `Handler` methods the engine leaves at their empty default —
split seven into an ✅ half and a refused-or-missing half, re-marked `BEL` as a 🛑, and confirmed two
(DECRQSS, XTGETTCAP) as plainly partial. The finding is **mode 2026**: `vte` batches a synchronized
update in its own `Processor` and bounds a stuck one with a 150 ms timeout, but that expiry is the
application's to drive (`sync_timeout()` then `stop_sync()`), and cmote drives neither. A remote that
sends `CSI ? 2026 h` and then stops writing holds the visible screen at its pre-BSU state until it sends
the closing `l` or pushes 2 MiB. Nothing leaks and the session is unharmed — a stuck picture, not a stuck
client — but it is a remote-triggered effect on cmote's own window, which is the thing §6 spends its
length refusing. The fix is small and has a shape already in the codebase: an `iced::window::frames()`
subscription while an update is pending, the way `SnackbarTick` and `QuitTick` are driven, calling
`stop_sync` once the instant passes. Not taken in §65, which was an audit.

What is left in §5 (blink, double-height lines, left/right margins, rectangular ops, synchronized output,
and the PNG/JPEG-carrying kitty and iTerm2 image protocols) is legacy, rare, invisible in practice, or a
decoder dependency — **no item of real UX value remains anywhere in this document.**

**One line of hardening surfaced in §62 and was taken in §63.** Re-deriving every refusal from the
crates showed that cmote had been leaving `alacritty_terminal`'s `config.osc52` at its default,
`Osc52::OnlyCopy` — chosen upstream as *"a compromise between entirely disabling it (the most secure)
and allowing paste"*, which is not the same thing as a refusal. So an OSC 52 **write** was parsed,
decoded and raised as `Event::ClipboardStore`, and the thing that stopped a remote poisoning the local
clipboard was the catch-all arm of `Replies::send_event`. The outcome was correct and always had been;
what was missing is that the refusal was *inherited* rather than stated — a future upstream default, or
a `Config` touched for an unrelated reason, would change it with nothing failing. `engine_config` now
sets `osc52: Osc52::Disabled`, which moves the refusal to the engine boundary, makes the **read**
refusal explicit instead of a side effect of `OnlyCopy`, and gives the `refuses_*` family in
`term/iterm.rs` a sibling to sit beside. The `Config` moved out of `Terminal::new` into a named
function to make that assertable at all, so the kitty-keyboard flag beside it is now pinned too.

**§64 then closed the smaller version of the same hole, one column over.** The two ⚠️ colour rows were a
working query and a refused set under one mark — now four rows, `(query)` ✅ and `(set)` 🛑 apiece — and
the refused half had no test — it was held up by the
renderer's structure (`ui/grid.rs` cannot reach the engine's colour table) plus the fact that nobody had
wired that table into `report_color`. The first of those is a real guarantee; the second is the §63
pattern exactly, a correct outcome nothing asserts. Two tests now set a colour and prove the answer does
not move, and each asserts the engine *did* record the set first, so neither can pass because the set was
silently dropped on the way in. The same pass deleted a cost this document had invented for the refusal
(see §6): the full-screen repaint a colour set was said to cost does not happen, because cmote never
reads the engine's damage flags. Nothing shipped and **no answer changed** — the two ⚠️ rows became a
✅ and a 🛑 apiece because that is what they always were, and the rows just stopped
claiming more than they knew.

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

**§54 then closed the OSC column's last item of real value, and turned four ❌ rows into decisions** —
🛑 and 🤷 rows since the legend grew marks of their own — and the split was instructive: OSC 9 is
refused by cmote's own scanner and pinned by a test, while `OSC 777` and `kitty 99` are refused by
nobody at all, since `vte` has no arm for either.
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
"still-missing" lens of §2–§6. Every ✅/⚠️/❌/🛑/🤷 below was verified against the real sources — the
engine crate (`alacritty_terminal-0.26.0`), its parser (`vte-0.15.0`), and cmote's own layer
(`term/`, `ui/grid.rs`) — not from memory.

Legend: **✅** full · **⚠️** partial or a deliberate quirk · **❌** not supported · **🛑** refused, by
cmote's own code · **🤷** refused in principle, by nothing in particular.

The last three are different in kind, which is why each carries its own mark rather than one mark and a
footnote:

- **❌** is a *gap* — a sequence that could still land, and several since have.
- A code whose **two halves deserve opposite marks gets two rows**, not one averaged mark: `OSC 52` has
  been split into `(write)` and `(read)` since §62, and §64 split `OSC 4` and `OSC 10 / 11 / 12` into
  `(query)` and `(set)` the same way. "Partial" on its own hides which half a program can rely on, and
  these halves are not partial in either direction — the query is answered in full and the set is refused
  on purpose. **§65 swept the rest of the ⚠️ rows** and split seven more, in every pairing the marks
  allow: ✅/🛑 (`SetUserVar`, mode 12), ✅/🤷 (mode 3, mode 80) and ✅/❌ (`CSI ! p`, the locking shifts,
  mode 2026). Two ⚠️ rows survive that sweep, DECRQSS and XTGETTCAP, and both are partial in the plain
  sense: one answer, given for some inputs and honestly declined for the rest.
- **🛑** is a *decision* recorded in §6 that **cmote enforces**: a scanner allow-list, an event dropped
  in the listener, a renderer that never reads the value. It never becomes work, and the row names the
  code that performs it — usually with a test pinning it by name, so the refusal cannot regress
  unnoticed.
- **🤷** is the same decision with **nothing behind it**: the sequence dies upstream — no `vte` dispatch
  arm, or a `Handler` method `alacritty_terminal` leaves at its empty default body — so cmote is never
  offered it and pays nothing to refuse it. The row names *where* it dies. These are stances, not
  guarantees: an engine bump could start handing the sequence over, and then the listener's catch-all is
  the only thing standing there. The distance between agreeing with a refusal and performing one is what
  §57 is about, and it is worth seeing in the column rather than reading for.

### OSC — Operating System Command

| Code | Feature | Status | Note |
|---|---|---|---|
| 0 | Icon name + window title | ✅ | title shown; icon name dropped (`term/mod.rs`) |
| 1 | Icon name alone | ❌ | no arm in `vte` — `0` and `2` both map to the title handler and `1` matches nothing at all, so it does nothing. Nothing is lost: cmote shows no icon name anywhere, so there would be nowhere to put it |
| 2 | Window title | ✅ | control chars stripped (anti-spoof) |
| 4 (query) | Palette entry query | ✅ | answered from cmote's scheme: `report_color` resolves the slot through the shared const table `ui/grid.rs` paints from, so the answer never disagrees with the screen. Pinned by `a_palette_colour_query_reports_that_slot` |
| 4 (set) | Palette entry set | 🛑 | the theme is chrome the **user** chose (§6) — the same refusal row 104 carries for the reset side. The engine records the value in `Term::colors` and nothing in `src/` reads that table; `ui/grid.rs` paints through a style resolver that is never handed a terminal, so the renderer's half of the refusal is structural. Since §64 the reply's half is pinned by `a_palette_colour_set_does_not_move_the_query_answer`, which proves the engine stored the set before asserting the answer ignores it |
| 7 | Working directory | ✅ | cmote's own scanner (`term/cwd.rs`, §17) |
| 8 | Hyperlinks | ✅ | rendered + Ctrl-click; web/mail only (`link.rs`, §24) |
| 9 | Desktop notification | 🛑 | a notification leaves the window and lands on the desktop (§6, §54). cmote's own scanners perform this one: `term/progress.rs` matches `9;4;` and `term/cwd.rs` matches `9;9;`, so a bare `9;<text>` is *seen* and declined — pinned by `the_other_osc_nine_sequences_are_left_alone`. `vte` has no OSC 9 arm, so the engine would never have offered it either |
| 9;4 | Progress reporting | ✅ | per-tab bar on the chip + the taskbar button mirrors the active tab (`term/progress.rs`, §54); all five states, share clamped |
| 9;9 | Working directory (ConEmu) | ✅ | the **Windows** spelling — a bare native path, sometimes quoted — read beside OSC 7 and iTerm's `CurrentDir` in the one scanner (`term/cwd.rs`, §17). This row was missing until §60's audit, which is odd company for the spelling a Windows client is likeliest to meet |
| 10 / 11 / 12 (query) | Default fg / bg / cursor colour query | ✅ | scheme-accurate — `report_color` resolves against `palette`, the same source `ui/grid.rs` paints from; cursor reports the **fg**, since the cursor is drawn by inverting the cell. The half programs actually use: `OSC 11 ?` is how one picks a light or dark colourscheme to suit the terminal. Two tests pin it, one per role |
| 10 / 11 / 12 (set) | Default fg / bg / cursor colour set | 🛑 | same fixed scheme as `4 (set)` (§6), and it costs nothing to ignore: this was said to cost "a full repaint for no change" until §64, but `mark_fully_damaged` sets one bool and cmote calls neither `damage()` nor `reset_damage()`, so that repaint never happened. Pinned by `a_default_colour_set_does_not_move_the_query_answer` |
| 22 | Mouse pointer shape | 🤷 | the pointer is window-wide chrome and already contested by four of cmote's own shapes (§6) — but **no cmote code performs this refusal**: `vte` dispatches OSC 22 to `set_mouse_cursor_icon`, a `Handler` method left at its empty default body, which `alacritty_terminal` never overrides, so the sequence dies in the engine and cmote is never offered it. A decision cmote would make and a cost it does not pay; nothing enforces it and no test pins it, unlike `term/iterm.rs`'s `refuses_*`. If an engine bump ever raised an event for it, the listener's catch-all would drop it — the outcome is robust, the *reason* is unpinned |
| 50 | Cursor shape (`CursorShape=`) | ✅ | a **third** spelling of DECSCUSR's shape, and the one that arrives for free: `vte` dispatches it to `set_cursor_shape`, which writes the same `cursor_style.shape` DECSCUSR writes, and `term/screen.rs` reads that field. Block / bar / underline, with no blink to drop — this spelling has none. Undocumented until §60's audit found it working |
| 52 (write) | Clipboard write | 🛑 | remote must not poison local clipboard (§6). Refused **at the boundary and again behind it** since §63: `engine_config` sets `osc52: Osc52::Disabled`, so `clipboard_store` returns before an event exists, and the catch-all arm of `Replies::send_event` would still drop the event if it ever arrived. Until §63 only the second of those was true, the field sitting at the crate's `OnlyCopy` default — the weakest 🛑 in this table, now the most explicit |
| 52 (read) | Clipboard read | 🛑 | remote must not read local clipboard (§6) — the same two lines, and the direction where being explicit matters most: `OnlyCopy` refused the read as a *side effect* of allowing the write, so the read's refusal was never stated anywhere. `Disabled` states both |
| 104 | Reset palette entry | 🛑 | no effect — the reset side of the fixed scheme (§6): the engine restores its own table and `ui/grid.rs` never reads it |
| 110 / 111 / 112 | Reset fg / bg / cursor colour | 🛑 | no effect — same fixed scheme (§6), named there beside the sets it undoes |
| 133 | Shell integration (semantic prompts) | ✅ | scanner (`term/osc133.rs`, §34): per-tab status dot + jump-to-prompt + select-command-output; A/B/C/D tracked, exit code from D |
| Kitty 21 | Colour by semantic name | 🤷 | same fixed scheme as 4 / 10 / 11 / 12 — the theme is cmote's, not the remote's (§6) — but nothing performs the refusal: `vte`'s OSC arms are `0`/`2`, `4`, `8`, `10`–`12`, `22`, `50`, `52`, `104`, `110`–`112` and **nothing else**, so OSC 21 reaches no handler, and cmote has no scanner for it |
| Kitty 99 | Rich notifications | 🤷 | a notification, in a third spelling (§6, §54) — and, unlike OSC 9, one nothing here declines: no `vte` arm, no cmote scanner |
| iTerm 1337 File | Inline images | ❌ | a PNG/JPEG payload, so it needs an image-format decoder — cmote's own images are sixel, which needs none (§5, §41) |
| iTerm 1337 `SetMark` | Explicit bookmark on a line | ✅ | amber gutter tick + Ctrl+Shift+Up/Down (`term/iterm.rs`, §55); additive over §34, whose marks are prompt-derived and cannot mark mid-output |
| iTerm 1337 `CurrentDir` | Working directory | ✅ | third spelling, read beside OSC 7 / 9;9 (`term/cwd.rs`, §55) |
| iTerm 1337 `SetUserVar=gitBranch` | Per-session variable | ✅ | shown as a pill on the chip (§55). Base64-decoded, UTF-8 checked, control chars stripped, capped at 32 chars — counted in `chars`, so a multi-byte branch name is cut at a character boundary and cannot panic — and drawn BESIDE the endpoint label so it cannot pass for the host. A value that fails to decode leaves the pill alone instead of clearing it (`a_value_that_will_not_decode_leaves_the_branch_alone`), the same rule §54 gives progress |
| iTerm 1337 `SetUserVar` (any other name) | Per-session variable | 🛑 | §55's allow-list applied a second time, to the NAMES: `term/iterm.rs` matches the name whole against `gitBranch` **before decoding anything**, so there is deliberately no map for a remote to fill — and with no title template there would be no reader for a second name anyway. Pinned by `only_the_one_honoured_variable_name_is_kept`, which rejects `kubeContext`, `gitBranchy` and a bare value with no name (§65) |
| iTerm 1337 `Copy` | Clipboard write | 🛑 | **OSC 52 write by another name** (§6, §55); pinned by a test so the refusal cannot regress |
| iTerm 1337 `SetProfile` / `SetColors` | Theme repaint | 🛑 | the fixed-scheme refusal in a new costume (§6, §55) |
| iTerm 1337 `SetBackgroundImageFile` | Background image | 🛑 | a theme repaint **and** a remote naming a file to decode (§6, §41, §55) |
| iTerm 1337 `StealFocus` / `RequestAttention` | Raise / flash the window | 🛑 | the effect escapes the tab (§6, §54, §55) |
| iTerm 1337 `ClearScrollback` | Drop the scrollback | 🛑 | destroys the user's own record (§55); `CSI 3J` is the sanctioned spelling |
| iTerm 1337 `CursorShape` / `ReportCellSize` | — | ❌ | redundant — but not by the route this row claimed until §60's audit. `CursorShape` is DECSCUSR **and** OSC 50 over again. `ReportCellSize` follows from `CSI 14t` ÷ `CSI 18t`, pixels over cells; **`CSI 16t` is not the alternative** — it has no arm in `vte` and cmote does not answer it, as the window-ops table says |
| iTerm 1337 (every other key) | — | 🛑 | `term/iterm.rs` is an **allow-list**, so an unvetted key does nothing by default (§55) |
| 777 | urxvt notification | 🤷 | a notification, in a fourth spelling (§6, §54); no `vte` arm, no cmote scanner |

### CSI — cursor movement & editing

| Code | Feature | Status | Note |
|---|---|---|---|
| A / B / C / D | Cursor up / down / fwd / back | ✅ | |
| E / F | Cursor next / prev line | ✅ | |
| G / H (+ f) | Absolute position | ✅ | HVP `f` too |
| I / Z | Forward / backward tab | ✅ | |
| d / \` | Vertical / horizontal PA | ✅ | |
| a / e | Horizontal / vertical PR | ✅ | the parser aliases HPR to CUF and VPR to CUD (`ansi.rs`), so they move but do not have their own arm |
| s / u | Save / restore cursor | ✅ | ANSI.SYS form. The **bare** `CSI s` only — a parametrised one is DECSLRM and is cancelled before the engine can mistake it for this (§57, below) |
| @ / P / X | Insert / delete / erase char | ✅ | |
| L / M | Insert / delete line | ✅ | |
| J | Erase in display | ✅ | |
| 3 J | Erase scrollback | ✅ | |
| K | Erase in line | ✅ | |
| Ps " q | Character protection (DECSCA) | ✅ | cmote's own scanner (`term/protect.rs`, §56); the engine has no arm for it, so protection rides a bit cmote borrows in the engine's per-cell flag word — invisible to both the engine and the renderer, and guarded at build time |
| ? J / ? K | Selective erase (DECSED / DECSEL) | ✅ | all three extents, applied by cmote in place (§56). Protected cells survive; a **plain** `CSI J` / `CSI K` still takes them, which is the point of two verbs |
| ! p (the DECSCA part) | Soft reset — protection | ✅ | cmote's own scanner drops DECSCA protection with the pen (`term/protect.rs`, §56), which is the one piece of soft-reset state cmote owns |
| ! p (everything else) | Soft reset — the rest | ❌ | `vte`'s `csi_dispatch` has `('p', [b'$'])` and `('p', [b'?', b'$'])` for DECRQM and **no arm for `('p', [b'!'])`**, so origin mode, autowrap, the keypad mode, cursor visibility, the scrolling region, the pen and the charset designations all survive a soft reset untouched. A **gap**, not a policy — nothing here refuses it, and `ESC c` (RIS) does have an arm, so a program that wants state cleared has a spelling that works (§65) |
| b (REP) | Repeat character | ✅ | handled in the vte parser (`ansi.rs`) |
| S / T | Scroll up / down | ✅ | |
| r (DECSTBM) | Scrolling region (top / bottom) | ✅ | vertical only |
| s (DECSLRM) | Left / right margins | ❌ **safely** | the margins themselves stay out, on **price rather than capability** — there is a seam (`Processor::advance` is generic over `Handler`, which `Term` merely implements), but a delegating wrapper over a 71-method trait whose every method has a default empty body degrades **silently** on an engine bump, and nothing emits DECSLRM outside a conformance suite. Costed in §5. What §57 fixed is the **collision**: `vte`'s `('s', [])` arm is save-cursor and ignores its parameters, so `CSI Pl;Pr s` used to *save the cursor*, overwriting the one saved-cursor slot the program had its own value in. cmote now cancels that final byte before the engine sees it (`term/cancel.rs`), so a margin request does nothing at all — which is what "unsupported" should mean |
| g | Tab clear | ✅ | |
| ? 5 W | Tab stops every 8 columns (DECST8C) | ❌ | **parsed and dropped** — `vte` calls `set_tabs`, and `alacritty_terminal` never overrides the empty default (§5) |
| Ps SP k | Select character path (SCP) | ❌ | **parsed and dropped** — same shape: `vte` calls `set_scp`, the engine never overrides it. Bidi anyway, which cmote does not do |
| $ z (DECERA) | Erase rectangular area | ✅ | cmote's own scanner and cell writer (`term/rect.rs`, §58) — the engine matches `$` only in DECRQM, so all four of this family fall through unhandled. Corners default to the page edges, an end past the edge clamps, and a rectangle described backwards or starting off the page is a **no-op** rather than one cmote invents by swapping corners |
| $ { (DECSERA) | Selective erase rectangular area | ✅ | the same rectangle by the selective verb (§58): protected cells stand, and the plain `$ z` still takes them. This was the piece §56 unblocked and left unbuilt — the per-cell protection it needed already existed |
| $ x (DECFRA) | Fill rectangular area | ✅ | one character across a box, stamped from the **pen**, so the fill carries the colours and attributes a printed glyph would have (§58). `Pch` is an **allow-list** — 32–126 and 160–255, as xterm allows — so a remote cannot paint the page with C0, C1, DEL or unassigned code points |
| $ v (DECCRA) | Copy rectangular area | ✅ | whole cells move, so colour, attributes, the OSC 8 link and DECSCA protection travel with the glyph (§58). The source is read out **whole first**, because the overlapping case — scroll a sub-window by copying it over itself — is what the sequence is for. A copy running off the page is trimmed to what fits; the two page parameters are ignored, cmote having one page |
| Ps * x (DECSACE) | Attribute change extent | ✅ | picks which shape the pair below act on: `0` / `1` the wrapped **stream** between two points (the default, and what a terminal powers up in), `2` the **rectangle** (§59). Absorbed by cmote's scanner rather than reported — it is a mode, and only the scanner sees a mode and the requests it governs in stream order, so each one leaves carrying the extent that was in force. A value DEC never defined leaves the mode where it was. RIS resets it; DECSTR does not, DEC's published list for that not naming it. Note the intermediate: `* x` is this, `$ x` is DECFRA |
| $ r / $ t (DECCARA / DECRARA) | Change / reverse attributes in a rectangle | ✅ | the attribute half of the family §58 shipped the content half of (§59) — corners first, then a small DEC-defined selector list, folded to three masks at parse time so the walk costs the same however long the list. `$ r` sets and clears (`0 1 4 5 7 22 24 25 27`, later wins); `$ t` flips (`0 1 4 5 7` only — "off" has no meaning for a verb that flips). Attributes only: never a colour, never a glyph, and **never the flag word wholesale**, which would take cmote's DECSCA protection bit with it (§56). An unknown selector is ignored and the rest of the list still applies, as an SGR does; a malformed *number* still drops the sequence. Blink is parsed and dropped — the engine has no bit for it |
| Pid;Pp;Pt;Pl;Pb;Pr * y (DECRQCRA) | Rectangle checksum | ✅ | the one sequence in the family that answers rather than acts (`term/rect.rs`, §60). The algorithm is **xterm's `xtermCheckRect` at its DEC-compatible default**, copied rather than derived: each cell weighs its character code plus 0x04 protected / 0x08 hidden / 0x10 underline / 0x20 reverse / 0x80 bold, a plain space is trimmed unless it is the rectangle's first cell, and the total is negated and reported as `DCS Pid ! ~ XXXX ST`. The corners start at parameter **2**; the page number is ignored, cmote having one. Answered from the page as it stood **where the question sat**, and clamped to the visible page, so the scrollback cannot be read through it. Refused a rectangle under origin mode like the rest of the family — but still answered, with the checksum of no cells, because a query dropped on the floor stalls the program that asked. **Blink never lands** (no engine flag), a DEC-charset cell weighs its Unicode point, and a never-written cell reads as a written blank |
| Ps SP q (DECSCUSR) | Cursor style | ✅ | block / underline / bar; blink dropped |
| 5n / 6n | Device status report | ✅ | |
| c / > c | Primary / secondary DA | ✅ | unblocks vim / tmux startup; since §41 cmote amends the engine's DA1 to add attribute **4**, so programs know it draws sixels (`term/query.rs`) |
| = c | Tertiary DA | ✅ | answered by cmote's scanner with a constant unit id (§36) — this row read ❌ until §41 spotted it, having been left behind when §36 shipped it |
| ? Pi;Pa;Pv S | Graphics attributes (XTSMGRAPHICS) | ✅ | colour registers and max image size, from the decoder's real limits (§41) |
| Ps $ p / ? Ps $ p | Request mode (DECRQM) | ✅ | engine answers **both** spellings — the ANSI one (`CSI 4 $ p` → `CSI 4;2$y`, insert mode reset) as well as the private one. This row named the private form alone until §60's audit |
| # p / # q | Colour palette stack (XTPUSHCOLORS / XTPOPCOLORS) | 🤷 | downstream of the fixed scheme (§6): a stack over a palette that is never read has nothing to save or restore, so ignoring push, set and pop alike is consistent rather than lossy. `vte` has no CSI arm for either final byte, so — as with OSC 22 — this is a decision cmote agrees with rather than one it carries out |

### ESC — single sequences

| Code | Feature | Status | Note |
|---|---|---|---|
| ESC D / ESC M | Index / Reverse index | ✅ | |
| ESC E | Next line | ✅ | |
| ESC H | Set tab stop | ✅ | |
| ESC 7 / ESC 8 | Save / restore cursor | ✅ | |
| ESC c (RIS) | Full reset | ✅ | |
| ESC = / ESC > | Keypad app / numeric | ✅ | tracked, and encoded for the numpad keys with no NumLock meaning (Enter, `* + , - / =`); digits deliberately keep their NumLock behaviour (DECKPAM, §2, §36) |
| ESC #8 (DECALN) | Screen alignment test | ✅ | |
| ESC ( / ) / * / + | Designate charset G0 / G1 / G2 / G3 | ✅ | all four slots — `configure_charset` maps the four intermediates — but only ASCII (`B`) and DEC line drawing (`0`); any other final falls to `unhandled!()`. Designating G2 / G3 works and is inert in practice, since nothing can invoke them (below) |
| ESC N / ESC O | Single shift G2 / G3 | ❌ | |
| SI / SO (LS0 / LS1) | Locking shift G0 / G1 | ✅ | `vte`'s `execute` maps SI to `set_active_charset(G0)` and SO to G1 — the two spellings anything in practice uses |
| LS2 / LS3 / LS1R / LS2R / LS3R | The other locking shifts | ❌ | no `esc_dispatch` arm for `n`, `o`, `~`, `}` or `\|`, so each reaches no handler. With SS2 / SS3 missing too (above), G2 and G3 can be designated and never invoked — a gap nobody here declines (§65) |
| ESC #3–6 | Double-height / width lines | ❌ | not represented (§5) |
| ESC SP F / G | 7 / 8-bit control output | ❌ | |
| ESC % G | UTF-8 charset | ✅ | engine is always UTF-8 |

### DCS — Device Control String

| Code | Feature | Status | Note |
|---|---|---|---|
| DCS $ q (DECRQSS) | Request status string | ⚠️ | genuinely partial, and honest about it — the mark stands after §65's audit. **Every** request draws a reply, but only `m` (SGR) carries data, rebuilt from the live pen so it is what the grid paints. Every other setting (DECSTBM, DECSCUSR, DECSCA…) answers `DCS 0 $ r ST`, the standard's "I do not report that", rather than a guess (`term/query.rs`, §33) |
| DCS + q (XTGETTCAP) | Termcap query | ⚠️ | same shape, and it also stands: every request answered, two capabilities stated — `TN` (`xterm-256color`, the name cmote requested for the pty) and `Co` / `colors` (256). The rest reply unknown on purpose, since their wire values are ambiguous and 24-bit SGR works whether or not a capability query confirms it (§33) |
| CSI > q (XTVERSION) | Terminal version | ✅ | replies `cmote(<ver>)` (`term/query.rs`, §33) |
| CSI = c (DA3 → DECRPTUI) | Tertiary device attributes | ✅ | replies a **constant** unit id `00434D45`, never a machine-derived one (`term/query.rs`, §36) |
| DCS … q | Sixel graphics | ✅ | decoded in-house and composited over the grid; the picture is anchored to an absolute document line and reserves its cells (`term/sixel.rs`, `term/graphics.rs`, §41). The alternate screen has its own page of them, on the same coordinate with the history at zero — so `ranger` previews and `mpv --vo=sixel` draw |
| DCS tmux; … | tmux passthrough | ❌ | |

### SGR — text styling

| Code | Attribute | Status | Note |
|---|---|---|---|
| 1 | Bold | ✅ | |
| 2 | Dim / faint | ✅ | faded toward bg |
| 3 | Italic | ✅ | bundled IBM Plex Mono face |
| 4 | Underline | ✅ | |
| 5 / 6 | Slow / rapid blink | ❌ | **dropped by the engine** — `vte` parses it, `alacritty_terminal` has no arm and no cell flag, so it never reaches cmote (§5, §36) |
| 7 | Reverse video | ✅ | |
| 8 | Hidden / conceal | ✅ | copy still yields the text |
| 9 | Strikethrough | ✅ | |
| 21 / 4:2 | Double underline | ✅ | |
| 4:3 / 4:4 / 4:5 | Curly / dotted / dashed underline | ✅ | drawn as our own quads |
| 53 | Overline | ❌ | not carried |
| 30–37 / 40–47 / 90–97 / 100–107 | 16 ANSI colours | ✅ | |
| 38;5 / 48;5 | 256-colour indexed | ✅ | |
| 38;2 / 38:2 | Truecolor (`;` and `:`) | ✅ | both spellings |
| 58;5 / 58;2 | Underline colour | ✅ | |

### DECSET / DECRST private modes

| Code | Mode | Status | Note |
|---|---|---|---|
| 1 | Application cursor keys | ✅ | arrows send SS3 |
| 3 (side effects) | DECCOLM's clear | ✅ | the engine's `deccolm` resets the scrolling region and clears the grid — what the sequence is actually used for |
| 3 (column resize) | 132 / 80 columns | 🤷 | not performed, and not by cmote: the engine's own comment is *"setting 132 column font makes no sense, but run the other side effects"*, and its DECRQM answers `NotSupported`. cmote owns its tabbed window and would refuse a remote resize on the same grounds as `CSI 1–10 t` (§6) — but it is never asked (§65) |
| 5 (DECSCNM) | Global reverse video | ❌ | |
| 6 | Origin mode | ✅ | |
| 7 | Auto-wrap | ✅ | |
| 12 (the mode) | Blinking cursor — tracked | ✅ | the engine sets `cursor_style.blinking` and DECRQM reports it back |
| 12 (the blink) | Blinking cursor — drawn | 🛑 | cmote runs no animation timer, so the cursor is always steady, and both lines of that are cmote's own: `term/screen.rs`'s `CursorShape` deliberately carries no blink, and the engine's `Event::CursorBlinkingChange` lands in the catch-all arm of `Replies::send_event`. Worth knowing about the ✅ above: DECRQM will report the mode **set**, which is true of the mode and false of the screen — cmote does not intercept the engine's reply to soften it (§65) |
| 25 | Show / hide cursor | ✅ | |
| 45 | Reverse wrap | ❌ | |
| 69 (DECLRMM) | Left / right margin | ❌ | not in the engine's mode list, so setting it is ignored and DECRQM answers `0`, "not recognised" — the honest reply, and the one that tells a conformant program not to spell `CSI s` as DECSLRM. §57 covers the program that sends it anyway |
| 80 (behaviour) | Sixel scrolling | ✅ | cmote always scrolls — the modern default, and what emitters assume (§41) |
| 80 (the mode) | DECSDM | 🤷 | `vte`'s `NamedPrivateMode` has no 80, so the engine takes it as `PrivateMode::Unknown(80)`, logs "ignoring unknown mode" and returns; DECRQM answers `NotSupported`, which is the honest reply. A program that sets DECSDM to *stop* scrolling does not get that, and nothing here declines it (§65) |
| 1000 / 1002 / 1003 | Mouse: normal / btn / any | ✅ | `term/mouse.rs` |
| 1004 | Focus events | ✅ | cmote sends CSI I / CSI O |
| 1006 | SGR mouse | ✅ | |
| 1007 | Alt-scroll | ✅ | |
| 1016 | SGR-pixel mouse | ❌ | |
| 1049 | Alternate screen | ✅ | no scrollback there, by design |
| 2004 | Bracketed paste | ✅ | with an injection scrub |
| 2026 (batching) | Synchronized output | ✅ | and it is `vte`'s `Processor` that does it, not the engine: BSU buffers the stream, ESU flushes it inside one `advance`, so a frame really is atomic. The engine's own mode arm is `()` and its DECRQM answers `Reset` — both correct |
| 2026 (abort timeout) | Synchronized output | ❌ | `vte` bounds a stuck update with `SYNC_UPDATE_TIMEOUT` (150 ms), but the expiry is the **application's** to drive — `Processor::sync_timeout()`, then `stop_sync()` — and cmote calls neither. A remote that sends BSU and then goes quiet holds the visible screen at its pre-BSU state until it sends ESU or pushes 2 MiB (`SYNC_BUFFER_SIZE`, which flushes). Found by §65's audit; see §7 |
| 2027 | Grapheme clustering | ❌ | |
| 2031 | Colour-scheme reporting | ❌ | |
| 2048 | In-band resize | ❌ | |
| 4 | Insert / replace (IRM) | ✅ | ANSI mode, not a `?` private one — hence out of the run above |
| 9 | X10 mouse (press-only) | ❌ | engine never implemented it |
| 20 | Newline mode (LNM) | ✅ | ANSI mode, not a `?` private one |

### Graphics, window ops, keyboard, C0

| Feature | Status | Note |
|---|---|---|
| Sixel images | ✅ | decoded and composited by cmote itself, no engine work (§41) |
| Kitty graphics protocol / unicode placeholders / animation | ❌ | its payloads are PNG/RGBA chunks, so it needs an image-format decoder — a dependency and a security decision, not a rendering gap (§5, §41) |
| ReGIS | ❌ | a vector language; no users worth an interpreter (§5) |
| iTerm2 inline images (OSC 1337) | ❌ | same reason as kitty: a PNG/JPEG payload (§5, §41) |
| Graphics capability report | ✅ | XTSMGRAPHICS (`CSI ? Pi;Pa;Pv S`) answered from the decoder's real limits — 256 registers, 4096×4096 / 4 Mpx; a *set* honestly refused (`term/query.rs`, §41) |
| Window iconify / move / resize / raise / maximize / fullscreen (CSI 1–10 t) | 🤷 | cmote owns its tabbed window; a remote can't drive it (§6) — and the mark moved here in the same pass that added it: `vte`'s `('t', [])` arm handles **14 / 18 / 22 / 23 only** and sends every other parameter to `unhandled!()`, so there is no `Handler` method for window manipulation at all. Nothing to refuse, nothing to pin |
| Window / position / state reports (CSI 11 / 13 t) | ❌ | |
| Text area in pixels / chars (CSI 14t / 18t) | ✅ | the two size *queries* are answered |
| Cell size (CSI 16 t) | ❌ | |
| Title stack (CSI 22 / 23 t) | ✅ | `push_title` / `pop_title` |
| **Kitty keyboard protocol** | ✅ | engine tracks the flag stack; cmote encodes CSI-u (`term/kitty.rs`, §25) |
| **xterm modifyOtherKeys** — set (`CSI > 4 ; n m`) | ✅ | scanned out of the stream by cmote (`term/modkeys.rs`, §9); the engine has no arm, this being an input-encoding hint rather than a screen operation |
| **xterm modifyOtherKeys** — query (`CSI ? 4 m`) | ✅ | answered `CSI > 4 ; Pv m` by the same scanner (§61) — the SET form, so a program can write the reply back to restore the state. Read as ❌ between §60's audit, which found `vte` dispatching it to a `report_modify_other_keys` the engine leaves at its empty default, and §61, which closed it. **Resource 4 only**: XTMODKEYS carries seven, cmote holds one, and the reply being an XTMODKEYS control leaves no way to say "not mine" — so the other six draw silence rather than an invented level. Answered where the question sits in the stream, not where the chunk ends |
| ENQ answerback | 🤷 | a lone `0x05` in binary output would type a string into the shell (§6, §36) — a decision cmote holds and nothing carries out: `vte`'s `execute` matches HT / BS / CR / LF / VT / FF / BEL / SUB / SI / SO and drops `0x05` to a `debug!`, and cmote's scanner has no arm for it. Answerback is refused by never having been written, which is the cheapest refusal in the document and the least pinned |
| BEL | 🛑 | accepted and **silent** by decision, not by absence: `vte` dispatches it and `alacritty_terminal` implements it as `Event::Bell`, so the event really arrives and the catch-all arm of `Replies::send_event` drops it (§6 — a remote may change what its own tab looks like and nothing more). The last refusal in this document riding a fall-through alone: unlike OSC 52 there is no config field to state it in, and unlike the colour sets there is no renderer that structurally cannot read it (§63, §65) |
| BS / HT / LF / CR | ✅ | |
| SO / SI | ✅ | charset shift |

**Shape of it.** The whole legacy VT100 / xterm core is ✅ — cursor motion, editing, SGR, full
colour, alternate screen, mouse, bracketed paste, focus, DA1 / DA2 / DSR / DECRQM, DECSCUSR, REP, the
kitty keyboard protocol, the application keypad, and — since §33, completed by §36 — every identity
query the engine dropped (XTVERSION, DECRQSS SGR, XTGETTCAP, DA3), and — since §56 — the VT220
protected-cell erase it dropped as well, and — since §58, §59 and §60 — the whole VT420 rectangular
family, checksum query included. The **deliberate** part of what is missing used to be most of the ❌
column and now carries two marks of its own. **🛑** is what cmote's code refuses and its tests pin:
the remote clipboard (OSC 52 both ways), desktop notifications in the OSC 9 spelling, the dangerous
half of iTerm's OSC 1337 namespace, and a fixed colour scheme that makes every palette set and reset a
no-op. **🤷** is what cmote would refuse and never gets the chance to: answerback, remote window
control (`CSI 1–10 t`), the remote pointer shape (OSC 22), the palette stack (`CSI # p / # q`), and
notifications in their other three spellings — each one dead in `vte` or in a `Handler` default before
cmote sees a byte. That leaves the plain ❌ column short, and worth reading as the real list: the
PNG/JPEG image protocols (a decoder dependency and a security decision, §41), blink (the engine drops
it), the newer private modes (2027 / 2031 / 2048) and left-right margins. That last one is no longer a *capability* gap at all: §5 costs out the
delegating-`Handler` build that would do it, and the reason it stays ❌ is that such a wrapper degrades
silently on an engine bump, in
exchange for a sequence nothing outside a conformance suite emits. Since §57 it is also a gap that
costs nothing to have, rather than one that quietly took the program's saved cursor with it. All
catalogued with their cost in §5 — which read as the *only* section with anything open in it until
§60's audit put one row back into §3.

§56 is worth reading as a method rather than a feature. Every earlier addition worked by scanning a
sequence out of the stream and keeping the answer BESIDE the grid — a cwd, an exit code, a picture's
anchor. Protection could not be kept beside the grid, because it is per-cell state that has to survive
scrolling and reflow, and a map of it would have meant re-implementing the grid to keep the two
aligned. So instead cmote borrowed the one unused bit in the engine's per-cell flag word and let the
engine carry protection as if it were bold. That is a third way in, next to "scan it out" and "accept
the engine's limit", and the reason DECSERA above is now a rectangle rather than a wall.

§57 found a fourth, and a different kind of gap to go with it. Every row in these tables until now was
some flavour of "the engine ignores this"; DECSLRM is the one where the engine ignores nothing and gets
it *wrong* — `vte` dispatches its final `s` to save-cursor without reading the parameters, so a margin
request cmote cannot honour was still costing the program its saved cursor. A sequence like that cannot
be scanned out and applied beside the grid, because the problem is not what cmote fails to do with it,
it is what the engine does. So `process` now cancels the offending byte in flight — advance up to it,
feed the state machine's own CAN in its place, resume after it. "Refuse it properly" is the fourth way
in, and the cheapest: a ❌ that costs nothing is worth more than most ✅s.

§60 closed the last row of §8's CSI table and then swept the two tables above it against the crate
sources, which turned up six rows the code and the doc disagreed about — and only one of them a gap.
Three sequences **worked and were not written down**: OSC 50 (a third spelling of the cursor shape,
arriving free through the same engine field DECSCUSR writes), OSC 9;9 (the Windows cwd spelling, of
all things to be missing from a Windows client's table) and the ANSI form of DECRQM. One row
**contradicted another**: `ReportCellSize` was refused as redundant to a `CSI 16t` that this document
already recorded as unanswered, three tables further down. Two rows called a refusal **policy** when
the engine had in fact dropped the sequence first — true in spirit, wrong about who was doing it, and
worth the correction precisely because §57 is a whole section about the difference between a gap that
costs nothing and one that costs something. The single real find is the **modifyOtherKeys query**: a
sequence a program sends and then waits on, which nothing here answered. **§61 closed it** — the row
above, and the only line of code the audit turned into work.

The lesson is about the shape of the audit rather than its findings. Every one of the six came from
reading `vte`'s dispatch arms and `alacritty_terminal`'s `Handler` impl and asking *which trait
methods are left at their empty default* — not from reading this document and asking whether it looked
right. A row is only as good as the last time somebody checked it against a crate, and the two rows
that were wrong about *why* had been right about *what* for long enough to stop being read.

**§62 then made that distinction a mark instead of a sentence.** The refusals had been ❌ rows with an
italic *(policy)* tag, and the two the audit had corrected carried *(policy, and free)* — a footnote
doing the work of a column. Splitting them into **🛑** (cmote refuses it) and **🤷** (nobody does)
forced every refusal to be re-derived from the crates rather than inherited, and that turned up two
more rows wrong in exactly the §60 way. **`CSI 1–10 t`** — iconify, move, resize, raise, maximize,
fullscreen — read as cmote holding its window against a remote; in fact `vte`'s `('t', [])` arm handles
14 / 18 / 22 / 23 and drops every other parameter to `unhandled!()`, so no `Handler` method for window
manipulation exists to leave at a default. **ENQ answerback** read as a refusal too, and it is one — but
`vte`'s `execute` has no `0x05` arm, so what refuses it is that nobody ever wrote the reply.

The mark also earned its keep in the other direction, by making one row *harder* than its old tag. **OSC
52 write** was the single refusal on this page the engine actively handed over: `alacritty_terminal`'s
`config.osc52` defaults to `Osc52::OnlyCopy`, cmote did not set it, so `clipboard_store` fired a real
`Event::ClipboardStore` and the only thing that stopped a remote poisoning the local clipboard was the
catch-all arm in `Replies::send_event`. A genuine 🛑, and it worked — but the *weakest* 🛑 here, being a
default cmote inherited plus a fall-through rather than a named refusal with a test on it. **§63 set
`osc52: Osc52::Disabled`** and pinned it, so it is now the most explicit refusal in the table instead of
the least. That is the whole of what a change of marks turned into work, and a fair argument for making a
column state its mechanism: nothing about the *behaviour* was wrong, and looking anyway found the one
place where the reasoning lived outside the code.

**§64 pointed the same question at the ⚠️ rows, which had never been asked to justify themselves.** A
partial is easy to leave alone: it admits up front that something is missing, so it draws none of the
suspicion a ❌ or a 🛑 does. Both colour rows turned out to be two rows in a trench coat — a query
answered in full and pinned by a test, and a set refused exactly as row 104's 🛑 is refused — averaged
into one mark that told a reader neither, so they are two rows each now. Worse, the note *justified* the
refused half with a cost:
"a full repaint for no change". There is no repaint. `set_color` calls `mark_fully_damaged`, which sets
one bool in the engine's damage tracking, and cmote calls neither `damage()` nor `reset_damage()`
anywhere, so the flag is written and never read. The refusal was right, the mechanism was right, and the
reason given for it was invented — the mirror image of §60's failure, where the mechanism was invented
and the outcome was right. Both come from the same habit: writing down what a sequence *ought* to cost
instead of reading what it does.

**§65 finished the sweep, and the ⚠️ rows turned out to be the least examined in the document.** Ten
remained; seven were two answers under one mark and are now two rows each, one was a refusal wearing a
partial's clothes (`BEL` — `alacritty_terminal` really does raise `Event::Bell`, and cmote's catch-all
really does drop it, so it is a 🛑 and always was), and two are genuinely partial. The pattern is worth
naming: a ❌ invites someone to close it and a 🛑 invites someone to check it, but a ⚠️ invites nothing —
it has already admitted to being incomplete, so it is never asked *which part*. That is how mode 2026 sat
for this long reading "cmote already atomic", which was true, beside an undriven abort timeout that lets a
remote freeze the screen. The audit that finds a thing like that is the same one every time: read the
dispatch arms, then ask who performs each half.

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
- **The engine is interceptable, which is why the margins row is a price and not a wall.**
  `Processor::advance<H>(&mut self, handler: &mut H, bytes)` is generic over `H: Handler`
  (`vte-0.15.0/src/ansi.rs:298`), and `Term` only *implements* that trait
  (`impl<T: EventListener> Handler for Term<T>`, `term/mod.rs:1059`) — so a wrapper can sit between
  the parser and the engine, override what it needs and forward the rest, without a fork.
  `Cursor::input_needs_wrap` is a **`pub` field** (`grid/mod.rs:52`), so even the pending-wrap flag —
  the state that decides where a line breaks — is reachable from outside `Term::input` (77 lines,
  `term/mod.rs:1060`). The catch is in the same place: `Handler` declares **71 methods and every one
  has a default empty body** (`ansi.rs:495`), so a forwarding gap — today's or a future version's — is
  a silent no-op rather than a compile error. That is the hazard §5 prices the feature against, and
  unlike §57's borrowed flag bit it cannot be turned into a build failure.
- **No arm for any rectangular operation** (what §58 walked into). `vte-0.15.0/src/ansi.rs`'s CSI
  dispatch matches the `$` intermediate in exactly two places — `('p', [b'$'])` at `:1703` and
  `('p', [b'?', b'$'])` at `:1707`, the two DECRQM spellings — so `$ z`, `$ {`, `$ x`, `$ v`, `$ r` and
  `$ t` all reach the unhandled arm and are dropped whole. `Cell` fields are public (`term/cell.rs:134`)
  and `Cell::default` gives `c: ' '`, so a fill is the pen cloned with one field changed and an erase is
  `From<Color> for Cell` (`:257`) — the same value `Cell::reset` (`:252`) writes, which is why an erased
  cell comes back protectable. **Origin mode is readable, the region is not**: `TermMode::ORIGIN`
  (`term/mod.rs:66`) comes back through `Term::mode()`, but `scroll_region` (`:301`) is a private field
  with no accessor — hence §58's refusal rather than an approximation.
- **DECSLRM lands on save-cursor** (the §57 misparse). `vte-0.15.0/src/ansi.rs:1737` is
  `('s', []) => handler.save_cursor_position()` — no parameters read, so `CSI 5;70 s` reaches
  `save_cursor_position` (`term/mod.rs:1619`), which assigns `self.grid.saved_cursor`, the single slot
  `ESC 7` and `CSI s` share and `restore_cursor_position` (`:1626`) reads back. Mode 69 is absent from
  `NamedPrivateMode` (`ansi.rs:938-968`), so DECSET 69 is ignored (`set_mode`'s `Unknown` arm,
  `term/mod.rs:2100`) and DECRQM answers `ModeState::NotSupported` = 0 (`report_private_mode`,
  `:2087`). The cancel byte cmote feeds instead: 0x18 in `State::CsiParam` falls to the catch-all
  `_ => self.anywhere(...)` (`vte-0.15.0/src/lib.rs:252`), and `anywhere` runs `execute(byte)` and sets
  `State::Ground` — no `csi_dispatch` — while `execute` (`ansi.rs:1296`) has no CAN arm, only the
  `debug!` fallback. `advance_ground` calls `reset_params()` on the next ESC (`lib.rs:605`), so the
  abandoned parameters cannot leak into the following sequence. SUB (0x1a) takes the same transition but
  calls `substitute()` (`term/mod.rs:1443`), which is a `trace!` today and displayable by definition —
  hence CAN.
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

- **§65's audit anchors.** `execute` (`vte/src/ansi.rs:1296`) maps HT / BS / CR / LF / VT / FF / **BEL**
  / SUB / **SI** / **SO** and drops the rest to a `debug!` — so `SI`/`SO` are the only locking shifts
  that exist, and `bell()` (`term/mod.rs:1437`) really does raise `Event::Bell`. `esc_dispatch`
  (`ansi.rs:1773`) designates charsets for G0-G3 through the `(`/`)`/`*`/`+` intermediates and has **no
  arm** for `n`, `o`, `~`, `}` or `|`. `csi_dispatch` has `('p', [b'$'])` and `('p', [b'?', b'$'])` for
  DECRQM and **none for `('p', [b'!'])`**, so DECSTR reaches nothing. `deccolm` (`term/mod.rs:792`)
  clears the region and grid with the comment *"setting 132 column font makes no sense"*, and DECRQM
  answers `ColumnMode => NotSupported` (`:2085`). `BlinkingCursor` sets `cursor_style.blinking` and
  raises `Event::CursorBlinkingChange` (`:1987`, `:2036`), and DECRQM reports it (`:2053`).
  `PrivateMode::Unknown` — which is what 80 is, `NamedPrivateMode` having no DECSDM — is logged and
  ignored (`:1937`, `:2000`) and reports `NotSupported` (`:2087`). Synchronized output lives in the
  parser, not the engine: `SYNC_UPDATE_TIMEOUT = 150ms` and `SYNC_BUFFER_SIZE = 2MiB`
  (`ansi.rs:36`, `:39`), `advance` enters `advance_sync` only when `pending_timeout()` is already true
  (`:303`), and nothing expires a stuck update except the application calling `stop_sync` — which cmote
  never does (no hit for `sync_timeout` / `stop_sync` / `pending_timeout` in `src/`).

### cmote (`c:/sources/github_clemeno/cmote/src/`)

- **`term/mod.rs`** — the `Replies` listener answers the events that expect a report and drops
  the rest (`~:228-258`): `Event::PtyWrite` (the engine's DA / DSR / DECRQM / cursor-position,
  accumulated whole), `ColorRequest` (OSC 10 / 11 / 12 / 4, resolved against cmote's scheme via
  `report_color`), `TextAreaSizeRequest` (`CSI 14t`, from the grid + cell pixel size),
  `Title` / `ResetTitle` (OSC 0 / 2, sanitized). **Dropped**: `ClipboardLoad` / `ClipboardStore`
  (OSC 52), the bell, and colour *set* requests. `SCROLLBACK = 10_000`. **Since §63 the clipboard pair
  no longer arrives at all**: the engine settings moved out of `Terminal::new` into `engine_config()`,
  which sets `osc52: Osc52::Disabled` beside `kitty_keyboard: true` — so `clipboard_store` /
  `clipboard_load` return inside the engine, before an event exists, and the catch-all is the second
  line rather than the only one. Both fields are pinned by tests
  (`the_engine_is_told_to_refuse_the_remote_clipboard`, `the_engine_is_told_to_speak_the_kitty_keyboard_protocol`),
  which is the reason the `Config` is a named function at all. **§64 pinned the colour *sets* the same
  way**, from the other side: `a_palette_colour_set_does_not_move_the_query_answer` and
  `a_default_colour_set_does_not_move_the_query_answer` each set a colour, assert the engine recorded it
  (`self.term.colors()`, the crate's public accessor), and assert the query still answers from
  `palette`. Nothing in `src/` reads `Term::colors` — grep returns no hit — and `set_color`'s
  `mark_fully_damaged` writes a bool cmote never reads, since neither `damage()` nor `reset_damage()` is
  called anywhere. The seam hides the
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
  `Terminal::modify_other_keys` and threaded into `keymap::encode`. **Since §61 it also answers**
  the question (`CSI ? 4 m` → `CSI > 4 ; Pv m`): `feed` returns the bytes owed, built the moment the
  question's final byte is read, so the reply carries the level as it stood *there* rather than as
  the chunk left it — a test asserts both orders in one write. The `?` marker now opens the same
  parameter run `>` did, which every DECSET and DECRST in the stream enters and then abandons on its
  own final byte; a test pins that they draw no reply and do not disturb the level. Resource 4 only,
  and a second parameter drops the sequence (§54's rule) — the reply format is an XTMODKEYS control,
  so there is no way to answer "not mine" except by not answering.
- **`term/iterm.rs`** — the OSC 1337 **allow-list** (§55). `Iterm::feed` runs on the shared framer and
  returns `(offset, Report)` for the honoured keys only; `parse` strips the `1337;` prefix and matches
  `SetMark` **whole**, so `SetMarkAnything` is not it. One honoured key today, and everything else —
  including keys nobody here has heard of — yields nothing, which is what makes the namespace safe
  without an enumerated deny-list. `MAX_PAYLOAD` is deliberately far below an `iTerm2 File=` payload:
  refusing to buffer megabytes of base64 is the cheapest way to mean §41's refusal. The dangerous keys
  (`Copy`, `SetProfile`, `SetColors`, `SetBackgroundImageFile`, `StealFocus`, `RequestAttention`,
  `ClearScrollback`, `File`) each have a test asserting they produce nothing. `SetMark` is applied
  through `Terminal::process`'s split advance into `osc133::Prompts::record_user_mark`, kept in a ring
  separate from the prompt marks — a bookmark has no command state, exit code or output span, so
  `output_at_prompt` must never resolve one — surfaced by `Terminal::user_mark_rows` and drawn as an
  amber gutter tick (`ui/grid.rs`), while `jump` chains both rings so Ctrl+Shift+Up/Down visits either.
  `CurrentDir=` is handled in `term/cwd.rs` instead, beside the two cwd spellings it duplicates.
  `SetUserVar=` honours the single name `gitBranch` — the allow-list applied to names as well as keys,
  which is what means there is no remote-keyed map to bound. `parse_user_var` is three-valued so the
  three cases stay distinct: not an assignment (keep what we hold), an EMPTY value (the shell left the
  repository — clear it), a value fit to draw. A bad base64 or non-UTF-8 payload lands in the first
  case, so rubbish cannot wipe a real reading. `sanitize` strips control characters and caps the value
  at `MAX_VALUE_CHARS` counted in `chars`, on the way IN. Surfaced by `Terminal::branch` and drawn by
  `ui/tabs.rs` as a dim pill AFTER the endpoint label — remote-chosen text in cmote's own chrome must
  not be able to pass for the label that says which machine the user is typing into.
- **`term/osc.rs`** — the shared OSC framer (§17, §34, §54, §55). One chunk-safe byte machine
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
- **`term/protect.rs`** — the selective-erase scanner (§56), and the one place cmote writes *inside* the
  engine's cells. A chunk-safe CSI state machine (`Protect::feed`) reading **DECSCA** (`CSI Ps " q`),
  **DECSED** (`CSI ? Ps J`), **DECSEL** (`CSI ? Ps K`), plus **RIS** and **DECSTR** as protection
  clears, and — only while the pen is armed — every **SGR**, since `Attr::Reset` assigns the whole flag
  word and would otherwise unprotect a run mid-way. All three of final byte, private marker and
  intermediates are matched together, which is what keeps the near-misses out: `CSI 2 J` is a plain
  erase, `CSI > 4 ; 2 m` is XTMODKEYS not an SGR, `CSI 1 SP q` is DECSCUSR not a DECSCA. Offsets are
  **one past** the final byte — the opposite of a prompt mark — because a pen change must land after
  the SGR that wiped it and an erase after the engine has ignored it. Protection itself is stored
  nowhere here: `PROTECTED_BIT` is bit 15 of the engine's per-cell flag word, unnamed by the engine, so
  `term/mod.rs`'s `set_pen_protection` stamps it on `grid.cursor.template` and the engine then carries
  it through scroll, insert/delete, reflow and the alternate-screen swap as if it were bold. Invisible
  both ways (`Cell::is_empty` tests named flags with `intersects`; `screen.rs` exposes only named
  attributes) and guarded by a `const _: () = assert!(…)` beside `DEFAULT_ROWS`, so an engine version
  that claims bit 15 fails the **build** instead of shipping unerasable text. `selective_erase` writes
  the pen's background straight into the grid via `grid_mut` — a deliberate break with
  `reserve_cells`'s inject-VT-sequences rule, because the engine's plain `CSI 2 J` *scrolls the
  viewport into history* rather than blanking it, and the CUP+ECH alternative would move a cursor the
  erase is defined never to move. `spans` is pure row/column arithmetic, so all six region shapes are
  tested without a terminal.
- **`term/rect.rs`** — the rectangular-operations scanner and geometry (§58). One chunk-safe CSI
  machine reading the four `$` sequences the engine drops — **DECERA** (`$ z`), **DECSERA** (`$ {`),
  **DECFRA** (`$ x`), **DECCRA** (`$ v`) — and two pure functions doing all the arithmetic: `area`
  resolves 1-based inclusive corners against the page (0 or omitted means the edge; an end past the
  edge clamps; a crossed pair or a start off the page yields `None`, so cmote never invents a rectangle
  by swapping corners), and `copy_extent` trims a copy to the room at its destination. Offsets are
  **one past** the final byte as §56's are; these name their own coordinates and never touch the
  cursor, so the split is only about ordering against the text in the chunk. `numbers()` refuses the
  whole sequence on any unparseable parameter (§54's rule) — a misread corner erases the wrong cells —
  and `fill_char` is an allow-list of 32–126 / 160–255. `term/mod.rs` writes the cells in four methods:
  `erase_area` (the pen's background, protection honoured only for DECSERA), `fill_area` (the pen
  cloned with a new glyph), `copy_area` (source read out whole first, because the overlapping case is
  the point of DECCRA) and `apply_rectangle`, which **refuses every one of them while `TermMode::ORIGIN`
  is set** — a `ponytail:` limit, since DECOM makes the corners region-relative and the engine's
  `scroll_region` has no accessor.
- **`term/rect.rs`, the attribute half** (§59) — the same module grew three more sequences the engine
  drops: **DECCARA** (`$ r`), **DECRARA** (`$ t`) and **DECSACE** (`* x`). `vte` matches `*` in no CSI
  arm at all and `$` only in the two DECRQM spellings, so all three fall through unhandled. DECSACE is
  a mode and is **absorbed by the scanner**, which is the one place that sees it and the requests it
  governs in stream order; each attribute request therefore leaves carrying its own `Extent`, and
  `term/mod.rs` never holds the mode. The extent is a parameter of `area` rather than something it
  reads, because it changes a *rule*, not a walk: a rectangle whose right corner is left of its left
  one is undrawable, while the same numbers as a stream are an ordinary run round the wrap. Selector
  lists fold to a `Change { on, off, flip }` at parse time — later wins, as in an SGR — and
  `Change::apply` is pure, so the whole table is tested without a terminal. `term/mod.rs` holds the
  one translation to engine names (`RECT_ATTRIBUTES`) and `attribute_area` sets **named bits one at a
  time**; assigning `Flags` wholesale would silently drop cmote's DECSCA protection bit (§56), which a
  test pins by underlining a protected form and then selectively erasing it. **Blink has no engine
  flag** — `Flags` names inverse, bold, italic, dim, hidden, strikeout, five underline styles and the
  wide-character marks, and nothing blinks — so DECCARA's `5` / `25` and DECRARA's `5` are read and
  dropped there, the rest of the list unaffected.
- **`term/rect.rs`, the checksum** (§60) — **DECRQCRA** (`* y`), the eighth sequence in the module and
  the only one that answers rather than acts. Its corners start at parameter **2** (`Pid` and `Pp`
  come first), which is the one thing about its grammar that is easy to get wrong; `Pp` is ignored as
  DECCRA's two are, which also settles DEC's "`Pp` = 0 means all of page memory" — with one page, the
  page is all of them. The arithmetic is **copied, not derived**: `Checksum::cell` / `finish` are
  xterm's `xtermCheckRect` with no extension bits, which is the mode xterm tuned against a real VT520,
  so it is DEC's answer by way of the implementation every suite compares against. Each cell weighs its
  code point plus 0x04 protected (read through `protect::is_protected`, since it is not in `Flags`),
  0x08 hidden, 0x10 underline, 0x20 reverse, 0x80 bold; a cell that finishes at exactly 0x20 is trimmed
  unless it is the rectangle's first; the total is taken mod 2^16 and **negated**, which is why real
  text reports a number just under 0x10000 and is the detail most easily got backwards. The reply
  (`DCS Pid ! ~ XXXX ST`) goes into the **same buffer the engine's own replies use**, pushed at the
  split point — so it answers from the page as it stood where the question sat, and orders correctly
  against a DSR in the same write with no second reply path. Origin mode costs it the rectangle and not
  the reply: it answers `0000`, because a query dropped on the floor stalls the program (§33). Three
  divergences are stated rather than hidden: **blink** never lands (§59's hole), a **DEC-charset** cell
  weighs its translated Unicode point where xterm weighs the byte it remembers, and a **never-written**
  cell is indistinguishable from a written blank, so a rectangle starting on virgin grid reports 0xFFE0
  where xterm reports 0x0000. Answering at all is a **security judgment, made explicitly**: a one-cell
  checksum inverts in a subtraction, so this is a screen readback — but every byte on that page came
  from the pty the reply goes back down, which is exactly what OSC 52's refused read form is not. Two
  properties are enforced, not assumed: the rectangle clamps to the **visible page** (no scrollback),
  and the answer is a function of grid cells and nothing about cmote or the machine.
- **`term/cancel.rs`** — the misparse scanner (§57), and the only one here that exists because the
  engine does **not** ignore something. `Cancel::feed` is a chunk-safe CSI state machine looking for
  one shape: a final `s` with at least one parameter byte, no private marker and no intermediate —
  **DECSLRM**, the VT420 left/right margins, whose final byte `vte` dispatches to save-cursor
  (`('s', []) => handler.save_cursor_position()`, parameters unread). Left alone, a margin request
  overwrites the engine's one saved-cursor slot, and the program's own `CSI u` then lands wherever the
  request happened to sit. The parameter count is the only evidence available, since DECLRMM (mode 69)
  — the mode that disambiguates the byte on a real VT420 — is one the engine never accepts; the bare
  `CSI s` therefore still saves the cursor, and every save-cursor in the wild is that spelling. Offsets
  name **the final byte itself**, a third convention next to a prompt mark's start-of-sequence and a
  selective erase's one-past-the-end, because that byte is the one being replaced: `process` advances
  the engine up to it, feeds **CAN** (0x18) in its place and resumes after it. Feeding nothing would
  leave the engine's parser mid-CSI, taking the next final byte in the stream as this sequence's —
  `CSI 5;70 s` then `hello` would dispatch `('h', [])` with parameters 5 and 70. CAN because the ANSI
  state machine defines it as the cancel (`anywhere()` → `execute`, state Ground, no dispatch), rather
  than SUB, which is *defined* to be displayable, or a final byte that merely has no arm today.
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
