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
through a `Replies` listener, and answers only the few queries that need cmote's own data (its
colour scheme, its cell pixel size). The old `term::compat` (cursor-move rewriter) and
`term::answer` (reply synthesizer) modules were **deleted** in the swap — the engine does both.

Effort is now just *where the work lives*, not a hard wall:

- **[keymap]** — cmote's input encoder (`term::keymap`); engine-independent.
- **[reply]** — extend cmote's reply path (the `Replies` listener in `term::mod`) for a query
  the engine does not answer itself.
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
  answered rather than left guessing.
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

## 3. Still open — query → reply (niche; `[reply]`, some need DCS)

The high-value query class is already closed: DA / DSR / DECRQM by the engine, the colour and
pixel-size queries by cmote's listener. What the engine still does **not** answer:

| Missing | What blocks on it | Reply shape | Tag | Src |
|---|---|---|---|---|
| **DA3 tertiary** (`CSI =c`) | terminal-id probes | `DCS !\|<hex> ST` | [reply] | [xterm] |
| **XTVERSION** (`CSI >q`) | modern feature detection | `DCS >\|cmote(ver) ST` | [reply] | [xterm] |
| **DECRQSS** request-setting (`DCS $q … ST`) | editors/multiplexers restoring SGR / scroll region / cursor style | `DCS 1$r … ST` | [reply] (needs a DCS reply path) | [DEC] |
| **XTGETTCAP** (`DCS +q <hex> ST`) | apps querying terminfo caps directly | `DCS 1+r … ST` | [reply] (needs a DCS reply path) | [xterm] |
| **Answerback** (ENQ `0x05`) | legacy identification | configurable string (usually empty) | [reply] | [ECMA-48] |

The engine's `identify_terminal` handles the primary and secondary DA intermediates only — the
`=` (tertiary) intermediate is dropped — so DA3 falls to cmote if ever wanted. All of these are
low UX value: modern applications rely on the DA / DECRQM answers that already work.

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
| **OSC 133 shell-integration** (semantic prompt marks) | niche; a stream scanner beside the cwd tracker could capture them | [seam] low pri | [community] |

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
- **Synchronized output `?2026`** — not observed in the crate; safe to ignore today, a strict
  implementation would buffer a frame. `[community]`, low pri.

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
for the gap combos only) — kept for the programs that speak it rather than kitty.

The `[engine-limit]` items are the only remaining large moves, and only **images** (sixel /
kitty graphics) carry real UX value — the rest (double-height lines, left/right margins,
rectangular ops) are legacy and rare. For "support *any* documented app UX", graphics is the one
outstanding ceiling-raiser; everything else above is A-sized and engine-independent.

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
  engine types behind `Terminal` + `ScrollMotion`.
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
