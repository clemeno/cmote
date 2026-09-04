# cmote — Terminal Compatibility Plan

A reference inventory of **what cmote's terminal still lacks to render or drive any
documented remote-terminal-application UX**, and what each remaining gap would cost. Every
entry is a sequence specified in an official or widely-adopted source — nothing here is
speculative.

**How this document cites itself.** Its own nine chapters are **parts** — `part 0` to `part 8` — and
every `§NN` is a section of [`PLAN.md`](PLAN.md). They used to be numbered the same way, and for the
low numbers that was not a style difference but an ambiguity: a reader following `§6` from here landed
on PLAN's connection-and-authentication flow rather than on the refusals, and one sentence in part 2
managed to use `§1` for this document and `§25` for PLAN's. §111 renamed them. PLAN's side of the
convention was already unambiguous — it writes the file name out when it means a part of this one.

**Rewritten 2026-07-29, after the §23 engine swap.** The baseline is now
**`alacritty_terminal` 0.26** — a full VT implementation — replacing the old **`vt100`
0.16.2** subset. The previous edition of this document was written against `vt100` and framed
every gap as a `[bolt-on]` / `[engine]` split, because that crate was a deliberately small VT
subset that could neither parse nor represent most of the spec. **That ceiling is gone.** What
remains is a much smaller, concrete set of genuine gaps, grouped below by where the work lives.

**State as of §42.** Every *engine-independent* item this document ever listed is now either shipped
or refused with its reason recorded: input (part 2), query→reply (part 3) and the rendering/attribute layer
(part 4) are closed. Part 6 holds what cmote refuses on purpose. §36 also *corrected* this document: blink was
listed as cmote's policy choice when in fact the engine drops the attribute entirely — and **§60's audit
corrected it again**, reopening part 3 by one row (`CSI ? 4 m`, a query nothing answered) and fixing five
more rows in part 8 that had drifted from the crates; **§61 then closed that row**, so part 3 stands again.
**§62 corrected two more in the same direction** — `CSI 1–10 t` and ENQ answerback were credited to
cmote as refusals nothing in cmote performs — while splitting part 8's refusals into **🛑** (cmote's code
refuses it) and **🤷** (nothing does; it dies upstream). **§63 then took the one item that split turned
up as work**: OSC 52's refusal moved from an inherited crate default plus a catch-all to a stated
`osc52: Osc52::Disabled`, with a test on the field. No row changed status; the mechanism behind two of
them did.
**§64 ran the same check over the colour rows** — the last partial rows in part 8's OSC table — and found one
mark covering two opposite answers and one cost that does not exist: `OSC 4` and `OSC 10 / 11 / 12`
answer a query fully and refuse a set, and the note charged the refused set with a full-screen repaint
that never happens, since `mark_fully_damaged` sets a bool nothing in cmote reads. Both rows now name
each of them a query answered in full and a set refused on purpose. Each is now **two rows**, the way
`OSC 52` has been two rows since §62, and the refused half is pinned by a test in each direction.
**§65 then audited every remaining partial row against the crates**, split seven more the same way,
re-marked `BEL` as the 🛑 it always was, and found one real gap behind a comfortable-looking mark: cmote
never drives `vte`'s synchronized-update timeout, so a remote can hold the visible screen still with eight
bytes (mode 2026, and part 7) — **driven since §122**, over every tab rather than the visible one. **§66 split the last two and retired the partial class**: every row in part 8 now states
one answer with one mechanism, and the two halves that had been hiding behind "honest" turned out to be
gaps with small work behind them (DECRQSS's other selectors, XTGETTCAP's truecolor caps) — **§123 built
four of the first and one of the second, §152 the remaining four selectors, and §153 found that the last
of the capabilities was a refusal rather than a gap**.
**§67 narrowed the last loose mark**: **✅** now means *supported*, not *full*, and a row has to say how
much — an empty note being the explicit claim that nothing is withheld. Sweeping for rows that had been
leaning on the reader found one supported sequence with no row at all (`1005`, UTF-8 mouse), one
unsupported spelling with none (`CSI ? 6 n`, shipped in §82), one row carrying two different reports (DSR, now `5n` and
`6n`), and seven rows whose extent had been left to the reader. **§68 then split the last rows that stated
two answers in one** — `OSC 0`, `OSC 8`, DECSCUSR, XTSMGRAPHICS, DECKPAM, charset designation and the
XTMODKEYS query — so part 8 now holds one answer and one mechanism per row, with no exceptions left over.
**§69 then cashed one of those splits in, which is the first time this sequence of audits produced a
feature rather than a correction.** `OSC 1` (icon name) had been half of a single ❌ row; alone, it was
answerable, and the answer was that cmote has a tab strip to put it on and two shells on one host that
today look identical. It is now scanned by cmote itself and drawn on the chip after the endpoint
(`term/icon.rs`). Its other half went the other way: the icon half of `OSC 0` is refused, and part 6 records
why. One ❌ row became one ✅ and one 🛑.
**§70 then found the reverse — a row whose *reason* had expired without anyone noticing.** `iTerm 1337
File` carried ❌ because inline images "need an image-format decoder": true when it was written, and untrue
since §53 took `image` as a direct dependency for the file preview. What is left is not a cost at all but a
refusal cmote's own code performs twice over and a test already pins, so the row is 🛑. Its neighbour moved
too, though not its mark: kitty graphics keeps ❌, because its cost is the *protocol* rather than the parser
— part of its format space is raw RGBA — and because nothing in cmote refuses it, `vte` swallowing every
APC byte before a `Perform` method runs. A mark can outlive the argument that set it, which is a different
failure from the ones §66–§69 swept for, and the only one a re-read of the crates does not catch.
**§71 took the row next to it**, which had been carrying two keys and one word — *redundant* — and found
that the word was true and the mark was not. `CursorShape` and `ReportCellSize` are refused by the same
allow-list that refuses `Copy` and `File`, so they are 🛑, and they are the first refusals in this document
with **no danger behind them at all**: one is a fourth spelling of a field with a single writer, the other
a question whose answer would advertise fluency in a protocol §70 declines. Both are now pinned by name —
which matters more here than for the dangerous keys, because a refusal that protects nothing is the one a
later reader deletes as a courtesy.
**§72 closed the gap the same three sections kept walking past.** `CSI ! p` — DECSTR, the soft reset — had
been ❌ for everything except the DECSCA bit since §65, on the grounds that reproducing the engine's reset
beside it could not be verified. Both halves of that turned out to be wrong. cmote does not reproduce
anything: it feeds the engine the same reset spelled in the sequences the engine already handles, so the
engine stays the only writer of its own state, and every item is then readable back through DECRQM, DECRQSS
and the cursor. And the sequence is not rare — cmote asks for `TERM=xterm-256color`, whose `is2` and `rs2`
both *open* with `\E[!p`, so every `tput init`, every `reset` and every ncurses startup was sending cmote a
reset it dropped on the floor. The row is ✅, and the two halves §65 split are one row again: the
one-answer rule cuts both ways, and a row split because its halves disagreed rejoins when they stop.
**§73 asked the same question of the next row down and got the opposite answer, which is the point of
asking.** DECSLRM — left/right margins — read `❌ **safely**`, a mark with an adverb propped beside it,
which is the partial mark §66 retired wearing a coat. Both halves of the row were re-derived: the
*capability* stays unbuilt, because unlike a soft reset it has no shorthand to translate into and the
delegating `Handler` wrapper that would build it makes cmote a second writer of engine state; and the
*traffic* claim part 5 rested on — "essentially nothing emits DECSLRM" — was wrong, `xterm-256color`
declaring `mgc`, `smglp`, `smglr` and `smgrp`, though none of them from an init or reset string, which is
the difference from §72. What did move is the mark. ❌ is defined here as a sequence that *could still
land*, and since §57 this one cannot: `term/cancel.rs` cancels the final byte in flight, twenty-two tests
pin it, and the legend names §57 itself as the reason the 🛑 / 🤷 split exists in the column. So the row
is **🛑**, the capability's ❌ moves to the row that actually carries it (mode 69, DECLRMM), and the 🛑
legend widens to say that a refusal cmote performs may be taken on part 5's price as well as part 6's policy.
**§74 asked the same question one row further down the same table and got the third possible answer:
build it.** DECST8C (`CSI ? 5 W` — clear every tab stop, then set one every eight columns) read ❌
because `vte` parses it and `alacritty_terminal` leaves `Handler::set_tabs` at the trait's empty default.
Accurate, and the wrong end of the question. §72's route asks whether a missing sequence is a *shorthand*
for sequences the terminal already takes, and this one is — TBC, CR, HTS and CUF are all ✅ — so cmote
scans it out (`term/tabs.rs`) and feeds the engine the long spelling, the engine writing its own tab table
as it already writes its own reset. Three consecutive rows, three answers, from one question asked of each:
§72 translate, §73 refuse, §74 translate again. The row also turned up something none of the sweeps had
looked for, because choosing which movement the walk could use meant measuring them: `CSI A`, `CSI B`,
`CSI G`, `` CSI ` `` and `CSI e` all hand the engine the line the cursor is **already** on, and the engine
adds the scrolling region's top to whatever line it is handed — so under origin mode with a region below
row 1, every one of them moves the cursor down the page. Four ✅ rows now carry that, two of which had an
empty note, which under §67's rule was the strong claim. The walk is spelled with CR and CUF, which do
not go near that code, and a test pins the choice.
**§75 finished the set of answers one question can have.** SCP (`CSI Ps SP k`, select character path)
read ❌ under a note that said "bidi anyway, which cmote does not do" — a stance sitting beneath the mark
for a gap, which is §73's finding wearing a different coat. Put to §72's question it answers no: bidi is a
capability, and there is no spelling of a character path the engine already takes, so there is nothing to
translate into. Put to §74's it answers no again: nothing in cmote refuses it and there is nothing to
repair, so a 🛑 would mean writing refusal code for a sequence that already dies in `vte`'s empty
`set_scp`. What is left is the mark the legend describes word for word — **🤷**, a decision with nothing
behind it, dying in a defaulted `Handler` method — and the work the row produced is the decision itself,
which part 6 had never stated. Four consecutive rows, four different answers: §72 translate, §73 refuse, §74
translate again, §75 shrug, on the record.
**§76 overturned §75 in the very next section, and landed on a mark neither had offered.**
Asked to build SCP rather than to classify it, the re-derivation found what both readings had walked
past: the sequence has **two update modes**, and every argument against it had been costing the wrong
one. `Ps2 = 2`, "presentation to data", really does ask cmote to write the engine's grid — that is
§73's refusal and it stands, with a 🛑 row of its own now. `Ps2 = 1`, "data to presentation", asks the
terminal to derive its drawing from its data, which is what cmote does every frame from a grid it never
stores a drawing in. So the character path is a rule about the derivation, the grid is untouched, and the
scrollback, the search, the selection and a copy all go on reading the order the host sent. What made it
small was that cmote already had the second coordinate space §75 said it would need: one function
(`scp::flip`, its own inverse) shared by the renderer and the pointer path, and one crossing point
(`cell_under`) that every click already went through. **One row became two, ✅ and 🛑**, which is the
one-answer rule doing what it is for. The lesson is narrower than "re-derive everything": a refusal that
rests on a *parameter's* cost has to name which parameter, or it charges the whole sequence for the
expensive one.
**§77 found the same shape one row over, and the tell was that the refusal described a different
program.** `OSC 22` (the mouse pointer shape) had read 🤷 since §54 under three reasons, and only one of
them was a claim about a *crate* rather than about cmote — "`none` is in the vocabulary, so a remote could
hide the local pointer". Checked, it is false: `cursor_icon::CursorIcon` has no hidden variant and its
`from_str` no `"none"` arm, so no payload any terminal accepts can spell it. The other two — that a
pointer shape is *window-wide*, and that it would fight the four shapes cmote's own widgets ask for —
describe `winit::window::Window::set_cursor` and a window cmote does not drive. The grid is an iced
`mouse_area`; `mouse_area::interaction` stops at that widget's edge, and all four contested shapes are on
widgets that are siblings of the grid and never over it. So the scoping the entry said would have to be
built was the only thing on offer, and the arbitration it feared had no second party. **One row became
two again, ✅ and 🛑** — but the split is not §76's. There the line ran between two *parameters*; here it
runs between two *kinds of claim*: the five shapes that describe the text under the pointer are the
remote's to choose, and the shapes that speak with cmote's own voice — `grab` and the resize family,
which are what the splitters and drag handles say, and `wait` / `progress` / `not-allowed`, which assert
that cmote itself is busy or refusing — are not. §75, §76 and §77 in a row: the 🤷 column is where an
argument goes to stop being re-read.

**§78 asked a row in the neighbouring family and got the answer the previous three did not: the argument
was wrong and the mark was still right.** `kitty 21` (colour by semantic name) carries set, reset *and*
query in one sequence — `OSC 21 ; key=value ; key=? ; key= ST` — and its entry read "same fixed scheme as
4 / 10 / 11 / 12 — the theme is cmote's, not the remote's". But rows `4` and `10 / 11 / 12` are each
**split**, query ✅ and set 🛑, and this row had copied only the *set* half's reason and stretched it over
a sequence that carries both. §76's shape, one family over: a refusal charged for its most expensive
parameter. So the query half had never had a reason written, and writing one is the whole of §78. It is
four things. OSC 21 is a **dialect** query rather than a generic one — the five in `term/query.rs` are
sent blind by anything, while this one is sent after the caller has already concluded kitty, and nothing
here says kitty (`TERM=xterm-256color`, XTVERSION answers `cmote(<ver>)`, XTGETTCAP `TN` answers
`xterm-256color`). For the keys cmote *could* answer it is a second **reader** of `report_color`, not a
second writer as `iTerm 1337 CursorShape` would have been — so the two spellings could never disagree,
and equally could never differ, which is the point. The keys that would justify it are the ones with no
single value here: selection changes only the *background*, so `selection_foreground` is whatever the
cell already had, and the cursor is drawn by **inverting** the cell, so `cursor_text_color` is per-cell
too — answering either means inventing a colour, which is precisely what `palette.rs` exists to stop.
And it would be cmote's first reply whose **length the requester sets**, n `key=?` pairs to n values.
None of that is an impossibility, and the entry now says so: `SELECTION_BG` is a real constant
(`ui/grid.rs`) that `palette.rs`'s own charter says belongs there, kitty's *keyboard* protocol does work
here (§25) so "nothing kitty lands" would overstate it, and `query.rs`'s own argument — an unanswered
query stalls its sender — applies to anything that does send it. A judgement, with what would flip it
named. The mark was then **verified rather than assumed**, which is §77's habit applied to a row that did
not move: `vte` 0.15.0's OSC arms are `0`/`2`, `4`, `8`, `10`–`12`, `22`, `50`, `52`, `104`, `110`–`112`
and nothing else, so 🤷 is right. No split either — a row splits when the two halves *answer* differently,
and here they answer the same for different reasons. **§73 asked which column a refusal belongs in; §78
is the case where the column was right and the argument was borrowed**, and only writing the reason out
in full finds that.

**§79 then moved two rows by writing code that changes nothing.** `kitty 99` and `OSC 777` are the
desktop notification in two more spellings, and the decision on all of them has been settled since §54:
a notification LEAVES the window — it lands on the desktop, outlives the tab, and on Windows sits in
the Action Center after the session is gone. But the two vendor spellings read 🤷, because nothing here
declined them: no `vte` arm, no cmote scanner. `term/notify.rs` now names all three in one place, and
`term/progress::Reports::feed` — which already frames every OSC payload cmote sees, and already owned
the bare-OSC-9 half — asks it about each payload before reading one. **Nothing on the screen changes**,
and the rows say so: cmote could not raise a notification if it wanted to. What changes is exactly what
§63 changed on the OSC 52 row, where the clipboard pair was refused only by a catch-all arm that
happened to drop the event. A refusal that rests on nobody happening to match the sequence is one no
test can see. §78's row is the mirror image and the two belong read together: there the mark was right
and the argument borrowed, so the fix was words; here the argument was right and the mark was borrowed
— 🤷 claims *upstream* refuses it, and upstream refused nothing, it simply never looked — so the fix was
code. The exclusion runs both ways, too: the two OSC 9 sub-codes cmote *does* honour (`9;4;` progress,
`9;9;` cwd) are named in the classifier with their trailing separator, so tightening this refusal can
never quietly take a shipped feature with it. **🤷 8 → 6, 🛑 28 → 30**, with the row count unmoved.
**§39 touched this
surface without moving a row** — the find bar's match washes are a local highlight, not a sequence
answered; see the note in part 4. **§40 likewise moves no row**: it changed the *coordinate space* the
selection and the copy path work in (viewport rows → absolute document lines), which is a cmote-side
refactor of reads the engine already served; see the note in part 4 and the `term/screen.rs` evidence.

**§41 is the first change in a long while that DOES move rows — and it corrects this document's biggest
standing claim.** Part 5 and part 7 said the last item of real UX value, graphics, needed *both* an engine fork
and a renderer compositor. The compositor half was true. The fork half was not: the engine's DCS hooks
are no-op debug logs, so a sixel payload is already followed to its terminator and dropped, and the
sequence can be scanned out of the same byte stream that reaches the engine — the tactic §17, §9, §33 and
§34 all use. So **cmote now draws sixel images** (`term/sixel.rs`, `term/graphics.rs`, PLAN §41), answers
**XTSMGRAPHICS**, and amends the engine's own **DA1** reply to advertise attribute 4. The rows that moved
are listed in part 5 and part 8; kitty graphics, iTerm2 OSC 1337 and ReGIS did not move, and the reason changed
from "the engine cannot" to "their payloads are PNG/JPEG, which is a decoder dependency and an attack
surface" (part 5). **Half of that second reason expired in §53 and the rows were corrected in §70**: the
decoder is in the tree, so iTerm2's `File=` is now a 🛑 cmote's own code performs, and kitty keeps its ❌
for the protocol rather than the parser.

**§42 moves no row back the other way, but it does fix a defect on this surface.** Word and line
selection (PLAN §42) is local UX, yet it needed the engine's line-wrap flag — and reading it exposed
that cmote's **copy across a wrapped line was inserting a newline** where every other terminal unwraps.
See the note at the end of part 4 and the `term/screen.rs` evidence for `line_wrapped`.

**Update trigger.** This document tracks only the *terminal* surface — `src/term/` and
`src/ui/grid.rs`. Update it when, and only when, a change touches those: a query newly answered, a
mode newly honoured, a sequence newly rendered, or an engine bump that moves the ceiling. Editor,
files-pane, and window-chrome work does **not** belong here. When terminal work does land, the edit
is two places: the gap list in parts 2–5 (or part 6, if the answer is a deliberate refusal — a decision is
also an edit) and the matching row in the part 8 matrix.

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

## Part 0 — The new baseline

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

## Part 1 — Baseline: what already works

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
- **Keeps 10 000 lines of scrollback** with a thin scrollbar in the right gutter (§23 Stage 8):
  the wheel and Shift+PageUp/PageDown/Home/End scroll the history, and typing snaps back to the
  live bottom. The bar is **draggable** since §116 — press and drag the thumb, or press the bare
  track to jump — so it is a control and not only an indicator; a press there is captured, so it
  reaches neither the text selection nor a mouse-aware program's reports.
  The alternate screen keeps no history, so scrolling is inert there by design — and with no history
  there is no thumb, so the bar takes no press away from a full-screen program either. That
  history is **searchable** (§35): Ctrl+Shift+F floats a find bar over the grid, and each hit is
  revealed (centred when off-screen) and turned into an ordinary selection, so Copy takes it
  (`term::search`). **Every hit on the visible screen is washed** in a second highlight colour (§39),
  the current one keeping the selection's own fill, so the bar shows where else the query is. The
  history is also **selectable and copyable as a document** (§40): a selection's endpoints are absolute
  line indices, so scrolling moves the highlight with its text and a copy — plain or styled HTML — reads
  the retained history rather than only the visible grid.
- **Lets the engine interpret** the whole VT stream, no cmote papering-over: the **DEC
  line-drawing charset** (older programs box-draw with it), **origin mode** (so cursor reports
  are origin-correct), **custom tab stops** (HTS / TBC, and DECST8C to put the default ones back since §74), the **autowrap toggle** (DECAWM),
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
- **Shows the window title** a program sets with OSC 0 / OSC 2 in the title bar (§23), and the **icon
  name** a program sets with OSC 1 on that tab's own chip, after the endpoint (§69). The two are separate
  surfaces on purpose: the title names the window, the icon name names the tab, which is what tells two
  shells on the *same* host apart. Both are stripped of control characters before they are drawn, and the
  icon name is capped where it is stored — a remote naming its own chip must not be able to spoof or crowd
  the label that says which machine it is (`term/icon.rs`).
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

## Part 2 — Input — closed (all `[keymap]`, engine-independent)

`modifyOtherKeys` (part 1), the **kitty keyboard protocol** (§25) and now **DECKPAM** (§36) are all
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

## Part 3 — Query → reply — closed again (`[reply]`)

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
- **XTGETTCAP** (`DCS + q <hex> ST`) → states the three capabilities cmote can give truthfully, in
  both of the spellings each of them has: terminal name (`TN` / `name`) `xterm-256color`, colour
  count (`Co` / `colors`) 256, and direct-colour depth (`RGB`) 8 bits a channel. **Those three are
  xterm's whole special-name list**; every other capability gets an honest unknown
  (`DCS 0 + r <name> ST`), which since §153 is a refusal rather than a gap — the entry a table here
  would transcribe is `xterm-256color`, sitting on the remote that asked. *(This paragraph read "the
  two caps" from §33 until §153, having survived §123 adding `RGB` — see §153.)*
- **DECRQSS** (`DCS $ q <sel> ST`) → reports **nine** settings from live state, never from a stored
  copy of the request: SGR from the live pen (the exact attributes the grid paints, rebuilt after the
  chunk advances so a set-then-query in one write is seen), the cursor shape, the protection bit, the
  scrolling region, the margins, the page's lines and columns in DEC's three spellings, and the
  attribute-change extent. Every other setting gets an honest `ps=0` (`DCS 0 $ r ST`) rather than a
  lie about state cmote renders fixed or cannot read — and for two of them, DECSASD and DECSSDT, the
  `0` would have been *true* and is refused anyway, because a program told the status line exists
  writes to it (§152). *(This paragraph said "reports SGR, and every other setting an honest ps=0"
  from §33 until §152, having survived §123 adding four more.)*
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

- **DECXCPR** (`CSI ? 6 n`) → `CSI ? <row> ; <col> R`, the cursor's position in DEC's private spelling
  (§82). `vte`'s CSI table holds `('n', [])` and no `('n', [b'?'])`, so the whole DEC-private DSR family
  reached nothing. Answered **not** in `term/query.rs` but in `term/dsr.rs`, and reported through the
  interruption advance, because a position is only true where the question sat: a version string and a unit id
  can wait for the end of a chunk, a cursor cannot. The numbers are the engine's own
  `grid.cursor.point`, so this spelling and the ANSI one cannot come to disagree. **Only `Ps = 6` is
  answered** — the other nine values xterm defines describe the user's machine, and are refused; see part 6.

- **The locator's two negatives** (`CSI ? 55 n` / `? 56 n`) → `CSI ? 53 n` "no locator" and
  `CSI ? 57 ; 0 n` "cannot identify" (§93). The two members of that same DEC-private family that are
  answered rather than refused, because a reply stating an **absence** advertises nothing.

- **The locator's third door** (`CSI Ps ' |`, DECRQLP) → `CSI 0 & w`, DECLRP with event code 0
  (§140). The same absence in the locator protocol's own vocabulary rather than DSR's: ctlseqs
  defines `Pe = 0` as "locator unavailable" and defines it to carry no position, no button mask and
  no page, so the empty answer is the protocol's, not cmote's invention. Answered through the
  interruption advance although it is a **constant**, so that a DECXCPR and a DECRQLP written in one
  breath come back in the order they were asked. Its two sibling sequences, DECELR and DECSLE, are
  read by the same scanner and deliberately answer nothing — see part 6 for why cmote declines to
  become a locator it could not keep exclusive with the engine's own mouse modes.

- **The colour scheme** (`CSI ? 996 n`) → `CSI ? 997 ; 1 n`, dark (§98). The newest reply here and the
  only one whose sequence is nobody's standard: contour's, adopted by ghostty, kitty and GNOME's vte,
  and asked by neovim, helix, zellij and tmux. Answered from a **constant** — the scheme is fixed (part 6)
  and `palette::DEFAULT_BG` is `#1e1e1e` — with a test that fails if the background ever stops being
  dark, so the constant cannot outlive its own premise. It narrows the rule §96 wrote for dialects: a
  reply must not name the program or the machine, and "my background is dark" names neither. It is not
  new disclosure either, `OSC 11 ; ?` being xterm's own spelling of the same fact and already answered.
  What made it worth doing is the failure it prevents: a program that cannot learn the background
  paints for the one it guessed, and a light guess over a dark scheme is a screen nobody can read.

- **DECRQM for private mode 69** (`CSI ? 69 $ p`) → `CSI ? 69 ; 1 $ y` set, `; 2` reset (§102). The
  odd one in this list, because the engine ALREADY answers this question and the answer is the
  problem: mode 69 is absent from its `NamedPrivateMode`, so it reports `0`, "not recognised". That
  was true and useful until §102 — a program told the mode is unknown will not ask for margins — and
  the moment cmote implemented the mode it became a lie a program acts on. Answered from the gate
  rather than beside it, since the gate is where the mode is held. It reports nothing but cmote's own
  state, so §71's second-source rule has nothing to catch: there is no other way to learn it.

- **DECRQM for the four other modes cmote holds itself** — `2048` (§148), `5`, `45`, `9` and `1016`
  (§149, §150). Every one of them is absent from `NamedPrivateMode` for mode 69's reason, so every one
  of them would draw the same `0`, and the answer is the same one: the gate holds the mode, so the gate
  answers for it, through one shared builder. The cost of getting it wrong differs per mode and is
  worth naming for 2048 in particular — a program told the resize mode is unknown falls back to
  SIGWINCH, which is the very thing it asked for the mode *because it cannot receive*.

- **The cell size** (`CSI 16 t`) → `CSI 6 ; height ; width t` (§147). The only reply in this list
  answered where the question SAT rather than after the chunk, and not because it could go stale — it
  cannot — but because its sibling `CSI 14 t` is answered by the ENGINE mid-advance, and a program that
  asks both reads the two replies by position. It is built from the same pair `CSI 14 t` multiplies, so
  the two cannot disagree; the four window questions beside it are refused, on the rule at the top of
  this list.

There is also one reply cmote does not *originate* but **amends**: the engine writes DA1 as `CSI ? 6 c`,
and attribute **4** is how a terminal says it draws sixels — which is what chafa's auto mode, `lsix` and
ranger's previewer read at startup. Since §41 cmote rewrites that reply on its way out
(`query::with_sixel_attribute`) rather than sending a second DA1 (the program would parse one of them as
input) or suppressing the engine's (that would mean cutting bytes out of an inbound stream mid-sequence).

The one remaining reply-class sequence, **answerback (ENQ `0x05`)**, is refused as policy rather than
carried as a gap — see part 6. XTQMODKEYS was the opposite while it lasted: not refused, just missed.

---

## Part 4 — Rendering / attributes — closed at this layer

**OSC 8 hyperlinks are now done** (§24), **including the Ctrl-hover underline** (v4.0.0) — the seam
surfaces the per-cell URI (`Cell::hyperlink`), Ctrl+click and a context-menu Open/Copy follow it,
`link` gates the scheme to http/https/mailto before opening, and the grid now underlines the whole
run of a link while Ctrl is held over it, so the link reveals itself before the click.

**Correction (§36): blink is not cmote's choice, it is the engine's ceiling.** Earlier editions of
this table listed **blink (SGR 5/6)** as `[policy]` — "the engine stores the bit; cmote draws steady
by choice". That was wrong, and re-checking it was part of §36: `vte` parses SGR 5/6 into
`Attr::BlinkSlow` / `BlinkFast`, but `alacritty_terminal` 0.26's `terminal_attribute` has **no arm
for either**, and its cell `Flags` carry **no blink bit at all** — the attribute is dropped before
cmote can see it. The row moved to part 5 as an `[engine-limit]`. (cmote's no-animation-timer policy is
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
that **no row of parts 2–6 or the part 8 matrix moves**: no host request produces it, no attribute is newly
honoured, and nothing about what a remote program can ask for changed. A local highlight over cells the
program already painted is cmote's own UX, like the selection it sits under.

**§40 moved the selection into document coordinates, and that answers no sequence either.** A
selection's endpoints are now absolute line indices instead of viewport rows, so the grid resolves the
row it is drawing into the line that row shows (`Marks::top_line` + `Screen::line_at`) and the copy path
reads lines straight out of the retained history (`Screen::line_cell`) rather than only the visible
grid. The engine is read in one more way and driven in none: **no row of parts 2–6 or the part 8 matrix moves**.
It is recorded here because the mapping now lives on this document's surface (`term/screen.rs`) and is
the one place a viewport row and a document line meet — the same coordinate the OSC 133 marks (§34) and
the search matches (§35, §39) are already stored in.

**§41 adds a THIRD layer to the grid, and this one does answer a sequence.** Inline sixel images
(`term/sixel.rs`, `term/graphics.rs`, `ui/grid.rs`) are the first drawing cmote does on a remote
program's instruction that the engine has no representation for at all — so unlike §39 and §40, rows
move: the sixel DCS, XTSMGRAPHICS, DECSDM and DA1's attribute 4 (part 3, part 5, part 8). Three things make it fit
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
whether a document line is continued by the next. **No row of parts 2–6 or the part 8 matrix moves.** Worth
recording anyway, because reading that flag corrected a real defect on this surface: a copy across a
wrapped line used to paste a **newline into the middle of a logical line** (a long path, a long command),
where every other terminal unwraps. `Selection::extract` and the HTML copy now join a wrapped row to the
next with nothing, and skip the trailing-blank trim on a wrapped row — a blank in the middle of a
logical line is a space, not the grid's width padding. It also fixed a one-cell blind spot on this
surface: a **one-character find-bar hit** (§35) and a **command whose output is a single character** (§34)
were both being revealed and then not highlighted, because a range one cell wide was indistinguishable
from a click that had not dragged.

**§85 gives the pen a stack** — XTPUSHSGR / XTPOPSGR (`CSI Pm # {` and `CSI # }`, aliased `CSI # p` and
`CSI # q`), which `vte` never sees: `csi_dispatch` matches no `#` intermediate at all. The row was in
this document as the *colour* stack's spelling until §84 read it back against ctlseqs, so it had been
declined with an argument about a palette nothing reads — while the attributes it actually stacks are
bold, italic, underline, reverse and the two colours, every one of which the renderer draws. Ignoring it
does not omit a feature, it leaves the **wrong pen**: a program that pushes, paints itself red and pops
goes on painting red.

Built on the two routes this layer already had. The pen is read where the push sat, through the same
template `Cell` DECRQSS reports (§33), and a pop feeds the engine that pen spelled in SGR — §72's answer
for DECSTR and §74's for DECST8C — so the engine stays the only writer of its own template (§71, §73).
Two details are cmote's own and are stated on the row: the restore string uses `4:3` / `4:4` / `4:5` and
SGR 58 so an underline substyle and its colour survive a round trip the DECRQSS reply could not describe,
and the borrowed DECSCA protection bit (§56) is read across the restore and put back, a stack of video
attributes having no business clearing a cell-protection setting.

---

## Part 5 — The new engine's own ceiling (`[engine-limit]`)

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
- **ReGIS / kitty graphics** — still ❌, but the reason has moved out of this section's premise. Nothing
  about the engine blocks either. ReGIS is a vector language with no users worth the interpreter; kitty's
  cost is its **protocol** — chunked transmission, image ids, placements, deletion commands, unicode
  placeholders, animation — and not the decoder this bullet billed for until §70, since `f=24` / `f=32`
  are raw RGB/RGBA and need none. `[DEC]` / `[vendor]`. The placement, reservation, compositing and
  capability machinery §41 built is protocol-agnostic, so kitty would still be a scanner over that
  machinery rather than a rethink — it is the scanner that is large.
- **iTerm2 inline images (`OSC 1337 File=`)** — 🛑 since §70, and no longer in this section on cost.
  `image` has been a direct dependency since §53, so the decoder this bullet once charged for is already
  in the tree; what stops the sequence is `term/iterm.rs`, by allow-list and by payload cap. The decision
  it carries out is part 6's, and it is about consent rather than parsers: a REMOTE must not get one run on
  bytes it pushed into the terminal stream unasked, which is a different question from a file the user
  pointed at and asked to open. `[vendor]`.
- **Blink** (SGR 5/6) — `vte` parses it (`Attr::BlinkSlow` / `BlinkFast`), but the engine's
  `terminal_attribute` has no arm for either and its cell `Flags` hold no blink bit, so the attribute
  never reaches the grid (§36, moved here from part 4). Showing it would take a per-cell scanner beside
  the engine (as `modkeys` is) *plus* the repaint timer cmote deliberately does not run. `[ECMA-48]`,
  low value. **§151 priced the first half of that and it is worse than "no bit":** the flag word is a
  `u16` with fifteen bits named and the sixteenth already borrowed for §56's protection, so the store
  would have to be a per-cell map beside the engine — dropped on every reflow, the way §34's marks and
  §41's images are, and unbounded in remote input the way a per-LINE map is not (§12). The overline
  (SGR 53) sits beside it in part 8 and fails one layer earlier, in `vte`'s SGR table.
- **~~Double-width / double-height lines~~ (DECDWL / DECDHL, `ESC#3-6`) — SHIPPED in §146.** The
  engine really does not represent them — there is no `DoubleHeight` or `DECDHL` symbol in the crate —
  and that turned out not to matter, because a line's SIZE is not something the engine has to know:
  the grid holds the text the host sent and the size is a rule the renderer applies, which is §76's
  arrangement for the character path applied to the other axis. DECDHL is exact (both axes by the same
  factor is a uniform scale, which iced can express); DECDWL's glyphs are not fattened, because double
  width with single height is anisotropic and iced 0.14 has no such scale for text — measured at
  `iced_wgpu/src/text.rs:625-626`, where whatever transformation is in force is reduced to one number.
  The row in part 8 carries the full account. `[DEC]`.
- **~~Left / right margins~~ (DECSLRM, VT420) — SHIPPED in §102.** Read this bullet as the record of
  a verdict that was reversed, because the reversal turned on a single sentence being wrong.

  The bullet below refused the build twice, on the hazard named under *What blocks it*: every
  `Handler` method has a default empty body, so a method left unforwarded — or one a future
  `alacritty_terminal` adds — compiles cleanly and silently drops a sequence, and that "**cannot**"
  be caught at build time the way §57's borrowed flag bit could.

  It can. **`#[deny(clippy::missing_trait_methods)]`** on the `impl` block reports every method left
  to its default, so a missing one is a build error under the gate's `-D warnings`; and if a future
  clippy drops the lint, `unknown_lints` fails the build instead. There is no version of the future
  where the file goes quiet on its own. The deny was verified by removing `bell()` and watching the
  build fail, rather than assumed. What it does NOT catch is a method present but forwarding wrongly,
  which is ordinary code tested as ordinary code.

  The lesson is not about clippy. **A cost estimate ages differently from a fact**, and this document
  has been re-deriving its FACTS every sweep since §66 while carrying its PRICES forward unexamined.
  The wall here was never the trait; it was one sentence about the tooling, written once and quoted
  three times. §98 found the same failure in the row marks — a table of correct rows looking exactly
  like a complete one — and this is its twin in the prose.

  Everything below is left as it stood, since the arithmetic of the build was right and only the
  verdict was wrong. The one number that came in low: it estimated 400–600 lines and the whole of
  §102 — gate, region mirror, margins, DECIC/DECDC and their tests — came to rather more.

  ---

  The engine's scroll region is vertical only
  (`set_scrolling_region(top, bottom)`), and horizontal ones reach into what printing, wrapping,
  `IL`/`DL`, `ICH`/`DCH` and every scroll do. `[DEC]`.
  An earlier reading of this row called it impossible without re-implementing the grid.
  That was wrong, and the correction is worth writing down because it is the only row in this document
  whose verdict rests on price rather than on a wall:

  - **There is a seam.** `Processor::advance<H: Handler>` is generic and `Term<T>` merely *implements*
    `Handler`, so cmote can pass a wrapper that holds the margin state, overrides the margin-sensitive
    methods and forwards the rest. No fork, no patch.
  - **The state that decides where a line breaks is public.** `Cursor::input_needs_wrap` is a `pub`
    field, so the pending-wrap flag can be read and set from outside — the piece assumed to be sealed
    inside `Term::input`.
  - **§58 already built the hard primitive.** Scrolling a column band by a row IS a rectangular copy
    plus an erase, and `copy_rect` / `erase_rect` exist.

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
  method breaks nothing. *[§102: this is the sentence that was wrong. See the head of the bullet.]*
  Add the smaller ones — a margin wrap has to set `WRAPLINE` itself or copy,
  search and reflow read the line as two; margins are per-screen and have to ride the alternate-screen
  swap; resize reflows assuming full width, so margins would reset on resize as xterm's do. And there is
  no §72 shortcut in reach: a soft reset could be *translated* into sequences the engine already takes,
  but margins are a capability with no shorthand to translate into, so the wrapper would make cmote a
  second writer of the engine's own state — cursor, wrap flag, scrolled cells — which is what §71 and §72
  were both careful not to become.

  *[§102 on the three smaller ones. The `WRAPLINE` half was answered the other way and deliberately:
  a margin wrap sets **no** flag, because the flag belongs to the whole ROW and inside a narrow band
  the rest of that row is another column of the page — joining on it would splice unrelated text into
  every copy taken across a wrap. Costs a wrapped word being copied in two pieces; keeps the copy
  honest. **Margins are not per-screen** either, and for a reason the wrapper made visible: the
  ENGINE's vertical region is not per-screen — `swap_alt` does not touch `scroll_region` — so making
  the horizontal axis behave differently from the vertical one would have been cmote inventing an
  asymmetry DEC did not write. Resize does drop them, as predicted. And the "second writer" worry
  turned out to be the wrong shape: the gate is a **gate**, not a second author — it pre-empts a
  decision and then delegates, and the engine still writes every cell it wrote before, except inside
  a narrowed band where the engine has no opinion at all.]*

  Against that, the traffic. **§73 corrected this bullet on the facts**: it used to say "essentially
  nothing emits DECSLRM outside a conformance suite", and the terminfo for the TERM cmote asks for says
  otherwise, declaring all four margin capabilities.

  ```
  mgc=\E[?69l,
  smglp=\E[?69h\E[%i%p1%ds,
  smglr=\E[?69h\E[%i%p1%d;%p2%ds,
  smgrp=\E[?69h\E[%i;%p1%ds,
  ```

  §72 found exactly this shape and it turned out to be a real gap, which is what makes the difference in
  the answer worth stating. `\E[!p` sits in `is2` **and** `rs2`, so every `tput init`, every `reset` and
  every ncurses startup was sending it unasked. No margin capability appears in any init or reset string
  (`is2` and `rs2` are `\E[!p\E[?3;4l\E[4l\E>`, and `rs1` is RIS) — those four go out only when an
  application deliberately decides to use margins, and ncurses' own rendering never does. Declared is not
  emitted. So the answer is still no, on price.

  What **§57** changed is the cost of *refusing* it. DECSLRM shares its final byte with save-cursor,
  and `vte`'s arm for that byte ignores its parameters, so the refusal was not free: `CSI 5;70 s`
  *saved the cursor*, overwriting a value the program meant to restore from later. cmote now cancels
  that byte in flight, so the request does nothing at all — the `s` row in part 8, which §73 re-marked 🛑 so
  the column says who performs that, PLAN §57 and §73, and `term/cancel.rs`.

  *[§102 on the traffic paragraph, which is the part that came out best. It is still true that no
  init or reset string emits a margin capability — and the same terminfo turned out to prove
  something the refusal never needed: all four capabilities **set mode 69 first**
  (`smglr=\E[?69h\E[%i%p1%d;%p2%ds`). A program that means margins says so before it asks, which is
  exactly what makes it safe to stop cancelling the parametrised `s` and let it mean save-cursor
  again. §73 read this terminfo to check a traffic claim; the fact that settled §57's guess was
  sitting in it the whole time.]*
- **~~VT420 rectangular ops~~ — SHIPPED in §58.** DECERA (`$ z`), DECSERA (`$ {`), DECFRA (`$ x`) and
  DECCRA (`$ v`) all read as engine limits until §56 built the hard half of them: writing cells
  straight into the grid, and knowing which of them a program protected. `vte` matches `$` only in the
  two DECRQM spellings, so all four fall through unhandled and are cmote's — a grammar, some clamping
  arithmetic and four small methods (`term/rect.rs`). One limit is disclosed rather than solved:
  **origin mode is refused**, because with DECOM set the corners count from the top of the scrolling
  region and the engine keeps that region private. See PLAN §58.
- **The scrollback's newest end cannot be shortened, and §101 worked around it rather than through
  it.** `Grid::update_history` is the only public way to drop history rows and it drops the OLDEST
  ones: `Storage::shrink_lines` lowers a length, and the ring's `compute_index` puts the oldest row at
  the far end of that length. kitty's UNSCROLL needs the opposite — the rows nearest the page, moved
  onto it and gone from the history. So the rows nearest the page are **overwritten**: the rest of the
  history walks up over them, the spares end up at the oldest end, and `update_history` is then asked
  to drop exactly that many from there. Rows are moved rather than copied (`mem::replace` around one
  cloned spare), so a full scrollback costs pointer moves and not a megabyte of cells. This is the one
  place cmote depends on **which end** an engine method trims from — a fact its documentation does not
  state, only its arithmetic — and the tests that would catch a change are behavioural: a document read
  end to end after an unscroll, in order, with nothing repeated. See PLAN §101.
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
- **~~Tab stops every 8 columns~~ (DECST8C, `CSI ? 5 W`) — SHIPPED in §74, and not by this section's
  route.** It sat here as an engine limit on the strength of `Handler::set_tabs` being left at the
  trait's empty default — true, and the wrong end of the question. A tab-stop reset is a **shorthand**
  rather than a capability: TBC, CR, HTS and CUF are all ✅ already, so cmote scans the sequence out
  (`term/tabs.rs`) and feeds the engine the long spelling, exactly as §72 does for the soft reset, and
  the engine goes on being the only writer of its own tab table. This is the second row moved by asking
  whether a missing sequence has a **translation** rather than a price — the same question §73 put to
  margins and got "no shorthand" back from. It also turned up an engine defect in the cursor movements
  the walk had to choose between; see the `A / B / C / D` and `G` rows in part 8. See PLAN §74.
- **~~The mouse pointer shape~~ (`OSC 22`) — SHIPPED in §77, and it was never this section's item.** It
  had the engine-limit signature — `vte` parses the sequence in full and hands it to
  `set_mouse_cursor_icon`, a `Handler` method left at the trait's empty default — but it was filed as a
  Part 6 policy, so this section never had to justify it. Both readings were arguing a **cost cmote does not
  pay**: a pointer shape is window-wide only if a terminal drives the window, and cmote's grid is an iced
  `mouse_area` whose `interaction` stops at that widget's edge. Third row in a row moved by the scanner
  route, and the first where what had to be checked was not a sequence's semantics but a *sibling
  crate's* — one of the three reasons on the record (`none` is in the vocabulary) was simply false about
  `cursor_icon`. See PLAN §77.
- **Synchronized output `?2026`** — **done (§122)**, and it was the last item on this list with an
  argument for building rather than a reason not to. The **vte parser batches** the run between
  `?2026h` and `?2026l` (`vte-0.15.0/src/ansi.rs` BSU/ESU) while `alacritty_terminal`'s mode handler
  is a no-op (`SyncUpdate => ()`) and DECRQM reports it reset — so the batching half was always
  right, and cmote paints atomically from the grid anyway. What was missing is the **abort
  timeout**: the 150 ms `vte` allows an unclosed bracket is stored as an `Instant` and never compared
  to the clock by the crate, so a remote that opened one and went quiet held the visible screen at
  its pre-BSU state indefinitely. cmote now drives it — `Terminal::held_update_expiry` reports the
  instant, `release_held_update` applies the frame once it passes, and an `iced::window::frames()`
  subscription runs only while some terminal in the window is holding one. `[community]`.

---

## Part 6 — Deliberately excluded (🛑 / 🤷 in part 8 — policy, not gap)

Every refused row in part 8 is one of these — and **since §102 that sentence is true again without an
exception attached to it.** DECSLRM was the odd one out for twenty-nine sections: marked *refused by
cmote's own code*, because `term/cancel.rs` stopped the sequence dead, but the decision behind it a
*price* recorded in part 5 rather than a policy recorded here. It is *supported* now, so the only thing
left in this section is policy. The
distinction the exception taught is worth keeping even though its example is gone: **the mark records
who performs a refusal, the section it points at records why it was taken** — and a refusal filed
under price is one to re-read when the price moves, which is exactly what happened to this one.

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

That bell is the **🛑** in part 8, and §65 had to correct its mark to say so: it had been a partial reading
"accepted, silent", as though nothing had decided anything. Something did. `vte` dispatches `BEL` and
`alacritty_terminal` implements it — `bell()` is `self.event_proxy.send_event(Event::Bell)` — so the
event genuinely arrives and cmote's catch-all drops it. It is the last refusal in this document standing
on a fall-through alone: OSC 52 got a config field in §63, and the colour sets have a renderer that
structurally cannot read them, but a bell has neither.

**The icon half of OSC 0** — refused since §69, and the odd one out in this section, because nothing here is
dangerous. `OSC 0` sets the icon name and the window title to the *same string*; cmote honours `OSC 1` and
draws an icon name on the tab chip, so it could honour this spelling too at the cost of one `or_else`. It
does not, because of what actually sends it: Debian's stock `PS1` carries `\[\e]0;\u@\h:\w\a\]`, so `OSC 0`
arrives on **every prompt of every session**. Honouring the icon half would put `user@host: ~` permanently
on every chip — the endpoint that is already on the chip, plus the directory that is already in the title
bar — and would leave no room for the one thing an icon name is worth having for, a program naming itself.
So this refusal is about noise rather than risk, which is a reason worth writing down precisely because it
is the sort a later reader would otherwise undo as an oversight.

It is a **🛑** and not a **🤷** even though `vte` drops the icon half too: `vte` routes `OSC 0` to the title
handler, cmote takes the title from it, and it is `term::icon`'s prefix match — `1;` and nothing else — that
declines the rest. The bytes are in cmote's hands when the decision is made, which is the whole difference
§57 is about. Two tests hold it, and the second is the one that matters: it asserts the window title *moved*
before asserting the icon name did not, so a future change that stopped parsing `OSC 0` at all would fail
rather than quietly resemble the same refusal.

**A blinking cursor** is refused on price rather than on principle, and it is cmote's own refusal
(part 8's `12 (the blink)`). A remote's `CSI ? 12 h` is tracked by the engine and reported back by DECRQM,
but nothing draws it: `term/screen.rs`'s `CursorShape` deliberately carries no blink and cmote runs no
animation timer, so the cursor is always steady — the same call part 4 makes for SGR blink and DECSCUSR's
blinking shapes. The cost is admitted: a program that blinks the cursor to draw the eye gets a steady
one, and DECRQM will tell it the mode is set, which is true of the mode and not of the screen.

**Blinking TEXT** joined this list in §161, which is worth reading as a correction to a mark rather than
as a new refusal — the sentence above had said "the same call part 4 makes for SGR blink" since §65,
while part 8's row called it a gap. §151 priced the row on storage alone and stopped there: the engine's
flag word is full, so there is nothing to put the attribute in. What it had not weighed is that the
timer above is refused too, and a bit that came free on somebody else's release would therefore light
nothing. A feature that needs a *decision* reversed rather than a version bumped is not a gap. So the
gate now drops `Attr::BlinkSlow` and `Attr::BlinkFast` by name instead of handing them to an engine with
no arm for either — identical on screen, and the difference is that a test can see who declined.

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

**kitty's `OSC 21` is this policy in one sequence — and its query half needed a reason of its own
(§78).** `OSC 21 ; key=value ; key=? ; key= ST` does all three jobs at once, any number of `key=value`
pairs per sequence, over semantic names rather than numbers: `foreground`, `background`, `cursor`,
`cursor_text_color`, `selection_foreground`, `selection_background`, `visual_bell_color`,
`transparent_background_color1`–`8`, `color0`–`color255`. The **set** and **reset** pairs are the fixed
scheme above, verbatim — the same refusal `4 (set)` and `110` / `111` / `112` carry, in a namespace that
happens to be spelled in words. The **query** pairs are not, and until §78 this section said they were:
it borrowed the set half's sentence and let it cover the whole sequence, which is exactly the mistake
§76 caught one family over.

The query half is refused on its own four grounds. **It is a dialect query, not a generic one.** The
five queries `term/query.rs` sniffs (XTVERSION, DECRQSS, XTGETTCAP, DA3, XTSMGRAPHICS) are sent blind by
programs that have concluded nothing about the terminal; OSC 21 is sent *after* a caller has concluded
kitty, and nothing in this stack ever says kitty — cmote asks the remote for `TERM=xterm-256color`,
answers XTVERSION with `cmote(<version>)` and XTGETTCAP `TN` with `xterm-256color`. **For the keys cmote
could answer it carries no new information**: `foreground`, `background`, `cursor` and `color0`–`color255`
all resolve through `report_color`, which is what `OSC 10` / `11` / `12` / `4` already answer from. That
makes it a second *reader* of one source rather than a second *writer* of one field, so unlike the
fourth cursor-shape spelling (§71) the two could never disagree — but they could never differ either,
which is the whole of what a second spelling would buy. **The keys that would justify it have no single
value here.** Selection changes only the background (`SELECTION_BG`, `ui/grid.rs`), so
`selection_foreground` is whatever the cell already had; the cursor is drawn by inverting the cell, so
`cursor_text_color` is per-cell as well; `visual_bell_color`, the eight transparent-background colours
and the mark colours name features cmote does not have. Answering any of them means **inventing** a
colour, and `palette.rs` opens by saying why a terminal must not: an answer that disagrees with what the
grid paints breaks the colour-scheme detection the query exists for. **And it would be cmote's first
reply whose length the requester sets** — n `key=?` pairs produce n values in one reply, where every
reply cmote writes today is one bounded answer to one question.

What this is *not* is an impossibility, and the row says so rather than leaving a later reader to
rediscover it. `selection_background` is a genuine constant that `palette.rs`'s own charter — one source
of truth for the renderer *and* the query answerer — says belongs in `palette.rs`. kitty's **keyboard**
protocol does work here (§25), so a program that found one kitty protocol answering has some reason to
try another. And `query.rs`'s own argument applies unchanged to anything that does send OSC 21: a
program that asks and hears nothing stalls until its timeout. So this is a judgement about what asks,
not a wall — and it would flip if cmote ever advertised kitty anywhere, or if `palette.rs` grew the
selection colours for some other reason.

**§98 narrowed the first of those four grounds and the row did not move**, which is the best evidence
that four grounds were worth writing down separately. cmote now answers `CSI ? 996 n`, the dark/light
question, and that sequence is no more xterm's than OSC 21 is — so "a dialect query, not a generic one"
cannot by itself be what refuses a reply. What replaced it is narrower: a reply must not name the
**program** or the **machine** (§36), and must not be a second source for something cmote can observe
(§71). The dark/light answer is neither, and it restates a fact `OSC 11 ; ?` already gives out in
xterm's own spelling. OSC 21's query half still fails the other three grounds untouched — it would
invent colours cmote does not have, its reply's length is the requester's to choose, and for the keys
it could answer it carries nothing `OSC 4` / `10` / `11` / `12` do not.

**Remote-triggered desktop notifications** — `OSC 9;<text>`, `OSC 777` (urxvt) and `kitty 99` (rich
notifications) are all the same feature in three spellings, and all three are **refused on purpose**
(§54). A notification *leaves the window*: it lands on the user's desktop, outlives the tab, and on
Windows sits in the Action Center after the session is gone. That hands a remote a channel to the
machine itself, and a compromised or merely chatty host would spam it. cmote's rule throughout is that
a remote may change what its own tab looks like and nothing more.

**Since §79 that refusal is performed rather than merely held.** `term/notify.rs` names the three
spellings in one place and `term/progress::Reports::feed` — the module that already frames every OSC
payload cmote sees, and that already owned the bare-OSC-9 half — asks it about each payload before
reading one. **The behaviour is unchanged and the section does not pretend otherwise**: nothing in
cmote could raise a notification if it wanted to, so none of the three was ever going to do anything.
What changes is what §63 had to change on the OSC 52 row. There the clipboard pair was refused only by
a catch-all arm that happened to drop the event; here `kitty 99` and `OSC 777` were refused by *nobody*
— no `vte` arm, no cmote scanner — and the bare `OSC 9;<text>` only by the accident that the progress
and cwd scanners are looking for `9;4;` and `9;9;` and so did not recognise it. A refusal that rests on
nobody happening to match the sequence is one no test can see and one a later hand can undo without
noticing they have. Three tests now fail if any of the three starts being read, and the exclusion runs
the other way too: the two OSC 9 sub-codes cmote *does* honour are named in the classifier with their
trailing `;`, so tightening this refusal can never quietly break progress (§54) or the working
directory (§17).

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

**`File=` is the key in that namespace whose refusal outlived its stated reason** — until §70, which is
worth recording because the correction went the opposite way from every other one in this document. The
row read ❌ *"a PNG/JPEG payload, so it needs an image-format decoder"*, and that was true when it was
written. It stopped being true in §53, which takes `image` as a **direct** dependency with five codecs on
so the file preview can open what a user asked for. The dependency this key was refused over is already
in the tree, and so are the placement, reservation, compositing and eviction machinery §41 built.

What stands is the part §41 wrote down in advance and §53 was careful not to touch: *the refusal was
never "cmote owns no PNG parser" — it is that a **remote** must not get one run on bytes it pushed into
the terminal stream unasked.* The difference is **consent, not caps**, which matters because caps are
copyable and consent is not. §53 decodes one file the user pointed at, with the format pinned to its
leading bytes and the decode bounded by `image::Limits`; `File=` would be bytes a remote chose, at
whatever rate and count it liked, in a format the payload itself names. There is a second difference
worth saying: sixel's decoder is cmote's own six hundred lines, auditable here, while PNG / JPEG / GIF /
WebP are third-party parsers on a path a remote drives — pure Rust, so not the memory-unsafety class, but
panics and decompression bombs are live.

So the mark is **🛑** and not ❌: `term/iterm.rs` refuses this key twice over, by allow-list and by a
`MAX_PAYLOAD` set below what a base64 image needs, and one test asserts both. A cost that has already
been paid elsewhere is not a reason, and leaving it written as one invites a later reader to "fix" it.

**`CursorShape=` and `ReportCellSize` are the two keys refused for no danger at all** — §71, and the odd
pair in this section, since everything above it is here because something would go wrong. These are here
because nothing would.

`CursorShape=N` is the same instruction cmote already takes twice. DECSCUSR (`CSI Ps SP q`) and OSC 50
are both dispatched by `vte` onto the engine's single `cursor_style.shape`, which `term/screen.rs` reads;
a program that wants a bar cursor has two spellings here that work, and iTerm2's numbering is OSC 50's
numbering. Taking a third would mean reaching that field from OUTSIDE the engine — cmote's scanner has no
other way in — so it would be a second **source** for one piece of state rather than a second spelling of
the first, which is the arrangement in which two of them eventually disagree and the renderer has to pick.
The refusal costs a program nothing and the acceptance would cost a field its single writer.

`ReportCellSize` is different in kind, because it is a **query**: honouring it means REPLYING, and a
reply is an advertisement. cmote is not short of the answer — the GUI sets the cell size through
`Terminal::set_cell_pixels`, and `CSI 14t` is answered by multiplying it by the grid. What makes this
spelling the wrong one to answer is *why it is asked*: in iTerm2 the question is asked in order to size
an **inline image**, which is `File=`, which is refused a few paragraphs up. Answering it precisely and
then dropping the picture is a worse outcome for the sender than silence, which is what lets it fall back
to a protocol cmote does draw. This is the same standard the rest of the document already holds itself
to — §41 refuses an oversized sixel outright rather than clipping it, because "a refusal draws nothing;
a clip would silently misreport what the host sent", and XTMODKEYS answers only its own resource because
"there is no way to say *not mine* except by not answering". **This paragraph used to end with a
supporting clause that §147 took away**: the vendor key was "not being singled out" because `CSI 16 t`,
the standard spelling of the same question, went unanswered too. That is no longer true — `CSI 16 t` is
answered, and `CSI 14 t` was already — and the clause was weak to begin with, because a refusal that
needs a second refusal to look even-handed has not stated its own reason. The reason above never
depended on it: what is refused is the door into `File=`, not the arithmetic. The test that pins the
refusal now answers the standard question in **both** its spellings on the lines below it.

Both are **🛑** on the mechanism that refuses every unlisted key — the allow-list — and both are now
pinned by name, which is the point of doing it at all. A refusal with a threat behind it defends itself;
a refusal whose only reason is *"we already answer this"* is precisely the one a later reader deletes as
a courtesy to a program that did not need it.

**The nine DEC-private status reports that describe the machine** — `CSI ? Ps n` is a family, and §82
answers exactly one member of it. DECXCPR (`Ps = 6`) reports where the cursor is, which is a fact about
the remote's own output and is now shipped. The other nine xterm defines report the terminal's
**equipment**: a printer (`15`), the user-defined-key store's lock (`25`), the KEYBOARD's nationality
(`26`), a locator's availability and type (`55` / `56`), macro space (`62`), a memory checksum (`63`),
a data-integrity self-test (`75`) and a multi-session controller (`85`). Each is refused on the standard
one paragraph up — a reply is an advertisement — and none of the equipment exists here, so "ready",
"unlocked" or a byte count would each be a claim about hardware that is not there.

`26` is the one refused on more than tidiness. It answers with the keyboard's nationality, and §36 fixed
the rule it would break: **cmote's identity replies name the program, never the person's machine.** That
is why DA3 sends a constant unit id rather than the serial number DEC hardware put there. A remote must
not learn the layout in front of the user off a query the user never sees.

The refusal is `term/dsr.rs`'s allow-list, one value wide — the construction `term/iterm.rs` uses for
OSC 1337 keys and `term/pointer.rs` for pointer shapes — and it is pinned at both ends, in the scanner
and at the boundary, by tests that name all nine.

**Remote-set mouse pointer shape** — `OSC 22` is **half shipped and half refused** since §77, and the
half that ships is the part this entry used to say was impossible. What survives here is only the
refusal; the sequence's own row in part 8 carries the rest.

This entry argued the refusal on three grounds and **two of them were wrong about cmote rather than
about the sequence**, which is worth leaving on the record rather than quietly deleting:

- *"The pointer is window-wide, so it fails the same test §54 applies to progress"* — true of
  `winit::window::Window::set_cursor`, which a terminal built straight on the windowing layer would
  have to call, and which cmote never touches. The grid is an iced `mouse_area`;
  `mouse_area::interaction` applies while the pointer is inside that widget and stops at its edge.
- *"The pointer is already contested, and the arbitration is hand-rolled unsafe code"* — the four
  contested shapes (`ResizingHorizontally` / `ResizingVertically` on the two splitters, `Grab` /
  `Grabbing` on the drag handles, the last two painted by §51's own `WM_SETCURSOR` subclass because
  Windows ships no hand cursor) sit on widgets that are **siblings** of the grid and never over it.
  There was no fifth voice to arbitrate and nothing was added to that subclass.
- *"`none` is in the vocabulary, so a remote could hide the local pointer"* — it is not.
  `cursor_icon::CursorIcon`, which is what every terminal resolves an OSC 22 name through, has no
  hidden variant and its `from_str` no `"none"` arm. The hazard cannot be spelled.

What is genuinely refused, and refused by `term/pointer.rs`'s allow-list rather than by absence, is
every shape that **makes a claim cmote's own chrome is entitled to make**. The five kept shapes
(`default`, `text`, `pointer`, `crosshair`, `cell`) describe the content under the mouse, which on the
grid is the remote's own output. The rest divide into two refusals and a remainder:

- `grab`, `grabbing`, `move` and the fourteen resize shapes are **cmote's vocabulary**. Those exact
  shapes are what the splitters and the drag handles say, so a remote painting one over the grid
  teaches an affordance that is not there. Same class as a spoofed window title (§55, §69).
- `wait`, `progress`, `not-allowed` and `no-drop` **speak for the client**. A remote must not be able
  to make cmote look hung, or as though it were refusing the user's input.
- `help` and `context-menu` announce a menu that is cmote's; everything else left over has no meaning
  inside a text grid, and can be added later with a reason attached.

That is a **🛑**: the list is the parser, and three tests pin it — two on the list itself and one at
the seam, which sets an allowed shape first so the refusal is that shape surviving.

**Answerback (ENQ `0x05`)** — refused for the same reason, and this is why xterm ships it empty too
(§36). The trigger is a *single ordinary byte*, so any binary output that happens to contain `0x05`
— a `cat` of a binary, a corrupt download, a stray progress stream — would type the answerback string
into the shell's input as if the user had. That is a remote-driven side effect on the user's keyboard
in exchange for legacy identification nobody asks for; the DA / DECRQM / XTVERSION / DA3 answers cover
every probe a modern program makes.

The refusal is a **🤷** rather than a **🛑** in part 8, and precisely because it is: `vte`'s `execute`
matches HT / BS / CR / LF / VT / FF / BEL / SUB / SI / SO and drops `0x05` to a `debug!`, and cmote's
own scanner has no arm for it either. Answerback is refused by never having been written, which is a
decision this section stands behind and no code enforces.

**Reading the screen back in bulk — `CSI > Pl ; Pr t` (contour's buffer capture) and `CSI > Ps b`
(its semantic-block query)** — refused since §98, and they are where §60's line finally gets tested
from the other side. DECRQCRA is *allowed* to read the page, and the argument was that every byte on
that page came from the pty the reply goes back down, plus two enforced properties: the rectangle
clamps to the **visible page**, and what comes back is a 16-bit checksum rather than the text. Both of
these break the first property and neither has the second. A capture's whole purpose is to reach into
the scrollback — which in an SSH client can hold the output of a session that ended before this one
began, on a different host, under a different account — and it returns the text itself. The block query
is worse in kind, not in degree: it returns the **command lines, prompts, output and exit codes** cmote
records for its own gutter marks (§34), which is the one thing in this program built by watching what
the user does.

It is also the cleanest illustration of §96's half-rule going the other way. cmote *reads* OSC 133,
a dialect it does not claim, and that is allowed because a read produces no reply. The block query is
the same data flowing outward, and outward is the direction the rule binds.

Both are **🤷**: `vte` has no arm for either final byte under a `>` marker, so nothing here refuses
them and this section is the whole of the refusal. Worth stating anyway — contour gates its own query
behind a four-word token the terminal mints, which is the vendor agreeing about the danger rather than
disposing of it, since the token travels the same wire the answer does.

**A remote's payload becoming a local process — `OSC 88`, the Terminal Resume Protocol** — refused
since §98, and it is a category this section did not have. Everything above is about a remote reaching
something the user owns: their clipboard, their colours, their desktop, their keyboard. This one is a
remote handing the terminal a **command line** — `arm ; cmd=<base64> ; args=<base64> ; cwd=<path>` — to
be run if the terminal ever restarts. The proposal's own framing is that a program knows best how to
relaunch itself, which is true in the situation it was written for, where the program and the terminal
sit on one machine and answer to one person. cmote is an SSH client and the two ends are not the same
person: the program declaring itself is on the far side of the wire, and the relaunch would happen
here. Nothing that arrives over the pty becomes a local process, and there is no configuration that
makes it one.

That is a **🛑**, and the `query` operation is refused with the rest rather than answered "not
supported". A reply is an advertisement (§71) and this one is the advertisement that *brings* the arm;
the honest-negative exception §93 carved out does not reach it, because "I do not support this" is a
statement about a feature the sender can then stop asking for, while here the whole exchange is one the
terminal is better off never having been in. `term/notify.rs` names it, and a boundary test
(`a_remotes_font_change_and_relaunch_specification_get_nothing`) fails if any of it starts being read.

**Implicit bidi — the Unicode Bidirectional Algorithm** — refused, and this entry is what is left of a
larger one. §75 put the whole of SCP here and §76 took most of it back out: the **character path** is
shipped, per line, and the row in part 8 is ✅. What stays refused is the other half of ECMA-48's bidi model,
and the distinction is worth keeping because the two are usually said in one breath.

ECMA-48 pairs SCP with **BDSM**, bidirectional support mode (`CSI 8 h` / `8 l`), and its default is
**explicit**: the sender has already put the characters in the order it wants them laid down, and the
terminal only has to know which way to lay them. That is the half cmote implements. **Implicit** mode is
the other half — the terminal runs the Unicode Bidirectional Algorithm over each line and works the order
out itself, so that a number inside an Arabic run comes out left-to-right without anyone saying so. cmote
does not do that and is not going to.

The reason is that the algorithm is not the expensive part; the **ambiguity it introduces** is. Under
implicit bidi the mapping between a data column and a presentation column stops being a function of the
line's direction and becomes a function of its *content*, recomputed whenever the content changes. §76's
mirror is one involution (`scp::flip`) shared by the renderer and the pointer, which is what makes a click
on a mirrored line provably land on the character that was drawn there. A UBA-derived mapping is a
per-line table that both sides would have to agree on, and it would have to be rebuilt on every write to
that line, then threaded through the find bar's match spans, the Ctrl-hover link runs and the selection's
own arithmetic. `[ECMA-48]`, and deliberately not priced the way margins are in part 5: the work is not in the
sequence, so a line count would be a fiction.

Note what this costs a program, because it is not nothing and is not much. A host that wants right-to-left
text laid out correctly has to run the algorithm itself and send the result — which is what a host does
today for **every** terminal that has no bidi, xterm included, and cmote's contribution is that the path
it then asks for is now honoured instead of dropped.

And note who does the declining: nobody has to. `vte`'s `NamedMode` names only `Insert = 4` and
`LineFeedNewLine = 20`, so BDSM arrives as `Mode::Unknown(8)` and reaches nothing. cmote is in explicit
mode and cannot be asked out of it, which is consistent rather than lucky — this is the **🤷** shape, one
paragraph after the OSC 22 entry above, except that it has no row of its own in part 8 because no sequence
carries it that cmote answers.

One parameter of SCP itself is refused too, and that one **is** cmote's own code: `Ps2 = 2`,
"presentation to data", asks the terminal to write the drawing back into the grid. That is the engine's
state and the only copy of what the host sent, so `term/scp.rs` drops the whole sequence rather than
taking the path and ignoring the update mode. It has its own row in part 8 and its own 🛑.

---

## Part 7 — Recommendation

**There is no A-sized item left.** Input (part 2), query→reply (part 3) and the rendering/attribute layer
(part 4) are all closed; what remains is the engine's own ceiling (part 5) and the two sequences cmote refuses
on purpose (part 6). §60's audit briefly reopened part 3 — `CSI ? 4 m` went unanswered — and §61 closed it in a
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
added to the engine's own **DA1** reply. Details in PLAN §41; the moved rows are in part 5 and part 8.

**§65 swept the partial rows and turned up one item of real work.** Auditing all ten the way §60 taught —
`vte`'s dispatch arms first, then which `Handler` methods the engine leaves at their empty default —
split seven into an ✅ half and a refused-or-missing half, re-marked `BEL` as a 🛑, and confirmed two
(DECRQSS, XTGETTCAP) as plainly partial. The finding is **mode 2026**: `vte` batches a synchronized
update in its own `Processor` and bounds a stuck one with a 150 ms timeout, but that expiry is the
application's to drive (`sync_timeout()` then `stop_sync()`), and cmote drives neither. A remote that
sends `CSI ? 2026 h` and then stops writing holds the visible screen at its pre-BSU state until it sends
the closing `l` or pushes 2 MiB. Nothing leaks and the session is unharmed — a stuck picture, not a stuck
client — but it is a remote-triggered effect on cmote's own window, which is the thing part 6 spends its
length refusing. The fix is small and has a shape already in the codebase: an `iced::window::frames()`
subscription while an update is pending, the way `SnackbarTick` and `QuitTick` are driven, calling
`stop_sync` once the instant passes. Not taken in §65, which was an audit.

*[**§122 took it, in almost exactly that shape, and found one thing the sketch above had wrong.** The
clock cannot go through `broadcast` the way `SnackbarTick` does: that reaches each region's on-screen tab,
and a held frame's whole hazard is the tab you are NOT looking at — a background shell is still being fed
(§26), so it can be mid-frame, and nothing on screen would hint that the tab you switch to is showing a
screen from a minute ago. So the walk is over every tab, and over every parked identity's terminal
(§45), and the replies each release produces go back by that identity's own route. The rest went as
predicted: two methods on `Terminal`, one frame subscription behind one condition, and the release fed
through the gate because held bytes reach the engine exactly as live ones do. PLAN §122.]*

**§66 retired the partial class and inherited two small gaps from it.** Splitting the last two partial rows —
DECRQSS and XTGETTCAP — meant deciding what their declined halves are, and neither is a refusal: cmote
answers honestly ("not reported", "unknown") because it has no reporting code, not because a policy says
no. So both are ❌, and both are answerable. **DECRQSS** could report three more selectors from state that
already exists: `SP q` (DECSCUSR) from `Screen::cursor_shape`, `" q` (DECSCA) from the protection bit
§56 owns, and `r` (DECSTBM) from the engine's region behind one new seam getter — the note claiming the
cursor "renders fixed" has been stale since §60 shipped the shapes. **XTGETTCAP** could state `Tc` and
`RGB`, the two capabilities a shell actually asks about, cmote's 24-bit SGR being real. Neither is large
and neither is urgent — a program that gets an honest "no" behaves correctly today, which is why they sat
unnoticed under a mark that said "partial" and meant "unexamined".

*[**§123 took both, and only one of the four items above survived contact with its source.** DECRQSS went
as written — the three selectors were exactly where §66 said, and a fourth came free, because §102 had
since given cmote the margins DECSLRM reports. The one it got wrong is `Tc`. "The two capabilities a shell
actually asks about, cmote's 24-bit SGR being real" is true about the SGR and wrong about the query: `Tc` is
tmux's own extension, absent from xterm's special-name list AND from ncurses' recognised user capabilities,
and it is a **boolean** in a reply grammar whose recognised form is `<NAME>=<VALUE>`. There is no value to
send, so answering it means inventing one on nobody's authority — it is a 🛑, not the ❌ this paragraph
assumed. `RGB` is the one that had an answer all along, and the "ambiguous wire value" the code comment had
pleaded is settled in one line of ncurses' `user_caps(5)`: a numeric `RGB` is the bits per channel, so `8`.
The lesson is §70's, one column over — a note pleading ambiguity is a note nobody has looked up. PLAN
§123.]*

*[**§152 and §153 finished both rows, and both had been worked from a sample.** §66 and §123 each named
"three more selectors" and "two capabilities" without reading either list out. §152 read xterm's DECRQSS
list entire — fifteen selectors and three extensions — and found **four** more answerable from state
that already existed (DECSLPP, DECSCPP, DECSNLS off the grid, DECSACE off §59's scanner), plus two,
DECSASD and DECSSDT, whose truthful `0` is refused because it is an invitation: a program told the
status line exists writes to it, cmote ignores the set silently, and the text lands on the user's page.
§153 read XTGETTCAP's list and found one spelling missing (`name`, the terminfo half of `TN`) and that
the remainder is a **refusal**, not a gap — xterm answers key capabilities from `tigetstr`, and the
entry cmote would transcribe is `xterm-256color`, sitting on the remote that asked. It also killed the
folklore under §66's original sentence: tmux does not send XTGETTCAP at all. PLAN §152, §153.]*

**§67 is the same discipline applied to ✅ itself.** "Full" was the one mark that asked a reader to
believe a row rather than check it, so it now reads *supported* and the note carries the extent. The sweep
cost nothing and paid for itself: one supported sequence had no row at all (`1005`, the UTF-8 mouse
encoding, live on the seam since the mouse shipped), DSR was one row for two different reports, one row
was right for a reason it never gave (`ESC % G`), and one more ❌ came to light (`CSI ? 6 n`, which §82
later shipped once its stated cost turned out not to exist). None of
that is work — it is the table saying what it already does.

**§68 closed that consequence rather than leaving it as a note.** The ✅ rows carrying a "but only…"
clause are now split in ✅/❌ or ✅/🛑 pairs: `OSC 0`'s title against its icon half (❌ at the time; a 🛑
since §69 built the other half), `OSC 8`'s three openable schemes against every other one (drawn, never launched — `link.rs`),
`ESC ( ) * +`'s `B` and `0` against the 94-charsets `vte` drops, DECSCUSR's shape against its blink (a
refusal cmote performs, the engine having stored the flag), XTSMGRAPHICS' read against its set (answered
`status 3`), DECKPAM's encoded keys against the numpad digits NumLock owns, and the XTMODKEYS query's
resource 4 against the six cmote does not track. One case needed no split: DECSTBM's horizontal twin is
DECSLRM, which has had a row of its own since §57, so the note points at it. Deciding each pair's second
mark is what the split is worth — three turned out to be refusals cmote performs, four gaps nobody had
named.

What is left in part 5 (blink, double-height lines, left/right margins, rectangular ops, synchronized output,
and kitty graphics — iTerm2's inline images left for the 🛑 column in §70) is legacy, rare, invisible in
practice, or a whole protocol's worth of work — **no item of real UX value remains anywhere in this
document.**

*[That list has been overtaken three times over: the rectangular ops shipped in §58, the margins in
§102, and synchronized output's abort timeout in §122. All three were on it as things "left", and all
three turned out to be movable — which is the reading of this paragraph worth keeping. It was right that
nothing here was blocking a user; it was wrong to let that double as a reason not to re-price the items.
What is left of part 5 after §122 is blink, double-height lines and kitty graphics: two the engine drops
outright, and one whole protocol.]*

*[And a fourth time, for the same reason: **the double-height lines shipped in §146**, on the same
beside-the-engine tactic — the engine has no notion of a line's size and does not need one, because
the size is a rendering rule and not grid state (§76). What is left of part 5 is blink and kitty
graphics. The pattern is now four for four, and the lesson is the one this paragraph keeps having to
learn: "the engine does not represent it" prices an item as engine work, and for everything on this
list so far it was not.]*

*[**§151 broke the run, and the break is worth more than the streak was.** Blink was priced properly
for the first time and the answer is that the beside-the-engine tactic really does not reach it. Every
one of the four above worked because the thing being kept beside the engine was keyed by LINE — a
character path, a line size, a picture's anchor — or was not per-cell state at all. Blink is per CELL,
and per-cell state has exactly one cheap home: the engine's own 16-bit flag word, which names fifteen
bits and had its sixteenth borrowed by §56. A map beside it would be dropped on every reflow and
unbounded in remote input (§12). So the four-for-four reading stands with a boundary drawn under it:
"the engine does not represent it" is not a price, but "the engine has no room for it" is — and telling
the two apart takes counting the bits, which is what nobody had done. What is left of part 5 is still
blink and kitty graphics, and neither is a mispricing now.]*

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

**§64 then closed the smaller version of the same hole, one column over.** The two partial colour rows were a
working query and a refused set under one mark — now four rows, `(query)` ✅ and `(set)` 🛑 apiece — and
the refused half had no test — it was held up by the
renderer's structure (`ui/grid.rs` cannot reach the engine's colour table) plus the fact that nobody had
wired that table into `report_color`. The first of those is a real guarantee; the second is the §63
pattern exactly, a correct outcome nothing asserts. Two tests now set a colour and prove the answer does
not move, and each asserts the engine *did* record the set first, so neither can pass because the set was
silently dropped on the way in. The same pass deleted a cost this document had invented for the refusal
(see part 6): the full-screen repaint a colour set was said to cost does not happen, because cmote never
reads the engine's damage flags. Nothing shipped and **no answer changed** — the two partial rows became a
✅ and a 🛑 apiece because that is what they always were, and the rows just stopped
claiming more than they knew.

The **DECKPAM** subset shipped as a seam getter (`Screen::application_keypad`) plus one guarded branch
in `keymap::encode` — the numpad keys with no NumLock meaning to lose, and explicitly *not* the digits
(part 2, §36). **DA3** shipped as a `CSI =` state in the same scanner that answers XTVERSION, with a
constant unit id chosen so the reply cannot fingerprint the machine (part 3, §36).
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
shell-integration** (part 4's old low-pri row) shipped as `term::osc133`: the stream is scanned for the
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
double-height lines, kitty graphics) is legacy,
invisible in practice, or a protocol nobody here has asked for; iTerm2's inline images were on this list
until §70 moved them to the other column, the rectangular ops shipped in §58, the margins in §102, and
synchronized output's abort timeout in §122. For "support *any* documented app UX",
there is no outstanding ceiling-raiser left; every item this document ever listed is either shipped or
refused with its reason written down. *[§146 took the double-height lines off that list too, leaving
blink and kitty graphics.]*

*[§102 read this paragraph as a warning rather than a summary. "No outstanding ceiling-raiser left" was
true of USER-visible features and was quietly doing duty as "nothing left worth costing" — and the
margins, on that same list, went on to ship. The tactic named here also stopped being the whole story:
§102's route stands IN FRONT of the engine rather than beside it, which is still not touching the
engine and is not the same claim.]*

**§54 then closed the OSC column's last item of real value, and turned four ❌ rows into decisions** —
🛑 and 🤷 rows since the legend grew marks of their own — and the split was instructive: OSC 9 is
refused by cmote's own scanner and pinned by a test, while `OSC 777` and `kitty 99` are refused by
nobody at all, since `vte` has no arm for either. **§79 closed that split**: the three spellings are
one decision, so they now sit in one place (`term/notify.rs`) under one mark, and all three read 🛑.
`OSC 9;4` progress reporting shipped (`term/progress.rs`) — a per-tab bar on the chip and the taskbar
button mirroring the active tab. The same pass wrote down the stance the notification rows had been
missing: `OSC 9;<text>`, `OSC 777` and `kitty 99` are one feature in three spellings and are **refused**
(part 6), because a notification escapes the tab and lands on the desktop; `kitty 21` is refused for the
reason 4 / 10 / 11 / 12 already were, a fixed scheme. Those rows now read as choices rather than as
work not yet done, which is the difference between a gap and a policy. **§78 later found that the
`kitty 21` half of that sentence was doing two rows' work with one row's argument** — 4 / 10 / 11 / 12
are *split*, query answered and set refused, and OSC 21 carries both halves in one sequence — so the
query half got a reason of its own. Same mark, different sentence; see part 6 and PLAN §78.

**`OSC 22` was then decided too, which empties the OSC column: every row is now shipped or refused with
its reason written down, and none is merely outstanding.** The mouse-pointer shape looked at first like
the one cheap gap left — tab-local, and cmote already owns a cursor mechanism to hang it off (`cursor.rs`,
§51). Looking properly reversed that, and **§77 reversed it back**, because two of the three reasons were
about a cmote that does not exist. "The pointer is *window-wide* chrome that travels over the strip, the
panes and the dialogs" describes `winit::window::Window::set_cursor`, which cmote never calls; the grid
is an iced `mouse_area` and `mouse_area::interaction` stops at that widget's edge. "cmote's cursor is
already contested by four shapes of its own, arbitrated inside a hand-rolled `WM_SETCURSOR` subclass" is
true, and all four of those shapes are on widgets that are siblings of the grid and never over it, so
there was no contest and nothing was added to the subclass. Only the third reason was checkable against a
crate, and it was **false**: `none` is not in the vocabulary — `cursor_icon::CursorIcon` has no hidden
variant and its `from_str` no `"none"` arm, so the hazard cannot be spelled. What was left underneath was
the refusal that had never been separated out: the shapes that speak with cmote's own voice. The row is
now ✅ for the five that describe the text under the pointer and 🛑 for the rest, and the cost the entry
had admitted — no I-beam over text — is paid back. See part 6 and §77.

---

## Part 8 — Feature support matrix (vs the published catalogues)

A per-sequence audit against the escape-sequence catalogues published at
[vtdn.dev](https://vtdn.dev), [contour](https://contour-terminal.org/vt-sequence/) and
[otty](https://docs.otty.sh/vt/terminal-comparison) — the first alone until §98, which added the other two and found
thirty-four sequences this table had never named — so support is legible one line at a time rather
than only as the "still-missing" lens of parts 2–6. Every ✅/❌/🛑/🤷 below was verified against the real sources — the
engine crate (`alacritty_terminal-0.26.0`), its parser (`vte-0.15.0`), and cmote's own layer
(`term/`, `ui/grid.rs`) — not from memory.

Legend: **✅** supported · **❌** not supported · **🛑** refused, by cmote's own code · **🤷** refused in
principle, by nothing in particular. **Four marks, and four is the whole set** — a fifth once meant
"partial or a deliberate quirk", and §66 retired it for the reason the rule under the bullets gives.

**The marks live in the Status column and nowhere else** (§81). A note that has to speak of a mark —
its own history, or another row's — spells it in words: *supported*, *not supported*, *refused*,
*refused with nothing behind it*. A symbol inside a note reads at a glance as the row's answer, and
several of these notes recite two or three of them while describing what the row USED to carry, so the
eye that skims a column of marks finds the wrong ones sitting beside the right one. One row, one mark,
one place to look for it.

**The Note column defines the feature and nothing else** (§83). A note says what the sequence, attribute
or mode *is* — the parameters it takes, the reply it draws, the extent cmote honours — briefly and
exactly, and then points at what argues the row: the section number, and the module where cmote's own
code sits. The argument itself lives in parts 2–7 and in `PLAN.md`'s numbered sections, which is where it was
written first. The notes had grown into second copies of it, several of them hundreds of words long, and
a table that has to be read a paragraph at a time is not a table — it is a document filed in a grid. The
pointer is what makes the trade checkable: every fact a note used to carry is still recorded where the
pointer sends the reader.

**✅ says *supported*, not *complete*** — §67 narrowed it deliberately. The note carries the extent: which
parameters, which spellings, which direction. Since §83 no note is empty, so the strong claim — the whole
sequence works with nothing withheld — is a definition that names no extent at all, and §67 swept the
table so that claim is only made where it is true. The reason for the narrowing is the same one that
retired the partial mark: "full" is a word
a reader supplies for themselves, and every wrong row this document has found was one somebody read
generously.

The last three are different in kind, which is why each carries its own mark rather than one mark and a
footnote:

- **❌** is a *gap* — a sequence that could still land, and several since have.
- **🛑** is a *decision* **cmote enforces**: a scanner allow-list, an event dropped in the listener, a
  renderer that never reads the value, a final byte cancelled in flight. The mark says *who performs the
  refusal*, not why it was taken — nearly all of them are part 6's, on policy, and one is part 5's, on price
  (DECSLRM, §73). Since §83 the row names the feature and points at the section that argues the refusal
  rather than re-arguing it, and at the module the refusing code sits in. The refusal itself never
  becomes work, and it is usually pinned by a test that section names, so it cannot regress unnoticed.
- **🤷** is the same decision with **nothing behind it**: the sequence dies upstream — no `vte` dispatch
  arm, or a `Handler` method `alacritty_terminal` leaves at its empty default body — so cmote is never
  offered it and pays nothing to refuse it. *Where* each one dies is recorded under Evidence since §83,
  rather than in the row that would only be quoting it. These are stances, not
  guarantees: an engine bump could start handing the sequence over, and then the listener's catch-all is
  the only thing standing there. The distance between agreeing with a refusal and performing one is what
  §57 is about, and it is worth seeing in the column rather than reading for.

And one rule over the three: **one row, one answer, one mechanism.** A code that answers some requests and
declines others gets a row for each, rather than one mark averaging them — `OSC 52` has been `(write)` and
`(read)` since §62, §64 split `OSC 4` and `OSC 10 / 11 / 12` into `(query)` and `(set)`, §65 split seven
more (✅/🛑 for `SetUserVar` and mode 12, ✅/🤷 for modes 3 and 80, ✅/❌ for `CSI ! p`, the locking shifts
and mode 2026), and §66 split the last two, DECRQSS and XTGETTCAP. **§72 rejoined one of them** — `CSI ! p`
became ✅ on both halves, and a split whose halves agree is a row saying one thing in two places, which the
rule is against for the same reason it is against the reverse. **§143 met that edge twice and did NOT
rejoin**, which is worth recording as the rule's boundary rather than an exception to it: both of §65's
charset splits are now ✅ on both halves, and neither is one fact in two rows. `ESC ( B` / `0` against
the other finals is the two sets `vte` DISPATCHES against the eleven it drops — two doors, argued
differently — and SI / SO against the other locking shifts is the two that reach GL and are live
against three that reach GR, where nothing can ever land because cmote is UTF-8 only (§67). Same mark,
different fact. The rule is about a row that cannot be CHECKED, and each of these four can. That is why there is no "partial" mark
here: a row saying two things at once cannot be checked, and every finding this document has recorded came
from checking one. Sections below that speak of a row having been "partial" are describing what it used to
carry, not a mark still in use — the symbol itself is gone from this document, so it cannot be copied into
a new row by someone skimming for an example. **§68 paid the last of the cost**: the ✅ rows that had been
carrying a "but only…" clause are split too, so every row in this table is now one answer — `OSC 0`'s title
against its icon half, `OSC 8`'s three schemes against every other one, DECSCUSR's shape against its
blink, XTSMGRAPHICS' read against its set, DECKPAM's keys against its digits, and the two charset finals
that work against the rest. Where a row's second half already had a row of its own — DECSTBM's horizontal
twin is DECSLRM — the note points at it instead of repeating it.

**§69 is the first row a split turned into shipped work**, and the case for the rule. `OSC 0`/`OSC 1` had
been one ❌ row reading "dropped wherever it is spelled"; splitting it in §68 made the two halves answerable
separately, and they turned out to have opposite answers. `OSC 1` is now ✅ — a scanner of cmote's own, a
name on the tab chip. The icon half of `OSC 0` is now 🛑, refused for a reason that only became visible once
it had to be stated on its own line. The unsplit row could not have produced either: it had already told the
reader there was nothing here to decide.

**§70 is the first row corrected for a reason that expired rather than a reason that was wrong**, which is
a failure mode none of §66–§69 would have caught. Every sweep before it re-derived the marks from the
crates, and against the crates `iTerm 1337 File` read correctly: not supported. What had changed was
somewhere else entirely — §53 took the decoder this row was refused over as a direct dependency, for the
file preview — so the row's *cost* quietly became zero while its note went on charging for it. What was
left underneath is a refusal `term/iterm.rs` performs twice and a test already pinned, i.e. a 🛑 that had
been sitting in the ❌ column since before those marks existed. The lesson for the next sweep: a note that
names a price has to be re-read whenever the price is paid somewhere else, and no amount of re-reading
`vte` will surface that.

### OSC — Operating System Command

| Code | Feature | Status | Note |
|---|---|---|---|
| 0 (title half) | Window title | ✅ | the title half of `OSC 0 ; text`, which sets the window title and the icon name to the same string; control characters stripped (`term/mod.rs`) |
| 0 (the icon half) | Icon name | 🛑 | the icon-name half of that one sequence — the same string the title half already carries (§69, `term/icon.rs`) |
| 1 | Icon name | ✅ | `OSC 1 ; text` sets the icon name alone; drawn on the tab chip after the endpoint, control characters stripped, capped at 24 characters, empty clears (§69, `term/icon.rs`) |
| 2 | Window title | ✅ | `OSC 2 ; text` sets the window title alone; control characters stripped (anti-spoof) |
| 3 | X11 window property | 🤷 | `OSC 3 ; prop=value ST` sets a property on the terminal's X11 window, for another X client to read. There is no X server under this one — cmote is a Windows program — so the sequence names a thing that cannot exist here, and the refusal is of the *shape* rather than the effect: a remote does not get to write metadata the machine's other programs read (part 6, §98) |
| 4 (query) | Palette entry query | ✅ | `OSC 4 ; index ; ? ST` asks for a palette slot, in `index ; spec` pairs so one sequence may ask about several; each is answered as an `rgb:` triplet from the scheme `ui/grid.rs` paints (§64, §87, `report_color`) |
| 4 (set) | Palette entry set | 🛑 | `OSC 4 ; index ; spec ST` writes one palette slot — the theme the user chose (part 6, §64) |
| 5 / 105 / 106 | Special colours — set / reset / enable | 🤷 | `OSC 5 ; <slot> ; <spec> ST` tints an SGR ATTRIBUTE rather than a palette entry — slot `0` bold text, `1` underline, `2` blink, `3` reverse video, `4` italic — with `105` resetting them and `106` enabling or disabling one. The fixed scheme reaches further than the palette: a remote colouring "all bold text" is choosing what the user's screen looks like by another door (part 6, §98) |
| 7 | Working directory | ✅ | `OSC 7 ; file://HOSTNAME/CURRENT/DIR ST` announces the shell's working directory — macOS Terminal's sequence originally, and the one a new tab inherits its directory from (§17, §89, `term/cwd.rs`) |
| 8 (http / https / mailto) | Hyperlinks | ✅ | `OSC 8 ; params ; uri ST` opens a hyperlink over the cells that follow and `OSC 8 ; ; ST` closes it; underlined under Ctrl-hover, followed on Ctrl-click or from the right-click menu (§24, `link.rs`). `params` is a `:`-separated `key=value` list of which the spec defines exactly one key, `id`, tying a link's separated runs together: "character cells that have the same target URI and the same nonempty id are always underlined together on mouseover". The Ctrl-hover underline covers **every cell carrying that same link**, wherever it sits — the identity being the URI *and* the id, so a link the program split into runs lights up whole and one address written twice stays two links (§88, §92) |
| 8 (any other scheme) | Hyperlinks | 🛑 | the same sequence carrying any other URI scheme — a scheme decides which local program the OS launches, so the link is drawn and never opened (§24, `ALLOWED_SCHEMES`). The spec leaves this open on purpose: "It's up to the terminal emulator to decide what schemes it supports" (§88) |
| 9 | Desktop notification | 🛑 | `OSC 9 ; text` raises a desktop notification, which leaves the window and lands on the desktop (part 6, §54, §79, `term/notify.rs`) |
| 9;1 | Sleep the terminal (ConEmu) | 🛑 | `OSC 9 ; 1 ; ms ST` asks the terminal to stop for that many milliseconds. Refused by name since §90, and it is the one refusal in this document that is not about something leaving the tab: it is a remote spending the **user's own time**, holding the window still in front of the person at the keyboard for as long as it likes. Pinned by `the_sleep_and_the_message_box_are_refused_as_themselves` and at the boundary by `a_remotes_sleep_and_message_box_get_nothing` (part 6, §89, §90, `term/notify.rs`) |
| 9;2 | GUI message box (ConEmu) | 🛑 | `OSC 9 ; 2 ; "txt" ST` raises a modal dialog carrying the remote's text. The notification refusal's own argument, one step further: a notification leaves the window, a dialog leaves the window **and takes the focus**, wearing cmote's identity while it does (part 6, §54, §89, §90, `term/notify.rs`) |
| 9;3 | Tab text (ConEmu) | ✅ | `OSC 9 ; 3 ; "txt" ST` names the tab — ConEmu's spelling of what `OSC 1` does, quoted in its own documentation and read through the same module, so there is one writer of the chip name and two doors to it (§71's test for a second spelling). Capped, sanitised and appended after the endpoint exactly as `OSC 1` is, so the new spelling buys a remote no more of the chip than it had; an empty name clears it (§69, §89, §90, `term/icon.rs`) |
| 9;4 | Progress reporting | ✅ | `OSC 9 ; 4 ; st ; pr ST` reports task progress on what ConEmu drives as the Windows taskbar: `0` removes it, `1` sets it to `pr` (0–100), `2` is an error state, `3` indeterminate, `4` paused. All five, drawn per tab and mirrored on the taskbar button (§54, §89, `term/progress.rs`) |
| 9;9 | Working directory (ConEmu) | ✅ | `OSC 9 ; 9 ; "cwd" ST` — ConEmu's working-directory spelling, a bare native Windows path, quoted in ConEmu's own documentation of it. Read beside OSC 7 and iTerm's `CurrentDir` in one scanner (§17, §89, `term/cwd.rs`) |
| 10 / 11 / 12 (query) | Default fg / bg / cursor colour query | ✅ | `OSC 10/11/12 ; ? ST` ask for the default foreground, background and cursor colours; a list walks UP from the code it starts at, so `OSC 10 ; ? ; ?` asks for the foreground and then the background. Answered from the scheme the grid paints, the cursor reporting the foreground since it is drawn by inverting the cell (§64, §87) |
| 10 / 11 / 12 (set) | Default fg / bg / cursor colour set | 🛑 | the same three codes carrying a colour spec — the fixed scheme again (part 6, §64) |
| 13 / 14 / 113 / 114 | Mouse pointer colours | 🤷 | the pointer's own foreground and background, and their resets — the ink of the cursor the user moves with their hand. `vte` reaches only as far as `12`, so these never arrive; the scheme argument covers them and one more besides, that the pointer belongs to the desktop rather than to the tab (part 6, §98) |
| 15 / 16 / 18 / 115 / 116 / 118 | Tektronix colours | 🤷 | foreground, cursor and background of the **Tektronix 4014 window** xterm can open beside its own, and their resets. There is no Tek emulation here and no plan for one, so the codes name a window that does not exist (§98) |
| 17 / 19 / 117 / 119 | Selection colours | 🤷 | the highlight's background and foreground, and their resets. The selection is drawn by `ui/grid.rs` over cmote's own colours and is the one surface the **user** operates directly — a remote that could set both halves could make a selection invisible, which is the UX-stability argument rather than the scheme one (part 6, §98) |
| 22 (`default` / `text` / `pointer` / `crosshair` / `cell`) | Mouse pointer shape | ✅ | `OSC 22 ; name` sets the mouse pointer shape over the grid; these five describe the content under the pointer, apply only while the pointer is inside the grid, and are cleared on both directions of the alternate-screen swap (§77, `term/pointer.rs`) |
| 22 (any other shape) | Mouse pointer shape | 🛑 | the same sequence naming any other CSS shape — the resize and grab shapes are cmote's own vocabulary, and `wait`, `progress`, `not-allowed` and `no-drop` make a claim about the client (§77, `term/pointer.rs`). One divergence from xterm, which on a name it does not know "uses the resource's default `xterm` shape": a refused name here leaves the pointer **as it was** rather than resetting it, so a remote cannot clear a shape it is not allowed to set (§88) |
| 30 | Tab name | ✅ | `OSC 30 ; name` — contour's `SETTABNAME`, the **third** spelling of the chip label beside `OSC 1` and ConEmu's `OSC 9;3`, read through the same module so there is one writer and three doors to it (§71's test). Capped at 24 characters, control characters stripped, appended after the endpoint and never in place of it, an empty name clearing it — a remote gains no more of the chip than the two older spellings already gave. Matched on the whole number, so `OSC 3` and kitty's `OSC 30001` are not it. **The thinnest-sourced row in this table**: one line of contour's sequence index, its detail page unreachable, and acted on anyway because being wrong costs a tab chip (§89, §98, `term/icon.rs`) |
| 50 (`CursorShape=`) | Cursor shape | ✅ | `OSC 50 ; CursorShape=0/1/2` sets the cursor to block, bar or underline — a third spelling of DECSCUSR's shape, with no blink to carry. **Not xterm's `OSC 50`**, which is the row below: this payload is another terminal's convention that `vte` happens to parse on the same code (§71, §88) |
| 50 (a font) | Set the font | 🛑 | xterm's own `OSC 50`: "Set Font to `Pt`", by name or by an index into its font menu (`#` for absolute, `#+` / `#-` relative). The font is chrome the **user** chose — the argument the fixed colour scheme rests on (part 6), and the one xterm itself gates behind an `allowFontOps` resource that defaults to off. Refused by name in `term/notify.rs` since §91, the `CursorShape=` payload on the same number excluded first so the refusal and that feature cannot disagree; `vte` drops it to `unhandled` as well (§88, §91) |
| 60 | Set every font face | 🛑 | `OSC 60 ; <faces>` — contour's `SETFONTALL`, which gets or sets every face, style and size at once. `OSC 50`'s refusal at a larger size and refused in the same place, by name, with no `CursorShape=` exception to carve out because this number carries one meaning (part 6, §91, §98, `term/notify.rs`) |
| 52 (write) | Clipboard write | 🛑 | `OSC 52 ; c ; <base64>` writes the local clipboard (part 6, §63) |
| 52 (read) | Clipboard read | 🛑 | `OSC 52 ; c ; ?` reads the local clipboard back to the remote (part 6, §63) |
| 88 | Terminal Resume Protocol | 🛑 | `OSC 88 ; <op> [ ; key=value ]… ST`, a **proposal** (v1, otty's): a program declares how it should be **relaunched** if the terminal restarts — `arm` stores a base64 `cmd`, `args` and `cwd`, `clear` withdraws it, `query` asks whether the terminal supports it. The one refusal in this document where the remote's payload would become a **local process**, at a moment nobody is watching; the query is refused with the rest, because "supported" is the advertisement that brings the arm (part 6, §12, §98, `term/notify.rs`) |
| 104 | Reset palette entry | 🛑 | `OSC 104 ; index` puts one palette slot back to its power-on colour, several indices in one sequence, and **`OSC 104` bare resets all 256** — the reset side of the fixed scheme (part 6, §87) |
| 110 / 111 / 112 | Reset fg / bg / cursor colour | 🛑 | reset the default foreground, background and cursor colours — the same fixed scheme (part 6) |
| 133 | Shell integration (semantic prompts) | ✅ | `OSC 133 ; A/B/C/D`, BEL- or ST-terminated, marks where the prompt, the command and its output begin and end; drives the per-tab status dot, jump-to-prompt and select-command-output, exit code from the optional field after `D` — optional in the grammar, `"D", [ ";", exitcode ]`, with the bare form a documented spelling rather than a tolerated malformation. Trailing `key=value` fields are ignored; the three named ones have their own rows below (§34, §95, §96, `term/osc133.rs`) |
| 133 `A ; click_events=1` | Mouse clicks in the prompt area | 🛑 | the field on the prompt-start mark that asks the terminal to "enable mouse click reporting for the prompt area" — input reporting switched on by a payload whose declared job is saying where the prompt sits, around the modes that gate it (§10), after which a click inside the prompt would behave unlike one a line above. This scanner cannot reach a mouse mode; a test states the refusal rather than leaving it incidental (§95, `term/osc133.rs`) |
| 133 `A ; k=s` | Secondary prompt (PS2) | ✅ | the field marking a **continuation** prompt — kitty's shell integration prepends it to zsh's `PS2`, once per line of a command still being typed, and `PS1` carries no `k=` at all. Read in order to **suppress** the mark: a continuation prompt is not a new prompt, and treating it as one both litters the gutter with ticks and files the finished command against its last continuation line instead of its prompt. An unknown `k=` value stays a prompt start, since losing an anchor is worse than gaining one (§97, `term/osc133.rs`) |
| 133 `C ; cmdline=` / `cmdline_url=` | The command line being run | 🛑 | the field on the output-start mark carrying the command line itself — shell-quoted by zsh, percent-encoded by fish. Refused on the second-source rule (§71): the command line is already on the grid, in the rows between the `B` and `C` marks, so the field is the shell's *assertion* about something cmote *observes*, and the two can disagree with the remote winning. Showing it would also need a surface that does not exist (§97, `term/osc133.rs`) |
| 133 `A ; cl=m` | Multi-line prompt hint | 🛑 | VS Code's field on the prompt-start mark, saying the prompt runs over several lines. Refused on the same ground as the command line: the prompt's extent is the grid between `A` and `B`, which cmote already records, so the hint restates a fact it can read. Nothing would change if it were honoured — a jump anchors on the `A` line, the prompt's first, hint or no hint (§71, §97, `term/osc133.rs`) |
| 133 (any other phase letter) | Further prompt phases | ❌ | the letter space past `A`/`B`/`C`/`D`. `N`, `P` and `L` are all emitted somewhere and the reachable accounts **disagree about what they mean**: Konsole is documented as tracking the prompt as "A/N/P", a zsh write-up uses `133;P;k=i` for `PS1` and `133;P;k=s` for `PS2`, and a Ghostty fork uses `133;P` for a prompt *redraw* that must not open a new block. Not a decision — a letter cannot be supported or refused until it means one thing. An unrecognised one yields no mark rather than the nearest guess, because a wrong mark moves a prompt jump or mis-bounds a command's output where no mark leaves both alone (§96, §97, `term/osc133.rs`) |
| 888 | Dump internal state | 🤷 | contour's `DUMPSTATE`, which writes the emulator's internal state to its debug stream. A remote deciding when the terminal logs itself, and what into; the log is the developer's tool and not a channel a host gets to drive (§98) |
| Kitty 21 | Colour by semantic name | 🤷 | `OSC 21 ; key=value ; … ST` names colours instead of numbering them — `foreground`, `background`, `selection_foreground`, `selection_background`, `cursor`, `cursor_text`, `visual_bell`, `transparent_background_color1`–`7`, and `0`–`255` for the palette. `key=?` queries, a **bare key with no `=`** resets, any number of pairs at a time. Refused as a **dialect** cmote never claims: `TERM`, XTVERSION and XTGETTCAP's `TN` all say xterm. §78's second reason expired — it held that answering the keys cmote lacks means inventing a colour, and the protocol's own answer for an undefined one is an **empty value** (part 6, §78, §89) |
| Kitty 30001 / 30101 | Colour stack — push / pop | 🤷 | the save and restore half of kitty's colour protocol, pushing the whole palette onto a stack and popping it back. Named in §89's reading of that page and given no row until now; refused as the same **dialect** the row above is, and over a palette that is never read either way (part 6, §78, §89, §98) |
| Kitty 99 | Rich notifications | 🛑 | `OSC 99 ; metadata ; body ST` — a desktop notification whose metadata is a `:`-separated `key=value` list: `p` the payload type, `i` an identifier for updating a notification already shown, `d` a done flag, `e` base64, `f` the application name, `u` the urgency (`0` low, `1` normal, `2` critical) and `n` an icon name. Refused for the one reason the plain spellings are: **a notification leaves the window** (part 6, §54, §79, §89, `term/notify.rs`) |
| iTerm 1337 File | Inline images | 🛑 | `OSC 1337 ; File=<args>:<base64>` draws an inline image from a base64 payload (part 6, §70, `term/iterm.rs`) |
| iTerm 1337 `SetMark` | Explicit bookmark on a line | ✅ | `OSC 1337 ; SetMark` bookmarks the cursor's line; amber gutter tick, walked with Ctrl+Shift+Up/Down, and able to mark mid-output where §34's prompt-derived marks cannot (§55, `term/iterm.rs`) |
| iTerm 1337 `CurrentDir` | Working directory | ✅ | `OSC 1337 ; CurrentDir=<path>` — iTerm's working-directory spelling, the third one (§55, `term/cwd.rs`) |
| iTerm 1337 `SetUserVar=gitBranch` | Per-session variable | ✅ | `OSC 1337 ; SetUserVar=<name>=<base64>` sets a per-session variable; the one honoured name is drawn as a pill beside the endpoint, UTF-8 checked, control characters stripped, capped at 32 characters (§55, `term/iterm.rs`) |
| iTerm 1337 `SetUserVar` (any other name) | Per-session variable | 🛑 | the same key under any other variable name — with no title template there is no reader for a second one (§55, `term/iterm.rs`) |
| iTerm 1337 `Copy` | Clipboard write | 🛑 | `OSC 1337 ; Copy=:<base64>` writes the local clipboard — OSC 52's write by another name (part 6, §55) |
| iTerm 1337 `SetProfile` / `SetColors` | Theme repaint | 🛑 | `SetProfile=<name>` switches the whole iTerm profile and `SetColors=<key>=<value>` sets one colour by role — `fg`, `bg`, `bold`, `link` — as RGB, RRGGBB or a preset name. The fixed scheme in another costume (part 6, §55, §89) |
| iTerm 1337 `SetBackgroundImageFile` | Background image | 🛑 | `SetBackgroundImageFile=<base64>` names a local image file to draw behind the grid, an empty value removing it (part 6, §41, §55, §89) |
| iTerm 1337 `StealFocus` / `RequestAttention` | Raise / flash the window | 🛑 | `StealFocus` brings the window to the foreground; `RequestAttention=<yes\|once\|no\|fireworks>` flashes it for attention, for as long as the value says. Effects that leave the tab (part 6, §54, §55, §89) |
| iTerm 1337 `ClearScrollback` | Drop the scrollback | 🛑 | drops the scrollback; `CSI 3 J` is the sanctioned spelling (§55) |
| iTerm 1337 `CursorShape` | Cursor shape | 🛑 | `OSC 1337 ; CursorShape=0/1/2` — a fourth spelling of the one cursor-shape field, and the only one that would reach it from outside the engine (part 6, §71, `term/iterm.rs`) |
| iTerm 1337 `ReportCellSize` | Cell size — query | 🛑 | `OSC 1337 ; ReportCellSize` asks for the cell's height and width, and a scale factor beside them; what asks it is sizing an inline image (part 6, §71, §89, `term/iterm.rs`) |
| iTerm 1337 (every other key) | — | 🛑 | the rest of iTerm's `OSC 1337` namespace, which `term/iterm.rs` meets as an allow-list (§55) |
| 777 (`notify`) | urxvt notification | 🛑 | `OSC 777 ; notify ; title ; body` — the same refusal in urxvt's spelling. OSC 777 is a **dispatcher** (`777;<module>;…`) and only the `notify` module is this feature; another module is unimplemented rather than declined, a different question with a different mark. The attribution is folklore, not a citation: urxvt's own manual page documents no OSC 777 at all (part 6, §54, §79, §89, `term/notify.rs`) |

### CSI — cursor movement & editing

| Code | Feature | Status | Note |
|---|---|---|---|
| A / B / C / D | Cursor up / down / fwd / back | ✅ | CUU / CUD / CUF / CUB move the cursor up, down, forward and back by `Ps`, stopping at the page edge. The two vertical ones drift downward under origin mode with a region below row 1, an engine defect (§74) |
| E / F | Cursor next / prev line | ✅ | CNL / CPL move the cursor `Ps` lines down or up and to column 1 |
| G / H (+ f) | Absolute position | ✅ | CHA sets the column; CUP and HVP set row and column together, `H` and `f` alike. `G` carries the origin-mode drift the row above names (§74) |
| I / Z | Forward / backward tab | ✅ | CHT / CBT move the cursor forward or back `Ps` tab stops, over the stops `ESC H` and `CSI g` maintain — eight columns apart at power-on. Column only, under every mode |
| d / \` | Vertical / horizontal PA | ✅ | VPA sets the row and HPA the column, each leaving the other alone. `` ` `` shares CHA's arm and so its origin-mode drift (§74) |
| a / e | Horizontal / vertical PR | ✅ | HPR / VPR move `Ps` columns right or rows down; the parser aliases them to CUF and CUD, so `e` inherits CUD's origin-mode drift (§74) |
| s / u | Save / restore cursor | ✅ | SCOSC / SCORC, the ANSI.SYS save and restore of the cursor. A parametrised `CSI s` is DECSLRM **only while mode 69 is set** and is a save-cursor otherwise, which is the rule a real terminal uses and the one §102 gave back — §57 had to cancel every parametrised `s` on the parameter count alone. The deferred wrap rides along with the position (§57, §102) |
| @ / P / X | Insert / delete / erase char | ✅ | ICH inserts blanks at the cursor, DCH deletes characters and pulls the line left, ECH erases in place without moving the tail |
| L / M | Insert / delete line | ✅ | IL / DL insert or delete `Ps` lines at the cursor, scrolling the rest of the region |
| J | Erase in display | ✅ | ED erases from the cursor to the end of the screen, to its start, or the whole screen. `Ps` is read as a NUMBER on cmote's side of it too, so an inline picture goes when `CSI 002 J` or `CSI 2;5 J` takes the text it sat beside and not only when `CSI 2 J` does (§106, `term/graphics.rs`) |
| 3 J | Erase scrollback | ✅ | xterm's extension to ED: drop the scrollback |
| K | Erase in line | ✅ | EL erases to the end of the line, to its start, or the whole line |
| Ps " q | Character protection (DECSCA) | ✅ | DECSCA marks the cells written after it protected or erasable, which decides whether a selective erase takes them (§56, `term/protect.rs`) |
| ? J / ? K | Selective erase (DECSED / DECSEL) | ✅ | DECSED / DECSEL are the selective erase — ED's and EL's three extents, sparing protected cells, where a plain `CSI J` / `CSI K` still takes them (§56) |
| ! p (DECSTR) | Soft reset | ✅ | the soft reset: pen, modes, charsets, scrolling region and saved cursor back to their power-on values without clearing the screen. DEC's published list is eighteen items and cmote sends the eleven anything here models — DECTCEM, IRM, DECOM, DECAWM, DECNKM, DECCKM, DECSTBM, the charsets, SGR, DECSCA and DECSC; the other seven (KAM, DECNRCM, DECAUPSS, DECSASD, DECKPM, DECRLM, DECPCTERM) name state neither `vte` nor the engine nor cmote has. **One deliberate departure**: DEC's list says "Autowrap (DECAWM): No autowrap" and cmote leaves it **on**, because `xterm-256color` declares `am` and its `rs2` sends no `\E[?7h` after this (§72, §94) |
| b (REP) | Repeat character | ✅ | repeats the preceding graphic character `Ps` times — the engine prints that many more copies of it (§84) |
| S / T | Scroll up / down | ✅ | SU / SD scroll the region up or down `Ps` lines, the cursor staying where it is |
| Ps SP @ / Ps SP A | Scroll left / right (SL / SR) | ✅ | ECMA-48's horizontal twins of SU / SD — xterm writes them "shift left / right `Ps` column(s)". Every row of the **visible page** moves sideways, the edge the content left goes blank in the pen's background, and the cursor does not move: the data slides under it. Whole cells travel, so colours, attributes, the OSC 8 link and DECSCA protection come along, as they do under DECCRA. An omitted or `0` count is one column and a count past the width blanks the page. A wide glyph with only one half left on the page is blanked rather than drawn as a dangling lead or continuation. **Bounded by the scrolling region** since §102: a shift is a scrolling operation, so a status line parked outside the band stays where it is while the band slides under it. That bound is what §100 wanted and could not have — the region is private inside the engine, so the shift was REFUSED under origin mode instead, DECOM standing in as evidence that a region existed. The proxy was both too much (a program with origin mode and no region got nothing) and too little (a region set without origin mode was shifted straight through); the mirror retired it (§58, §100, §102, `term/rect.rs`, `term/region.rs`) |
| Ps + T | Scroll down, filling from scrollback (UNSCROLL) | ✅ | **kitty's**, not contour's — contour's own definition credits it (`"Scroll Down with Scrollback Fill (kitty unscroll)"`), which §98 recorded the wrong way round. SD with the top filled from the **scrollback** instead of with blanks, for the shell that prints completions under the cursor and scrolls the user's text away: plain SD would blank exactly what this exists to restore. The lines are **moved**, not copied — a copy would leave the same text in the scrollback and on the page, once per completion, for the life of the session. `Ps` defaults to 1 and clamps to the page; the rows pushed off the bottom are discarded; the cursor does not move. Where the scrollback cannot fill the request the remainder is blank, which is kitty's own rule and what makes the **alternate screen** correct with no special case — that page keeps no history. The one operation here that changes how many lines the document has, so every absolute anchor cmote holds — prompt marks, bookmarks, command spans, picture anchors, right-to-left flags — is renumbered with it (§101, `term/rect.rs`) |
| r (DECSTBM) | Scrolling region (top / bottom) | ✅ | sets the scrolling region's top and bottom lines and homes the cursor; every operation that scrolls honours it. The horizontal twin is DECSLRM below |
| s (DECSLRM) | Left / right margins | ✅ | sets the left and right margins, the horizontal half of DECSTBM — and everything that follows is a consequence rather than a separate feature: a line breaks at the **right margin** and goes on at the **left** one, a carriage return goes to the left margin, ICH and DCH stop at the margins, and every scroll (SU, SD, IL, DL, IND, RI) moves only the band of columns between them while the rest of the page stands. Under **origin mode** the columns a program names are counted from the left margin, the relationship DECOM already had with the region's rows. **The mode is the whole rule**: this byte is DECSLRM only while `? 69` below is set and is a save-cursor otherwise, which is how a real terminal reads it and what §57 could not do — it cancelled every parametrised `s` on the parameter count alone, because the engine refused the mode that settles it. A row pushed out of a narrowed band is **discarded, never scrollbacked**: the history holds whole lines and that row is a slice of one. Built on the `Handler` gate part 5 costed and refused twice (§57, §73, §102, `term/margins.rs`, `term/gate.rs`, `term/gatediff.rs`) |
| g | Clear tab stop (TBC) | ✅ | TBC clears the tab stop under the cursor (`0`) or all of them (`3`) — the two DEC defined for a one-page terminal (§67) |
| ? 5 W | Tab stops every 8 columns (DECST8C) | ✅ | DECST8C puts tab stops back every eight columns; performed by walking the page with CR and CUF and setting each stop with HTS, so the engine's table stays its own (§74, `term/tabs.rs`) |
| Ps ; Ps SP k | Select character path (SCP) — data to presentation | ✅ | SCP picks the character path for the line under the cursor — `Ps1` `2` is right to left — and `Ps2 = 1` says the presentation is derived from the data, so the row is mirrored as it is drawn while the grid, scrollback, search, selection and copy stay in data order. Pictures are placed by column and not mirrored (§76, `term/scp.rs`) |
| Ps ; 2 SP k | Select character path (SCP) — presentation to data | 🛑 | the other update mode of the same sequence: `Ps2 = 2` asks the terminal to write the drawing back into the data component — over the only copy of what the host sent (§76, `term/scp.rs`) |
| $ z (DECERA) | Erase rectangular area | ✅ | erases the rectangle given by top, left, bottom and right; corners default to the page edges, an end past the edge clamps, and a backwards rectangle is a no-op (§58, `term/rect.rs`) |
| $ { (DECSERA) | Selective erase rectangular area | ✅ | the same rectangle by the selective verb: protected cells stand (§58) |
| $ x (DECFRA) | Fill rectangular area | ✅ | fills a rectangle with one character stamped from the pen, so it carries the colours and attributes a printed glyph would. DEC's own range: "`Pch` can be any value from 32 to 126 or from 160 to 255. If `Pch` is not in this range, then the terminal ignores the DECFRA command" — which is what cmote does, dropping the whole sequence (§58, §84, §94) |
| $ v (DECCRA) | Copy rectangular area | ✅ | copies a rectangle to another origin, whole cells — colour, attributes, the OSC 8 link and DECSCA protection travel with the glyph, and the overlapping case is read out whole first. The two page parameters are ignored (§58) |
| Ps * x (DECSACE) | Attribute change extent | ✅ | picks the shape the two requests below act on: `0` (**DEC's stated default**) and `1` the wrapped stream between the corners, `2` the rectangle. RIS resets it and DECSTR does not — DEC's soft-reset list does not name it, and §72 honours that list rather than widening it. Note the intermediate — `* x` is this, `$ x` is DECFRA (§59, §84, §94) |
| $ r / $ t (DECCARA / DECRARA) | Change / reverse attributes in a rectangle | ✅ | set or flip attributes over a rectangle, from a DEC-defined selector list — `$ r` takes `0 1 4 5 7 22 24 25 27`, `$ t` only `0 1 4 5 7`. Attributes alone: never a colour, never a glyph. Blink is parsed and dropped, the engine having no bit for it (§59) |
| Pid;Pp;Pt;Pl;Pb;Pr * y (DECRQCRA) | Rectangle checksum | ✅ | checksums a rectangle and answers `DCS Pid ! ~ XXXX ST` — xterm's `xtermCheckRect` at its DEC-compatible default, clamped to the visible page so the scrollback cannot be read through it. The page number is ignored (§60, `term/rect.rs`) |
| Ps # y (XTCHECKSUM) | Select checksum extension | 🤷 | "the bits of `Ps` modify the calculation of the checksum returned by DECRQCRA": `0` do not negate the result, `1` do not report the VT100 video attributes, `2` do not omit the checksum for blanks, `3` omit it for cells never initialised, `4` do not mask the cell value to 8 bits. cmote answers **one** calculation, the DEC-compatible default the row above describes. Decided in §99 rather than left open: bit `3` is one cmote *cannot* perform — the engine's grid starts full of blanks indistinguishable from written cells, §60's own disclosed divergence — and honouring four of five would hand a program that set the fifth a number computed under rules it did not choose. Dies in the parser, `csi_dispatch` matching no `#` intermediate at all, and pinned at the boundary by the checksum being the same number before and after the request (§60, §88, §94, §99) |
| Ps ' } / Ps ' ~ (DECIC / DECDC) | Insert / delete column | ✅ | the column twins of IL / DL: open `Ps` blank columns at the cursor's column and push the rest of every line right, or delete `Ps` and pull the tail left — across **every row of the scrolling region**, which is the whole difference from ICH and DCH. Bounded by the left and right margins, and **legal without them**, the band then being the page; a column pushed past the right margin is gone and the ones that arrive carry the pen's background. Refused when the cursor sits outside either band, xterm's test and the one IL and DL apply. `vte` has no arm for the apostrophe intermediate at all, so unlike SL, SR and UNSCROLL these have no engine behaviour to be mistaken for — only each other (§58, §98, §100, §102, `term/rect.rs`) |
| Ps SP q (DECSCUSR) — shape | Cursor style | ✅ | picks the cursor's shape: block, underline or bar, `0` the default; `7`+ is undefined (§84) |
| Ps SP q (DECSCUSR) — blink | Cursor style | 🛑 | the odd values of the same parameter ask for a blinking cursor; cmote runs no animation timer and its seam carries no blink, so the cursor is steady (§65, `term/screen.rs`, §84) |
| 5n | Device status report | ✅ | DSR 5 asks whether the terminal is healthy; answered `CSI 0 n`, ok |
| 6n | Cursor position report | ✅ | DSR 6 (CPR) asks where the cursor is; answered `CSI <row> ; <col> R`, one-based, from the live cursor |
| ? 6 n | Extended cursor position (DECXCPR) | ✅ | DECXCPR, the DEC-private spelling of the same question; answered `CSI ? <row> ; <col> R`, one-based and with **no page parameter**, which is xterm's form and the reply cmote sends. Answered where the question sits in the stream, from the cursor the ANSI spelling reports — so both report the absolute row, ignoring origin mode (§74, §82, `term/dsr.rs`) |
| ? 15 / 25 / 26 / 75 / 85 n | Printer / UDK lock / keyboard status / data integrity / multi-session | 🛑 | the DEC-private status reports: a printer (`CSI ? 10/11 n`, ready or not), a user-defined-key store's lock (`? 20/21 n`), the keyboard (`CSI ? 27 ; Pl ; … n`, whose first parameter is its **language** — `1` North American), data integrity (`? 70 n`) and a multi-session controller (`? 83 n`). Equipment cmote does not have, and `26` would name the user's machine (part 6, §36, §82, `term/dsr.rs`, §84) |
| ? 55 / 56 n | Locator status / type | ✅ | ask whether a DEC locator is present and what type it is. Answered with xterm's own negatives — `CSI ? 53 n` "no locator" and `CSI ? 57 ; 0 n` "cannot identify". They are the two members of the DSR family that are answered rather than refused, because they **advertise nothing**: a reply that states an absence is the one a terminal without the equipment can make truthfully, and silence would leave the sender waiting out a timeout to learn the same thing. This note read "which is the whole extent: the DEC locator protocol itself is not implemented and has no row" until §140, which gave the protocol its two rows below and made DECRQLP a third door onto the same absence (§82, §84, §93, §140, `term/dsr.rs`) |
| ? 996 n | Colour scheme — dark or light | ✅ | asks whether the terminal's scheme is dark or light; answered `CSI ? 997 ; 1 n`, dark, from a constant — cmote's scheme is fixed (part 6) and its background is `#1e1e1e`. **The one reply cmote sends in a sequence xterm does not define**, which narrows §96's half-rule rather than excepting it: a reply must not name the program or the machine (§36), and this names neither. Nor is it a new disclosure — `OSC 11 ; ?` is xterm's own spelling of the same fact and cmote answers it, so this is one writer with two doors (§71). Implemented by contour, ghostty, kitty and GNOME's vte; asked by neovim, helix, zellij and tmux, each of which paints for a guessed background when nobody answers (§93, §98, `term/dsr.rs`) |
| ' \| (DECRQLP) | Locator position request | ✅ | asks where the locator is, once. Answered `CSI 0 & w` — DECLRP with **event code 0**, which ctlseqs defines as "locator unavailable - no other parameters sent" and which therefore carries no position, no button mask and no page. That is the protocol's **own word** for the fact the two rows above state in DSR's spelling, so all **three doors** onto "there is no locator" answer alike where two of them did before. It passes §93's test by the same margin the other two do: a reply stating an **absence** advertises nothing, and silence would leave the sender waiting out its own timeout to learn it. Unconditional, because cmote has no locator under any mode — the answer cannot depend on DECELR, so there is no state here to get wrong (§93, §140, `term/locator.rs`) |
| ' z / ' { / ' w (DECELR / DECSLE / DECEFR) | DEC locator — the settings | 🛑 | DECELR arms the locator and picks the unit its coordinates are counted in, DECSLE picks which button transitions send a report, DECEFR sets the filter rectangle DECSLE `0` cancels. **Read and deliberately inert** since §140: a terminal with no locator has nothing to arm, no coordinate to count and no transition to select, so recording what they said would build state whose every reader is unreachable. The refusal is the interesting half, because cmote **has** a mouse and reports it in xterm's spelling (modes 1000–1006) — so "no locator" is a choice about protocols, not a fact about hardware. xterm holds **one** `send_mouse_pos`, in which DECELR and `CSI ? 1000 h` overwrite each other and the two protocols can never both be live; in cmote the xterm modes are the **engine's** and DECELR would be cmote's, and neither can clear the other without becoming a second writer of engine state (part 6, §71, §73). A locator built here could therefore run at the same time as mode 1006, and a program that set both would read two protocols' reports interleaved where xterm sends one — a divergence in the byte stream it READS, which is worse than an absence it can detect with the row above (§93, §140, `term/locator.rs`) |
| ? 62 n | Macro space (DECMSR) | 🛑 | DECMSR reports the space left in the terminal's macro store, answering `CSI Pn * {` (part 6, §82, §84) |
| ? 63 n | Memory checksum (DECCKSR) | 🛑 | DECCKSR checksums the terminal's own memory — macros and user-defined keys — and answers in the `DCS Pt ! ~ xxxx ST` envelope DECRQCRA uses for a rectangle of the screen (part 6, §82, §84) |
| c / > c | Primary / secondary DA | ✅ | DA1 and DA2 — what the terminal is and what firmware it claims; cmote amends the engine's DA1 with attribute **4**, sixels (§41, `term/query.rs`) |
| = c | Tertiary DA | ✅ | DA3 asks for the terminal's unit id; answered with the constant `00434D45` and never a machine-derived one (§36, `term/query.rs`) |
| ? Pi ; 1 / 2 / 4 S | Graphics attributes — read / reset (XTSMGRAPHICS) | ✅ | `CSI ? Pi ; Pa ; Pv S` asks about a graphics limit: `Pi` is the item — colour registers (`1`) or sixel geometry (`2`) — and `Pa` the action, `1` read, `2` reset, `4` read the maximum. All three answer `status 0` with the decoder's real limit, a reset landing on that same fixed number; ReGIS (`Pi = 3`) is answered unknown item (§41, `term/query.rs`, §84) |
| ? Pi ; 3 ; Pv S | Graphics attributes — set | 🛑 | the set action of the same sequence, `Pa = 3`, asking to move a limit to `Pv`; answered `status 3`, failure, with the value unchanged (§41, `term/query.rs`, §84) |
| Ps $ p / ? Ps $ p | Request mode (DECRQM) | ✅ | DECRQM asks whether a mode is set; answered `CSI Ps ; Pm $ y` in the ANSI spelling as well as the private one, `Pm` being `0` not recognised, `1` set, `2` reset, `3` permanently set, `4` permanently reset (§60, §94) |
| # P / # Q / # R | Colour palette stack (XTPUSHCOLORS / XTPOPCOLORS / XTREPORTCOLORS) | 🤷 | save the dynamic and ANSI-palette colours onto a stack, pop them back, and — the third, added in §98 — report how many are on it, answered `CSI Pn # Q`. Over a palette that is never read, so the depth to report is always the depth of a stack nothing pushes to (part 6, §84, §98) |
| ? Pm s / ? Pm r (XTSAVE / XTRESTORE) | Save / restore private modes | ✅ | `CSI ? Pm s` remembers the current setting of each named DEC-private mode and `CSI ? Pm r` puts them back — xterm's, and the pair `term/cancel.rs` has always tested it does *not* mistake for DECSLRM. The risk this row named is closed: a program that saves `? 25`, hides the cursor and restores gets its cursor back. **A save READS and a restore FEEDS** — the value comes from `Screen::private_mode` over the engine's own mode word, and goes back as the `CSI ? Ps h` / `l` the engine already implements, so the engine stays the only writer of its modes (§71, §73) and the store is a record of what a mode WAS, never an answer to what one IS. Covers every mode the terminal models — `1`, `6`, `7`, `12`, `25`, `1000`–`1007`, `1042`, `1049`, `2004`, and `69`, which is cmote's own. **Modes with no state to read are not saved and not restored**: `3` (DECCOLM) erases the page instead of leaving a bit, `2026` lives inside the parser, and anything the engine never heard of is `Unknown` to it — so a restore of one does nothing rather than guessing. A mode never saved is likewise not restored, which is a deliberate divergence from xterm's zeroed array: guessing "reset" would let an unpaired restore turn the cursor off, the same stuck state from the other direction. This row read ❌ until §141 on the grounds that a saved copy makes cmote "a second source" for the modes — see part 6 for why that was too strong (§57, §98, §141, `term/savemodes.rs`) |
| > Ps ; Pn b (SBQUERY) | Query semantic command blocks | 🤷 | contour's: with DEC mode 2034 set, the terminal returns **the command line, the prompt, the output and the exit code** of recent blocks as JSON in a `DCS > 1 b … ST`, keyed by a four-word token it hands out when the mode is enabled. cmote records exactly this (§34) and would be the rare terminal that could answer — which is why it is worth stating that it will not. It is a **reply** in a dialect cmote does not claim (§78, §96), and it hands a host the output of commands it did not run. That contour gates its own feature behind a token is the vendor agreeing about the danger, not disposing of it (part 6, §12, §98) |
| > Pl ; Pr t (XTCAPTURE) | Report the screen buffer | 🤷 | contour's buffer capture: `Pl` picks logical or visual lines, `Pr` how many, and the terminal sends them back as UTF-8 text in a run of `PM 314 ; … ST` strings. The line §60 drew for DECRQCRA is exactly this one — the checksum is allowed because it is clamped to the **visible page**, and a capture's whole purpose is to reach past it into the scrollback, which in an SSH client can hold the output of a session that ended before this one began (part 6, §12, §60, §98) |
| > Ps s (XTSHIFTESCAPE) | Who gets shift-click | 🤷 | xterm's: lets the application ask for shift-modified mouse events instead of the terminal keeping them. Shift is the user's override — the way to select text while a program holds the mouse — and a remote does not get to take it (part 6, §10, §98) |
| > Ps M (SETMARK) | Bookmark the cursor's line | ✅ | contour's CSI spelling of what iTerm's `OSC 1337 ; SetMark` does, which cmote already shipped (§55) — so this is a second door to one instruction, and it lands in `term/iterm.rs` beside the meaning rather than in a module of its own. All three parts of the sequence are matched together, and here that is not pedantry: `CSI Ps M` with no private marker is **DL**, delete lines, which the engine implements and every full-screen program uses — a tick in the gutter on every deleted line is what the marker test prevents (§56). `Ps` is contour's mark KIND and is not read: cmote has one kind, and `SetMark` carries no parameter at all. This was "a gap and a cheap one … left open because a scanner is real work"; §111 made a scanner one line (§55, §98, §111, §112, `term/iterm.rs`) |
| Ps , ~ (DECPS) | Play a sound | 🤷 | DEC's tone generator — "controls the sound frequency or notes", the parameters unread here beyond that. BEL's refusal with a keyboard attached: sound leaves the tab, and where the bell is one ring this is a remote holding the speaker for as long as its parameters say (part 6, §63, §98) |
| Ps * z (DECINVM) | Invoke a macro | 🤷 | runs a macro previously defined by `DCS … ! z` (DECDMAC) — a stored sequence a remote plays back by number. cmote refuses to report a macro store (DECMSR, DECCKSR) and has none; nothing that arrives over the wire becomes a stored program here (part 6, §82, §98) |
| Ps $ \| / Ps * \| (DECSCPP / DECSNLS) | Columns / lines per page | 🤷 | set the page to 80 or 132 columns, or to a given number of lines — DECCOLM's argument in two more spellings, and a remote resizing the window the user sized. Their **report** halves are answered (§152): a refused set costs a resize that did not happen, which leaves every later report agreeing with what the program sees (part 6, §65, §98, §152) |
| Ps $ } / Ps $ ~ (DECSASD / DECSSDT) | Status display | 🤷 | split a status line off the page (`$ ~` picks its type) and direct output into it (`$ }`). A second writable surface with its own cursor, which is neither in the engine nor in cmote's grid model; DECSTR's published list names DECSASD, and §72 sends nothing for it for this reason (§72, §94, §98) |
| Ps U / Ps V / Ps SP P / Ps SP Q / Ps SP R | Page positioning (NP / PP / PPA / PPR / PPB) | 🤷 | move to the next page, the previous one, or an absolute / relative / backward page. cmote is a **one-page** terminal — DECRQCRA's page parameter is ignored for the same reason (§60) — so there is nowhere to go. The intermediates are as ECMA-48 defines them; the index this row came from omits them, and ECMA-48 itself is still unread here (§98) |
| Ps p (DECSSCLS) | Scroll speed | 🤷 | "set scroll speed", in contour's index; what its values mean was not read. cmote scrolls in one step and runs no animation timer — the same absence that keeps the cursor from blinking — so there is no speed here to set (§65, §98) |
| Ps " p (DECSCL) | Conformance level | 🤷 | sets which VT level the terminal parses as — VT100, VT220 and up — and on xterm performs a **hard reset** along the way. cmote parses one dialect and says so in `TERM`, XTVERSION and XTGETTCAP alike; a remote switching it would be choosing how the user's screen is read, with a screen clear thrown in (§78, §96, §98) |
| Ps " v (DECRQDE) | Report the displayed extent | ✅ | asks how much of the page is visible and where; answered `CSI Ph ; Pw ; Pml ; Pmt ; Pmp " w` since §144, and **the reply's parameter list has now been read** — lines, columns, the left-most column displayed, the top line displayed, the page. The question is about a page LARGER than the screen (a VT420 held up to 144 lines and showed 24), so on a terminal whose page IS its screen the last three are 1 and the first two are the grid. **`Pmt` stays 1 when the user has scrolled back, and that is the decision in this row**: the scrollback is history the engine keeps below the page, not part of it (§60), and where the viewport is parked is a fact about the person at the keyboard that changes under a wheel the remote cannot see — §36's rule, the one that keeps `CSI ? 26 n` refused. Answered after the chunk with the other identity queries rather than from an interruption, because a geometry does not move while a chunk is read: only a resize moves it, and a resize arrives from the window and not from the wire. DEC writes the request with no parameter, so a value it never defined on that final byte is a different sequence and goes unanswered — and all three parts are matched together, `CSI ... $ v` being DECCRA and `CSI " q` / `CSI " p` being DECSCA and DECSCL (§36, §98, §144, `term/query.rs`) |
| Ps $ w (DECRQPSR) | Report presentation state | ✅ | asks for the cursor information report (DECCIR — position, pen, charsets, flags) or the tab-stop report (DECTABSR), each answered in a `DCS … $ u ST` envelope. Both answered since §143. DECCIR reports the cursor DECXCPR reads — one-based and **absolute**, ignoring origin mode, which is §74's convention for cmote's other cursor report, with DECOM carried in its own flag byte so a reader can still work out the origin-relative row. Its ten fields come from the pen (bold, underline, reverse — **blink is always 0**, the engine having no bit for it, §59), the protection bit §56 borrows, origin mode, a wrap owed read from **both** holders (§102's margins own it when the band is narrowed), and the character-set state §143 built — which is why that state and this report arrived together, GR being written by three sequences and read by nothing else in the program. DECTABSR reads a **mirror** of the engine's private tab table, kept for the same reason `term/region.rs` mirrors the scrolling region and written at all four points the engine is told (HTS, TBC, RIS, resize); a page with every stop cleared answers an EMPTY data string rather than nothing, because the program asked. **The question this row left open is settled**: there is no "I do not report that" form in this family the way there is in DECRQSS — DEC defines `Ps = 0` as "Error. Request ignored" — so an unrecognised request is answered with silence, and the silence is the standard's (§66, §98, §143, `term/presentation.rs`) |
| # { / # } (and `# p` / `# q`) | Video-attribute stack (XTPUSHSGR / XTPOPSGR) | ✅ | push the current video attributes onto a stack and pop them back; `Pm` names which to save in SGR's own numbering, with `30` / `31` for the two colours, and no parameter at all saves them all. Ten levels, as xterm has it — an eleventh push is dropped, and so is the pop that matches it, so the levels below stay paired. cmote's own scanner (`term/sgrstack.rs`, §85), which reads the pen where the push sat and restores it by feeding the engine that pen spelled in SGR — never by writing the template itself (§71, §73). Underline substyles and the underline colour ride the round trip, which the DECRQSS reply cannot report; blink does not, the engine having no flag for it, and the OSC 8 link is not an attribute and does not travel. **RIS empties the stack** — DECSTR does not, on the same split DECSACE has, and neither does the alternate-screen swap (§86) |

### ESC — single sequences

| Code | Feature | Status | Note |
|---|---|---|---|
| ESC D / ESC M | Index / Reverse index | ✅ | IND moves the cursor down one line and RI up one, each scrolling the region at its edge |
| ESC E | Next line | ✅ | NEL moves down one line and to column 1, scrolling at the region's foot |
| ESC H | Set tab stop | ✅ | HTS sets a tab stop at the cursor's column — the sequence §74's tab-stop reset is built out of |
| ESC 7 / ESC 8 | Save / restore cursor | ✅ | DECSC / DECRC save and restore the cursor with its pen, charsets and origin mode |
| ESC c (RIS) | Full reset | ✅ | the hard reset: every setting back to power-on, the screen cleared |
| ESC = / ESC > | Keypad application / numeric | ✅ | DECKPAM / DECKPNM put the numeric keypad in application or numeric mode; tracked on the seam and encoded for the keys with no NumLock meaning to lose — Enter and `* + , - / =` (part 2, §36) |
| ESC = — the numpad digits | Keypad application mode | 🛑 | the digits' half of application keypad mode, which would send `SS3 p`–`SS3 y` in place of NumLock's own output — the user's switch, not a remote's (part 2, §36, `term/keymap.rs`) |
| ESC 6 / ESC 9 (DECBI / DECFI) | Back / forward index | ✅ | the horizontal twins of RI and IND: one column back or forward, and AT the margin the band slides sideways under the cursor instead — DEC's own words, "if the cursor is at the left margin, all screen data within the margins moves one column to the right". The margin meant is the left/right one, so with a band set the columns outside it stand still, and every row of the scrolling region moves (§102's wall, since a sideways scroll is a scrolling operation). The column pushed past the far margin is gone and the one that arrives carries the pen's background. The cursor half is asked of the ENGINE in its own spelling — CUB and CUF, which the gate already bounds by the margins — so cmote does not become a second writer of the cursor for the sake of two bytes (§71). The scroll half is `shift_band_columns_at`, the same band arithmetic DECIC and DECDC use, at the left margin rather than at the cursor. What had kept these unbuilt was the SCANNER: `vte` dispatches `ESC 6` and `ESC 9` to nothing at all (`ansi.rs:1799-1821`, they fall to `unhandled!`), so they needed an escape-sequence machine of their own — until §111 gave the directory one and this cost two match arms (§98, §100, §102, §111, §112, `term/rect.rs`) |
| ESC #8 (DECALN) | Screen alignment test | ✅ | fills the screen with `E` and homes the cursor — the alignment test |
| ESC ( / ) / * / + — `B` and `0` | Designate ASCII / DEC line drawing | ✅ | SCS designates a 94-character set into G0–G3, one intermediate per slot; `B` is ASCII and `0` DEC line drawing. The two `vte` dispatches, and since §143 the gate records them in cmote's own table instead of forwarding them — forwarding as well would have been the second writer §71 and §73 refuse. The designations ride the saved cursor (DECSC / DECRC) and a bank per screen, which is the engine's own arrangement kept rather than a new one: its slots live on the grid cursor and swap with the grid, so a program that left G1 on line drawing has never handed the shell a screen of box corners. "G2 and G3 can be designated and nothing here can invoke them" was true until §143 (§143, `term/charset.rs`) |
| ESC ( / ) / * / + — any other final | Designate another 94-charset | ✅ | the other 94-character sets. Twelve national replacement sets and JIS-Roman since §143 — UK, Dutch, Finnish, French, French Canadian, German, Italian, Norwegian/Danish, Portuguese, Spanish, Swedish, Swiss — each a handful of substitutions inside ASCII, from xterm's ctlseqs for the final bytes and DEC's own position tables for the glyphs, cross-checked against xterm's `charsets.h` where the two disagreed. `vte` sends every final but `B` and `0` to `unhandled!()`, so cmote took the whole mechanism over: the engine's four slots stay ASCII for the life of the session and every substitution is made in `Gate::input`, which is ONE writer rather than two (§71, §73). The line-drawing set is not re-implemented — `Charset::map` calls the engine's own table for it. **The big sets are refused, not pending**: DEC Supplemental (`<`, `%5`), DEC Technical (`>`), DEC Greek / Hebrew / Turkish / Cyrillic and JIS-Katakana are each a full 94-glyph table read from no primary source here, and an unrecognised final leaves the slot as it was — which is what a terminal without the set does anyway (§65, §143, `term/charset.rs`) |
| ESC N / ESC O | Single shift G2 / G3 | ✅ | SS2 / SS3 invoke G2 or G3 for the next character only, and since §143 they do. Consumed by ANY character including a zero-width combining one — DEC defines the shift as lasting for the next graphic character and a combining mark is one, so a shift that survived a character would be a shift that survived the wrong one. Reported in DECCIR's `Sflag`, which has a bit for each, so the two have to be told apart rather than counted (§65, §143, `term/charset.rs`) |
| SI / SO (LS0 / LS1) | Locking shift G0 / G1 | ✅ | LS0 and LS1 lock G0 or G1 into GL — the two spellings anything in practice uses, and the two `vte` dispatches. Since §143 they write cmote's own charset state rather than the engine's, through the GATE: the rule the directory keeps is that the gate takes what `vte` dispatches and a scanner takes what it drops, and it is not tidiness here — the soft reset is SYNTHESISED and fed through a path that runs no scanner, so `\E(B\E)B\E*B\E+B\017` in `SOFT_RESET` would reset nothing if the gate stopped listening (§72, §143) |
| LS2 / LS3 / LS1R / LS2R / LS3R | The other locking shifts | ✅ | the other locking shifts, which lock G1–G3 into GL or GR. The row's complaint is closed: G2 and G3 are invocable since §143, by LS2 / LS3 and by the single shifts above. **The two halves are not equally live, and the split is worth stating.** LS2 and LS3 lock into GL and change what is drawn. LS1R, LS2R and LS3R lock into GR, and **nothing can ever land in GR**: cmote decodes UTF-8 always (§67), so what reaches `Gate::input` is a `char` and not a byte in a half — a stream that meant to use the right half writes a byte past 0x7f and `vte` reads it as UTF-8. They are implemented anyway because DECCIR names the slot in GR as one of its ten fields, and a terminal that answered that field from a constant while ignoring the three sequences that set it would be reporting a state it had refused to keep. That is also why GR's power-on value is G1 rather than G0: there is no LS0R, so G1 is the lowest slot GR can ever name (§65, §143, `term/charset.rs`) |
| ESC #3–6 | Double-height / width lines | ✅ | DECDHL, DECDWL and DECSWL — a line drawn double height (top and bottom halves), double width, or back to single. Answered since §146, as a **rendering rule** and not a write to the grid: the attribute is held beside the engine, keyed by absolute document line, and the scrollback, the search, the selection and a copy all go on reading the text the host sent — §76's arrangement for the character path, applied to the other axis (§71, §73). **DECDHL is exact**: double height and double width are both axes by the same factor, which is a UNIFORM scale of two, so the glyphs are drawn at twice the font size with twice the line height and the row's own clip takes the half that belongs in it. **DECDWL is the one divergence, and it is measured**: double width with SINGLE height is an anisotropic scale, and iced 0.14 has none for text — `Transformation::scale` takes one factor and the wgpu pipeline reduces whatever is in force to a single `scale` before glyphon sees it (`iced_wgpu/src/text.rs:625-626`). So a double-width line's CELLS are twice as wide and its glyphs are not, one cell drawn per box. That was chosen over the two alternatives rather than fallen into: drawing at 2× and clipping to one row cuts every glyph in half, and ignoring the sequence puts a title at the full width the program did not ask for, which breaks the LAYOUT — the thing programs actually depend on here. Only the columns a double line SHOWS are planned, which is also what keeps the SCP mirror exact on one (§76) and what the pointer path halves by before its own flip. The near miss is `ESC # 8`, DECALN, which the ENGINE performs — matching the intermediate alone would leave a conformance suite's alignment page double width. Not clamped: the cursor and autowrap are the engine's and know nothing of a line's size, so a program that writes past the visible half puts text on the grid that is not displayed — which is what a VT does with the text, and a divergence in where the cursor ends up (part 5, §76, §101, §146, `term/lineattr.rs`) |
| ESC SP F / G | 7 / 8-bit control output | ✅ | S7C1T / S8C1T choose whether the terminal's own replies use 7-bit or 8-bit C1 controls — `ESC [` or a single 0x9b, `ESC P` or 0x90, `ESC \` or 0x9c. Answered since §145, and it changes only what cmote WRITES: both forms of every C1 are read on the way in, always, by the engine's parser and by every scanner here. **The hazard is the requester's and is stated rather than guarded against**: the single-byte C1 controls are not valid UTF-8, and a program that asks for them and then reads its terminal's replies through a UTF-8 decoder sees replacement characters where the introducer was. That is what the sequence exists to ask for; cmote powers up 7-bit, so nothing arrives in the state by accident. Implemented as one flag on the reply buffer — the one place every answer in the program passes through, the engine's included — plus a WATERMARK, which is the only real design in it: a chunk can switch half way through, so the buffer is sealed at the switch and again at the drain, and the mark is what stops the final pass retroactively rewriting answers already promised in the other spelling. One thing had to move for it: the sixel amendment to the engine's DA1 (§41) now happens where the reply ARRIVES rather than over the drained buffer, because in the 8-bit form `ESC [ ?` is a single byte and the amender would not have found it. RIS resets the form; DECSTR does not, DEC's published list not naming it (§72). One divergence: the replies built AFTER the chunk (`term/query.rs`'s six) are sealed in the setting as the chunk left it, so `CSI c` then `ESC SP G` in one write answers 8-bit where xterm answers 7 (§67, §72, §145, `term/c1.rs`) |
| ESC % G | UTF-8 charset | ✅ | selects UTF-8 as the encoding — supported in the sense that the parser decodes UTF-8 always, the sequence itself reaching nothing, which is equally why `ESC % @` back to ISO-8859-1 has nowhere to go (§67) |

### DCS — Device Control String

| Code | Feature | Status | Note |
|---|---|---|---|
| DCS $ q (DECRQSS — the nine reported settings) | Report a setting | ✅ | `DCS $ q <sel> ST` asks what a setting currently is; answered `DCS 1 $ r <params><sel> ST` from live state, never from a stored copy of the request. `m` (SGR) rebuilds the pen the grid paints with, opening `0` and listing only what is set; `SP q` (DECSCUSR) reports the shape being DRAWN, so a blink request is answered with its steady twin because cmote runs no animation timer; `" q` (DECSCA) reads the protection bit §56 borrows on the pen; `r` (DECSTBM) reads cmote's own mirror of the region the engine keeps private; `s` (DECSLRM) reads the margins §102 gave it; and since §152 `t` / `$ \|` / `* \|` (DECSLPP / DECSCPP / DECSNLS) read the grid — the same number twice for the lines, cmote's page being exactly its screen — and `* x` (DECSACE) reads the extent §59 stamps onto each attribute request. **A report is a sequence the program could send back to get the same state**, which decides the numbering both ways: a position is 1-based and a count is itself, and DECSACE's stream reports `0` rather than the `1` that may have set it (§33, §66, §102, §123, §152, `term/query.rs`, `term/mod.rs::decrqss_report`) |
| DCS $ q `" p` (DECRQSS — DECSCL) | Report the conformance level | 🛑 | asks which VT level the terminal parses as. cmote parses one dialect and names it identically in `TERM`, XTVERSION and XTGETTCAP; it holds no VT level, so stating one means inventing it. The same fact the `CSI " p` row refuses from the other side (§78, §96, §98, §152) |
| DCS $ q `$ }` / `$ ~` (DECRQSS — DECSASD / DECSSDT) | Report the status display | 🛑 | ask which display output goes to, and what type the status line is. `0` is **true** of both — cmote has no status line and never leaves the main display — and it is refused anyway, because it is an invitation: a program told the feature exists turns the status line on and writes to it, cmote ignores both sets silently (part 6), and the text lands on the user's page. A refused DECSCPP costs a resize that did not happen; a refused DECSASD costs the program's next paragraph written over the screen, which is why the geometry settings above are reported and these two are not (§152) |
| DCS $ q `) {` / `, \|` / `, }` (DECRQSS — DECSTGLT / DECAC / DECATC) | Report a colour table setting | 🤷 | VT525-only colour-table settings, naming features cmote does not have at all; answered `DCS 0 $ r ST` (§152) |
| DCS $ q `> Pm f` / `> Pm m` / `> Pm t` (DECRQSS — XTQFMTKEYS / XTQMODKEYS / XTSMTITLE) | Report an xterm resource | 🛑 | xterm's own three extensions, in DECRQSS's marker form. The one cmote holds state for is XTQMODKEYS, and **its question already has an answered spelling** — `CSI ? 4 m` → `CSI > 4 ; Pv m` since §61 — so a second reply format no source publishes would be invented for a fact already reachable. `Tc`'s refusal, one family over (§61, §123, §152) |
| DCS + q `TN` / `name` / `Co` / `colors` / `RGB` (XTGETTCAP) | Report a capability | ✅ | `DCS + q <hex-name> ST` asks for a terminfo capability; `TN` and its terminfo spelling `name` answer `xterm-256color`, the name requested for the remote pty, `Co` / `colors` answer `256`, and `RGB` answers `8` — the numeric form ncurses' `user_caps(5)` defines as bits per channel, which is what cmote's SGR 38;2 path takes. **These are xterm's whole list of special names**, and two of the three have two spellings; `Co`/`colors` was paired from §33 and `TN`/`name` only from §153, so a program asking in the terminfo spelling got "unknown" for a fact stated one line above. The reply echoes the name that was *asked*. Hex both ways (§33, §123, §153, `term/query.rs`) |
| DCS + q `Tc` (XTGETTCAP — tmux's truecolor flag) | Report a capability | 🛑 | tmux's own extension, in neither xterm's list of XTGETTCAP special names nor ncurses' recognised user capabilities — and a pure **boolean**, which this reply grammar cannot spell: a recognised name is answered `<NAME>=<VALUE>`, so answering `Tc` means inventing a value on nobody's authority. `RGB` is the question with an answer, and it is answered (§123) |
| DCS + q (XTGETTCAP — every other capability) | Report a capability | 🛑 | the same request for any other capability, including the real terminfo key names; answered `DCS 0 + r <NAME> ST`, unknown, with the requested name echoed back. **Read ❌ until §153**, on the note that a full answer needs a terminfo database cmote does not carry — which is exactly right about the mechanism (xterm's `xtermcapString` reads `tcap_fkeys`, filled by `tigetstr`/`tgetstr` — a static lookup, not a string generated from the current DECCKM) and wrong about the mark. The entry cmote would be copying is **`xterm-256color`**, which cmote names in `TN` on purpose and which sits on the remote that asked; a hard-coded transcript of it would be a second copy of a database cmote does not own, wrong on some machines the day it was written. `Ms` is unknown for a second reason: it is the OSC 52 capability, and cmote refuses the remote clipboard (§66, §123, §153) |
| CSI > q (XTVERSION) | Terminal version | ✅ | asks for the terminal's name and version; answered `cmote(<ver>)` (§33, `term/query.rs`) |
| CSI = c (DA3 → DECRPTUI) | Tertiary device attributes | ✅ | the reply half of DA3 — the terminal's unit id, a constant `00434D45` (§36, `term/query.rs`) |
| DCS … q | Sixel graphics | ✅ | sixel graphics, a bitmap written six pixels at a time; decoded in-house and composited over the grid, anchored to an absolute document line and reserving its cells, with its own page on the alternate screen (§41, `term/sixel.rs`, `term/graphics.rs`) |
| DCS tmux; … | tmux passthrough | ✅ | tmux's passthrough, which wraps a sequence meant for the terminal beyond tmux. **Read ❌ until §154**, which measured it: cmote implements none of it and is transparent to it, by three of `vte`'s own rules composing. `ESC P t` is a complete introducer (`t` is a final byte), so `mux;` is the string's payload and every reader drops it — nothing here claims a DCS ending in `t`. A payload ends at its first ESC, which also opens the next sequence. And a second ESC restarts the escape rather than nesting, so tmux's doubled escapes collapse for free. What is left is the payload's own sequences on the ordinary path: same gate, same scanners, same offsets, **same refusals** — a wrapped OSC 52 read and a wrapped notification draw nothing, exactly as the plain spellings do, because every scanner reads the raw chunk. Two divergences, both stated rather than fixed: cmote un-doubles nothing (so an emitter that forgot to double is understood here and would not be by tmux), and a payload opening with plain text is swallowed, since until its first ESC it is still the DCS's. Fixing the second means suppressing bytes on their way IN, which cmote does nowhere (§154) |
| DCS … ! z / DCS … { / DCS … \| | Define macro / download charset / user-defined keys (DECDMAC / DECDLD / DECUDK) | 🤷 | the three sequences that let a host leave something **behind** in the terminal: a macro to be replayed by number (DECINVM's partner), a downloaded soft character set, and new meanings for the function keys. Each turns a remote's payload into terminal state that outlives the command that sent it, and the keyboard's meanings are the user's (part 6, §36, §98) |
| DCS $ p (STP) | Set terminal profile | 🤷 | contour's: replace the terminal's whole configuration profile by name. iTerm's `SetProfile` in DEC's envelope, and the fixed scheme's argument covers it whole (part 6, §55, §98) |
| DCS … ! g (GIP) | Good Image Protocol | 🛑 | contour's image protocol — `o=u` upload, `o=r` render, `o=s` upload-and-render, `o=d` release, `o=q` query, in a DCS envelope. **Read ❌ until §155**, on a note that said "the protocol's own bookkeeping is [the price]" without reading the bookkeeping. Reading it gives three prices, each decisive. **The advertisement**: GIP's own feature detection is DA1 attribute `90`, and cmote's DA1 is `CSI ? 6 ; 4 c` — so nothing sends GIP here, and the first line of an implementation is the promise rather than the sequence. There is no honest partial, because `90` says *this specification*, not one fifth of it. **The lifetime is counted in CELLS**: the refcount rises by the number of cells displaying fragments and falls as they are overwritten, which needs per-cell image identity — the engine's flag word is full (§151), a map beside it is dropped on every reflow, and a per-cell map is unbounded in remote input (§12). §41 met that requirement differently and that is why sixel works: an image is anchored to an absolute document line and reserves its cells, so no cell remembers anything. **The pool**: `o=u` stores an image under a name the remote chose, which is the class part 6 refuses whole (DECDMAC / DECDLD / DECUDK, §98). Today the string is dropped by every reader — both DCS scanners insist on final byte `q` — and DA1 is pinned by a test that asserts 6 and 4 and never 90 (§41, §98, §151, §155) |

### SGR — text styling

| Code | Attribute | Status | Note |
|---|---|---|---|
| 1 | Bold | ✅ | the heavier weight of the face |
| 2 | Dim / faint | ✅ | faded toward the background |
| 3 | Italic | ✅ | the italic face, from the bundled IBM Plex Mono family |
| 4 | Underline | ✅ | a single line under the text |
| 5 / 6 | Slow / rapid blink | 🛑 | the text flashes, slowly or fast. `vte` turns both into `Attr::BlinkSlow` / `BlinkFast`, and since §161 the **gate drops them by name** rather than forwarding them to an engine that had no arm for either. **Read ❌ until §161**, on the first of two prices: the per-cell flag word is a `u16` with the engine's fifteen bits and §56's borrowed sixteenth, so there is nowhere to store the attribute, and a map beside the grid is dropped on every reflow and unbounded in remote input (§12, §151). The second price is what makes this a refusal — **cmote runs no animation timer and never will** (§65), so a bit that came free would light nothing, and this is the same decision the two cursor-blink rows in this table already carry. Blink is refused at four doors now, this being the one an ordinary `CSI 5 m` reaches; the other three are DECCARA / DECRARA's `5`, DECRQCRA's `0x40` weight and `CursorShape`. The whole SGR run around it still applies (part 5, §36, §56, §65, §151, §161, `term/gate.rs`) |
| 7 | Reverse video | ✅ | swaps the cell's foreground and background |
| 8 | Hidden / conceal | ✅ | the text is not drawn; a copy still yields it |
| 9 | Strikethrough | ✅ | a line through the text |
| 21 / 4:2 | Double underline | ✅ | two lines under the text, in both spellings |
| 4:3 / 4:4 / 4:5 | Curly / dotted / dashed underline | ✅ | curly, dotted and dashed underlines, drawn as cmote's own quads |
| 53 | Overline | 🤷 | a line above the text. It never reaches cmote at all: `vte`'s SGR table (`ansi.rs:1834-1908`) has no `[53]` arm, so the parameter falls to `_ => None` and is SKIPPED — the run around it is dispatched and this one value is not. That is a different layer from the blink rows above, which do reach the gate; §151 separated the two, and the flag word would be full for this one either way |
| 30–37 / 40–47 / 90–97 / 100–107 | 16 ANSI colours | ✅ | the eight ANSI colours and their bright halves, as foreground and as background |
| 38;5 / 48;5 | 256-colour indexed | ✅ | a colour by index into the 256-colour cube |
| 38;2 / 38:2 | Truecolor (`;` and `:`) | ✅ | a colour by its red, green and blue components, in both the `;` and `:` spellings |
| 58;5 / 58;2 | Underline colour | ✅ | colours the underline apart from the text |

### DECSET / DECRST private modes

| Code | Mode | Status | Note |
|---|---|---|---|
| 1 | Application cursor keys | ✅ | DECCKM — the arrow keys send `SS3 A`–`SS3 D` in place of `CSI A`–`CSI D` |
| 3 (side effects) | DECCOLM's clear | ✅ | DECCOLM's other half: the scrolling region is reset and the screen cleared — what the sequence is actually used for |
| 3 (column resize) | 132 / 80 columns | 🤷 | DECCOLM proper, which switches the page between 132 and 80 columns (part 6, §65) |
| 5 (DECSCNM) | Global reverse video | ✅ | reverse video over the whole screen at once — a RENDERING rule, so the grid is untouched and turning the mode off puts the page back with nothing to unwind, exactly as the character path and the line sizes are (§76, §146). Swapped by XOR against SGR 7 and the cursor, so an even number of them cancels the way a real terminal's do; the rich copy swaps with it; cmote's own chrome — the selection fill, the find bar's wash — does not (§39, §40, §149, `palette.rs`, `term/decmodes.rs`) |
| 6 | Origin mode | ✅ | DECOM — row 1 becomes the scrolling region's top and the cursor cannot leave the region |
| 7 | Auto-wrap | ✅ | DECAWM — a glyph printed in the last column wraps to the next line instead of overwriting |
| 12 (the mode) | Blinking cursor — tracked | ✅ | the blinking-cursor mode as a tracked bit — set, reset, and reported back by DECRQM |
| 12 (the blink) | Blinking cursor — drawn | 🛑 | the same mode as something drawn; cmote runs no animation timer, so the cursor is steady whatever DECRQM reports (§65) |
| 25 | Show / hide cursor | ✅ | DECTCEM — whether the cursor is drawn at all |
| 45 (XTREVWRAP) | Reverse wrap | ✅ | reverse wraparound: a BACKSPACE at the leftmost column backs up to the rightmost column of the previous line — the xterm manual page's own sentence on the `reverseWrap` resource, and the extent honoured. ctlseqs names the mode and says nothing about its behaviour, so the four things it leaves open were answered by the narrowest reading — and §161 read xterm's own `cursor.c` and settled two of them. **DECAWM is coupled**, each mode being masked with `WRAPAROUND` before it is read; the top of the page stops this one, that being 1045's job. The other two stand as divergences named rather than closed: CUB does not wrap here, and cmote does not require the line above to be a WRAPPED one, which xterm's 45 does — so cmote's 45 reaches as far as 1045 does on every row but the first. The edges are the margins' when a band is set (§102, §149, §161, `term/decmodes.rs`) |
| 1045 (XTREVWRAP2) | Extended reverse wrap | ✅ | xterm's *extended* reverse wraparound: the same backspace as 45, with the top of the page no longer stopping it — the first row's left edge carries round to the last row's right edge. A second mode beside 45 rather than a modifier on it, `cursor.c` testing the two flags separately, so either alone enables the wrap; and coupled to DECAWM as 45 now is. **Read ❌ until §161**, on a note that said no source states what it widens: two do, and the change log states it twice over by putting the stop and the mode that lifts it in one patch (§149, §161, `term/decmodes.rs`, `term/gate.rs`) |
| 69 (DECLRMM) | Left / right margin | ✅ | enables the left and right margins DECSLRM sets. Absent from the engine's mode list, so it is held by the gate and never reaches the engine — and **DECRQM is answered here too**, `1` set or `2` reset, because the engine's own answer is `0`, "not recognised", which was true until §102 and is now a lie a program would act on. Setting the mode opens the band to the whole page and resetting it throws the band away, so a program can put margins down and pick them up again without the next one having to guess. RIS, a soft reset and every resize clear it. **This row used to be where the margin gap lived** (part 5, §73, §102, `term/margins.rs`, `term/gatediff.rs`) |
| 80 (behaviour) | Sixel scrolling | ✅ | what sixel scrolling mode governs: cmote always scrolls, the modern default and what emitters assume (§41) |
| 80 (the mode) | DECSDM | 🤷 | DECSDM as a mode — setting it asks a sixel not to scroll the page (§65) |
| 1000 / 1002 / 1003 | Mouse: normal / btn / any | ✅ | mouse reporting: presses and releases, the same plus drag, or all motion. Left, middle, right and the vertical wheel are encoded; the extra buttons and the horizontal wheel are not (`term/mouse.rs`) |
| 1004 | Focus events | ✅ | the terminal sends `CSI I` and `CSI O` as the window takes and loses focus |
| 1006 | SGR mouse | ✅ | the SGR mouse encoding, `CSI < b ; col ; row M` / `m` — a release keeps its button and the coordinates are unbounded |
| 1005 | UTF-8 mouse | ✅ | the UTF-8 mouse encoding, which widens the classic one's coordinates; tracked on the seam, `1006` taking precedence when both are set — and `1016` over both since §150 (§67, §150) |
| 1015 | urxvt mouse | ✅ | urxvt's own encoding — `ESC [ <b>;<x>;<y> M`, the classic three fields written as decimal parameters, so the 223 ceiling is lifted; the final byte is always `M`, so a release cannot name its button and says 3 as the one-byte form does. **Only `b` carries the classic +32 bias**, stated by the defining source and pinned by that source's own worked example, which is what the test asserts. ctlseqs says "Enable urxvt Mouse Mode" and no more, so the definition read is urxvt's own manual page. cmote's own mode, on the encoding ladder between 1006 above and 1005 below (§150, §161, `term/mouse.rs`) |
| 1007 | Alt-scroll | ✅ | the wheel sends arrow keys while the alternate screen is up |
| 1016 | SGR-pixel mouse | ✅ | the SGR reports with **pixels in place of cells** — same button field, same `M`/`m` split, one-based from the top-left of the text area. cmote's own mode, sitting at the top of the encoding ladder, so a program that also set 1006 gets the more specific of the two. **The coordinate convention is reasoned rather than quoted**: every reachable source says only "Enable SGR Mouse PixelMode, xterm", and §150 names the inference in writing rather than presenting it as read (§150, `term/mouse.rs`) |
| 1049 | Alternate screen | ✅ | the alternate screen: the cursor is saved and a cleared page swapped in. No scrollback there, by design |
| 2004 | Bracketed paste | ✅ | bracketed paste — a paste arrives wrapped in `CSI 200~` and `CSI 201~`, with an injection scrub |
| 2026 (batching) | Synchronized output | ✅ | synchronized output: BSU holds the visible screen still and ESU flushes the buffered stream inside one advance, so a frame is atomic (§65) |
| 2026 (abort timeout) | Synchronized output | ✅ | the 150 ms bound on a stuck update, which the application must drive because `vte` stores the instant and never reads it: a frame clock runs while any terminal in the window holds a bracket, and the frame is applied once its expiry passes (§122) |
| 2027 | Grapheme clustering | ❌ | grapheme clustering — a cluster occupies one cell rather than each code point taking its own. What cmote does without it is measured rather than guessed (§151): `Term::input` decides width per CHARACTER and appends only ZERO-width followers to the cell before them, so a family emoji is three wide cells across six columns. The missing piece is not a bit but a METHOD — there is no engine call to append a wide character to the preceding cell — so this gap is upstream, and a test records the six columns so a version bump that closes it is noticed. **§161 swept this column and left the mark where it is**, which is a decision and not an omission: 🛑 would claim cmote's code refuses grapheme clustering and no code here does, while ✅ would claim a cluster occupies one cell. ❌ is the honest third thing — a gap that could still land, on somebody else's release (§151, §161) |
| 2031 | Colour-scheme reporting | ❌ | the **unsolicited** half of dark/light reporting: with it set the terminal sends `CSI ? 997 ; 1 n` / `; 2 n` every time its scheme changes. Not in the engine's mode list, so DECRQM answers `0`, not recognised — the truth rather than a shortfall, cmote's scheme having no way to change. A program told `0` polls instead, and the question it polls with is answered (the `? 996 n` row). §148 re-read this beside mode 2048, which cmote DOES hold, and left it: claiming a mode whose notification can never fire would stop the polling that works. The gap is `palette.rs`'s fixed scheme, not this row. **§161 swept this column and left it**, and this is the one row in the table where the sweep's own question has a wrong answer available: implementing the mode would leave a program measurably WORSE off than the gap does, because DECRQM's honest `0` is what keeps it polling `CSI ? 996 n`, which cmote answers (§6, §98, §148, §161) |
| 2034 | Semantic block reporting | 🤷 | arms contour's semantic-block query and mints the token that authenticates it. Refused with the query itself — a mode whose only effect is to enable a refused reply has nothing else left to do (§98) |
| 2048 | In-band resize | ✅ | in-band resize notification, for a program that cannot see SIGWINCH: `CSI 48 ; rows ; cols ; height ; width t` on every size change, and once the moment the mode is FIRST set, which the specification requires and is the only way a program learns the size it is starting from. Cells then pixels, each pair height before width; the pixels are the same cell size the two `t` queries above report, so the three cannot disagree. Off at power-up and put back by RIS — not by DECSTR, which is on nobody's published list. cmote's own mode: the engine has never heard of 2048, so the gate keeps it, answers its DECRQM, and lets XTSAVE read it (§102, §141, §145, §148, `term/inband.rs`) |
| 4 | Insert / replace (IRM) | ✅ | IRM — a printed glyph pushes the rest of the line right rather than overwriting it. An ANSI mode, not a `?` private one, hence out of the run above |
| 9 | X10 mouse (press-only) | ✅ | the original protocol: `CSI M Cb Cx Cy`, each field biased by 32, on **button press only** and with **no modifier bits** — both from XFree86's untruncated copy of ctlseqs, which is where §150 read the Mouse Tracking section this document's usual source cuts off before. cmote's own mode, at the bottom of the mode ladder: a program that also set one of the engine's three gets that one, which diverges from xterm's single-variable model on purpose (§9, §150, `term/mouse.rs`) |
| 20 | Newline mode (LNM) | ✅ | LNM — a linefeed also returns the carriage. An ANSI mode, not a `?` private one |

### Graphics, window ops, keyboard, C0

| Feature | Status | Note |
|---|---|---|
| Sixel images | ✅ | the DCS `q` bitmap format, decoded and composited by cmote itself with no engine work (§41) |
| Kitty graphics protocol / unicode placeholders / animation | ❌ | kitty's image protocol — an APC string carrying chunked transmission, image ids, placements, deletions, unicode placeholders and animation; `f=24`/`f=32` payloads are raw RGB and need no decoder at all (part 5, §41, §70) |
| ReGIS | ❌ | DEC's vector graphics language, drawn by an interpreted command stream (part 5) |
| iTerm2 inline images (OSC 1337) | 🛑 | the OSC-framed inline image, on a framer cmote already runs; the OSC table's `iTerm 1337 File` row carries it (§70) |
| Graphics capability report | ✅ | XTSMGRAPHICS' read — 256 colour registers, 4096×4096 and 4 Mpx, the decoder's real limits. The set action has its own row in the CSI table (§41, `term/query.rs`) |
| Window iconify / move / resize / raise / maximize / fullscreen (CSI 1–10 t) | 🤷 | `CSI 1–10 t` lets a remote iconify, move, resize, raise, maximize or fullscreen the window cmote owns (part 6) |
| Window state report (CSI 11 t) | 🛑 | asks whether the window is iconified; `CSI 1 t` for no, `CSI 2 t` for yes. Refused twice over: it names the person rather than the program (§36), and a terminal that is one pane of one tab has no true answer to give — a pane in a background tab is not iconified and is exactly as invisible (§147, `term/window.rs`) |
| Window position report (CSI 13 t) | 🛑 | asks where the window sits on the desktop; `CSI 3 ; x ; y t`, and `CSI 13 ; 2 t` for the text area's own corner. Refused on §36's rule, and a `0 ; 0` would be a lie where a refusal is a stall the sender can detect (§147, `term/window.rs`) |
| Screen size reports (CSI 15 / 19 t) | 🛑 | ask how large the whole display is, in pixels and in characters. Refused with the two rows above and for their first reason: a monitor's size is a fact about the desk cmote sits on. Neither had a row here before §147 (§36, §147, `term/window.rs`) |
| Text area in pixels / chars (CSI 14t / 18t) | ✅ | the text area's size, asked in pixels and in characters |
| Cell size (CSI 16 t) | ✅ | one cell's height and width in pixels, answered `CSI 6 ; height ; width t` from the same pair the row above is built from, so the two cannot disagree. Answered where the question SAT rather than after the chunk — alone among cmote's sniffed queries, because its sibling `CSI 14 t` is answered by the engine mid-advance and a program asking both reads the replies by position (§147, `term/window.rs`) |
| Lines per page (CSI Ps t, Ps ≥ 24 — DECSLPP) | 🤷 | xterm reads `CSI Ps t` with `Ps` of 24 or more as DECSLPP, "set the page to `Ps` lines" — the resize family in the numbering of the window operations, and refused with them: the height is the user's. Its **report** half is answered, which is the split §152 drew (part 6, §147, §152) |
| Title stack (CSI 22 / 23 t) | ✅ | push and pop the window title; the `; 0` / `; 1` / `; 2` that would name icon or window title alone is ignored, and the stack is capped at 4096 (§67) |
| **Kitty keyboard protocol** | ✅ | CSI-u key reporting over a flag stack — disambiguated keys, event types and associated text (§25, `term/kitty.rs`) |
| **xterm modifyOtherKeys** — set (`CSI > 4 ; n m`) | ✅ | `CSI > 4 ; n m` picks how modified keys are encoded, `n` being `0`, `1` or `2` — an input-encoding hint rather than a screen operation (§9, `term/modkeys.rs`) |
| **xterm modifyOtherKeys** — query (`CSI ? 4 m`), resource 4 | ✅ | asks resource 4 back; answered `CSI > 4 ; Pv m`, the **set** form, so a program can write the reply straight back, and answered where the question sits in the stream (§61) |
| **xterm modifyOtherKeys** — query, the other six resources | 🛑 | XTMODKEYS carries seven resources — ctlseqs numbers them 0, 1, 2, 3, 4, 6, 7, with no 5 — and cmote tracks one. A query for any of the rest draws silence, the reply being an XTMODKEYS control with no way to say "not mine": an answer would assert a level for a knob the key encoder does not have, and a program that then SET it would be ignored while the next query reported the invented number back. **Read ❌ until §163**, on that true observation and past what cmote does with it: `report` is an **allow-list one resource wide**, reached through a full parse — the construction `term/dsr.rs` refuses `CSI ? 26 n` with (§36, §82), `term/iterm.rs` OSC 1337 keys and `term/pointer.rs` pointer shapes, all three 🛑. The set half is the same allow-list, and both are pinned by tests naming all six (§60, §61, §68, §163, `term/modkeys.rs`) |
| ENQ answerback | 🤷 | a lone `0x05` asks the terminal to type a configured string back into the shell (part 6, §36) |
| BEL | 🛑 | `0x07` rings the bell — a sound, or a visual flash of the window (part 6, §63) |
| BS / HT / LF / CR | ✅ | backspace, tab to the next stop, linefeed, and carriage return. HT is stored as well as performed: the engine writes the `\t` into the first cell it skipped so a COPY of columnar output gets a real tab back, and `ui::grid` draws that cell as the blank it is — a control character handed to the text shaper would jump to its own tab stop and displace the rest of its run (§117) |
| SO / SI | ✅ | the charset shift — SO locks G1 into GL and SI locks G0 back |

**Shape of it.** The whole legacy VT100 / xterm core is ✅ — cursor motion, editing, SGR, full
colour, alternate screen, mouse, bracketed paste, focus, DA1 / DA2 / DSR / DECRQM, DECSCUSR, REP, the
kitty keyboard protocol, the application keypad, and — since §33, completed by §36 — every identity
query the engine dropped (XTVERSION, DECRQSS — five settings since §123 and nine since §152 — XTGETTCAP, DA3), and — since §56 — the VT220
protected-cell erase it dropped as well, and — since §58, §59 and §60 — the whole VT420 rectangular
family, checksum query included, and — since §72 — **DECSTR**, the soft reset every `tput init` opens
with and no arm in `vte` ever heard. The **deliberate** part of what is missing used to be most of the ❌
column and now carries two marks of its own. **🛑** is what cmote's code refuses and its tests pin:
the remote clipboard (OSC 52 both ways), desktop notifications in **all three** spellings — the OSC 9
one since §54, kitty's `99` and urxvt's `777` since §79 — the dangerous
half of iTerm's OSC 1337 namespace — **inline images among them since §70** — and a fixed colour scheme
that makes every palette set and reset a no-op. Since §71 it also holds two refusals with **no danger
behind them at all**, `CursorShape` and `ReportCellSize`, which is worth noticing about the mark: 🛑 says
cmote's code performs the refusal, never that the thing refused was dangerous. **§152 through §155 put
four more in the column, and three of them came out of ❌** — DECRQSS's status-line settings, where the
truthful `0` is refused because it is an invitation; every XTGETTCAP capability past the three special
names, because the entry a table here would transcribe belongs to the remote that asked; and contour's
Good Image Protocol, whose first move is an advertisement (DA1 attribute `90`) cmote will not make. The
fourth, DECSCL's report, was never a gap. **The same four sections moved one row the other way**: the
tmux passthrough wrapper is ✅ and cmote implements none of it, being transparent to it by three of
`vte`'s own rules composing (§154) — which is the ✅ that says *what the sequence asks for is what
happens*, the reading `ESC % G` has carried since §67. **§156 then swept every document in the tree
and moved nothing here**, which is the result worth recording rather than the absence of one: all
**41** `term/*.rs` modules have an entry, and every `.rs` path this document names either exists or
is named as a crate's own or as deleted. The staleness §156 found was all in the documents that
*stopped* being cited — the README, last written at §127 — and this one is touched by every section,
which is what a row-per-sequence structure buys over prose. **🤷** is what cmote would refuse and never gets the chance to: answerback, remote window
control (`CSI 1–10 t`), the palette stack (`CSI # P / # Q`, §84), kitty's colour-by-name (`OSC 21`), and the
two DECSET modes nothing can turn on (`3`, `80`) — each one dead in `vte` or in a `Handler` default
before cmote sees a byte. **§151 added the overline (SGR 53) to that column**, and the way it got there
is the sharpest example of what the mark means: `vte`'s SGR table has no arm for the parameter, so it
falls to `_ => None` and is skipped while the run around it is dispatched. The blink beside it in the
same run DOES reach cmote and stays ❌, which is two marks for what had been one row's worth of
"missing cell attributes". §75 added the **character path** (SCP) to that list and §76 took it back off;
§77 then took off the **remote pointer shape** (OSC 22), which had sat there since §54. That is the one
thing about 🤷 worth watching: it is the mark most likely to be a conclusion rather than a finding,
because nothing fails while it is wrong — three sections in a row now, and both times the entry had
argued the sequence against an architecture cmote does not have. **§79 found the fourth way it goes
wrong**, and it is the plainest: 🤷 says *upstream* refuses this, and for `kitty 99` and `OSC 777`
upstream refused nothing — it simply never looked, which is not the same claim. Six rows were left in the
column at §79, and the two that left it did so without a single byte of behaviour changing. **§98 took it
to twenty-six**, and that jump is the whole of what reading three more catalogues found: not one mark
moved for being wrong, and thirty-four rows appeared that had never existed — the special and Tektronix
and selection colours, the page family, DEC's macros and status line, contour's buffer capture and its
semantic-block query. A column that quadruples on one reading was never six rows long; it was six rows
*known*, and the same caution now applies to the other three. That leaves the plain ❌ column, worth reading as the real list: the
kitty graphics protocol (a protocol's worth of work, not the decoder this document charged it for until
§70), ~~blink (the engine drops
it — and §151 measured *why*: the per-cell flag word is a `u16` with fifteen bits named and the
sixteenth already borrowed by §56, so the gap is a bit that does not exist)~~ — **§161 moved blink
to 🛑**, having weighed the second price §151 had not: the bit could come free on somebody else's
release, but the animation timer is cmote's own refusal and is already spent twice in the same table,
so the feature cannot land without a decision being reversed rather than a version being bumped —
~~the newer private modes
(2027 / 2031 / 2048)~~ — **2048 SHIPPED in §148**, 2031 stays a gap whose blocker is the fixed colour
scheme rather than the row, and 2027 stays one whose blocker is a missing engine METHOD rather than a
bit (§151) — left-right margins, and — since §98 — the
**horizontal-scroll family**, SL / SR, DECBI / DECFI and DECIC / DECDC, which read as one piece of
absent machinery wearing six names. **§100 built it and moved two of them**: SL and SR are supported,
and what it cost was forty lines beside the rectangles, because the hard half — writing cells into the
engine's grid — had been paid in §56.

**§102 finished the family bar one pair, and §112 finished it.** The margins themselves and DECIC /
DECDC came with §102, leaving DECBI / DECFI as a gap with both halves of their definition already
written as code — §100's sideways scroll and §102's margins. What was actually missing was neither
half: it was a way to READ two escape sequences, which needed a state machine of its own for two
bytes until §111 gave the whole directory one. All six names are supported.

The family was named as one piece of machinery in §98 and turned out to be exactly that: each section
that built part of it made the next part smaller, and the last part was built by a section that was
not about it at all.

What made the margins movable was not new machinery but a corrected sentence. Part 5 had costed the
delegating-`Handler` build twice and refused it both times because such a wrapper "degrades silently
on an engine bump" and that could not be caught at build time — which stopped being true, and had
probably stopped being true before either refusal was written. The head of part 5's bullet carries the
correction and the lesson: **this document re-derives its facts every sweep and had been carrying its
prices forward unexamined.** §98 found the same failure in the marks; a price quoted three times is no
better evidence than a row nobody re-read.

Whatever is left of that list is still catalogued with its cost in part 5 — which read as the *only*
section with anything open in it until §60's audit put one row back into part 3.

§56 is worth reading as a method rather than a feature. Every earlier addition worked by scanning a
sequence out of the stream and keeping the answer BESIDE the grid — a cwd, an exit code, a picture's
anchor. Protection could not be kept beside the grid, because it is per-cell state that has to survive
scrolling and reflow, and a map of it would have meant re-implementing the grid to keep the two
aligned. So instead cmote borrowed the one unused bit in the engine's per-cell flag word and let the
engine carry protection as if it were bold. That is a third way in, next to "scan it out" and "accept
the engine's limit", and the reason DECSERA above is now a rectangle rather than a wall.

**§72 names a fifth — §57's, below, was the fourth: translate it.** A sequence the engine has no arm for can sometimes be spelled
in sequences it does have arms for, and then the work is a lookup table rather than an implementation.
DECSTR is exactly that shape — every item on DEC's list is a mode, a region, a pen or a charset the
engine already takes an ordinary sequence for — so cmote scans the reset out and feeds the engine the
long spelling of it. §41 did this in one place already (a picture's cells are reserved with ECH and LF
rather than written), and the property that makes it worth naming is the one §71 argued for: nothing
gains a second writer, so nothing can end up with two answers. It only works where the missing sequence
is a *shorthand*; where it is a capability — margins, kitty graphics — there is nothing to translate
into, which is why this does not quietly empty the ❌ column.

**§102 names a sixth, and it is the one every earlier section had ruled out: stand in front of the
engine.** `Processor::advance<H: Handler>` is generic over the handler and `Term` merely *implements*
`Handler`, so a type holding `&mut Term` can be passed in its place, answer a dozen calls itself and
forward the rest. Two things need it and nothing else can give them:

- **Reading back what the engine decided.** The vertical scrolling region is private, with no
  accessor and no reply arm. A scanner beside the stream can watch DECSTBM go past on the wire — but
  not the RESETS, which happen inside the engine on RIS and on resize. The `Handler` boundary catches
  all of them, because that is where the engine is TOLD.
- **Pre-empting a decision.** Margins change what PRINTING does, and there is no repairing that
  afterwards: by the time the glyph is on the grid it is at the wrong columns.

The reason this was refused for seven sections is at the head of part 5's margins bullet, and it comes
down to one sentence about the tooling that had stopped being true. What makes it a *route* rather
than a one-off is the guard: `#[deny(clippy::missing_trait_methods)]` turns a forgotten forward — or
one a version bump introduces — into a build error, which is the property part 5 said was unavailable.
The distinction that keeps it inside §71's rule is that a gate is not a second author: it pre-empts a
decision and delegates, and the engine still writes every cell it wrote before.

§57 found a fourth, and a different kind of gap to go with it. Every row in these tables until now was
some flavour of "the engine ignores this"; DECSLRM is the one where the engine ignores nothing and gets
it *wrong* — `vte` dispatches its final `s` to save-cursor without reading the parameters, so a margin
request cmote cannot honour was still costing the program its saved cursor. A sequence like that cannot
be scanned out and applied beside the grid, because the problem is not what cmote fails to do with it,
it is what the engine does. So `process` now cancels the offending byte in flight — advance up to it,
feed the state machine's own CAN in its place, resume after it. "Refuse it properly" is the fourth way
in, and the cheapest: a refusal that costs nothing is worth more than most ✅s. **§73 gave it the mark to
match** — a row cmote's own code stops dead is a 🛑, and this one had been reading ❌ with the word
*safely* propped beside it, which is the retired partial mark wearing a coat.

**§102 kept the machinery and retired the guess it rested on.** The byte is still cancelled and the
CAN argument is unchanged — but only when mode 69 says the sequence really is DECSLRM. §57 had to
decide on the parameter count alone, because the engine refused the mode that settles it; cmote holds
the mode now, so a parametrised `s` without it is a save-cursor again, which is what a real xterm
makes of it. Worth noting what that says about the fourth route: a refusal built on the best evidence
available is still a refusal built on evidence, and it stops being right the moment better evidence
turns up. The 🛑 that §73 was right to award is now a ✅, on the same code.

§60 closed the last row of part 8's CSI table and then swept the two tables above it against the crate
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

*[**§147 took the other half of that same `unhandled!()` and found a 🛑 where the commands are a 🤷.**
The window QUESTIONS — 11, 13, 15 and 19 — die in the same arm, so by the mechanism alone they read
like the commands beside them. They are not: a scanner beside the stream reaches every one of them, and
`term/window.rs` answers exactly one — `CSI 16 t`, the cell size, which is a fact about the terminal —
and declines four that name where the window sits, whether it is minimised and how large the display
is. So the mark turns on whether cmote *could* have acted, not on where the sequence dies, and this is
the pair that shows the difference: same arm, same `unhandled!()`, one 🤷 and one 🛑, because the
commands cannot be performed from a scanner and the questions can be answered from one.]*

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

**§64 pointed the same question at the partial rows, which had never been asked to justify themselves.** A
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

**§65 finished the sweep, and the partials turned out to be the least examined rows in the document.** Ten
remained; seven were two answers under one mark and are now two rows each, one was a refusal wearing a
partial's clothes (`BEL` — `alacritty_terminal` really does raise `Event::Bell`, and cmote's catch-all
really does drop it, so it is a 🛑 and always was), and two are genuinely partial. The pattern is worth
naming: a ❌ invites someone to close it and a 🛑 invites someone to check it, but a partial invites
nothing — it has already admitted to being incomplete, so it is never asked *which part*. That is how mode
2026 sat for this long reading "cmote already atomic", which was true, beside an undriven abort timeout
that lets a remote freeze the screen. The audit that finds a thing like that is the same one every time:
read the dispatch arms, then ask who performs each half.

**§66 then removed the mark itself.** The last two rows carrying it, DECRQSS and XTGETTCAP, were split
into the answer each gives (✅) and the answer each declines (❌) — and writing the declined halves down
settled what they are: not refusals. cmote says "not reported" and "unknown" because it has no reporting
code, not because anything decided the program should not know. Both are answerable from state that
already exists, which is precisely what a mark meaning "partial, and that is fine" had kept out of view
for four sections. The matrix now has four marks and one rule: **one row, one answer, one mechanism.** The
value of that is not tidiness — it is that every row is now a claim narrow enough to be wrong, and
therefore checkable. Six ✅ rows still carry a "but only…" clause and by the same rule are two rows each;
Part 7 names them, and they are honest as written, which is the difference.

**§67 then went after the last mark that could be read generously.** ✅ had meant "full", which put the
burden on a row to be complete and on a reader to notice when it was not — and a reader who supplies the
word "full" themselves is exactly the reader §60's six wrong rows survived. It now means **supported**,
with the extent in the note and an empty note reserved for the strong claim. Sweeping the table under that
definition was the cheapest audit in this sequence and still found `1005` — the UTF-8 mouse encoding,
tracked by the engine and read off the seam since the mouse shipped, with no row anywhere — and DSR
carrying two different reports under one tick. It also found a row that was ✅ for a reason it did not
give: `ESC % G` is honoured by the parser being UTF-8 always, not by any `%` arm, `vte` having none, which
is also why there is no way back out to ISO-8859-1. Seven rows gained the extent they had been leaving to
the reader: tabs and HTS, `CSI g`'s two parameters, the mouse's buttons, the title stack's one title and
4096-deep cap, and DSR's two halves. One new ❌: `CSI ? 6 n`, the private cursor-position spelling, which
reaches nothing — and which §82 turned into a ✅, the reason recorded here for refusing it having been a
page number xterm does not send either.

**§68 spent the rule's last instalment.** §66 had named six ✅ rows that carried a "but only…" clause and
left them, on the grounds that they at least *said* their second half — which was true, and was also the
argument every partial row had made before it. Splitting them cost seven pairs and forced seven decisions
the notes had been dodging: `OSC 8`'s refused schemes, DECSCUSR's blink and XTSMGRAPHICS' set turn out to
be refusals **cmote performs** (an allow-list, a seam that drops a flag the engine stored, a `status 3`
written by cmote's own code), while charset designation, ~~the XTMODKEYS query's other six resources~~
and `OSC 0`'s icon-name half are gaps nobody had ever marked as such. That is the whole value of the rule
stated in one line: a second half left inside a note is a decision nobody has had to make, and this
document's entire audit history is what happens when those pile up. **§163 moved the XTMODKEYS half of
that sentence to 🛑** — splitting the row was right and the mark that came out of it was not, because
the split asked "is this answered?" and stopped there. `modkeys::report` is an allow-list one resource
wide, exactly the construction the same paragraph credits `OSC 8`'s schemes with; §68 had the mechanism
in front of it and read the row as a gap because the *reply format* has no way to say "not mine".

**§161 swept one table's ❌ column and found the four rows were four different things.** The DECSET
list carried four unsupported modes and the sweep asked one question of each — can this be built, or
is the mark a decision? **Two were built.** 1015 wanted a source xterm does not carry, the mode
belonging to another terminal, so the definition is `urxvt(7)`'s and that page's worked example is the
test's expected bytes. 1045 wanted a source §149 had looked for and not found — and *why* it was not
found is worth more than the mode: §149 read the specifications, and this mode is defined only by
xterm's change log and its `cursor.c`. **Two did not move, and neither is an omission.** 2027 is a
missing engine method (§151), so ✅ would be a lie and 🛑 would claim a refusal no code here performs;
2031 is the one row in this document where shipping the mode would leave a program measurably WORSE
off, because DECRQM's honest `0` is what keeps it polling a question cmote answers (§148). So the
column halved without a single mark changing meaning, and what the two that shipped have in common is
that **the work was never the code — it was finding the sentence**. Note what the sweep did NOT
produce: no 🛑. An unsupported column is exactly where the temptation to write one is strongest, and
writing it on either row that stayed would be §79's failure again, a mark claiming somebody declined
where nobody looked. **The sweep also corrected a shipped row**: the same `cursor.c` shows mode 45
masked with `WRAPAROUND`, so both reverse wraps are now coupled to DECAWM where §149 had them free on
a report rather than a reading. One divergence was found and deliberately left — xterm's 45 requires
the line above to be a *wrapped* line and cmote's does not — because closing it changes what a shipped
mode does, which is a decision for a section about mode 45 and not a by-product of a sweep. It is
named in that row rather than kept here.

---

## Evidence

Audited file:line anchors behind the claims above, for later re-checking.

### `alacritty_terminal` 0.26.0 (registry crate — `…/alacritty_terminal-0.26.0/src/`)

- **Generates host replies** via `Event::PtyWrite`. `identify_terminal` (`term/mod.rs:1257`)
  answers **primary** DA (`ESC[?6c`) and **secondary** DA (`ESC[>0;<ver>;1c`) — the `=`
  (tertiary) intermediate falls to a debug no-op. `device_status` (DSR, `term/mod.rs:1332`) and
  `report_mode` (DECRQM, `term/mod.rs:2135`) reply likewise.
- **DSR is the ANSI spelling only** (§82). `vte-0.15.0/src/ansi.rs:1701` is
  `('n', []) => handler.device_status(next_param_or(0) as usize)` and there is **no `('n', [b'?'])`
  arm anywhere in the table**, so every DEC-private `CSI ? Ps n` reaches the unhandled arm. What the
  ANSI one answers is two arms wide: `5` writes `\x1b[0n`, `6` writes
  `format!("\x1b[{};{}R", pos.line + 1, pos.column + 1)` from `self.grid.cursor.point`
  (`term/mod.rs:1339-1342`) — **absolute, with no reading of `TermMode::ORIGIN`**, which is the same
  origin-mode divergence §74 measured on the movement sequences and which `term/dsr.rs` copies on
  purpose so the two spellings cannot disagree. xterm's own definition of the private reply, quoted
  from its ctlseqs: "`Ps = 6` ⇒ Report Cursor Position (DECXCPR). The response \[row;column\] is
  returned as `CSI ? r ; c R` (assumes the default page, i.e., "1")" — **no page parameter**, which is
  the fact that expired §67's reason for refusing the row. The same entry lists the nine other values
  (`15`, `25`, `26`, `55`, `56`, `62`, `63`, `75`, `85`) cmote refuses in part 6.
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
  corrects an earlier claim in part 4 that the engine stored it (§36).
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
  a silent no-op rather than a compile error. That is the hazard part 5 prices the feature against, and
  unlike §57's borrowed flag bit it cannot be turned into a build failure. *[§102: the last clause is
  the sentence that was wrong — `#[deny(clippy::missing_trait_methods)]` turns exactly that gap into a
  build failure, and the row shipped on it. Everything before the clause was right, which is why the
  build went in unchanged from what this entry describes.]*
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
  `:2087`). **That enum names fifteen modes and is the whole list**, which is why every private mode
  cmote holds itself takes the same route through the gate: 69 (§102), 2048 (§148), and 5 and 45
  (§149) are all `Unknown` to the engine, so claiming one there is the only way its DECRQM can be
  answered truthfully. The cancel byte cmote feeds instead: 0x18 in `State::CsiParam` falls to the catch-all
  `_ => self.anywhere(...)` (`vte-0.15.0/src/lib.rs:252`), and `anywhere` runs `execute(byte)` and sets
  `State::Ground` — no `csi_dispatch` — while `execute` (`ansi.rs:1296`) has no CAN arm, only the
  `debug!` fallback. `advance_ground` calls `reset_params()` on the next ESC (`lib.rs:605`), so the
  abandoned parameters cannot leak into the following sequence. SUB (0x1a) takes the same transition but
  calls `substitute()` (`term/mod.rs:1443`), which is a `trace!` today and displayable by definition —
  hence CAN.
- **The seam the gate sits in** (read for §102). `Processor::advance<H>` is
  `pub fn advance<H>(&mut self, handler: &mut H, bytes: &[u8]) where H: Handler`
  (`vte-0.15.0/src/ansi.rs:298-301`) — `Handler` is the ONLY bound, so any type implementing it can be
  passed where `Term` was. The trait itself is **71 methods, every one with a default empty body**
  (`ansi.rs:488-732`). `Term` exposes what the gate needs publicly: `grid()` (`term/mod.rs:645`),
  `grid_mut()` (`:650`), `mode()` (`:709`), and `Cursor::input_needs_wrap` is a `pub` field
  (`grid/mod.rs:52`).
- **The scrolling region has exactly four writers** (§102, and the reason a mirror cannot drift).
  `Term::scroll_region` is private (`term/mod.rs:301`) and assigned only at `:420` (`Term::new`),
  `:701` (`Term::resize`, "Reset scrolling region"), `:1843` (`reset_state`) and `:2174-2175`
  (`set_scrolling_region`). The last two are `Handler` methods and so pass through the gate; the first
  two are calls cmote itself makes. `swap_alt` (`:714-735`) does **not** touch it, which is why cmote's
  margins are not per-screen either. A fifth writer arriving in a version bump is the one disclosed
  hazard and nothing catches it at build time.
- **Two engine methods call their own siblings, not the gate's** (§102, and the trap the whole gate has
  to be read for). `Term::newline` (`:1470-1476`) is `self.linefeed(); if LINE_FEED_NEW_LINE {
  self.carriage_return(); }`, and `Term::goto_col` (`:1189`) is `self.goto(self.grid.cursor.point.line.0,
  col)`. Forwarding either would run the margin-blind version of a method the gate has just replaced.
  `goto` (`:1155-1170`) also shows the origin-mode defect §74 recorded: it adds `scroll_region.start` to
  the line it is given, so a pure COLUMN move through `goto_col` drags the cursor downward.
- **`Term::input` decides width before it writes** (`:1062-1136`). `c.width()` from `unicode_width`,
  then zero-width characters combine with the previous cell, then `input_needs_wrap` triggers
  `wrapline()` (`:960-980`), which sets `Flags::WRAPLINE` and returns the cursor to `Column(0)` — the
  wrong column under a left margin, and the reason the flag has to be taken away from the engine.
- **`clippy::missing_trait_methods` exists and fires** (clippy 0.1.97, rustc 1.97.1 — read for §102).
  Run across cmote it reports 117 sites; denied on the gate's `impl` it turns a forgotten forward into
  `error: missing trait method provided by default: <name>`. Verified by deleting `bell()` from the
  forwarding list and watching the build fail, not assumed. This is the fact that reversed part 5's
  refusal of the margins build.
- **No graphics, no double-height lines, no left/right margins, no `?2026`** — no `Sixel`,
  `graphics`, `DoubleHeight`/`DECDHL`, `left_right_margin`, or synchronized-update symbols in the
  crate source. Still true of the crate, and no longer true of cmote: all four are implemented beside
  it (§41, §146, §102, §122), which is what makes this bullet worth keeping — it is the measurement
  that priced each of them as engine work, and each time the price was wrong.
- **The synchronized-update timeout is set and never read** (`vte-0.15.0/src/ansi.rs`, read for
  §122). `SYNC_UPDATE_TIMEOUT` is 150 ms (`:36`) and `SYNC_BUFFER_SIZE` 2 MiB (`:39`);
  `StdSyncHandler::set_timeout` stores `Instant::now() + duration` (`:459`) and `pending_timeout`
  answers `self.timeout.is_some()` (`:469`) — no comparison to the clock anywhere in the crate.
  `Processor::advance` (`:298`) branches on `pending_timeout()` alone, so a bracket ends at its ESU,
  at 2 MiB (`advance_sync`, `:364`), or when the application calls `stop_sync` (`:314`). That last
  one is the whole reason §122 exists, and `sync_timeout()` (`:292`, then `:451`) is how the instant
  is read back out.
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
  arm** for `n`, `o`, `~`, `}` or `|` — nor for `N` or `O`, nor for any designation final but `B` and
  `0`. All of that is still true of `vte`, and since **§143** none of it is true of cmote: the seven
  spellings the parser drops are read beside the stream and the whole charset mechanism moved out of
  the engine, so `SI`/`SO` are no longer the only locking shifts that exist here. `csi_dispatch` has `('p', [b'$'])` and `('p', [b'?', b'$'])` for
  DECRQM and **none for `('p', [b'!'])`**, so DECSTR reached nothing until §72 fed it back in as sequences
  the engine does have arms for — and the two DECRQM arms are why the scanner matches marker and
  intermediates as well as the final byte, a mode question being one keystroke away from a reset.
  `deccolm` (`term/mod.rs:792`)
  clears the region and grid with the comment *"setting 132 column font makes no sense"*, and DECRQM
  answers `ColumnMode => NotSupported` (`:2085`). `BlinkingCursor` sets `cursor_style.blinking` and
  raises `Event::CursorBlinkingChange` (`:1987`, `:2036`), and DECRQM reports it (`:2053`).
  `PrivateMode::Unknown` — which is what 80 is, `NamedPrivateMode` having no DECSDM — is logged and
  ignored (`:1937`, `:2000`) and reports `NotSupported` (`:2087`). Synchronized output lives in the
  parser, not the engine: `SYNC_UPDATE_TIMEOUT = 150ms` and `SYNC_BUFFER_SIZE = 2MiB`
  (`ansi.rs:36`, `:39`), `advance` enters `advance_sync` only when `pending_timeout()` is already true
  (`:303`), and nothing expires a stuck update except the application calling `stop_sync` — which cmote
  never does (no hit for `sync_timeout` / `stop_sync` / `pending_timeout` in `src/`).

- **The OSC payload buffer is unbounded in this build** — §79 left that question open and §88 answers
  it. `MAX_OSC_RAW = 1024` (`vte-0.15.0/src/lib.rs:46`) bounds an `ArrayVec` that exists only
  `#[cfg(not(feature = "std"))]` (`:62`); with `std` — which is what `alacritty_terminal` pulls in —
  `osc_raw` is a plain `Vec<u8>` (`:64`) and `action_osc_put` (`:544`) pushes every byte with the
  fullness check compiled out. So a remote that writes `ESC ]` and then never terminates the string
  makes the parser accumulate the whole of it in memory. Ordinary text is bounded by `SCROLLBACK`
  = 10 000 lines; an unterminated OSC is bounded by nothing.
  **Why it was not fixed here.** `advance_osc_string` (`:407`) ends an OSC on `0x07`, `0x18`, `0x1A` or
  `0x1B`, and every one of those routes through `osc_end`, which **dispatches** what has accumulated.
  There is no abort that discards. So feeding a CAN the way §57 does for DECSLRM would deliver a
  truncated OSC and then leave the rest of the runaway payload to be **printed to the screen** as
  ground text — a megabyte of garbage in place of a memory cost. Discarding the bytes instead means
  filtering the stream on its way in, which §41 refuses for its own reasons. The remaining honest fix
  is a wrapper around the parser, which is the same price part 5 puts on the margins. Recorded, priced, not
  taken. *[§102 paid that price for the margins and this row does **not** come with it. The gate wraps
  the `Handler`, which sits DOWNSTREAM of the payload: the OSC string accumulates inside
  `vte::Parser` and the gate first hears about it at `osc_dispatch`, with the megabyte already
  buffered. Capping it needs interception UPSTREAM of the parser, which is a different and harder
  thing than what §102 built. Worth stating so the new route is not mis-cashed against a row it cannot
  reach.]*
- **The colour OSCs take lists, which the matrix had as single requests until §87.** `b"4"`
  (`ansi.rs:1366`) refuses an even parameter count and then walks `params[1..].chunks(2)`, so
  `OSC 4 ; 1 ; ? ; 3 ; ?` is two queries and two replies. `b"10" | b"11" | b"12"` (`:1422`) is one arm
  over three codes and **increments** `dynamic_code` per parameter, so a list walks up from wherever it
  started — `OSC 10 ; ? ; ?` is the foreground then the background — and stops at `NamedColor::Cursor`.
  `b"104"` (`:1496`) resets **all 256** when given no parameters at all and otherwise one per parameter.
  Both query forms are pinned end to end since §87, the rows having claimed them first.
- **Where the 🤷 rows die, and the two ❌ rows beside them** (§83 — the anchors the notes used to carry,
  moved here when the Note column became a definition). `vte` 0.15.0's **OSC arms** are `0`/`2`, `4`,
  `8`, `10`–`12`, `22`, `50`, `52`, `104`, `110`–`112` and **nothing else**, so kitty's `OSC 21`, plain
  `OSC 9`, kitty's `99` and urxvt's `777` all reach no handler (§78, §79). **XTWINOPS** is one arm,
  `('t', [])` (`ansi.rs:1739`), matching **14 / 18 / 22 / 23** and sending every other parameter to
  `unhandled!()` — which is why `CSI 1–10 t` has nothing to refuse, and why `CSI 11 / 13 / 15 / 16 / 19 t`
  fell to a scanner beside the stream when §147 came to them (§71, §147). That one arm is also why `t` is
  the only final byte in `differential`'s shape sweep that the engine answers on some parameters and drops
  on others, which is the near miss `term/window.rs` is built around. **Both `#` stacks** have no arm at all: the only `b'#'` in the file is
  `esc_dispatch`'s `(b'8', [b'#'])`, DECALN (`:1814`), so `csi_dispatch` never sees a `#` intermediate
  and the palette stack (`CSI # P` / `# Q`) and the video-attribute stack (`CSI # {` / `# }`, aliased
  `# p` / `# q`) fall through whole — two different sequences the matrix had as one until §84. The
  second of them is cmote's own since §85 (`term/sgrstack.rs`), scanned out of the stream before the
  engine ignores it; the first is still nobody's.
  **Kitty graphics** arrives as an APC string, and
  `State::SosPmApcString => self.anywhere(performer, byte)` (`vte-0.15.0/src/lib.rs:182`) drops every
  byte of it without calling a `Perform` method (§41, §70). **ENQ** and the two DECSET modes are in the
  bullet above.

### xterm's ctlseqs (`invisible-island.net/xterm/ctlseqs/ctlseqs.html`, read for §84)

The document cmote's `TERM` claims conformance to, and the source §84 checked §83's definitions against.
Quoted where a row's wording now rests on it.

- **`CSI # p` / `# q` are XTPUSH*SGR* / XTPOPSGR**, not the colour stack: "Push video attributes onto
  stack (XTPUSHSGR), xterm. This is an alias for `CSI # {`, used to work around language limitations of
  C#." The colour stack is the **capitals** — "`CSI # P` … Push current dynamic- and ANSI-palette colors
  onto stack (XTPUSHCOLORS)". The matrix had the lower-case pair labelled as the colour stack from §65
  until §84 split them. XTPUSHSGR's parameters "correspond to the SGR encoding for video attributes,
  except for colors (which do not have a unique SGR code)" — `30` foreground, `31` background — and "if
  no parameters are given, all of the video attributes are saved. The stack is limited to 10 levels."
- **The DSR-DEC replies** §82 refuses, each quoted for the row that names it: printer `CSI ? 10 n`
  (ready) / `? 11 n` (not ready); UDK `? 20 n` (unlocked) / `? 21 n` (locked); keyboard
  `CSI ? 27 ; 1 ; 0 ; 0 n` (North American) — the **first parameter is the keyboard's language**, which
  is the fact §36's rule turns on; data integrity `? 70 n` (ready, no errors); multi-session `? 83 n`
  (not configured). Locator: "`CSI ? 50 n` Locator available" / "`CSI ? 53 n` No Locator", and
  "`CSI ? 57 ; 1 n` Mouse" / "`CSI ? 57 ; 0 n` Cannot identify". Macro space is "Report macro space
  (DECMSR). The response is `CSI Pn * {`" — **no unit is given**, and the matrix claimed "units of 16
  bytes" until §84 removed it. Memory checksum "The response is `DCS Pt ! ~ x x x x ST`", which is the
  envelope `term/rect.rs` writes for DECRQCRA.
- **The DEC locator protocol** (§140, read from the text build). The four sequences: "`CSI Ps ; Pu ' z`
  Enable Locator Reporting (DECELR)", whose first parameter is `0` disabled / `1` enabled / `2`
  "enabled for one report, then disabled" and whose second "specifies the coordinate unit for locator
  reports" — `0` or omitted defaulting to character cells, `1` "device physical pixels", `2` character
  cells; "`CSI Pm ' {` Select Locator Events (DECSLE)", taking a **list**, where `0` is "only respond
  to explicit host requests (DECRQLP). This is default. It also cancels any filter rectangle" and
  `1`–`4` turn button-down and button-up transitions on and off; "`CSI Ps ' |` Request Locator
  Position (DECRQLP)", where "Ps = 0, 1 or omitted → transmit a single DECLRP locator report" and **no
  other value is defined**; and "`CSI Pt ; Pl ; Pb ; Pr ' w` Enable Filter Rectangle (DECEFR), VT420
  and up". The report they produce is `CSI Pe ; Pb ; Pr ; Pc ; Pp & w`, "Parameters are
  [event;button;row;column;page]", and the event code cmote sends is the first of eleven:
  **"Pe = 0 ← locator unavailable - no other parameters sent"** — the rest naming a host request (`1`),
  the four buttons' down and up transitions (`2`–`9`) and "locator outside filter rectangle" (`10`).
  What the document does **not** say is what xterm sends for a DECRQLP when the locator is disabled or
  when it is built without `OPT_DEC_LOCATOR`; xterm's source was not read, and §140's answer is
  unconditional precisely because it does not depend on the part that is unknown.
- **XTSMGRAPHICS' parameter order** is `CSI ? Pi ; Pa ; Pv S` — `Pi` the item (1 colour registers, 2
  Sixel geometry, 3 ReGIS) and `Pa` the **second** parameter (1 read, 2 reset, 3 set, 4 read maximum).
  The matrix spelled its two rows `? Pi;Pa;1 S` and `? Pi;Pa;3 S`, as though the action were third, from
  §41 until §84. `query.rs`'s `graphics_request` reads item then action, so the code was right and only
  the row was wrong — and it answers action **2** with a status 0 as well, which no row had said.
- **`CSI 16 t`** — "Report xterm character cell size in pixels. Result is `CSI 6 ; height ; width t`".
  **Height before width**, which is the opposite of the order the two are usually said in and the one
  thing about this reply that is easy to get backwards; the test that pins it says so in its name (§147).
- **The other four window questions**, read for §147 and written down here because three of them had
  never had a row. `Ps = 11` reports the window state, "if the xterm window is non-iconified, it returns
  `CSI 1 t`; if it is iconified, it returns `CSI 2 t`". `Ps = 13` reports the window position as
  `CSI 3 ; x ; y t`, with `Ps = 13 ; 2` reporting the text area's own corner. `Ps = 15` reports the
  **screen** size in pixels as `CSI 5 ; height ; width t` and `Ps = 19` the same in characters as
  `CSI 9 ; height ; width t`. All four are refused: they describe the desk cmote sits on rather than the
  terminal (§36), and 11 has no true answer to give from a pane of a tabbed window at all.
- **The Good Image Protocol's own specification** (§155), `contour-terminal/terminal-good-image-protocol`.
  The envelope is `DCS ! g <message> ST`, the `!` an intermediate and the `g` a final byte "consuming a
  single DCS slot while avoiding conflicts with existing sequences"; the command is a field, `o=u` /
  `o=r` / `o=s` / `o=d` / `o=q`; formats are `1` auto, `2` RGB, `3` RGBA, `4` PNG. The lifetime is
  **reference counted per screen cell** — the count starts at 1 on upload, "increments by the number of
  screen cells displaying fragments", decrements "when cells are overwritten or deleted", and the image
  is evictable at zero, FIFO. And: "Support for this specification is advertised via DA1 response code
  `90`." The repository describes itself as *a formalization of a proposal*, and contour gates it behind
  a `good_image_protocol` configuration key.
- **DECRQSS' selector list, entire** (§152). `ctlseqs`' `DCS $ q Pt ST` entry names fifteen selectors —
  `m` SGR, `" p` DECSCL, `SP q` DECSCUSR, `" q` DECSCA, `r` DECSTBM, `s` DECSLRM, `t` DECSLPP,
  `$ |` DECSCPP, `$ }` DECSASD, `$ ~` DECSSDT, `) {` DECSTGLT (VT525 only), `* x` DECSACE,
  `* |` DECSNLS, `, |` DECAC (VT525 only), `, }` DECATC (VT525 only) — and three xterm extensions in
  the marker form, `> Pm f` XTQFMTKEYS, `> Pm m` XTQMODKEYS and `> Pm t` XTSMTITLE. §66 and §123 each
  worked from a *sample* of this list; §152 is the first section to read it out. Note that the
  **XFree86 mirror is an older revision here** and gives only four selectors, which is the reverse of
  §150's finding about the same two copies — the untruncated one is not always the current one.
- **XTGETTCAP has exactly three special names**, and cmote answered two spellings of two of them.
  `ctlseqs`: "*Co* for termcap colors (or *colors* for terminfo colors)", "*TN* for termcap name (or
  *name* for terminfo name)", and "*RGB* for the ncurses direct-color extension". Everything else in
  the request is "termcap or terminfo capability names for **special keyboard keys**" (§153).
- **xterm answers a key capability out of a real database.** `xtermcap.c`'s `xtermcapString` reads
  `screen->tcap_fkeys`, which `loadTermcapStrings` fills with `tigetstr` / `tgetstr` — a static
  terminfo or termcap lookup, *not* a string generated from the terminal's current DECCKM or keypad
  state. So the matrix's "a full answer needs a terminfo database cmote does not carry" was exactly
  right about the mechanism, and §153 is about which database it would be a copy of.
- **ncurses' recognised user capabilities are six**, from `user_caps(5)`: `AX` (boolean), `E3`
  (string), `NQ` (boolean), `RGB` (boolean, numeric or string), `U8` (numeric), `XM` (string), plus the
  experimental `xm`. The page says "while the terminfo database may have other extensions, ncurses
  makes explicit checks for the following" — and there is **no second table** of the capabilities other
  applications use, which is what §123 relied on when it refused `Tc` and what §153 re-checked before
  refusing the rest (§123, §153).
- **The OSC list reads from the plain-text build, not the HTML one** (§87, §88). Every fetch of
  `ctlseqs.html` returns it truncated part-way through `Ps = 4` — "Change Color Number *c* to the color
  specif" — which is where §87 stopped. `ctlseqs.txt` reaches further and settled four rows:
  "Change VT100 text foreground color to *Pt*" (`10`), the same for background (`11`) and "Change text
  cursor color to *Pt*" (`12`); "Manipulate Selection Data… The parameter *Pt* is parsed as *Pc ; Pd*"
  (`52`); **"Change pointer cursor shape to *Pt*… If *Pt* is empty, or does not match any of the
  standard names, xterm uses the resource's default 'xterm' shape"** (`22`), which is the divergence
  that row now names; and **"Set Font to *Pt*… If *Pt* begins with a '#', index in the font menu,
  relative (if the next character is a plus or minus sign) or absolute"** (`50`) — xterm's `OSC 50` is
  the FONT, and the cursor-shape payload the matrix had on that code is another terminal's convention
  `vte` parses on the same number. §88 split the row. `OSC 8`, `104` and `110`–`112` are still past
  where even the text build is returned, and rest on `vte`.
- **The Mouse Tracking section is past the cut in both builds, and XFree86's copy is not** (§150). The
  DECSET list arrives whole — which is how `Ps = 9` and `Ps = 1 0 1 6` were readable at all — and the
  section that says what either mode DOES was truncated in every fetch of `ctlseqs.html` and
  `ctlseqs.txt` alike. `xfree86.org/current/ctlseqs.html` is an older revision of the same document,
  returned in full, and it settled X10 outright: **"X10 compatibility mode sends an escape sequence only
  on button press"**; **"xterm sends CSI M *Cb Cx Cy* (6 characters). *Cb* is button−1"**; **"encode
  numeric parameters in a single character as *value*+32"**; and, in the X11 paragraph below it,
  **"Modifier key (shift, ctrl, meta) information is *also* sent"** — the *also* being what says X10
  carries none. Being older, it has nothing on 1016, whose coordinate convention is therefore reasoned
  rather than quoted, and labelled as such in its row.
- **Mode 1015 is defined by urxvt's manual page, not by xterm's** (§161). ctlseqs gives it one line —
  "*Ps* = 1 0 1 5 → Enable urxvt Mouse Mode" — and the Mouse Tracking section that would say more is
  past the same cut, in the same two builds, that §150 recorded above; XFree86's older copy predates
  the mode entirely. `urxvt(7)` defines it, the mode being that terminal's: **"If mode 1015 is active,
  urxvt sends the sequence `ESC [ <b>;<x>;<y> M` where the parameters are provided as decimal numbers
  instead of octets and only `b` includes an offset of 32"**, with the worked example **"Shift-Button-1
  press at top row, column 80. `ESC [ 36 ; 80 ; 1 M`"**. The example is what makes the sentence
  checkable — shift (4) on button 1 (0) plus the bias is 36 — and it is the test's expected bytes.
- **Mode 1045 is defined by xterm's change log and its source, neither of which §149 had read** (§161).
  ctlseqs again gives one line, "Extended Reverse-wraparound mode (XTREVWRAP2)", which is why §149 left
  the row unsupported. The change log carries both halves as adjacent bullets of **patch #380
  (2023/05/09)**: **"disallow wrapping before the beginning of the screen, to the end of the screen,
  for cursor-back sequences"**, then **"add private mode 1045 which imitates the original xterm
  cursor-back reverse wrapping mode 45"** — so the mode is exactly the stop that patch added, lifted.
  `cursor.c`'s own comment says it from the other side — **"That was revised in 2023, using private
  mode 45 for movement within the current (wrapped) line, and 1045 for movement to 'any' line"** — and
  its code answers the rest: `rev` and `rev2` are separate tests, each masked with `WRAPAROUND`, and
  the wrap-round is `row = bottom + 1` followed by `--row`. **This is the first row in this document
  settled from an implementation rather than a specification**, and it is worth naming as such: what
  was read is what xterm DOES, which is the same standing the `vte` and engine reads have and a lower
  one than a published grammar.
- **OSC 8's own specification** (Egmont Koblinger's, the document VTE and iTerm2 implement): the
  sequence is `OSC 8 ; params ; URI ST` and is closed with `OSC 8 ; ; ST`; "params is an optional list
  of key=value assignments, separated by the `:` character", of which only `id` is defined — "character
  cells that have the same target URI and the same nonempty id are always underlined together on
  mouseover"; "both VTE and iTerm2 limit the URI to 2083 bytes"; and, for the row that refuses every
  other scheme, "it's up to the terminal emulator to decide what schemes it supports".
- **Two claims ctlseqs does *not* support**, and the rows now say so themselves: it gives **no accepted
  range for DECFRA's `Pch`** ("`Pc` is the character to use", and nothing more), so 32–126 / 160–255 is
  cmote's own allow-list rather than something xterm publishes; and it gives DECSACE's three values with
  **no default named**, so "the state a terminal powers up in" is `rect.rs`'s `#[default] Stream` and the
  VT420 manual behind it, not this document.

- **SL and SR, read for §100**, are two lines and the whole of what any reachable source says about
  them: **"Shift left `Ps` column(s) (default = 1) (SL), ECMA-48"** and **"Shift right `Ps`
  column(s) (default = 1) (SR), ECMA-48"**. Note the verb — *shift*, not *scroll* — and note what is
  absent: ctlseqs says **nothing about the margins**, neither DECSTBM's top and bottom nor DECSLRM's
  left and right, for these or for DECIC / DECDC beside them. So cmote's page-wide reading is the only
  one its sources describe, and the question of whether a real xterm stops at a scrolling region is
  open rather than answered against cmote — which is why §100 refuses the case where a region is
  likeliest rather than guessing at it. ECMA-48 itself, where the definition actually lives, is still
  unread here.

  *[§102 acted on the first half of that and not the second. SL and SR are now bounded by the
  scrolling region, because the region became readable and a shift is a scrolling operation — the
  refusal §100 built on DECOM is retired. Whether a real xterm agrees is **still open**: ctlseqs says
  no more than it did, and ECMA-48 is **still unread**, which is the same load-bearing gap §76 left on
  SCP. The bound is now cmote's reasoning rather than cmote's caution, which is a different kind of
  claim and worth marking as one.]*

- **The wrap column under margins is the mode's, not the cursor's** (§102). xterm's
  `ScrnRightMargin` / `ScrnLeftMargin` read the LEFT_RIGHT mode flag and never the cursor's column, so
  with DECLRMM set a line breaks at the right margin **wherever it started** — text begun left of the
  band flows rightward, meets the right margin and continues at the left one. cmote wrote the other
  rule first (text outside the band keeping the whole page), which reads better, is nobody's
  behaviour, and would have been a dialect. Corrected before it shipped, and pinned by a test that
  states the surprising half rather than hiding it.

- **The terminfo proves margins announce themselves.** All four capabilities `xterm-256color`
  declares set mode 69 before they place anything —
  `smglr=\E[?69h\E[%i%p1%d;%p2%ds`, `smglp=\E[?69h\E[%i%p1%ds`, `smgrp=\E[?69h\E[%i;%p1%ds`,
  `mgc=\E[?69l`. §73 read this to check a traffic claim; §102 read the same four lines for the fact
  that made it safe to stop cancelling every parametrised `CSI s` — a program that means margins says
  so first, so the mode can be trusted to tell DECSLRM from SCOSC.

### DEC's own manual (`vt100.net/docs/vt510-rm/`, read for §94)

The VT510 programmer reference, one page per sequence — the source for everything in the CSI table
that predates xterm, and the one §84 said it had not read.

- **DECSACE's default is `0`**, the wrapped stream, stated as such on its own page: "`0` (default):
  DECCARA or DECRARA affect the stream of character positions… `2`: DECCARA and DECRARA affect all
  character positions in the rectangular area." §84 could not confirm this from ctlseqs, which lists
  the three values and names no default, and softened the row to "cmote powers up in stream". It is
  DEC's default as well, and `rect.rs`'s `#[default] Stream` matches it.
- **DECFRA's range is DEC's, not xterm's.** "`Pch` can be any value from 32 to 126 or from 160 to
  255. If `Pch` is not in this range, then the terminal ignores the DECFRA command." §88 credited the
  range to xterm and recorded that ctlseqs states none; the range is real, the attribution was wrong,
  and DEC also prescribes the *behaviour* cmote already had — ignore the command, rather than clamp
  or substitute.
- **DECSTR's list is eighteen items**, and cmote sends the eleven anything in this stack models:
  DECTCEM, IRM, DECOM, DECAWM, DECNKM, DECCKM, DECSTBM, the charsets, SGR, DECSCA, DECSC. The other
  seven — KAM, DECNRCM, DECAUPSS, DECSASD, DECKPM, DECRLM, DECPCTERM — name state neither `vte`, nor
  the engine, nor cmote has, so nothing is left stale by not sending them (§72). The list says
  **"Autowrap (DECAWM): No autowrap"**, which is the departure §72 took deliberately and can now cite
  rather than paraphrase.
- **`CSI Ps # y` is XTCHECKSUM** (xterm's, from the text build of ctlseqs): "the bits of `Ps` modify
  the calculation of the checksum returned by DECRQCRA", with five bits for negation, video
  attributes, blanks, uninitialised cells and 8-bit masking. It had no row until §94.
- **DECRQM's reply values**: `0` not recognised, `1` set, `2` reset, `3` permanently set,
  `4` permanently reset.
- **DECRQPSR, DECCIR and DECTABSR** (read for §143). `CSI Ps $ w` takes `0` "Error. Request ignored",
  `1` for the cursor information report and `2` for the tab stop report — **and the family has no
  "I do not report that" form**, which is the question §98 left open on that row: DECRQSS has one
  (§66), this does not, so an unrecognised request is answered with silence and the silence is DEC's.
  DECCIR is `DCS 1 $ u Pr ; Pc ; Pp ; Srend ; Satt ; Sflag ; Pgl ; Pgr ; Scss ; Sdesig ST`, and all
  four S-fields are built alike: "Bit 8 — always 0", "Bit 7 — always 1", "Bit 6 — extension
  indicator", then the flags, which is a base of 0x40 with flags added on. Srend's four are bold,
  underline, blink and reverse; Satt's one is selective erase; Sflag's four are origin mode, SS2, SS3
  and a pending autowrap. Scss's four say which slot holds a 96-character set. Sdesig is "a string of
  intermediate and final characters indicating the character sets designated as G0 through G3", and
  the page's own example is `B0%5%5`. DECTABSR is `DCS 2 $ u D...D ST` with the example
  `9/17/25/33/41/49/57/65/73` — one-based columns, `/`-separated.
- **DECRPDE's parameter list**, which §98 recorded as unread: `CSI Ph ; Pw ; Pml ; Pmt ; Pmp " w`,
  where Ph is "the number of lines of the current page displayed excluding the status line", Pw the
  columns, Pml "the column number displayed in the left-most column", Pmt "the line number displayed
  in the top line" and Pmp "the page number displayed". DECRQDE itself is `CSI " v` with no parameter.
- **DECSTR's charset line says only "Default settings"** for "G0, G1, G2, G3, GL, GR" and never says
  what those are. §143 needed a value for GR and this is where the trail ends, so the choice is
  argued from elsewhere: there is no LS0R, so G1 is the lowest slot GR can ever name.

### The national replacement character sets (read for §143)

The twelve NRCS tables are not in ctlseqs — it names which final byte selects which set and stops
there — so the substitutions came from DEC's own position-by-position tables, cross-checked against
xterm's `charsets.h` wherever the first reading looked ambiguous. Three findings worth keeping:

- **Norwegian/Danish is documented in two versions**, a ten-position table (`@` → `Ä`, `[\]` → `ÆØÅ`,
  `^` → `Ü`, `` ` `` → `ä`, `{|}` → `æøå`, `~` → `ü`) and a six-position one with only the `ÆØÅ` /
  `æøå` pairs. xterm carries a **single** `map_NRCS_Norwegian_Danish` and it is the ten-position one,
  which is what settled cmote's. All three of its finals — `` ` ``, `E` and `6` — select it.
- **Several sets have two final bytes** (`C`/`5` Finland, `R`/`f` France, `Q`/`9` French Canada,
  `H`/`7` Sweden, and Norway/Denmark's three). DECCIR's `Sdesig` reports the spelling that was
  *designated*, so the two cannot be canonicalised to one — which is why cmote's table has an entry
  per spelling sharing one set of substitutions.
- **Dutch is the odd one**: it puts a plain `|` at 0x5d, so one of its replacements is an ASCII
  character standing in a different column. A first reading of a secondary source had that entry and
  `ƒ` one position apart from where they belong; the second reading is the one in the table.

### The vendor extensions (each terminal's own documentation, read for §89)

Two thirds of the OSC table is nobody's standard: sequences one terminal invented and others copied.
§87 recorded that they had no source at all and §88 went to xterm, which does not have them either.
This is that gap closed, one vendor at a time — and two of them could not be closed.

- **kitty's `OSC 21`** (`sw.kovidgoyal.net/kitty/color-stack/`) is `OSC 21 ; key=value ; … ST` over
  `foreground`, `background`, `selection_foreground`, `selection_background`, `cursor`, `cursor_text`,
  `visual_bell`, `transparent_background_color1..7` and the numbers `0`–`255`. A query is `key=?`; a
  **reset is the bare key with no `=`**, which the matrix had as `key=`. And the answer to a query for a
  colour the terminal does not have is an **empty value** — `OSC 21 ; foreground=rgb:ff/00/00 ; cursor= ST`
  is the documented example — which **falsifies §78's second reason** for refusing the row: answering
  the keys cmote lacks would not mean inventing a colour, the protocol has a way to say "not set". The
  dialect reason stands and the row does not move. Push and pop of the colour stack are `OSC 30001` /
  `OSC 30101`, neither of which the matrix has ever mentioned.
- **kitty's `OSC 99`** (`sw.kovidgoyal.net/kitty/desktop-notifications/`) is
  `OSC 99 ; metadata ; payload ST` with the metadata a `:`-separated `key=value` list: `p` payload type,
  `i` identifier, `d` done flag, `e` base64 encoding, `f` application name, `u` urgency (`0` low,
  `1` normal, `2` critical), `n` icon name.
- **iTerm2's `OSC 1337`** (`iterm2.com/documentation-escape-codes.html`) — the forms the matrix
  refuses, as iTerm2 writes them: `SetUserVar=[key]=[base64]`, `Copy=:[base64]`, `SetProfile=[name]`,
  `SetColors=[key]=[value]` over `fg`, `bg`, `bold` and `link`, `SetBackgroundImageFile=[base64]` with
  an empty value removing it, `RequestAttention=[yes|once|no|fireworks]`, `CursorShape=[0|1|2]`, and
  `ReportCellSize`, whose reply carries "height, width, and optional scale values" — the scale being a
  third number the matrix had not mentioned.
- **ConEmu's `OSC 9`** (`conemu.github.io/en/AnsiEscapeCodes.html`) is **multiplexed five ways**, and
  the matrix had three of them: `9;4;st;pr` progress (`0` remove, `1` set to `pr` 0–100, `2` error,
  `3` indeterminate, `4` paused), `9;9;"cwd"` the working directory — and **`9;1;ms` sleeps the
  terminal, `9;2;"txt"` raises a GUI message box, `9;3;"txt"` sets the tab text**, none of which had a
  row. What that page does *not* document is a bare `OSC 9 ; <text>` notification, which cmote's
  `term/notify.rs` attributes to ConEmu; that spelling may be Windows Terminal's alone.
- **`OSC 7`** is `\033]7;file://HOSTNAME/CURRENT/DIR\033\\`, "originates from macOS Terminal", and is
  what lets a new tab inherit a pane's directory (`wezterm.org/shell-integration.html`).
- **`OSC 133` is sourced at one remove** (corrected in §95; this section had it as unsourced). Its
  specification is Per Bothner's, hosted on `gitlab.freedesktop.org`, which serves an access-control
  interstitial to this reader, and most terminals point there rather than restating it. **Contour
  restates it** (`contour-terminal.org/vt-extensions/osc-133-shell-integration/`): the four commands,
  `ST` given as either `ESC \` or BEL, `D`'s exit code written `[ ; <ExitCode> ]` and so optional —
  though with no statement of what an absent one means — and two optional `key=value` fields,
  `click_events=1` on `A` and `cmdline_url=<percent-encoded>` on `C`, neither of which had a row. It
  credits no author beyond "inspired by FinalTerm" and lists no implementers, so it is one vendor's
  restatement standing in for the spec rather than the spec.
- **A second restatement, `vtdn.dev/docs/osc/osc133/`** (read for §96), is the better of the two and
  still not the spec — it cites only VS Code's shell-integration page and gives no URL for FinalTerm
  or Bothner. It supplies a **grammar** (`"133", ";", "D", [ ";", exitcode ], ( 0x07 | 0x1b, "\\" )`),
  gives the bare `OSC 133 ; D ST` its own line as **"Command finished (no exit code)"** — settling
  what §95 recorded as unstated — names a third field, **VS Code's `A ; cl=m`** for a multi-line
  prompt, and records **phase letters past the four**: Konsole "tracks … prompt (A/N/P)", with `N`
  and `P` given no syntax anywhere on the page. Its support table lists eleven implementers
  (Contour, foot, Ghostty, iTerm2, kitty, Konsole, VTE, WezTerm, Windows Terminal, tmux, cy) and
  thirteen that do not implement it — **Alacritty and xterm among them**, which is the citation §34's
  founding claim never had.
- **kitty's `unscroll`** (`sw.kovidgoyal.net/kitty/unscroll/`, read for §101) is the specification
  behind a row §98 attributed to the wrong terminal. Contour's own definition says so —
  `"Scroll Down with Scrollback Fill (kitty unscroll)"`, tagged `VTExtension::Unknown` — and kitty's
  page is where the semantics are: `CSI n + T`, the `+` chosen because it is "legal under ECMA 48
  and previously unused"; the lines are **moved** from the scrollback rather than copied; lines
  pushed off the bottom "are removed from display"; the maximum is implementation-defined but must
  reach one screenful; and where there is no scrollback — the alternate screen, an empty history —
  "the newly inserted lines must be empty". The motivation is quoted on the row: "many modern shells
  will show completions in a block of lines under the cursor, this causes some of the on-screen text
  to be lost even after the completion is completed… This escape code allows that text to be
  restored." Added in kitty 0.20.2.
- **kitty's shell integration** (`sw.kovidgoyal.net/kitty/shell-integration/`, read for §97) is not a
  write-up of the protocol but **the shell code that emits it**, which makes it the most useful of
  the three. Its zsh half prepends `\e]133;A;k=s\a` to `PS2` — "PS2 mark is needed when clearing the
  prompt on resize" — so a **secondary** prompt is an `A` mark carrying `k=s`, and `PS1` carries no
  `k=` at all. Its fish half emits `\e]133;A;special_key=1\a` for an ordinary prompt start, and the
  command line twice over: zsh `'\e]133;C;cmdline=%q\a'` (shell-quoted) against fish
  `'\e]133;C;cmdline_url=%s\a'` (percent-encoded). No `L`, `N` or `P` anywhere on the page.
- **`OSC 777` could not be sourced either**, and that is worth more than a shrug: urxvt's own manual
  page documents **no OSC 777 at all**. The attribution in this table and in `term/notify.rs` is
  folklore — the sequence is real and widely emitted, but "urxvt's" is a claim neither has a citation
  for.

### ncurses' own capability list (`invisible-island.net/ncurses/man/user_caps.5.html`, read for §123)

One page settled a question two sections had left as "ambiguous", which is the whole reason it is
recorded here rather than paraphrased in a row.

- **`RGB` is boolean, numeric OR string**, and each form means something different: it "asserts that the
  `set_a_foreground` (`setaf`) and `set_a_background` (`setab`) capabilities employ *direct colors*". A
  **numeric** one "tells ncurses what result to add to red, green, and blue" — the bits per channel; a
  string form gives the three channels' bit counts slash-separated; a boolean one makes ncurses derive
  the bits from `max_colors`. XTGETTCAP's reply grammar has a value slot and no boolean spelling, so the
  numeric form is the only one that fits it: cmote answers `8`.
- **`Tc` is not in this list at all.** It is not an ncurses-recognised user capability and it is not one
  of xterm's XTGETTCAP special names (`TN`, `Co`/`colors`, `RGB`) either — it is tmux's own extension,
  read from terminfo rather than asked for over the wire. That absence, plus its being a bare boolean,
  is what makes it a 🛑 in part 8 rather than a gap.

### The sequence catalogues (read for §98)

Part 8 was built against one catalogue, `vtdn.dev`. Three more were read end to end for §98 — contour's
sequence index, its extension pages, and otty's OSC and CSI trees — and between them they named
**thirty-odd sequences this table had never mentioned**. That is the finding: the gap was not in what
the marks said but in which rows existed, and a catalogue only shows you the rows you already have.

- **contour's sequence index** (`contour-terminal.org/vt-sequence/`) is the broadest of the four: every
  control code, ESC, CSI, OSC and DCS it implements, with mnemonic, notation and a line of
  description. What it gave this table: `OSC 3` (X11 property), `OSC 30` (`SETTABNAME`), `OSC 60`
  (`SETFONTALL`), `OSC 888` (`DUMPSTATE`), `XTREPORTCOLORS` (`CSI # R`), `UNSCROLL` (`CSI Ps + T`),
  `SETMARK` (`CSI > Ps M`), `DECSSCLS`, `DECSCL`, `XTSHIFTESCAPE`, `DECRQDE`, `DECRQPSR`, the locator
  trio, `DECINVM`, `DECSCPP` / `DECSNLS`, `DECSASD` / `DECSSDT`, `DECIC` / `DECDC`, `DECPS`,
  `DECBI` / `DECFI`, the page family, and the DCS macro / DRCS / UDK trio. Its notation drops the
  intermediates on several entries — `SL` is written `CSI 0..1 @`, which is ICH's spelling — so the
  rows above take the sequence from where it is defined and the *existence* of the row from here.
  **And it is a catalogue of what contour implements, not of what contour invented**: §101 found
  `UNSCROLL` credited in contour's own source to kitty, after §98 had filed it as contour's. An index
  entry names a sequence; it does not name an author.
- **contour's buffer capture** (`contour-terminal.org/vt-extensions/buffer-capture/`) is
  `CSI > Pl ; Pr t`: `Pl` picks logical or visual lines, `Pr` how many, and the reply is the screen's
  text in a run of `PM 314 ; <data> ST` strings ending in an empty one. The page carries **no security
  note at all**, which is worth recording next to §60's argument for allowing DECRQCRA.
- **contour's semantic block query** (`contour-terminal.org/vt-extensions/semantic-block-query/`) is
  the one that reads the OSC 133 zones back out: DEC mode `2034` arms it and the terminal answers with
  `DCS > 2034 ; 1 b T1 ; T2 ; T3 ; T4 ST`, four uint16s that every later query must echo;
  `CSI > Ps ; Pn ; T1 ; T2 ; T3 ; T4 b` then asks for the last block (`1`), the last N (`2`) or the
  one in flight (`3`), and the reply is JSON carrying **command, prompt, output, exit code**, with
  control characters escaped so the payload cannot break its own terminator. The token is the vendor
  agreeing that this is dangerous — and it defends only against a stream that did not see the enable
  reply, since the token travels the same wire.
- **contour's colour-scheme notification**
  (`contour-terminal.org/vt-extensions/color-palette-update-notifications/`) gives the sequence §98
  implements: `CSI ? 996 n` asks, `CSI ? 997 ; 1 n` is dark and `; 2 n` light, and DEC mode `2031`
  turns the same report into an unsolicited one. Its implementer list is the reason this row moved and
  the ones around it did not — contour, ghostty, kitty and **GNOME's vte** send it; neovim, helix,
  zellij and tmux ask it.
- **otty's OSC tree** (`docs.otty.sh/vt/osc/`) supplied the colour codes xterm defines and this table
  had never listed: `OSC 5` / `105` / `106` (the "special" colours, slot `0` bold, `1` underline, `2`
  blink, `3` reverse, `4` italic), `13` / `14` mouse pointer, `15` / `16` / `18` Tektronix, `17` / `19`
  selection, and the resets `113`–`119` that pair with them. It is also the only catalogue of the four
  that marks its own **partial** support — "parsed, not applied", "stored, not rendered" — which is the
  distinction §66 retired a mark for and is worth seeing somebody else keep.
- **otty's `OSC 88`** (`docs.otty.sh/vt/osc/osc-88`) is a **proposal, not a sequence in use**: the
  Terminal Resume Protocol, `OSC 88 ; <op> [ ; key=value ]… ST`, where `arm` hands the terminal a
  base64 `cmd`, `args` and `cwd` to **relaunch the program with** if the terminal restarts, `clear`
  withdraws it and `query` answers `OSC 88 ; supported ; v=<max> ST`. otty ships the reference
  implementation and the spec is open at `github.com/Otty-sh/osc-88`. It is the only sequence found in
  this sweep whose intended effect is a **local process**, and the row for it says so.
- **otty's CSI tree** (`docs.otty.sh/vt/csi/`) and **vtdn's CSI category**
  (`vtdn.dev/docs/category/csi-sequences/`) added no row at all — every sequence either was already
  here or arrived through contour's index. Two catalogues agreeing with the table is worth recording
  as evidence too, since it is what says the additions above are contour's breadth rather than this
  table's blindness.

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
  the prompt marks and the image events of a chunk into one offset-ordered list (`interruptions`, since the
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
  `modkeys`: since §111 it holds no state machine of its own at all, only the two shared framers
  (`csi::Framer`, `dcs::Framer`) and the tables that say which question a finished sequence asked —
  **XTVERSION** (`CSI > q`), **DA3** (`CSI = c`), **DECRQSS** (`DCS $ q <sel> ST`; `m` → `Sgr`, every
  other selector `Unsupported`) and **XTGETTCAP** (`DCS + q <hex>[;…] ST`). Both private-CSI queries
  answer only in their default parameter form — the shared `default_params` predicate (absent, or a
  single zero), since a non-zero param on the same final byte is a different private sequence; both DCS
  queries insist on no parameters at all. Sixel data cannot masquerade as a query because an ESC is the
  only thing that can interrupt a control string and an ESC ends it for the engine too (§111, measured),
  and `MAX_DATA` bounds what a hostile stream can make this scanner hold (§12). Reply builders `version_reply` /
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
- **`term/dsr.rs`** — the DEC-private DSR scanner (§82), and the one query answerer that is **not** in
  `query.rs`. `Dsr::feed` is the same chunk-safe CSI machine `term/tabs.rs` uses — marker, parameters and
  intermediates kept apart so a near miss is rejected rather than mistaken — and reports offsets ONE PAST
  the final byte. `is_cursor_request` is the allow-list: final byte `n`, marker `?`, no intermediates, and
  a **sole** parameter equal to 6, a second parameter ruling the sequence out rather than being ignored
  (a deliberate tightening over `tabs.rs`, DSR taking exactly one `Ps`). `cursor_reply(row, col)` is the
  pure formatter — `CSI ? row+1 ; col+1 R`, xterm's two-parameter form with no page — and the `+ 1` is
  the engine's own arithmetic from `device_status`, copied so the ANSI and DEC spellings of one question
  cannot disagree, origin-mode divergence included. `term/mod.rs` answers inside the interruption loop
  (`Interruption::Dsr` → `report_cursor_position`), reading `screen().cursor_position()` with the engine
  advanced exactly to the sequence and pushing the bytes into the same `replies` buffer the engine writes
  into — the path `rect.rs`'s checksum already took, and the reason two questions in one write come back
  in the order they were asked.
- **`term/sgrstack.rs`** — XTPUSHSGR / XTPOPSGR (§85), the same chunk-safe CSI machine again, matching
  the `#` intermediate with no private marker so DECSTR (`! p`), DECRQM (`$ p`) and DECSCUSR (`SP q`) are
  each one intermediate away and left alone. `Mask` turns xterm's eleven parameter values into cmote's
  own bitset once; an unrecognised value is ignored and the rest of the list applies (§59's DECCARA
  rule), an unreadable one drops the sequence, and a pop carrying any parameter at all is not ours.
  The state that outlives a chunk is in `term/mod.rs`: `saved_pens`, ten deep (`sgrstack::DEPTH`), and
  `dropped_pushes`, which is the one deliberate departure from xterm — an overflowing push is dropped
  there too, and counting it lets the matching pop be dropped with it so the levels below stay paired.
  `apply_sgr_stack` reads `cursor.template` on a push and, on a pop, feeds `merged_pen`'s SGR string
  back through `parser.advance`: the engine remains the only writer of its own template (§71, §73), and
  fed bytes bypass every scanner so this cannot feed itself. `pen_restore` is deliberately **not**
  `pen_sgr` — the DECRQSS reply reports every underline substyle as a plain `4`, which is honest as an
  answer and lossy as a restore, so this one emits `4:3` / `4:4` / `4:5` and SGR 58's underline colour.
  `protect::is_protected` is read across the restore and `set_pen_protection` puts it back, the opening
  `CSI 0 m` otherwise assigning the borrowed DECSCA bit (§56) along with the flag word it lives in.
  The scanner reads `ESC c` as well (§86) and the stack is emptied on it, counter included — the same
  byte `term/scp.rs` reads for its own store, read again here rather than borrowed, so neither module
  depends on the other's idea of where a sequence sat. DECSTR is deliberately not a reset for this,
  which is the split DECSACE already has.
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
- **`term/graphics.rs`** — the image scanner and store (§41). `Images::feed` holds no state machine of
  its own since §111: `dcs::Framer` cuts out the sixel strings (`ESC P <params> q … ST`, with BEL and
  8-bit ST accepted) and the RIS, `csi::Framer` the `CSI 2 J` / `CSI 3 J` erases, and what is left here
  is `is_sixel` / `is_reset` / `erase` deciding what each one means. Its two event kinds report
  **opposite** offsets on purpose: a picture past
  its DCS (the cursor is only right once everything before it has been drawn), an erase *before* its
  sequence (`CSI 3 J` drops the engine's history, so which placements it takes must be decided first).
  `Placement` carries the absolute `line`, `col`, the reserved `rows`/`cols`, the pixel `width`/`height`
  and an **iced image handle** — minted once at decode so the renderer's texture cache keys off a stable
  id instead of re-uploading every frame. Caps: `MAX_PAYLOAD` 16 MiB per picture (past it the string is
  abandoned and nothing decoded), `MAX_IMAGES` 64 and `MAX_TOTAL_BYTES` 64 MiB, evicted oldest-first;
  `clear_screen` / `clear_scrollback` split on the first visible line, `clear` takes everything.
  The erase's parameter is read as a **number** since §106, with the engine's own `next_param_or(0)`
  default: it used to be compared byte-for-byte against `b"2"` and `b"3"`, which agreed with the engine
  on the one spelling everything sends and on no other — `CSI 002 J` and `CSI 2;5 J` both erase the
  screen there, and the pictures stayed on it. The run itself now comes from `csi::Params`, whose
  16-byte predecessor abandoned a padded erase for the same reason and to the same effect. A mid-sequence
  control byte is read through as well (`csi::passes_through`), since `CSI 0` LF `2 J` erases the screen
  as far as the engine is concerned.
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
  without an enumerated deny-list. `MAX_PAYLOAD` = 4096 is deliberately far below an `iTerm2 File=`
  payload: refusing to buffer megabytes of base64 is the cheapest way to mean §41's refusal, and it is
  the **second** mechanism behind that key rather than the only one, since the allow-list would decline
  `File` at any size. Both are asserted by the one test:
  `refuses_the_inline_image_key_without_even_buffering_it` feeds `MAX_PAYLOAD + 1` bytes. That pair is
  why the row is a 🛑 as of §70 and was an ❌ before it. The dangerous keys
  (`Copy`, `SetProfile`, `SetColors`, `SetBackgroundImageFile`, `StealFocus`, `RequestAttention`,
  `ClearScrollback`, `File`) each have a test asserting they produce nothing, and since §71 so do the
  two harmless ones (`CursorShape`, `ReportCellSize`) — the module header carries their reasons, and
  the boundary tests live where the effect would have been: `term/screen.rs` for the shape that must
  survive, `term/mod.rs` for the reply that must not go out. `SetMark` is applied
  through `Terminal::process`'s interruption advance into `osc133::Prompts::record_user_mark`, kept in a ring
  separate from the prompt marks — a bookmark has no command state, exit code or output span, so
  `output_at_prompt` must never resolve one — surfaced by `Terminal::user_mark_rows` and drawn as an
  amber gutter tick (`ui/grid.rs`), while `jump` chains both rings so Ctrl+Shift+Up/Down visits either.
  `CurrentDir=` is handled in `term/cwd.rs` instead, beside the two cwd spellings it duplicates.
  `SetUserVar=` honours the single name `gitBranch` — the allow-list applied to names as well as keys,
  which is what means there is no remote-keyed map to bound. `parse_user_var` is three-valued so the
  three cases stay distinct: not an assignment (keep what we hold), an EMPTY value (the shell left the
  repository — clear it), a value fit to draw. A bad base64 or non-UTF-8 payload lands in the first
  case, so rubbish cannot wipe a real reading. The value is strip-and-capped on the way IN by
  `osc::sanitize` at `MAX_VALUE_CHARS` = 32 — a local copy until §69 gave that rule a second caller and
  moved it to the shared module. Surfaced by `Terminal::branch` and drawn by
  `ui/tabs.rs` as a dim pill AFTER the endpoint label — remote-chosen text in cmote's own chrome must
  not be able to pass for the label that says which machine the user is typing into.
- **`term/osc.rs`** — the shared OSC framer (§17, §34, §54, §55). One chunk-safe byte machine
  (`Text`/`Escape`/`Payload`/`PayloadEscape`) recognising `ESC ] payload (BEL | ESC \)`, calling back
  once per completed payload with the byte offset **just past its terminator** — the coordinate §34
  needs to line a mark up with the grid, and which §17 and §54 ignore. `Framer<CAP>` takes its payload
  cap as a const parameter, so each scanner keeps deriving `Default` and keeps its own limit named in
  its own module (`cwd` 4096, `osc133` 512, `progress` 128, `icon` 512); past the cap the payload is
  abandoned and framing resumes (§12). This replaced three copies of the same machine that had already
  drifted. It used to say `graphics.rs` could not share it — a 16 MB binary payload whose overflow must
  keep scanning to the real terminator, "a different policy, not a different number" — and §111
  measured that: it is not a different policy either, because the only byte that can interrupt a
  control string is an ESC and an ESC ends it for the engine too, so following an overlong payload to
  its terminator and abandoning it on the spot are the same machine. `graphics` reads `dcs::Framer` now.
  **Four rules here are the engine's and three of them were nobody's** until §111: a byte the engine
  reads through between the ESC and the `]` keeps the sequence; an ESC ends this string and OPENS the
  next one (a second OSC starting inside the first used to be lost entirely); CAN and SUB end the
  string, where this framer read them into the payload and went on waiting, so a later BEL handed a
  caller a cancelled path with everything after it glued on; and the C0s the engine DROPS on the way
  in (`lib.rs:349`) are not part of the payload. On the cancel cmote is deliberately the stricter side —
  the engine dispatches what it had, cmote abandons it — which is safe here and nowhere else, because
  the engine has no handler behind any OSC cmote reads.
  Since §69 it also holds **`sanitize(text, max_chars)`**, the strip-and-cap every scanner needs before
  remote-chosen text is drawn in cmote's own chrome — control characters filtered, length capped in
  `chars` so a multi-byte name cannot be cut mid-codepoint. Moved here from `iterm.rs` when `icon.rs`
  became the second caller, which is this document's own "one adapter is a hypothetical seam, two is a
  real one" applied to the module the drift had already been found in once. Note what it does *not*
  have to cover on the scanner path: an ESC inside a payload either ends the OSC string or invalidates
  it, so `Framer` settles that byte before `sanitize` is reached — the strip is for the rest, and for
  any caller whose text did not come from a framed payload.
- **`term/icon.rs`** — the icon name a remote sets (OSC 1, §69). `Icon::feed` runs on the shared framer
  and keeps a latest-value `Option<String>`; `parse` matches the payload's `1;` prefix **whole**, so
  `10;` / `11;` / `12;` / `104;` / `110;` / `112;` and `1337;` cannot be mistaken for it — the last
  mattering most, since that namespace is one cmote actually reads. The name is trimmed, control-stripped
  and capped at `MAX_NAME_CHARS` = 24 on the way IN, the number chosen against `ui/tabs.rs`'s
  `MAX_LABEL_CHARS` = 48 so the usual `user@host — name` fits without eliding. An empty or
  all-control name **clears** it rather than drawing an empty suffix, which is how a program hands the
  chip back when its command ends. Surfaced by `Terminal::icon_name` (a borrow, not a clone — the scanner
  is a plain field with no lock in front of it) and drawn by `App::Tab::strip_label` **after** the
  endpoint, the §55 rule the branch pill carries. `vte` has no OSC 1 arm at all, so this scanner is the
  only thing in the stack that ever sees the sequence. It also performs part 6's refusal of the icon half of
  `OSC 0` by not matching `0;`. Parse-only, no engine, no widgets — fully unit-tested, and the two
  refusal tests were pinned by making `parse` accept `0;` and watching exactly those fail.
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
  **active** tab only (`taskbar.rs`). Parse-only, no engine, no widgets — fully unit-tested. Since §79
  `feed` also asks `term::notify` about every payload **before** reading one, and returns on a match:
  this module is where cmote performs the desktop-notification refusal, because it is the one already
  fed every OSC payload in the stream.
- **`term/notify.rs`** — the desktop-notification refusal, stated (§79). Not a scanner: one pure
  function, `refused(payload) -> Option<Spelling>`, naming the three dialects of a single decision —
  ConEmu's `9;<text>`, urxvt's `777;notify;…`, kitty's `99;…`. It keeps nothing and answers nobody,
  because there is nothing to keep: **it changes no behaviour**, and the rows it backs say so. Its
  whole value is that the refusal is now performed by cmote's own code and checkable by name, which is
  the difference §63 had to make on the OSC 52 row. The load-bearing detail is the *exclusion*: the two
  OSC 9 sub-codes cmote honours are named here with their trailing `;` — the same prefixes
  `term/progress.rs` and `term/cwd.rs` strip — so the classifier and those two can never disagree about
  which payload belongs to whom, and a future tightening cannot silently take progress (§54) or the
  working directory (§17) with it. Matching is on the whole numeric field, never a prefix of it, so
  `990;` and `999;` are not read as kitty's `99;`. urxvt's `777` is a dispatcher and only its `notify`
  module is this decision; another module is unimplemented rather than refused, which is a different
  row and a different mark. Six tests, none of which needs a terminal.
- **`term/protect.rs`** — the selective-erase scanner (§56), and the one place cmote writes *inside* the
  engine's cells. A chunk-safe CSI state machine (`Protect::feed`) reading **DECSCA** (`CSI Ps " q`),
  **DECSED** (`CSI ? Ps J`), **DECSEL** (`CSI ? Ps K`), plus **RIS** as a protection clear, **DECSTR** as
  the whole soft reset since §72 — the engine performs RIS itself, so only the borrowed bit is cmote's
  business there, while `CSI ! p` reaches no engine arm at all and is cmote's entire responsibility —
  and, only while the pen is armed, every **SGR**, since `Attr::Reset` assigns the whole flag
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
  tested without a terminal. The parameter run comes from `csi::Params` since §106, which fixed the
  worst bug this scanner has had: it used to cap the run at 32 **bytes** and abandon the sequence past
  them, so an SGR of 33 parameter bytes — `CSI 0;1;2;3;4;5;7;9;21;30;31;38;5;196m`, which true colour
  reaches with room to spare — was dropped here while the engine applied it, `Attr::Reset` and all.
  Nothing put the borrowed bit back, so **protection was silently lost across a long SGR**. `first_param`
  now saturates on overflow the way the engine does and still refuses a non-digit, which is the
  distinction that matters: a number past `u16` reads the same on both sides, while a field that is not
  a number at all is §54's malformed input and has to stay a no-op. A mid-sequence control byte no longer
  ends the sequence here either (`csi::passes_through`): `CSI 0;` LF `1 m` is an SGR the engine applies,
  `Attr::Reset` included, so giving up on it lost the borrowed bit exactly as the parameter cap did.
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
  `erase_rect` (the pen's background, protection honoured only for DECSERA), `fill_rect` (the pen
  cloned with a new glyph), `copy_rect` (source read out whole first, because the overlapping case is
  the point of DECCRA) and `apply_rectangle`, which **refuses every one of them while `TermMode::ORIGIN`
  is set** — a `ponytail:` limit, since DECOM makes the corners region-relative and the engine's
  `scroll_region` has no accessor.
- **`term/rect.rs`, the attribute half** (§59) — the same module grew three more sequences the engine
  drops: **DECCARA** (`$ r`), **DECRARA** (`$ t`) and **DECSACE** (`* x`). `vte` matches `*` in no CSI
  arm at all and `$` only in the two DECRQM spellings, so all three fall through unhandled. DECSACE is
  a mode and is **absorbed by the scanner**, which is the one place that sees it and the requests it
  governs in stream order; each attribute request therefore leaves carrying its own `RectExtent`, and
  `term/mod.rs` never holds the mode. The extent is a parameter of `area` rather than something it
  reads, because it changes a *rule*, not a walk: a rectangle whose right corner is left of its left
  one is undrawable, while the same numbers as a stream are an ordinary run round the wrap. Selector
  lists fold to an `AttributeChange { on, off, flip }` at parse time — later wins, as in an SGR — and
  `AttributeChange::apply` is pure, so the whole table is tested without a terminal. `term/mod.rs` holds the
  one translation to engine names (`RECT_ATTRIBUTES`) and `attribute_rect` sets **named bits one at a
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
  interruption point — so it answers from the page as it stood where the question sat, and orders correctly
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
  request happened to sit. **Until §102 the parameter count was the only evidence available**, since
  DECLRMM (mode 69) — the mode that disambiguates the byte on a real VT420 — was one the engine never
  accepted, so every parametrised `s` was cancelled on that guess. cmote holds mode 69 now, so the
  scanner reports the sequence and its two numbers and `process` decides: with the mode set it is
  DECSLRM and the byte is still cancelled, without it the byte is a save-cursor and is let through. The
  bare `CSI s` never reaches the decision, and every save-cursor in the wild is that spelling. Offsets
  name **the final byte itself**, a third convention next to a prompt mark's start-of-sequence and a
  selective erase's one-past-the-end, because that byte is the one being replaced: `process` advances
  the engine up to it, feeds **CAN** (0x18) in its place and resumes after it. Feeding nothing would
  leave the engine's parser mid-CSI, taking the next final byte in the stream as this sequence's —
  `CSI 5;70 s` then `hello` would dispatch `('h', [])` with parameters 5 and 70. CAN because the ANSI
  state machine defines it as the cancel (`anywhere()` → `execute`, state Ground, no dispatch), rather
  than SUB, which is *defined* to be displayable, or a final byte that merely has no arm today.
  Since §106 the parameter run carries **no cap at all**, which is the one scanner here where that is
  the safe answer: nothing is buffered — two `u16`s and two counters, whatever arrives — so §12's rule
  about unbounded remote input has nothing to bite on, while a cap meant abandoning a padded DECSLRM
  the engine went on to dispatch as a save-cursor. What rules the sequence out is its shape. A stray
  byte no longer ends the sequence either (`csi::passes_through`): the engine RUNS a mid-sequence C0
  where it sits and carries on, so `CSI 5;` LF `70 s` is a margin request there — and this scanner,
  the thing that would have stopped it reaching save-cursor, had stopped reading. Only CAN and SUB
  cancel a sequence, which is the same fact as cmote feeding CAN in place of a refused final byte.
- **`term/csi.rs`** — the facts every CSI scanner has to agree with the ENGINE about (§106), and since
  §111 the shared CSI framer itself, which `osc.rs` was the template for: **every** scanner under
  `term/` reads `csi::Framer` and keeps only the part that is theirs, deciding what a sequence MEANS.
  (The count is deliberately not stated here — it has gone stale four times and now lives in one place,
  the module header, with the check that settles it: `framer: super::csi::Framer`, one per scanner.) A
  scanner's limits are not a
  private choice: cmote and the engine read the same bytes, so wherever the two disagree about whether a
  sequence is well formed, one acts and the other does not — and two of those disagreements were live
  defects. `MAX_PARAMS` 32 is `vte`'s own (`params.rs:5`, the `is_full` test at `:49-51`, `ignoring` set
  at `lib.rs:454-517`, the sequence then dropped at `ansi.rs:1545-1548`), and exceeding it means
  abandoning the sequence *because that is what the engine does with it*. `MAX_DIGITS` 5 is **not** a
  limit the engine has — it folds every digit in with `saturating_mul` and never gives up on a run — so
  the run is CLAMPED rather than capped: digits past five SIGNIFICANT ones are dropped and the sequence
  lives, which is the engine's saturated answer (five digits already reach past `u16::MAX`) while
  bounding what a hostile stream can make cmote hold to under 200 bytes. Leading zeros cost nothing;
  counting them was the first attempt and read `CSI 000…002 J` as 0. `Params` holds the run for every
  scanner now, and keeps `started` apart from `bytes.is_empty()` because a dropped leading
  zero would leave the latter true — and a caller reads that as "a private marker is still legal here",
  which would make `CSI 0?J`, a sequence the engine drops outright, classify as a selective erase. It
  also reports a **sub-parameter** (`Csi::sub_parameters`) rather than acting on one, because the
  policy points two ways: `graphics` must READ `CSI 2:3 J`, which the engine erases on, and `rect` must
  REFUSE `CSI 2:3;5;7$z`, which the engine does nothing with (§111).
- **`term/dcs.rs`** — the same job for the OTHER door: `ESC`, and everything through it that is not a
  CSI (§111). One grammar for the DCS control strings (`query`'s DECRQSS and XTGETTCAP, `graphics`'
  sixel payloads) and for the two-part escape sequences, of which **RIS is the only one anything in
  cmote acts on**. It replaces six hand-rolled copies: two full DCS machines and four
  `after_escape && byte == b'c'` watchers, and every one of the four was wrong about `ESC` LF `c`, a
  reset the engine really performs. The introducer is the CSI grammar over again — marker, params,
  intermediates, final byte, with the same two refusals — so `csi::Params`, `csi::MAX_INTERMEDIATES`,
  `csi::passes_through` and `csi::Span` are all shared rather than restated. What is NOT shared is a
  policy: the payload cap is a const parameter (`osc::Framer`'s pattern), 256 for a selector and 16 MiB
  for a photograph, and a cap of 0 is the escapes-only framer the RIS watchers want. Three rules
  measured off `vte`'s own state table and none of them previously obeyed: **CAN and SUB end a string**
  (both old machines read them into the payload and went on waiting), **DEL and the high bytes are
  discarded rather than kept** in the payload, and a byte the engine reads through in the INTRODUCER
  keeps the string. BEL is accepted as a terminator, which is the one deliberate leniency — the engine
  reads it as a payload byte. What it does NOT frame: CSI, OSC and the SOS / PM / APC strings, each
  dropped back to ordinary text rather than followed, which is safe because an ESC is the only thing
  that can interrupt a control string and an ESC ends it for the engine too — the same fact that lets an
  abandoned string need no state at all.
- **`term/differential.rs`** — test-only, and the answer to how §106's defects were found (§106). Three of
  them came from reading `vte`'s source by hand and noticing that a constant counted the wrong thing,
  which neither scales nor survives a version bump. This module drives the **actual parser the engine is
  built on** — `alacritty_terminal` re-exports `vte`, so no new dependency — records what it dispatched,
  refused (`ignore`) and executed, and compares that against what cmote's scanner made of the same bytes.
  It found the fourth defect on its first run: the parser runs a mid-sequence C0 and carries on with the
  sequence around it, where every scanner gave up — fixed in the three the engine has an arm for, and
  then in the other five on a self-consistency test, since a stray byte must not change cmote's own
  verdict either. Thirteen tests: agreements pinned from both sides, and
  three **generated sweeps** — every spelling of a sequence (padding, and a read-through byte at each
  position), the same verdict when the bytes arrive one at a time, and 5760 shapes over marker ×
  intermediates × parameters × final byte × the ORDER of those parts. The sweeps found the last two
  defects, one of them only after an axis was added: the shape generator emitted the parts in the legal
  order alone, so it passed against the very guard it was written for until a malformed interleaving
  was generated too. §111 grew it to **twenty-five** tests as the first ten scanners
  migrated: 6720 CSI shapes, a second sweep over the DCS introducer — 180 shapes then, 240 since §154
  added the introducer a tmux passthrough opens with — a payload compared
  byte-for-byte against what the engine's own handler was given, and the ESC and ST rules that were
  wrong in all three framers — an ESC that terminates no control string still OPENS the next sequence
  for `vte`, which is why a payload cannot smuggle a CSI past a DCS-unaware framer, and why a second
  OSC starting inside the first has to arrive. What it
  does NOT cover: it compares the parser rather than the handler, which is
  the gap `term/gatediff.rs` below now fills; and `MAX_INTERMEDIATES` is 4 here — one place since §111,
  `csi::MAX_INTERMEDIATES` — against the engine's 2, which both sides refuse but by different routes.
- **`term/gate.rs`** — the one place cmote sits **between** the parser and the engine (§102), and the
  odd one out in this whole list: every other module here reads the byte stream a second time and acts
  beside the engine, which is why none of them can break what the engine does. This one implements
  `Handler` itself, holds `&mut Engine`, and is passed to `Processor::advance` in the engine's place. It
  answers about two dozen methods and forwards the rest through a `forward!` macro — generated rather
  than hand-written because sixty-odd near-identical bodies are sixty-odd chances to pass the wrong
  argument, while a wrong *signature* does not compile. `#[deny(clippy::missing_trait_methods)]` on the
  impl is what makes the whole route usable: a method left out of the list, today's or a future
  version's, is a build error rather than a silent no-op. Two traps it has to be read for — the engine's
  `newline` calls its OWN `linefeed` and `carriage_return`, and `goto_col` its own `goto`, so neither can
  be forwarded once the gate has replaced what they call.
- **`term/gatediff.rs`** — test-only, and the other half of the harness above (§102, §106). The gate
  re-implements a dozen of the engine's own `Handler` methods so margins can bound them, which is the
  most dangerous thing in this directory: every other module here acts *beside* the engine and cannot
  break it, while this one stands in its place. Each of those arms opens with `if !self.narrowed()` and
  forwards, so with no margins the gate must be a **pass-through** — checked by building a SECOND
  engine with the same config and no gate in front of it, feeding both the same stream, and comparing
  the whole document: every cell's character, attribute bits and both colours, the cursor, and the
  scrollback depth. Fifteen hand-picked margin-free streams, then the same fifteen again behind a
  full-width `CSI ? 69 h` `CSI 1 ; 10 s`, because a band at the page edges is not a band. The oracle is
  **measured**, not transcribed, which is what separates it from `term/region.rs`'s tests.
  Since §111 those fifteen are joined by a **generated** corpus — every hand-written gate arm x eight
  arrivals x four scrolling regions, 832 streams, run both ways — and by a check that reads `gate.rs`'s
  own source so an arm added there without a stream here fails a test. The generator is not decoration:
  deleting `insert_blank`'s `!narrowed()` guard leaves all fifteen hand-picked streams green and fails
  24 generated ones, because ICH was never among the fifteen. Two of its axes exist because a mutation
  got past it without them — how the cursor ARRIVES (parked by CUP, which clears the pending wrap,
  versus written to the end of a row) and a glyph AFTER the operation, since state an operation leaves
  behind is not in the document until something has to be drawn. The paths cmote acts alone on (§56,
  §58, §41, §72, §74, §85) cannot go to the oracle at all, and are compared cmote-against-cmote instead,
  plain against a full-width band: the scanners then cancel out and what is left is the gate's own
  `narrowed()` decision. The
  margins-ON rows have no oracle — the engine has no left margin to ask — so those two properties are
  a reading of xterm's definition and are labelled as such where they are declared: a band operation
  moves only the columns between the margins and leaves the cursor inside them (ten operations that act,
  each compared against a photograph of the page taken before), a row leaving a narrowed band is
  **discarded rather than filed in the scrollback** (depth AND history contents, which is why the page is
  built with three lines already scrolled off), and an operation the gate REFUSES because the cursor is
  outside the band changes **nothing whatsoever** — the one property where the whole document must come
  back identical, and the one the first version of this module got wrong: the refusals sat in the acting
  list, where an assertion about the columns outside the band was satisfied for free, and both
  `cursor_in_band()` guards could be deleted with every test still green. Five of the six properties are
  proved able to fail by mutation before being kept — a disabled `!narrowed()` guard costs two scrollback
  lines, a `narrowed()` widened by one column diverges on five streams (two only in the `WRAPLINE` flag),
  a `scroll_band` given the whole width disturbs 238 cells, and the two deleted `cursor_in_band()` guards
  let both refusals corrupt cells inside the band; deleting `shift_cells`'s cursor test instead PANICS on
  `right - cursor + 1`, so that one guards the arithmetic too. The sixth, the cursor's exact landing place,
  is arithmetic on a known page.
- **`term/region.rs`** — the engine's vertical scrolling region, mirrored (§102). `Term::scroll_region`
  is private with no accessor and no reply arm, which cost §100 a refusal and would have cost §102 the
  whole feature. The mirror is exact by construction: the field has **four writers**, two of them
  `Handler` methods the gate sees and two of them calls cmote makes, and the arithmetic here is a
  transcription of the engine's — including the case where a zero top leaves the region starting above
  the first row, mirrored rather than clamped so the two cannot part company.
- **`term/margins.rs`** — the left and right margins and the deferred wrap (§102). Pure state and
  arithmetic, no engine types, so the band rules are unit-tested with no terminal: where a column lands
  under origin mode, how far a cursor may travel, what an omitted or zero parameter means, and which
  requests are refused outright. Everything keys off **narrowed** rather than **enabled** — with the
  band at the page edges the engine keeps every operation, so an ordinary session runs on the code it
  ran on before §102 existed. It also holds cmote's own pending-wrap flag, taken off the engine because
  the engine fires its own at the screen edge and wraps to column 0 instead of to the left margin.
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
