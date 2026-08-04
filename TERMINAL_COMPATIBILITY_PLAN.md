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

**Update trigger.** This document tracks only the *terminal* surface — `src/term/` and
`src/ui/grid.rs`. Update it when, and only when, a change touches those: a query newly answered, a
mode newly honoured, a sequence newly rendered, or an engine bump that moves the ceiling. Editor,
files-pane, and window-chrome work does **not** belong here. When terminal work does land, the edit
is two places: the gap list in §2–§5 and the matching row in the §8 matrix.

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
colour scheme, its cell pixel size) and the three identity queries the engine drops — XTVERSION,
DECRQSS and XTGETTCAP, sniffed from the stream by `term::query` (§33). The old `term::compat` (cursor-move rewriter) and
`term::answer` (reply synthesizer) modules were **deleted** in the swap — the engine does both.

Effort is now just *where the work lives*, not a hard wall:

- **[keymap]** — cmote's input encoder (`term::keymap`); engine-independent.
- **[reply]** — extend cmote's reply path (the `Replies` listener in `term::mod`, or the
  `term::query` stream scanner) for a query the engine does not answer itself — the route §33 took
  for XTVERSION / DECRQSS / XTGETTCAP.
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
  underline / bar / hollow), steady — cmote runs no animation timer, so blink is dropped.
- **Keeps 10 000 lines of scrollback** with a thin, read-only scroll indicator (§23 Stage 8):
  the wheel and Shift+PageUp/PageDown/Home/End scroll the history, and typing snaps back to the
  live bottom. The alternate screen keeps no history, so scrolling is inert there by design.
- **Lets the engine interpret** the whole VT stream, no cmote papering-over: the **DEC
  line-drawing charset** (older programs box-draw with it), **origin mode** (so cursor reports
  are origin-correct), **custom tab stops** (HTS / TBC), the **autowrap toggle** (DECAWM),
  **REP** repeat, the (vertical) scroll region, and **alternate-screen** switching.
- **Answers host queries.** Primary / secondary **DA** (`CSI c`, `CSI >c`), **DSR** status and
  cursor-position (`CSI 5n`, `CSI 6n`), and **DECRQM** request-mode are answered by the engine.
  The **colour queries** (OSC 10 / 11 / 12 and OSC 4 palette) and the **pixel / text-area
  size** reports (`CSI 14t`, `CSI 18t`) are answered by cmote's listener from its own colour
  scheme and cell metrics — so a program probing the background to pick a light-vs-dark theme is
  answered rather than left guessing. The three **identity queries** the engine drops — XTVERSION
  (`CSI > q`), DECRQSS (`DCS $ q … ST`) and XTGETTCAP (`DCS + q … ST`) — are sniffed from the stream
  and answered by cmote itself (`term::query`, §33), so a program fingerprinting the terminal or
  reading back its SGR no longer stalls on a dropped query.
- **Shows the window title** a program sets with OSC 0 / OSC 2 in the title bar (§23).
- **Tracks and honours modes**: application-cursor DECCKM (arrows → SS3), bracketed paste
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
- **Reads OSC 133 shell-integration marks** (§34) for a per-tab command-status dot and
  jump-to-prompt (Ctrl+Shift+Up/Down), scanned out of the stream by `term::osc133` — the same
  tactic as `cwd`, but with the mark's grid line captured by splitting the engine advance at it.

---

## 2. Still open — input (all `[keymap]`, engine-independent)

Both `modifyOtherKeys` (§1) and the **kitty keyboard protocol** (§25, shipped) are now **done**.
What is left:

| Missing | What it unblocks | Tag | Src |
|---|---|---|---|
| **DECKPAM application keypad** — the numpad should send `ESC O p…y` while an app enables app-keypad mode | *near-nil value on a PC client* — see the note below | [seam+keymap] | [DEC] |

Kitty shipped by flipping the engine's `kitty_keyboard` config flag on — so the engine tracks the
push/pop/query stack and answers `CSI ? u` itself — and writing the key-press → `CSI u` encoder
(`term/kitty.rs`), the flag set read off the seam (`Screen::kitty_flags`). The one input gap that
remains is DECKPAM.

**Why DECKPAM is deprioritised.** cmote already mirrors xterm's default `numLock: true`
behaviour: with NumLock on the numpad sends its digit (the `pm2 ls` fix, keyed off the OS
producing `text`), with NumLock off it is navigation following DECCKM. Every ncurses full-screen
app sets DECKPAM as part of terminfo `smkx`, so *honouring it for the number keys would divert
NumLock-on digits to `ESC O q…y` inside vim / less* — re-breaking the exact digit-typing the
`pm2 ls` fix protects. The only genuinely safe DECKPAM wins on a PC are NumpadEnter → `ESC O M`
and the operators `+ - * /` → `ESC O k/m/j/o` (no NumLock ambiguity), which is tiny value. So
the "small, like DECCKM" framing does not hold here; it is parked below the higher-value items.

---

## 3. Still open — query → reply (niche; `[reply]`)

The high-value query class is closed. DA / DSR / DECRQM are answered by the engine; the colour
and pixel-size queries by cmote's listener; and **since §33** the three identity queries the engine
drops are answered by cmote's own stream scanner (`term::query`), the same out-of-band tactic
`cwd` / `modkeys` use for sequences the engine ignores:

- **XTVERSION** (`CSI > q`) → `DCS > | cmote(<ver>) ST` — full, a truthful name and build version.
- **XTGETTCAP** (`DCS + q <hex> ST`) → states only the two caps cmote can give truthfully —
  terminal name `xterm-256color` and 256 colours — and answers every other capability an honest
  unknown (`DCS 0 + r <name> ST`).
- **DECRQSS** (`DCS $ q <sel> ST`) → reports **SGR** from the live pen (the exact attributes the
  grid paints, rebuilt after the chunk advances so a set-then-query in one write is seen), and
  every other setting an honest `ps=0` (`DCS 0 $ r ST`) rather than a lie about state cmote renders
  fixed or cannot read.

What no layer answers, both low value:

| Missing | What blocks on it | Reply shape | Tag | Src |
|---|---|---|---|---|
| **DA3 tertiary** (`CSI =c`) | terminal-id probes | `DCS !\|<hex> ST` | [reply] | [xterm] |
| **Answerback** (ENQ `0x05`) | legacy identification | configurable string (usually empty) | [reply] | [ECMA-48] |

The engine's `identify_terminal` handles the primary and secondary DA intermediates only — the
`=` (tertiary) intermediate is dropped — so DA3 would fall to cmote if ever wanted. Both remaining
gaps are low UX value: modern applications rely on the DA / DECRQM / XTVERSION answers that work.

---

## 4. Still open — rendering / attributes

**OSC 8 hyperlinks are now done** (§24), **including the Ctrl-hover underline** (v3.x) — the seam
surfaces the per-cell URI (`Cell::hyperlink`), Ctrl+click and a context-menu Open/Copy follow it,
`link` gates the scheme to http/https/mailto before opening, and the grid now underlines the whole
run of a link while Ctrl is held over it, so the link reveals itself before the click. What remains
here is small and low-value:

| Missing | Note | Tag | Src |
|---|---|---|---|
| **Blink** (SGR 5/6) | the engine stores the bit; cmote draws steady **by choice** — it runs no animation timer (the same call made for the cursor). Could show a static marker; deliberately not animated | [policy] | [ECMA-48] |

**OSC 133 shell-integration is now done** (§34) — the stream scanner this row once anticipated
(`term/osc133.rs`, beside the cwd tracker). It drives a per-tab command-status dot and
jump-to-prompt (Ctrl+Shift+Up/Down); prompt marks are stored as absolute line indices so they ride
the scrollback, captured by splitting the engine advance at each mark. Select-command-output (the
C→D output range) is the one piece deliberately left for later.

---

## 5. The new engine's own ceiling (`[engine-limit]`)

`alacritty_terminal` 0.26 does not parse or represent these, so they would need an engine
fork/upgrade or a scanner bolted on beside it. This is the whole of the remaining hard ceiling —
short, and only images are high value:

- **Sixel / ReGIS / kitty graphics / iTerm2 inline images (OSC 1337)** — the crate carries **no
  graphics support at all**, so this needs both engine work *and* a compositor in the renderer.
  `[DEC]` / `[vendor]`. The one genuinely high-value item here.
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
The **bell** and any remote colour *set* request are dropped for the same "no remote-driven side
effects" reason. Answering an OSC 52 read query would be an injection vector and stays out.

---

## 7. Recommendation

With the engine swap, `modifyOtherKeys`, OSC 8 hyperlinks and the kitty keyboard protocol all
done, only one cheap, self-contained keymap win remains:

1. **DECKPAM application keypad** — *not* the quick win the earlier edition claimed (see §2): on
   a PC client it is a near-no-op at best and a `pm2 ls` regression at worst, so only the
   NumpadEnter / operator forms are worth anything, and only marginally.

The **kitty keyboard protocol** (was #1) shipped as `term::kitty` + a `keymap::encode` branch,
the inverse of the modifyOtherKeys split: the engine already implements the whole control plane
(push / pop / set / query, stack, alternate-screen swap), gated behind its `kitty_keyboard`
config flag — cmote flips that on, so there is no scanner and no reply path, and reads the active
flags off the seam (`Screen::kitty_flags`) to drive the `CSI u` encoder. Disambiguate, event types
(press / repeat / release, the key-up now forwarded from iced), report-all and associated text are
encoded; alternate keys best-effort (§25). `OSC 8 hyperlinks` (an earlier #1) shipped as a seam
getter (`Cell::hyperlink`) plus the `link` module: **Ctrl+click** or a right-click **Open link /
Copy link** follows it, the scheme gated to http/https/mailto and the URI handed to a launcher
that never builds a shell command line (§24); v3.x added the **Ctrl-hover underline** — the grid
finds the pointer's link run (`link_run_at`) and underlines it while Ctrl is held, driven off the
repaints the app already emits on a hover move or a modifier change, so it needs no new plumbing. `modifyOtherKeys` (an earlier #2) shipped as
`term::modkeys` + a `keymap::encode` branch: the stream is scanned for `CSI > 4 ; p m`, and a
Ctrl/Alt main-keyboard combo is reported as `CSI 27;mod;code~` (level 2 for every combo, level 1
for the gap combos only) — kept for the programs that speak it rather than kitty. **OSC 133
shell-integration** (§4's old low-pri row) shipped as `term::osc133`: the stream is scanned for the
A/B/C/D marks, prompts stored as absolute line indices so they ride the scrollback, and the result
drives a per-tab command-status dot and Ctrl+Shift+Up/Down jump-to-prompt — the same
scanner-beside-the-cwd tactic, but with each mark's grid line captured by splitting the engine
advance at it.

The `[engine-limit]` items are the only remaining large moves, and only **images** (sixel /
kitty graphics) carry real UX value — the rest (double-height lines, left/right margins,
rectangular ops) are legacy and rare. For "support *any* documented app UX", graphics is the one
outstanding ceiling-raiser; everything else above is A-sized and engine-independent.

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
| 4 | Palette entry set / query | ⚠️ | query answered from cmote's scheme; **set** ignored (fixed palette) |
| 7 | Working directory | ✅ | cmote's own scanner (`term/cwd.rs`, §17) |
| 8 | Hyperlinks | ✅ | rendered + Ctrl-click; web/mail only (`link.rs`, §24) |
| 9 | Desktop notification | ❌ | |
| 9;4 | Progress reporting | ❌ | |
| 10 / 11 / 12 | Default fg / bg / cursor colour | ⚠️ | query answered (scheme-accurate); **set** ignored |
| 22 | Mouse pointer shape | ❌ | |
| 52 (write) | Clipboard write | ❌ | *(policy)* — remote must not poison local clipboard (§6) |
| 52 (read) | Clipboard read | ❌ | *(policy)* — remote must not read local clipboard (§6) |
| 104 | Reset palette entry | ❌ | no effect (fixed palette) |
| 110 / 111 / 112 | Reset fg / bg / cursor colour | ❌ | no effect (fixed scheme) |
| 133 | Shell integration (semantic prompts) | ✅ | scanner (`term/osc133.rs`, §34): per-tab status dot + jump-to-prompt; A/B/C/D tracked, exit code from D |
| Kitty 21 | Colour by semantic name | ❌ | |
| Kitty 99 | Rich notifications | ❌ | |
| iTerm 1337 File | Inline images | ❌ | no image rendering (§5) |
| iTerm 1337 | Marks / vars / profiles | ❌ | |
| 777 | urxvt notification | ❌ | |

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
| Primary / secondary DA | c / > c | ✅ | unblocks vim / tmux startup |
| Tertiary DA | = c | ❌ | `=` intermediate is a no-op (§3) |
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
| Keypad app / numeric | ESC = / ESC > | ✅ | tracked; not yet used for numpad encoding (DECKPAM, §2) |
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
| Sixel graphics | DCS … q | ❌ | no graphics (§5) |
| tmux passthrough | DCS tmux; … | ❌ | |

### SGR — text styling

| Attribute | Code | Status | Note |
|---|---|---|---|
| Bold | 1 | ✅ | |
| Dim / faint | 2 | ✅ | faded toward bg |
| Italic | 3 | ✅ | bundled IBM Plex Mono face |
| Underline | 4 | ✅ | |
| Slow / rapid blink | 5 / 6 | ⚠️ | tracked, **not shown** — no animation timer (§4) |
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
| Sixel scrolling | 80 | ❌ | |
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
| Sixel / kitty graphics / placeholders / animation | ❌ | cmote draws no images (§5) |
| iTerm2 inline images | ❌ | |
| Window iconify / move / resize / raise / maximize / fullscreen (CSI 1–10 t) | ❌ | *(policy)* — cmote owns its tabbed window; remote can't drive it |
| Window / position / state reports (CSI 11 / 13 t) | ❌ | |
| Text area in pixels / chars (CSI 14t / 18t) | ✅ | the two size *queries* are answered |
| Cell size (CSI 16 t) | ❌ | |
| Title stack (CSI 22 / 23 t) | ✅ | `push_title` / `pop_title` |
| **Kitty keyboard protocol** | ✅ | engine tracks the flag stack; cmote encodes CSI-u (`term/kitty.rs`, §25) |
| **xterm modifyOtherKeys** | ✅ | scanned out of the stream by cmote (`term/modkeys.rs`, §9) |
| ENQ answerback | ❌ | no answerback string (§3) |
| BEL | ⚠️ | accepted, **silent** — bell event dropped |
| BS / HT / LF / CR | ✅ | |
| SO / SI | ✅ | charset shift |

**Shape of it.** The whole legacy VT100 / xterm core is ✅ — cursor motion, editing, SGR, full
colour, alternate screen, mouse, bracketed paste, focus, DA / DSR / DECRQM, DECSCUSR, REP, the
kitty keyboard protocol, and — since §33 — the identity queries the engine dropped (XTVERSION,
DECRQSS SGR, XTGETTCAP). Most of the ❌ column is **deliberate**: no images, no remote clipboard
(OSC 52), no remote window control (CSI t), no blink animation, and a fixed colour scheme so
dynamic-palette writes are query-only. The genuine plain gaps left are the newer private modes
(2027 / 2031 / 2048), selective / rectangular editing, and left-right margins — all catalogued with
their cost in §2–§5; of the identity queries only DA3 and answerback remain unanswered.

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
- **DECKPAM**: `set_keypad_application_mode` (`term/mod.rs:2180`) — the engine tracks the
  application-keypad mode.
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

### cmote (`c:/sources/github_clemeno/cmote/src/`)

- **`term/mod.rs`** — the `Replies` listener answers the events that expect a report and drops
  the rest (`~:228-258`): `Event::PtyWrite` (the engine's DA / DSR / DECRQM / cursor-position,
  accumulated whole), `ColorRequest` (OSC 10 / 11 / 12 / 4, resolved against cmote's scheme via
  `report_color`), `TextAreaSizeRequest` (`CSI 14t`, from the grid + cell pixel size),
  `Title` / `ResetTitle` (OSC 0 / 2, sanitized). **Dropped**: `ClipboardLoad` / `ClipboardStore`
  (OSC 52), the bell, and colour *set* requests. `SCROLLBACK = 10_000`. The seam hides the
  engine types behind `Terminal` + `ScrollMotion`. Since §33 `process` also drains the `term::query`
  scanner (`term/mod.rs:142-167`): the chunk is scanned for identity queries *before* the engine
  advances, then each completed query becomes a reply — XTVERSION / XTGETTCAP from static facts,
  `Decrqss(Sgr)` from the live pen via `pen_sgr(self.term.grid().cursor.template)`, built after the
  advance so a set-then-query in one write is seen.
- **`term/query.rs`** — the identity-query scanner (§33), the same out-of-band tactic as `cwd` /
  `modkeys`: a chunk-safe byte state machine (`Queries::feed`) recognising **XTVERSION** (`CSI > q`,
  empty/zero parameter only — a non-zero param is some other private query), **DECRQSS**
  (`DCS $ q <sel> ST`; `m` → `Sgr`, every other selector `Unsupported`) and **XTGETTCAP**
  (`DCS + q <hex>[;…] ST`). An unrecognised DCS is followed to its terminator (`DcsIgnore`) so sixel
  data cannot masquerade as a query, and `MAX_PARAMS` / `MAX_DATA` bound a hostile stream (§12).
  Reply builders `version_reply` / `decrqss_sgr_reply` / `decrqss_unsupported_reply` /
  `gettcap_reply`; `known_capability` states only `TN=xterm-256color` and `Co`/`colors=256`.
  Parse-only, no engine types, unit-tested per reply shape.
- **`term/screen.rs`** — engine-agnostic view. `Cell` getters: `contents`, `is_wide`,
  `is_wide_continuation`, `fgcolor`, `bgcolor`, `bold`, `dim`, `italic`, `hidden` (conceal),
  `strikeout`, `underline` (`UnderlineStyle`), `underline_color`, `inverse`, `hyperlink` (the
  cell's OSC 8 URI, §24). `Screen` getters: `size`, `cursor_position`, `display_offset`,
  `history_size`, `hide_cursor`, `cursor_shape`, `application_cursor`, `bracketed_paste`,
  `focus_reporting`, `mouse_mode`, `mouse_encoding`, `cell`, `kitty_flags` (the five active kitty
  protocol flags, read off `Term::mode()`, §25). **Not yet surfaced**: application-keypad, blink.
- **`term/keymap.rs`** — printable + layout, Ctrl → C0, Alt-as-meta, named keys including
  **F1–F24** and the **modified named keys** (`modifier_param` computes the xterm parameter,
  `letter_key` / `tilde_key` shape the two key families), **modifyOtherKeys** (`modify_other_key`
  / `other_key_bytes` emit the `CSI 27;mod;code~` form when the level is on), the numpad NumLock
  heuristic, and the bracketed-paste terminator scrub. It now also carries an input-modes bundle
  (`Modes` — DECCKM, the modifyOtherKeys level, the kitty flags) and a `KeyEvent` (press / repeat /
  release), and **dispatches to `term/kitty.rs` whenever a kitty flag is active**, superseding the
  legacy path; a legacy release yields nothing and a legacy repeat is a press. **Absent**: DECKPAM.
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
- **`term/osc133.rs`** — the shell-integration scanner (§34). `Scanner::feed` is the same chunk-safe
  byte machine as `cwd.rs` but returns *a list* of `(offset, Mark)` — A / B / C / D, with D's exit
  code parsed from its next field. `Prompts` holds the command state (`Idle`/`Prompt`/`Running`), the
  last exit, and the prompt lines as **absolute indices** (`history_size + row`), with `visible_rows`
  and `jump` doing the viewport-row math. `process` (`term/mod.rs`) splits the engine advance at each
  mark's offset to read the cursor line there. Parse-and-arithmetic only, no engine types — the
  scanner, the state machine, and the jump/visibility math are all unit-tested with no terminal.
  Surfaced by `Terminal::{command_state,last_exit,prompt_rows,jump_prompt}`; drawn as a per-tab dot
  (`ui/tabs.rs`) and a left-gutter tick (`ui/grid.rs::prompt_tick_rect`); jumped by Ctrl+Shift+Up/Down
  (`app.rs::prompt_jump`). Marks are cleared on resize (reflow invalidates absolute lines).
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
  underline while it is the hover target.
- **Deleted in the swap**: `term/compat.rs` (the cursor-move rewriter) and `term/answer.rs`
  (the reply synthesizer) — the engine parses every spelling and answers every query they used
  to cover.
