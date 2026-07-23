// term/mod.rs — the VT/ANSI terminal emulator wrapper (PLAN §9).
//
// Wraps `vt100::Parser`: feed it every `SshEvent::Output` chunk and it maintains
// a `Screen` — a grid of cells (glyph, colors, attributes) plus cursor position.
// The UI reads that grid to render; resizing the view resizes the parser.
//
// Stub for the walking skeleton — implemented in a later step.
