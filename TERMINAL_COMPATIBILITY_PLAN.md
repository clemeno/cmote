# cmote — Terminal Compatibility Plan

A reference inventory of **what cmote's terminal still lacks to render or drive any
documented remote-terminal-application UX**, and what each gap would cost. Every entry is a
sequence specified in an official or widely-adopted source — nothing here is speculative.

Produced 2026-07-28 by auditing the actual code:

- the emulator crate **`vt100` 0.16.2** (its `vte`-based `WrappedScreen` dispatch, its
  `Screen`/`Cell` public API, and what it silently drops), and
- cmote's own terminal layer (`src/term/*`, `src/ui/grid.rs`, `src/app.rs`).

File:line evidence for the audited claims is collected in the [Evidence](#evidence)
appendix, so any statement here can be re-checked against the source it came from.

Sources cited by tag:

- `[ECMA-48]` — ECMA-48 (5th ed.), the ANSI/ISO control-function standard.
- `[DEC]` — the DEC VT100 / VT220 / VT320 / VT420 / VT520 programmer reference manuals.
- `[xterm]` — *XTerm Control Sequences* (Thomas Dickey), the de-facto reference for
  everything past the DEC set.
- `[community]` — a spec adopted across terminals but owned by no vendor (OSC 8 hyperlinks,
  synchronized output `?2026`, OSC 133 shell integration).
- `[vendor]` — documented by one vendor only (kitty keyboard/graphics, iTerm2 OSC 1337).

---

## 0. The one structural fact

**`vt100` 0.16.2 is the ceiling, not cmote's code.** It is a deliberately small VT subset
sitting on the `vte` byte-tokenizer: control functions it has no arm for are routed to
no-op callbacks and discarded, and — decisively — **a whole class of state it never stores,
so cmote cannot render or report it no matter what we bolt on.** The crate is also a pure
state sink: it never generates a reply to any host query (that is why cmote answers DSR/DA
itself in `term/answer.rs`).

Consequently every gap below carries an **effort tag**:

- **[bolt-on]** — addable *beside* `vt100`, the way `term/compat.rs` (rewrite on the way in)
  and `term/answer.rs` (reply on the way back) already are: scan the output stream, encode a
  key, or read a getter the crate already exposes. **Engine-independent** — it survives a
  later engine swap unchanged.
- **[engine]** — impossible on `vt100` 0.16.2 because the crate cannot parse or represent it.
  Needs replacing the emulator. PLAN §16 already names the target: **`alacritty_terminal`**
  (or wezterm's `termwiz`), a full VT implementation.

The strategic split that falls out of this: the **[bolt-on]** items are the cheap,
high-value, self-contained wins (each is an "A-sized" module with unit tests); the
**[engine]** swap is the single large move that unlocks everything else at once, and is the
only path to "support *any* documented app UX".

---

## 1. Baseline — what already works

So the gaps read against a known floor. cmote today:

- **Renders** (per cell): bold (real bundled bold face), single underline (drawn as a rule),
  inverse, and full-depth colour — 16 / 256 / 24-bit truecolor, fg and bg. Draws **braille**
  (U+2800–28FF) and **rounded box corners** (U+256D–2570) from geometry, since no bundled
  monospace font carries them. Cursor is a visible-aware inverse block (§9, §11).
- **Rewrites on the way in** (`compat.rs`): the cursor-move alias spellings `vt100` lacks —
  HVP `f`→CUP, HPA `` ` ``→CHA, HPR `a`→CUF, VPR `e`→CUD, and `CSI s`/`u`→DECSC/DECRC.
- **Answers host queries** (`answer.rs`, v2.4): DSR status `CSI 5n`, cursor-position
  `CSI 6n`, extended cursor `CSI ?6n`, primary DA `CSI c`, secondary DA `CSI >c`.
- **Tracks & honours modes** `vt100` exposes: application-cursor DECCKM (arrows→SS3),
  bracketed paste `?2004`, cursor visibility `?25`, mouse `?9/1000/1002/1003` in SGR / UTF-8 /
  classic encodings, alternate-screen bit (parsed, but see §3 — **not read**).
- **Encodes input**: printable + layout, Ctrl-\*, Alt-as-meta, Enter/Tab/Backspace/Esc,
  arrows/Home/End (DECCKM-aware), Insert/Delete/PageUp/Down, **F1–F12**, numpad (NumLock
  aware), bracketed paste with injection scrub.
- **Reads the remote cwd** from OSC 7 / OSC 9;9 for the tree, pane and title (`cwd.rs`).

---

## 2. Query → reply gaps (the "app stalls on a timeout" class)

Highest UX value. Same family as the v2.4 work: the application writes the query downstream
and **blocks reading its stdin** until the answer arrives, so leaving it silent costs a
timeout stall or a wrong detection. All **[bolt-on]** — scan the stream, reply on the input
channel (the `SshCommand::Input` path `answer.rs` already uses).

| Missing | What blocks on it | Reply shape | Tag | Src |
|---|---|---|---|---|
| **OSC 11 / 10 / 12 colour query** (`OSC 11;?`) | **vim / neovim query the background colour to choose a light-vs-dark scheme** — the most-felt gap after v2.4; misdetection, not just a stall | `OSC 11;rgb:RRRR/GGGG/BBBB ST` | [bolt-on] | [xterm] |
| **OSC 4;n;? palette query** | apps probing a 256-palette entry | `OSC 4;n;rgb:… ST` | [bolt-on] | [xterm] |
| **XTWINOPS reports** — `CSI 18t` (text area in cells), `CSI 14t` (pixels), `CSI 19t`, `CSI 11t`, title `CSI 21t` | image tools, pixel-size probes | `CSI 8;h;w t` etc. | [bolt-on] | [xterm] |
| **DECRQM** request-mode (`CSI ?Ps$p`) | apps testing whether bracketed-paste / focus / a mode is supported | `CSI ?Ps;v$y` | [bolt-on] | [DEC][xterm] |
| **DECRQSS** request-setting (`DCS $q … ST`) — report current SGR / scroll region / cursor style | editors & multiplexers restoring state | `DCS 1$r … ST` | [bolt-on]* | [DEC] |
| **XTVERSION** (`CSI >q`) | modern feature detection | `DCS >\|cmote(ver) ST` | [bolt-on] | [xterm] |
| **DA3 tertiary** (`CSI =c`) | terminal-id probes | `DCS !\|<hex> ST` | [bolt-on] | [xterm] |
| **XTGETTCAP** (`DCS +q <hex> ST`) | apps querying terminfo caps directly | `DCS 1+r … ST` | [bolt-on] | [xterm] |
| **Answerback** (ENQ `0x05`) | legacy identification | configurable string (usually empty) | [bolt-on] | [ECMA-48] |

\* DECRQSS/XTGETTCAP need a small **DCS** scanner (vt100 drops DCS entirely), and a few
DECRQSS answers need internal state vt100 does not expose (e.g. the scroll-region bounds) —
those specific answers are **[engine]**.

**Related correctness bug:** a `CSI 6n` cursor report is **wrong under origin mode (DECOM)** —
it should be relative to the scroll region. `vt100` tracks the DECOM flag but exposes no
getter, so fixing this is **[engine]** (or a private DECOM tracker of our own).

---

## 3. Rendering / attribute gaps

| Missing | Note | Tag | Src |
|---|---|---|---|
| **Dim / faint** (SGR 2) | `vt100` *has* `cell.dim()`; our grid simply never reads it — draw the fg dimmed | [bolt-on] | [ECMA-48] |
| **Italic** (SGR 3) | `vt100` *has* `cell.italic()`; grid ignores it. **Fira Mono ships no italic face** — needs a bundled italic mono or a synthesised slant | [bolt-on] + font | [ECMA-48] |
| **Blink (5/6), Strikethrough (9), Conceal (8), Double-underline (21), Curly/undercurl (4:3), Underline-colour (58/59), Overline (53)** | `vt100` stores **none** of these bits | [engine] | [ECMA-48] / [xterm] (4:3, 58) |
| **Cursor shape / style** DECSCUSR (`CSI Ps SP q`) — block / underline / bar, steady / blink | grid draws an inverse block only; vt100 never parses it | [bolt-on] (own tracker + grid) | [xterm][DEC] |
| **DEC line-drawing charset** (`ESC(0` … SI/SO) | vt100 drops charset designation, so **ncurses / older apps that box-draw the VT100 way render literal letters (`lqqk`)** instead of borders | [bolt-on] as a compat-style charset translator, *or* [engine] | [DEC] |
| **Double-width / double-height lines** DECDWL / DECDHL (`ESC#3-6`) | vt100 drops `ESC#` | [engine] | [DEC] |

---

## 4. Modes / behaviours

| Missing | Note | Tag | Src |
|---|---|---|---|
| **Alternate-screen awareness** | vt100 exposes `alternate_screen()`; **cmote never reads it** — needed to keep scrollback primary-only and to gate alt-scroll | [bolt-on] | [xterm] |
| **Autowrap toggle** DECAWM `?7` | hard-wired ON in vt100 | [engine] | [DEC] |
| **Focus reporting** `?1004` (`CSI I` / `CSI O` on focus in/out) | vt100 does not track the mode; needs own scanner + iced window-focus events | [bolt-on] | [xterm] |
| **Alternate-scroll** `?1007` (wheel → arrow keys on the alt screen) | depends on alt-screen awareness above | [bolt-on] | [xterm] |
| **Custom tab stops** HTS / TBC | vt100 is fixed at every 8 columns, no stop table | [engine] | [ECMA-48] |
| **Left / right margins** DECSLRM `?69h` (VT420) | vt100 has no horizontal margins | [engine] | [DEC] |
| **REP** repeat `CSI Ps b` | vt100 drops it — the repeated glyphs vanish | [bolt-on] (expand in a rewriter) or [engine] | [ECMA-48] |
| **Synchronized output** `?2026` (batch redraw to stop tearing) | safe to ignore today; a strict impl would buffer a frame | [bolt-on] low pri | [community] |

---

## 5. Input protocol gaps (all keymap-side — no engine needed)

| Missing | Note | Tag | Src |
|---|---|---|---|
| **Modified named keys** — Ctrl/Shift/Alt + arrows / Home / End / F-keys → `CSI 1;<mod><L>` and `CSI <n>;<mod>~` | cmote drops the modifier on named keys today | [bolt-on] | [xterm] |
| **F13–F24** | unmapped (`_ => None`) | [bolt-on] | [xterm] |
| **DECKPAM application keypad** — app-keypad numpad should send `ESC O p…y` | vt100 exposes `application_keypad()`, **unused**; numpad decided by NumLock only | [bolt-on] | [DEC] |
| **xterm `modifyOtherKeys`** (`CSI >4;2m` → keys as `CSI 27;mod;code~`) | needs a mode scanner (the app sets it) + encoder | [bolt-on] | [xterm] |
| **Kitty keyboard protocol** (`CSI >flags u` … report `CSI code;mods;event u`) | larger; disambiguates every key + modifier | [bolt-on] | [vendor] |

---

## 6. Graphics / images / hyperlinks (the truly "exotic")

All need **DCS/APC** parsing (vt100 drops all of it) plus a compositor, so **[engine]** +
real renderer work, except where noted:

- **Sixel**, **ReGIS** — `[DEC]`.
- **Kitty graphics protocol**, **iTerm2 inline images (OSC 1337)** — `[vendor]`.
- **OSC 8 hyperlinks** (`OSC 8;;URI ST`, clickable) — `[community]`. A stream **[bolt-on]**
  scanner can capture them, but clickable rendering in the grid is the work.
- **OSC 133 shell-integration** (semantic prompt marks) — `[community]`, niche; a **[bolt-on]**
  scanner (cmote already scans OSC for the cwd).
- **DRCS soft fonts**, **VT320 status line**, **VT420 rectangular ops** (DECCRA / DECFRA /
  DECERA, and the **DECRQCRA checksum query** some conformance apps block on) — `[DEC]`,
  **[engine]**.

---

## 7. Scrollback

`term::SCROLLBACK = 0` — a **cmote choice**, not a vt100 limit (`Screen::set_scrollback`
exists). **[bolt-on]**, but the real work is the UI: a scrollbar, wheel handling off the
alt screen, and extending mouse selection across the scrolled-off region.

---

## 8. Deliberately excluded (policy, not gap)

**OSC 52 clipboard read/write** — kept out on purpose (§9, §12, §16): a remote could read or
poison the local clipboard, and cmote only touches the clipboard on an explicit *local*
action. Documented and intentionally unimplemented. Answering the OSC 52 `?` *read* query
would be an injection vector and stays out for the same reason the OSC title/clipboard query
variants are unanswered in `answer.rs`.

---

## 9. Recommendation — two independent tracks

1. **Bolt-ons (do regardless of the engine question).** Ranked by UX bite, each an A-sized,
   self-contained, unit-tested module that survives a later engine swap:
   1. **OSC 11 background-colour query** (vim / neovim light-vs-dark) — the clear next win.
   2. **Modified-key + F13–F24 encoding** (keymap only).
   3. **Focus reporting `?1004`**.
   4. **Cursor shape DECSCUSR** (own tracker + grid).
   5. **Window title from OSC 0/2** (cmote already scans OSC; today only cwd feeds the title).
   6. **Dim + italic** rendering (italic needs a font).
   7. **DECRQM / XTWINOPS / XTVERSION** replies.

2. **The engine swap to `alacritty_terminal`.** The only move that unlocks, in one step:
   blink / strikethrough / conceal / undercurl / underline-colour, the DEC line-drawing
   charset, DCS / sixel, custom tab stops, the autowrap toggle, double-width lines,
   origin-mode-correct CPR, and the rectangular ops. Large — it touches the whole
   render + state layer — but it is the documented ceiling-raiser (PLAN §16), and none of the
   track-1 work is wasted against it.

For "support *any* documented app UX," track 2 is unavoidable. Track 1 is where the cheap,
high-value wins live and can start immediately.

---

## Evidence

Audited file:line anchors behind the claims above, for later re-checking.

### `vt100` 0.16.2 (registry crate — `…/vt100-0.16.2/src/`)

- **Pure sink, no host replies.** `parser.rs` `process()` returns `()`; the `*_formatted`
  / `*_diff` methods in `term.rs` re-serialise *our own* screen for replay, not query
  answers. Unmatched control functions → `callbacks.rs` no-op `unhandled_*` hooks.
- **CSI present** (`perform.rs` `csi_dispatch`, impls in `screen.rs`): `@ A B C D E F G H
  J K L M P S T X d m r` and window-op `t` **subop 8 only**; private `?J ?K ?h ?l`.
- **CSI absent** (→ `unhandled_csi`): `f` HVP, `n` DSR, `c` DA, `g` TBC, `b` REP,
  `` ` `` HPA, `a` HPR, `e` VPR, `q`/`SP q` DECSCUSR, `p`-family (DECSTR/DECRQM), `s`/`u`,
  `Z` CBT, and **any non-`?` intermediate** (`>c`, `=c`, `$p`, `!p`, `SP q`).
- **SGR** (`screen.rs` `sgr`, store `attrs.rs` — a 5-bit `mode`): present = 0,1,2,3,4,7 +
  off-codes + 30-49 + 90-107 (16 / 256 / truecolor, `:` and `;` forms). **Absent** = 5/6
  blink, 8 conceal, 9 strike, 21 double-underline, 4:3 undercurl, 58/59 underline-colour,
  53 overline.
- **ESC present**: `7 8 = > M c` (+ vendor `ESC g` visual bell). **Absent**: `ESC( ) * +`
  charset (so **DEC Special Graphics G0/G1 unmapped**), `ESC#3-6` line size, `ESC H` HTS.
- **DEC private modes present** with getters: 1, 25, 47, 1049, 1000/1002/1003, 1005/1006,
  2004, and 9. Mode 6 (DECOM) is stored but **no getter**. **Absent**: 7 (DECAWM — wrap
  hardwired on), 12, **1004 focus**, 1007, 1015, 1047, 3, 5, 2026.
- **OSC**: only 0/1/2 (→ discarded callbacks, **no `title()` getter**) and 52 (→ callback).
  **Absent**: 4, 7, 8, 9;9, 10/11/12 (+ their `?` queries). 
- **DCS**: `Perform::hook/put/unhook` unimplemented → **all DCS silently consumed** (no
  sixel, no DECRQSS, no XTGETTCAP).
- **`Cell` getters**: `contents is_wide is_wide_continuation fgcolor bgcolor bold dim
  italic underline inverse` — no blink / strike / conceal / underline-colour / hyperlink.

### cmote (`c:/sources/github_clemeno/cmote/src/`)

- **`ui/grid.rs`** — `cell_style` (grid.rs:712) maps a `Cell` to a `CellStyle` carrying only
  `fg bg bold underline`. Reads `bold()` (:718), `underline()` (:719), `inverse()` (:723);
  colour depth full (ANSI_16 / 256-cube / `Rgb`, :96-116, :748). **Ignores `dim()` and
  `italic()` though vt100 exposes them.** Cursor = inverse block via `inverse ^ is_cursor`
  (:723-726); no cursor-shape. Braille (:568) and rounded corners (:550) drawn from
  geometry; Fira Mono Medium/Bold only (app.rs:31,42), **no italic face**.
- **`term/mod.rs`** — `Terminal` exposes `screen() cwd() resize() process()`; `process`
  returns the DSR/DA reply bytes (mod.rs:78). `SCROLLBACK = 0` (mod.rs:29). Does **not**
  surface title, alternate-screen, cursor shape, or application-keypad.
- **`term/keymap.rs`** — F1–F12 only (F13–F24 → `None`); modifiers dropped on named keys;
  **no** modifyOtherKeys / kitty / DECKPAM / focus report / answerback.
- **`term/mouse.rs`** — modes `?9/1000/1002/1003`; encodings classic / UTF-8 / SGR. **No**
  `?1015`, **no** `?1004` focus.
- **`app.rs`** — `App::title` (app.rs:2573) uses the endpoint + `Terminal::cwd()` (OSC
  7/9;9 tracker) only, **never a vt100/OSC-0/2 title**. **No** XTWINOPS handling. Every
  upstream byte path is one of six `SshCommand::Input` sites (resume `cd`, the answer.rs
  replies, keyboard, mouse reports, paste, programmatic `cd`); only the replies site answers
  host queries.
