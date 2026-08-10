# cmote

[![CI](https://github.com/clemeno/cmote/actions/workflows/ci.yml/badge.svg)](https://github.com/clemeno/cmote/actions/workflows/ci.yml)

A **native, portable SSH client for Windows 11 and macOS** written in Rust. A home
screen lists your saved connection targets; pick one (or start a new connection), fill
in host / port / user, pick an auth method (password, a private key — PEM or PuTTY
`.ppk` — keyboard-interactive for 2FA / OTP, or a key held by your **SSH agent / Pageant**),
connect. On success the server hands us a shell and cmote renders a **full VT
terminal** inside the window — a working interactive prompt, with a browsable tree of the
remote filesystem beside it, a grid of the current directory's files under it (keyboard
navigable, with a details popup and rubber-band multi-selection), the remote working
directory in the title bar, and file transfer both ways. Full-screen programs — btop,
vim, htop, midnight commander — draw properly and take the mouse. Reconnect to a saved
target and the shell and both panels come back to the directories you left them in. Open
as many sessions as you like in **tabs** — each fully independent, all in one window — and
**split** that window to watch two of them at once. Tunnel ports through the connection — local,
remote or a SOCKS proxy — and they come back on reconnect.

This is a **learning project**. The code is meant to be read as much as run, so it is
written didactically: it favours idiomatic Rust, explains *why* each choice was made,
and marks every deliberate shortcut with a `ponytail:` note so "simple" reads as
intent, not oversight. The full design rationale lives in [PLAN.md](PLAN.md); section
references below (§n) point into it.

## Features

- **Tabs — many independent sessions in one window.** Each tab is its own session: one can sit
  on the home list while another runs a shell, and every tab keeps its own terminal, folder tree,
  files pane, selection and dialogs. A background tab's shell keeps running and its listings keep
  arriving while you work in another. Mouse-only strip across the top: **click** a tab to switch,
  **"+"** to open a new one, **"×"** to close (a live session asks to confirm first), and **drag**
  a tab along the strip to reorder it. The pointer says so: an **open hand** over a tab, a **closed**
  one from the moment you press until you let go — the same pair over every grabbable thing in
  cmote, a **dialog header** included. Windows has no hand cursor of either kind, so cmote draws
  both and paints them itself. The saved targets and the unlocked vault are shared across every tab.
- **Split the window — two sessions side by side.** The two buttons at the right of the strip cut
  the window **beside** or **below**, and the new half is a whole small cmote: its own tab
  strip, its own tab, opened on the saved target list ready to connect somewhere. **One split, no
  more:** the buttons are there while the window is whole and gone once it is cut, so there is never
  a grid of regions too narrow to read. Close the second region and they come back. Splitting
  **doubles the window** in the direction you asked for, so the session already on screen keeps the
  size it had and does not reflow. **Drag a divider** to re-share the room; the share is kept as a
  proportion, so it survives resizing the window, and a **double-click on the divider** puts the two
  halves back to even. A **click** anywhere in a region gives it the
  keyboard and lights its strip, and that same click still lands where you aimed it. Closing a
  region's last tab closes the region and **shrinks the window back**, so the region you kept stays
  exactly the size it was — however you had dragged the divider or resized the window meanwhile.
  Closing the last tab of the last region still asks to quit.
- **Confirmed, clean quit.** Closing the **last** tab, or clicking the window's title-bar **×**,
  asks **Quit cmote?** first — telling you how many live sessions it will disconnect — so a stray
  click never drops your work. On confirm, every session is **disconnected cleanly** (a proper SSH
  channel close, not a yanked socket) and only then does the process exit; a wedged session can't
  hold quit open past a short timeout. **Ctrl+D** closes the current tab, but only once you're back
  on the home screen — on a live shell it stays EOF to the remote (the way you log out), so
  Ctrl+D logs out, and Ctrl+D again closes the tab, just like a terminal (§30).
- **Port forwarding — local, remote and dynamic tunnels.** The **Tunnels** button on the status
  bar opens a manager: add a **Local** (`-L`) forward to reach a service through the server, a
  **Remote** (`-R`) forward to expose a local service on the server, or a **Dynamic** (`-D`)
  SOCKS5 proxy that lets each connection pick its own target. A remote forward may listen on port
  **0** to let the **server pick a free port** (`-R 0`); the row then shows the port it chose. Each
  tunnel rides the same connection — no second login — and shows live / failed in the dialog, with a
  live **activity gauge** (`N open · M total`) counting the connections crossing it now and in all;
  the set is **remembered per target** and re-established when you reconnect. Binds to loopback by
  default. Addresses take a hostname, an IPv4, or a **bracketed IPv6** literal (`[::1]:8080`).
- **Home screen of saved targets** — every successful connection is remembered as a
  named target and listed alphabetically. Profiles only by default: **no passwords or
  passphrases in `targets.json`** (only host / port / user / auth method / key and certificate
  path) — a
  secret is stored only if you opt in to the encrypted vault (below). Click a
  target to select it, then click it again (or press **Enter**) to open it and pre-fill
  the form; **rename** it in place with **F2** or right-click → **Rename** (the list
  re-sorts); right-click also offers **Open** and **Delete** (deleting asks to confirm —
  cancelling keeps the target); **New connection** opens a blank form.
- **Filter the target list** — the box above the list narrows it as you type (**Ctrl+F** puts
  the cursor there). Plain text is a **fragment**, matching anywhere, so `prod` finds
  `web-production-01` from the first keystroke; type a `*` or a `?` and it becomes a **glob** over
  the whole row instead — `prod*` starts with, `*.db` ends with, `web-0?` is one character wide.
  Both the name and the `user@host:port` are matched, case-insensitively, and a `shown of total`
  tally says how much is hidden. **Enter** opens the selected target without leaving the box;
  **Esc** empties it (§49).
- Connection form: host, port, user, and an auth method.
- **Password** auth, or **private-key** auth with a native file picker (`rfd`).
- Key formats: OpenSSH / PEM (via `russh::keys`) and PuTTY **`.ppk`** (via
  `ssh-key`'s `from_ppk`). Encrypted keys prompt for a passphrase on their own screen —
  or pre-fill an optional passphrase field on the form (leave it empty to be prompted).
- **OpenSSH certificates** — under key auth, point the optional **Certificate** field at a
  `*-cert.pub` to authenticate with a CA-signed certificate (the key still signs; the
  certificate rides along). Picking a key auto-fills the `<key>-cert.pub` sibling when it exists
  — exactly like the command-line client — and **Clear** drops back to a plain key. The
  certificate path is remembered with the target (it is public, not a secret).
- **Keyboard-interactive (2FA / OTP)** — pick **Interactive** for challenge-response servers,
  and cmote also chains into it automatically after a password/key when the server asks for a
  second factor (key/password **plus** a one-time code). The server's prompts appear one field
  each — masked for a code or password, plain for a username — and are answered live; nothing
  is stored.
- **SSH agent / Pageant** — pick **Agent** to let a running agent hold the key and sign the
  challenge, so cmote never sees the private key and there is nothing to type. On Windows it
  looks for the OpenSSH agent (the `\\.\pipe\openssh-ssh-agent` pipe, or `SSH_AUTH_SOCK` when it
  points at one) and then Pageant; on macOS it uses `ssh-agent` via `SSH_AUTH_SOCK`. Every agent
  key is offered in turn until the server accepts one, and it still chains into 2FA afterwards.
- **Remember a secret (opt-in, portable)** — tick **Remember** on the form to keep that
  target's password or key passphrase in an encrypted vault (`secrets.age`), so a return visit
  pre-fills the masked field. The vault is one file protected by a **master passphrase** you
  choose (`age`: scrypt + XChaCha20-Poly1305), so it stays portable — it unlocks on any machine
  with the passphrase, unlike a machine-bound OS keyring. Off by default; the secret is saved
  only after a *successful* connect (a wrong password is never stored), and a forgotten master
  passphrase means the secrets are gone (no recovery, by design).
- **Trust-on-first-use** host-key verification against a portable `known_hosts`:
  first contact shows the fingerprint for explicit accept/reject; a later key **change** opens
  a loud override dialog — both fingerprints (stored vs presented), a possible-MITM warning, and
  reject / trust-once / replace, defaulting to reject and never auto-trusting (§8, §28).
- A full **VT terminal** — a complete VT engine (`alacritty_terminal`) whose grid cmote
  draws with iced — that reflows to the window size, forwarding the new pty size to the
  remote (§9, §23).
- **Full-screen programs draw properly** — btop, htop, vim, midnight commander. The screen
  is one widget that puts every glyph at the exact pixel its column starts at, so nothing a
  program prints can shift the line it is on; **braille** graphs and **rounded box corners**
  — glyphs no monospace font we could bundle actually carries — are drawn from their own
  geometry rather than borrowed from whatever font the system offers. The engine interprets
  the full escape-sequence set — the DEC line-drawing characters older programs box-draw
  with, custom tab stops, origin mode — so a program's screen lands where it belongs instead
  of coming out as wrapped, scrolling gibberish. It also **answers the queries a program
  blocks on or adapts to** — "where is the cursor?" (`CSI 6n`), "what terminal are you?"
  (`CSI c`), "what is your background colour?" (`OSC 11`, which lets an editor pick a light or
  dark colourscheme to match), "how big is your screen?" — that otherwise stall vim, tmux and
  less on a startup timeout or leave them guessing; cmote answers each with what it actually
  shows (§9, §23). **F1-F12** are mapped as the pty's terminfo entry describes them, and a
  **modifier held on a named key** now goes through: Ctrl+arrow for word-motion, Shift+arrow
  to select by line, Ctrl+Delete, modified F-keys, and F13-F24 all send the sequence xterm
  would, where before the modifier was dropped (§9). And when an editor asks for
  **modifyOtherKeys**, the Ctrl/Alt combos the plain terminal alphabet cannot spell — Ctrl+digit,
  Ctrl+punctuation, Ctrl+C as a distinct key rather than the interrupt — reach it unambiguously
  (§9). Editors that speak the newer **kitty keyboard protocol** (neovim, kakoune, helix, fish)
  get the fuller treatment: Esc told apart from an Alt combo, every key disambiguated, and — for
  the ones that ask — press / repeat / **release** events and the key's associated text, all in
  kitty's `CSI u` form (§25).
- **Text styling comes through** — colour (256-colour and truecolor), bold, faint, reverse
  video, concealed text, strikethrough, and every underline style a program reaches for:
  single, double, dotted, dashed and the curly one an editor draws under a spelling mistake,
  each in its own colour when the program sets one, plus **italic** — drawn from a bundled
  IBM Plex Mono face, since Fira Mono ships no italic of its own (§23).
- **The cursor takes the shape a program asks for** — a block, an underline, or a thin bar,
  whichever the remote picks with DECSCUSR (vim's insert-mode bar, say); drawn steady, since
  cmote runs no blink timer (§23).
- **Pictures show up in the terminal** — a program that sends a **sixel** image (`img2sixel`,
  `chafa -f sixel`, gnuplot, timg, matplotlib's sixel backend, `lsix`) gets a real picture, not a
  screenful of garbage. It is drawn over the cells it reserves, so the prompt lands underneath it and
  it scrolls up into the history with the output around it — scroll back and the plot is still there,
  on its own text. cmote also *tells* programs it can do this (the sixel device attribute and the
  graphics-capability query), which is what makes the tools that auto-detect send a picture rather
  than fall back to text art. Full-screen programs that draw on the alternate screen (`ranger`
  previews, `mpv --vo=sixel`) are not covered yet, and kitty's and iTerm2's own image protocols are
  not spoken (§41).
- **The remote is told when focus changes** — a program that turns on focus reporting (`?1004`,
  as tmux and vim do) hears `CSI I` / `CSI O` as the window gains or loses focus, so it can
  undim or pause a spinner. Moving cmote's keyboard to a side panel counts as the shell losing
  focus too, since the remote knows nothing of cmote's own panels (§23).
- **Scroll back over what left the screen** — cmote keeps 10 000 lines of history. The **wheel**
  scrolls it (whenever no full-screen program has claimed the wheel), **Shift+PageUp/PageDown**
  page through it and **Shift+Home/End** jump to the top and back to the live bottom; typing (or
  pasting) snaps you back to the prompt so what you send lands where it echoes. New output while
  you are scrolled up leaves you where you are reading. A thin **scroll indicator** appears at the
  right edge while you are scrolled up and disappears at the live bottom — its bar shows where you
  are in the history and how deep it runs. A full-screen program (vim, tmux, less) keeps its own
  pages, so scrolling there is theirs, not cmote's (§23).
- **Search the scrollback** — **Ctrl+Shift+F** floats a find bar over the grid. Typing searches all
  10 000 lines as you go (case-insensitive) and lands on the **newest** hit, scrolling it into view and
  selecting it, so Copy takes it with no extra step; **↑** / **↓** walk older and newer and wrap at
  both ends, and **Esc** closes the bar leaving the last hit selected. Every other hit **on screen** is
  washed in amber at the same time, so you can see how the query is spread through the output rather
  than only where you are in it (§35, §39). The bar keeps up with a **live** shell: a hit printed while
  it is open joins the count and the washes on the next frame, without dragging the view off whatever
  you are reading (§44).
- **Mouse text selection** (drag to select, highlighted in place) with **Copy** and
  **Paste** — from the status-bar buttons, a right-click menu, or the keyboard. **Double-click
  selects a word** and **triple-click the whole line**: a word is generous about what belongs to
  one, so a path, a URL, a `user@host:port` or a `KEY=value` comes back whole and is ready to paste
  straight back into the shell, while spaces, quotes, brackets and commas end it. A line means the
  *logical* line — a command too long for the window occupies several rows and is taken in full — and
  copying across that fold gives you the line as it was typed, not with a line break where the
  window's edge happened to be (§42). Resizing the window **clears the selection** rather than leave
  the highlight over text the reflow moved under it — a highlight that no longer matches what Copy
  would copy is worse than none (§43). **Ctrl+C**
  copies (when a selection exists; otherwise it is the shell's interrupt) as **styled HTML**
  that keeps the terminal's colours and attributes when pasted into a rich editor, with a
  plain-text fallback; **Ctrl+Shift+C** copies plain text only. **Ctrl+V** / **Ctrl+Shift+V**
  paste, **bracketed-paste** aware and stripping the paste-injection terminator (§9-§10).
- **Clickable links** — a program that marks text as a hyperlink (the OSC 8 escape, as
  `ls --hyperlink` and many build tools do) makes it followable: hold **Ctrl** and the link
  underlines under your pointer, then **Ctrl+click** opens it in
  your browser, or right-click for **Open link / Copy link**. cmote opens only http, https and
  mailto — a link's scheme decides which local program Windows launches, and the address comes
  from the remote, so anything else is refused (§24).
- **The mouse reaches the program that asked for it** — click a process in btop, a tab in
  tmux, a line in vim; the wheel scrolls what is under it. cmote forwards clicks, releases,
  drags and scrolls in the xterm protocols a program enables, and **holding Shift takes the
  pointer back** for text selection and cmote's own right-click menu (§9).
- **Remote folder tree** — a 2D explorer of the remote filesystem in the bottom strip, to
  the right of the files pane (the terminal keeps the full width above), over **SFTP**
  (falling back to `ls` on a server with the subsystem disabled).
  Click a folder to expand or collapse it; the tree **follows the shell**, opening the
  whole chain from `/` down to wherever you `cd`. Right-click a folder for **Open in
  terminal** (types a quoted `cd`), **New folder…**, **Upload…** (sends local files into that
  folder), **Upload folder…** (sends a whole local folder), **Rename…** (inline, like F2 on the
  home list), **Delete…**, **Copy name / relative path / full path** and **Refresh** (re-checks
  the folder is still there, under its name, and re-lists what is inside). A single folder opens
  and closes by clicking its row or with → / ←, so there is no menu Expand/Collapse. Its header
  names the folder on show — middle-ellipsised and capped at two lines — with a **copy button**, a
  **↻ refresh button** and a **collapse-all button** beside it. The refresh button (and **F5**
  while the tree has focus) re-lists **every open folder** at once, so the tree catches up in one
  press after you move or make a folder from the shell; collapse-all closes every branch back to
  the top level. Drag the splitter to resize the panel — the terminal reflows to match — or hide it
  with the status bar's **Folders** button; the `.*` checkbox in its header hides dot-folders
  (§18, §22). The splitter shows a **↔ resize cursor** and **lights up** while you hover or drag it,
  so it reads as grabbable (§31).
- **Remote files pane** — a grid of **every entry** in one directory, in the browser strip
  under the terminal (the folder tree shares that strip, on the pane's right, §18). Each cell
  is a wide row: a small icon in front of the name, with the **size,
  the modified date, and the permission word and `owner:group`** on a second, muted line underneath
  (`4.0 KB · 2026-03-20 11:46 · -rw-r--r-- cme:staff`, the date in the server's own wall clock but
  without the zone tag — that stays in the details popup; the mode reads `ls -l` style
  (`drwxr-xr-x`); a folder shows only the date, the mode and the owner, and anything the listing
  never learned reads as a dash). Each cell
  carries a **thin border** so the grid reads as distinct tiles. A name too
  long for its cell is **middle-ellipsised**, so the start *and* the extension survive. A big
  directory streams in batches of 1000 and the header counts as they land. Icons come from a
  bundled icon font, by category (folder, image, code, archive, document, audio, video, link,
  plain). Right-click an entry for **Open in terminal**, **Download…**, **Download folder…**
  (a lone directory, tree and all), **Rename…**, **Delete…**, **Copy name / relative path /
  full path** and **Refresh**; right-click **empty space** for **New folder…**, **Upload…
  here**, **Upload folder… here** and **Refresh**. The header carries an **up** button, the
  directory's path (middle-ellipsised to one line), a **copy button** for it and a **↻ refresh
  button** (re-lists the directory on show; **F5** does the same while the pane has focus). A
  **sort button** beside it drops a menu to order the grid by **Name**, **Last modified**,
  **Extension** or **Size**, **Ascending** or **Descending** — folders always stay grouped
  first. The order and the direction are each optional: picking the **lit** key clears the sort
  back to the default order, picking the **lit** direction unsets it (an unset direction sorts
  ascending), and the button lights up while a key is reordering the grid. **The chosen sort is
  remembered per target** and restored on reconnect. Drag the splitter to resize the pane, or hide it with the
  status bar's **Files** button; the same `.*` checkbox hides dot-files here too (§19). Its splitter
  shows a **↕ resize cursor** and lights up on hover or drag, the same feedback the tree's does (§31).
- **The window reopens at the size you left it.** cmote remembers the window's width and height
  across restarts in a small `settings.json` beside the saved targets, so it comes back the size
  you last made it (the panel sizes are remembered separately, per target, above). The terminal
  area is whatever is left — the window height minus the files pane and its handle — so the pty
  always matches what you see (§31).
- **Browsing never moves the console** — a click in the folder tree, a **double-click** on a
  folder in the grid, the pane's **up** button and **Enter** all point the *pane* somewhere
  else and leave the shell where it is, so you can look inside a directory without disturbing
  what is running. The shell moves only on a `cd` it can see: one you type, either panel's
  **Open in terminal**, or the status bar's **Sync**, which brings the shell (and with it the
  tree and the title) to the folder the pane is showing. Sync is disabled when the two already
  agree (§19).
- **Reveal brings the panes back to the shell** — the same button read the other way.
  After browsing away, the pane and the tree jump to the directory the shell is in: the tree opens
  the chain down to it and selects it, the pane lists it. It types nothing at the shell, so it is
  safe while a full-screen program is running, and it is the only way back when the shell has not
  moved — a shell sitting at the same prompt announces the same directory, which is not a move for
  the pane to follow. Disabled when the shell has never said where it is, when the bottom strip is
  hidden, or when both panels are already there (§19).
- **Keyboard focus across the three panels** — the shell, the folder tree and the files
  pane each take the keyboard. A session starts at the shell; a click focuses whatever was
  clicked, **Ctrl+Tab** cycles forward and **Ctrl+Shift+Tab** back (hidden panels are
  skipped), and the focused panel wears a ring so it is never a guess. In a panel the
  **arrow keys** walk the rows (in the grid, left/right move one cell and up/down a whole
  row), **PageUp/PageDown** jump a screenful at a time and **Home/End** leap to the first and
  last entry, **Tab / Shift+Tab** step next/previous, **Enter** opens, **F2** renames and **Esc**
  hands the keyboard back to the shell. **Shift** held on an arrow, a Page key or Home/End
  extends the selection instead of moving it. A keyboard-moved selection scrolls itself into
  view, only at the edges (§20).
- **The keyboard follows what you act on** — no panel answers to a plain character, so **typing
  while a panel holds the keyboard hands it back to the shell**, and the letter you typed goes to
  the prompt instead of vanishing. Anything under **Ctrl / Alt** stays a shortcut (the files pane
  keeps Ctrl+A), and the arrows, Tab, Enter, F2 and Esc stay the panel's. The same goes for the
  terminal's own commands: choosing **Copy selection / Paste / Upload… / Open link / Copy link**
  from its right-click menu (or the Copy / Paste buttons in the status bar) puts the keyboard back
  on the shell — so the Enter that runs a pasted command reaches it. **Ctrl+V** is that same Paste
  off the keyboard and behaves the same, from whichever panel has the ring. Merely opening the
  menu, or dismissing it, changes nothing; **Ctrl+C** stays where it is, since copying is not text
  going into the shell (§50).
- **A details popup beside the selection** — the entry's **full name** (the grid's label is
  narrow and may clip it), where a **symlink points**, the file's **MIME type**, its
  **modification time in the server's own timezone** (`2026-03-20 11:46:40 CEST (+02:00)` —
  the zone is read off the server once per session), its **size** (human, with the exact
  byte count behind it), its **permission word** (`drwxr-xr-x`, `ls -l` style) and its
  **`owner:group`** as names, not numbers. Anything the server
  would not say reads as a dash, and a button on the card copies **the whole thing** at once
  (§20, §22).
- **Selecting many entries at once** — drag a **rubber band** over the grid's empty space,
  **Ctrl+click** to add or remove one, **Shift+click** or **Shift+arrow** to take the run
  between two ends, **Ctrl+A** to take the lot; **Ctrl+drag** adds a band to what is already
  selected. The popup then summarises the set (how many, folders versus files, total size).
  A right-click *inside* the selection acts on all of it — the copy items join their results
  one per line and say how many they will take — while a right-click outside collapses onto
  that one entry first (§21).
- **File download** — right-click a file in the pane → **Download…**, pick where to save
  it in the native dialog, and it comes down over **SFTP** on its own channel with a
  progress bar in the status bar. Downloading a **multiple selection** asks for one
  destination folder instead of a dialog per file, queues the transfers (one at a time, one
  progress bar) and leaves any folders in the selection behind. If some of those names are
  already in the folder, one dialog asks about the whole batch before anything is written:
  **Skip them**, **Save alongside** (`notes-1.txt`), **Replace**, or **Cancel** (§19, §21).
- **File upload, one or many, into a folder** — pick local files with **Files…** (the
  picker is multi-select) and send them with **Upload**, over **SFTP** on its own channel so
  the shell keeps running. The confirmation lists what you picked under an **editable
  destination folder** — each file keeps its own name inside it, and an empty folder means
  the login directory. Upload starts from four places, each seeding that folder: the status
  bar (the shell's directory), the terminal's right-click **Upload…** (the shell's
  directory), the files pane's empty-space **Upload… here** (the pane's directory), and a
  folder's **Upload…** in the tree. Before a byte is sent, every destination name is checked
  on the server; if some are already there, **one dialog asks about the whole batch** —
  **Replace**, **Skip**, **Keep both** (`name-1.txt`) or **Cancel**. The files then go one at
  a time behind the status bar's progress bar, closing with `Uploaded N files`. A failure names
  its reason: one *before any bytes move* — a folder this account cannot write to, say — ends that
  file cleanly and skips to the next, and one *mid-copy* keeps its partial and offers **Resume**
  (below) (§17). When the batch lands, the files pane (and the
  tree) **re-list the destination folder** if they are showing it, so what you just sent appears in
  place without a manual Refresh (§29).
- **Create and delete remote entries** — **New folder…** (on the tree and the pane's
  empty-space menus) opens a small name dialog; **Delete…** (on either menu, and on a whole
  files-pane selection) removes what you picked over **SFTP** — a folder goes with its **entire
  subtree** — behind a confirmation that names the targets and warns it cannot be undone. Both
  fall back to `mkdir` / `rm -rf` on a server with the sftp subsystem disabled (§18).
- **Recursive folder transfer** — **Upload folder…** sends a whole local directory tree onto the
  server, and **Download folder…** pulls a whole remote one down, each recreating the tree on the
  other side and **merging** into a destination that already exists. When a file inside the tree
  would land on one already there, cmote asks **one file at a time**: **Overwrite**, **Keep both**
  (`name-1`), or **Skip** just this one; **Overwrite all** or **Skip all** to settle every later
  clash the same way; or **Cancel** the whole transfer (files already copied stay). **Symlinks are
  followed**: a link to a file sends the file, a link to a folder sends that folder's contents, and
  what lands on the other side is real files and real folders — the same thing `cp -L` gives you. A
  link that leads back up its own tree would never end, so that one is counted and left, as is a
  link pointing at something that isn't there; the notice says how many (§17, §19).
- **Cancel or resume a transfer** — while one is running the status bar shows a **✕**: press it to
  stop now — the partial file it was writing is deleted and the rest of a batch is dropped, since a
  deliberate cancel is final. If a transfer instead **fails mid-flight** (a hiccup on the link, not
  a cancel) its partial is kept and a **Resume** appears beside the notice; Resume picks up from
  exactly where it stopped — a byte-offset append for a single file, and for a folder a re-walk that
  size-compares every file so only the missing ones and the interrupted file's tail cross again.
  Resume works within one live session; a dropped *connection* tears down the session, so that is
  not resumable (§16, §17). A transfer the destination **refused** — no write permission on the
  folder — gets no Resume either: nothing was created, so there is nothing to pick up, and the
  notice simply says what the server said.
- **Timestamps are kept** — a transferred file keeps its **modification time** instead of being
  re-dated to "now", both when you upload and when you download, so a folder still sorts by date and
  a build still sees the right ages. Between two Unix machines the **permission bits** ride along too
  (a script stays executable); a Windows file has no Unix mode to carry, so only the timestamp
  travels. It is always on and never gets in the way — a server or disk that refuses the stamp is
  quietly skipped and the file itself is untouched (§17, §19).
- **Drag files in to upload them** — drag them off the desktop and drop them anywhere on the
  window; they upload into the **files pane's current directory**, reusing the same pre-scan and, on
  a name already there, the same **Overwrite / Keep both / Skip / Cancel** dialog the menu upload
  uses. While a drag is over the window the pane wears a **green ring** to say where the drop will
  land. Drop **any number of files and folders at once**: the files go as one batch, then each
  folder follows tree-and-all, one after another through the single progress bar — so a whole
  selection out of Explorer lands in one gesture. Dragging a remote file *out* onto the desktop is
  not offered: the GUI toolkit can receive an OS drop but cannot start one, so pulling files down
  stays the right-click **Download…** (§29).
- **The remote working directory in the window title** — cmote reads the `OSC 7` /
  `OSC 9;9` sequences shells emit on each prompt, so the title follows `cd` on POSIX *and*
  Windows remotes. fish and Windows Terminal-style prompts announce it themselves and are
  followed for free; a plain bash/zsh stays silent unless you add the prompt hook to your own
  shell config, in which case cmote picks it up too — cmote types nothing into the shell itself,
  so your command history stays clean (§17). When a program sets its own title (`OSC 0` /
  `OSC 2` — `vim` naming the file it is editing, say), that shows in the title bar instead; the
  host is always kept alongside so the window stays identifiable (§23).
- **Consistent dialogs** — the delete-target, disconnect, upload and overwrite
  confirmations, the host-key prompt, the passphrase prompt, and the error notice share
  one chrome: a header bar (question on the left, close ✕
  on the right, wired to the safe action), an explanatory body, and evenly-spaced footer
  buttons. Each **floats over the page it belongs to** (the connect-flow dialogs over the
  connect form, the disconnect modal over the shell) behind a dim backdrop; clicking the
  card never dismisses it (only a click outside does); the body message is **selectable and
  copyable** — drag to select, `Ctrl+C` to copy (handy for the host-key fingerprint or an
  error message); and the dialog is **draggable** by its header, clamped to the window (§10) — the
  header wears the **open hand** and closes it while you drag, the same cursor a tab does (§51).
- **Every copy says so** — any **Copy** (a menu item, a header's copy button, the details
  card's) raises a short toast at the bottom of the window that fades itself after three
  seconds, so a copy is never a silent no-op you have to test by pasting (§10).
- **Resuming where you left off** — a saved target remembers, per target, the **shell's
  directory**, the **files pane's directory**, the `.*` toggle, the **files-pane sort** (key and
  direction) and both **panel sizes**. On
  the next connection the pane reopens there, the tree reveals the chain down to it, and the
  shell is put back with a visible `cd`. The snapshot is written at every teardown — a clean
  Disconnect, a remote hangup, an error — and a value this session never learned never erases
  the one already saved. Profile metadata only: still **no secrets on disk** (§22).
- **Secrets** — passwords and key passphrases are held in memory and `zeroize`d on drop, and
  by default never written to disk (§12); only non-secret connection *profiles* are persisted,
  for the home list (§14). The one exception is the **opt-in** encrypted vault (§16): tick
  "Remember" and the secret is kept in `secrets.age`, sealed with a master passphrase you
  choose — portable across machines, and off unless you ask for it.

## Gestures and shortcuts

Everything the mouse and the keyboard do, by panel. The **focused** panel is the one that
gets a keystroke; a click focuses what it lands on, and the ring shows where the keyboard is.

**Anywhere in the window**

| Gesture | What it does |
|---|---|
| **Ctrl+Tab** / **Ctrl+Shift+Tab** | Move the keyboard to the next / previous panel — shell, folder tree, files pane (hidden panels are skipped) |
| Click a panel | Focus it |
| Type a letter while a panel has the keyboard | Hand it back to the shell and send that letter to the prompt — no panel answers to plain characters (Ctrl / Alt combinations stay the panel's) |
| Pick an item off the terminal's right-click menu | Do it, and put the keyboard back on the shell (opening or dismissing the menu does not) |
| **Ctrl+V** from anywhere | Paste into the shell and put the keyboard back on it — the menu's Paste off the keyboard |
| Hover anything grabbable — a **tab**, a **dialog header** | The pointer becomes an **open hand**; press and it **closes** until you let go, so the thing says it can be picked up and says when you have it. cmote draws both hands itself — Windows has neither. Splitters keep their **↔ / ↕** arrows: they resize rather than move, and the arrow says which way |
| Drag a tab along the strip | Move it to another slot; the chip that would receive it is outlined, and the order commits on the drop |
| **◨** / **⬓** at the right of the strip | Split the window beside / below — a fresh region on the target list, and the window doubles that way. Shown only while the window is whole: one split is the limit |
| Click a region | Give it the keyboard; its strip lights up |
| Drag a divider | Re-share the room between the two regions either side of it |
| Double-click a divider | Put the two regions back to an even share |
| **Ctrl+D** (home screen only) | Close the current tab; closing the last one asks to quit cmote. On a live shell it stays EOF to the remote instead |
| Window title-bar **×** | Ask **Quit cmote?**, then disconnect every session cleanly and exit |
| Drag a dialog's header | Move the dialog; **Esc** or ✕ takes the dialog's safe way out |
| Drag inside a dialog's body | Select its text; **Ctrl+C** copies it |

**Terminal (the shell)**

| Gesture | What it does |
|---|---|
| Drag across the grid | Select text (highlighted in place) |
| Right-click | Context menu: Copy / Paste / Upload… (into the shell's directory); on a link cell, **Open link / Copy link** too |
| **Ctrl+click** a link | Open an OSC 8 hyperlink in the browser (http/https/mailto only) |
| **Ctrl+C** | Copy the selection as styled HTML + plain text (rich paste keeps colours); with no selection, the shell's interrupt instead. Clears the selection after copying |
| **Ctrl+Shift+C** | Copy the selection as plain text only |
| **Ctrl+V** / **Ctrl+Shift+V** | Paste (bracketed-paste aware); both paste plain text |
| **Ctrl+Shift+F** | Open the scrollback find bar (pressed again, it refocuses the field). **↑** / **↓** step to the older / newer hit, wrapping; **Esc** or its ✕ closes it and leaves the last hit selected. Hits on screen are washed; the bar follows live output |
| **Ctrl+Shift+Up** / **Ctrl+Shift+Down** | Jump the view to the previous / next prompt (needs a shell with OSC 133 integration configured) |
| **Ctrl+Shift+O** | Select the last finished command's whole output; **press again** to step back a command at a time. A click on a prompt's gutter tick selects that command's output and the next press carries on back from there |
| **Copy / Paste** via the status-bar buttons or right-click menu | Same copy (rich) and paste |
| Click / drag / scroll **in a program that asked for the mouse** | Goes to that program (btop, vim, tmux, mc) instead of selecting |
| **Shift** + click or drag | Takes the pointer back: select text, or right-click for cmote's own menu |
| Any other key | Goes to the remote shell — arrows (SS3 form in application-cursor mode), **F1-F12**, **modified named keys** (Ctrl/Shift/Alt + arrows / Home / End / F-keys, F13-F24 included), **modifyOtherKeys** Ctrl/Alt combos (`CSI 27;…~`), and the **kitty keyboard protocol** (`CSI u`, incl. key-release events) when an editor turns either mode on |
| Drag either splitter | Resize the folder tree or the files pane; the pty is reflowed to match. The handle shows a resize cursor and lights up while hovered or dragged |
| **Sync** in the status bar | `cd` the shell to the folder the pane is showing (disabled when they already agree) |
| **Reveal** in the status bar | Jump the files pane and the folder tree to the folder the shell is in — nothing is typed at the shell (disabled when they already agree, when the strip is hidden, or when the shell has never announced its directory) |
| **Files…** / **Upload** in the status bar | Pick local files, then send them into the shell's directory |

**Folder tree** (right of the files pane, in the bottom strip — the status bar's **Folders** button hides it; shown only alongside the files pane)

| Gesture | What it does |
|---|---|
| Click a folder | Expand or collapse it, and select it |
| Right-click a folder | Open in terminal / Upload… / Rename… / Copy name / Copy relative path / Copy full path / Refresh |
| **↑** / **↓**, **Tab** / **Shift+Tab** | Walk the visible rows |
| **→** / **←** | Open / close the selected folder |
| **Enter** | `cd` the shell into it |
| **F2** | Rename in place (**Enter** commits, **Esc** abandons) |
| **F5** / header **↻** button | Refresh — re-list every open folder in one press |
| Header **collapse-all** button | Close every branch back to the top level |
| **Esc** | Give the keyboard back to the shell |
| Copy button in the header | Copy the path of the folder on show |
| `.*` checkbox | Hide or show dot-entries (shared with the files pane) |

**Files pane** (under everything — the status bar's **Files** button hides it)

| Gesture | What it does |
|---|---|
| Click an entry | Select it (and show its details popup) |
| Double-click a folder | Show it in the pane; the shell stays where it is |
| Click empty space | Clear the selection |
| **Drag** from empty space | Rubber-band selection; **Ctrl+drag** adds to what is selected |
| **Ctrl+click** | Add or remove one entry |
| **Shift+click** | Select everything between the anchor and here |
| **Ctrl+A** | Select every entry on show |
| Right-click an entry | The entry's menu — on a multiple selection it acts on all of it |
| Right-click empty space | Upload… here / Refresh |
| **Drag files from the desktop onto the window** | Upload them into the folder on show — any number of files and folders at once, the files as one batch then each folder tree-and-all; the pane rings green while they hover |
| Copy button in the details popup | Copy the whole details card |
| **←** / **→** | Move one cell; **↑** / **↓** move a whole row |
| **Shift** + those arrows | Extend the selection instead of moving it |
| **Tab** / **Shift+Tab** | Next / previous entry |
| **Enter** | Show the selected folder in the pane |
| **F2** | Rename in place |
| **F5** / header **↻** button | Refresh — re-list the directory on show |
| **Sort** button in the header | Menu: order by Name / Last modified / Extension / Size, Ascending or Descending; folders stay first. Pick the lit key to clear the sort, the lit direction to unset it (unset sorts ascending). Remembered per target |
| **Esc** | Give the keyboard back to the shell |
| ↑ button in the header | Show the parent directory |
| Copy button in the header | Copy the path of the directory on show |

**Home screen**

| Gesture | What it does |
|---|---|
| Click a target | Select it; click again (or **Enter**) to open it |
| Right-click a target | Open / Rename / Delete (deleting asks first) |
| Type in the filter box | Keep only the matching rows — a fragment, or a glob once you type `*` or `?`; name and endpoint both count |
| **Ctrl+F** | Put the cursor in the filter box |
| **Enter** (from the filter box) | Open the selected target without leaving the box |
| **Esc** | Empty the filter box (from inside it, press it twice: the first press leaves the field) |
| **F2** | Rename the selected target (**Enter** commits, **Esc** abandons) |
| **Delete** | Delete the selected target, after the confirmation (**Esc** cancels it) |
| **Tab** / **Shift+Tab** on the connect form | Move focus across the fields, the auth radios and Connect; **Enter** / **Space** activates the focused radio or button |

## Requirements

- **Rust** stable (developed against 1.91.0 on Windows, 1.97.1 on macOS).
- **Windows 11** — target `x86_64-pc-windows-msvc` and the **MSVC** toolchain (Visual
  Studio Build Tools with the VC++ x64 tools and the Windows SDK — the default MSVC
  linker). No NASM or C compiler: the `ring` crypto backend ships pre-generated
  assembly for this target (§2).
- **macOS Sequoia (Intel)** — target `x86_64-apple-darwin` and the **Xcode Command
  Line Tools** (`clang`), which compile `ring`'s crypto from source. No NASM (§2).
- No external SSH library on either target — the SSH stack is pure Rust (§12).

## Build and run

```sh
# Debug build and run
cargo run

# Optimized, self-contained portable binary
cargo build --release
# Windows → target/release/cmote.exe
# macOS   → target/release/cmote
```

On **Windows** the release `cmote.exe` is portable: copy it anywhere (including a USB
stick) and run it — no installer, no registry writes, no external runtime.

On **macOS** wrap the binary in a minimal app bundle so Finder launches it as a GUI
app (double-clicking a bare Unix binary would open a Terminal window instead):

```sh
cargo build --release
./bundle-macos.sh        # → target/release/cmote.app
open target/release/cmote.app
```

`cmote.app` is self-contained and relocatable — no installer or external runtime. It
is not code-signed or notarized, and will not be (a decision, not a to-do — §16), so the
first launch needs a right-click → **Open** to clear Gatekeeper's "unidentified developer"
prompt.

## Releases

Pushing a version tag (bare `MAJOR.MINOR.PATCH`, e.g. `4.0.0` — no `v` prefix, matching
the repo's tags) runs [`.github/workflows/release.yml`](.github/workflows/release.yml),
which builds the optimized binary on both targets and attaches these to a **draft** GitHub
Release:

- `cmote-<version>-x86_64-pc-windows-msvc.exe` — the portable Windows binary.
- `cmote-<version>-x86_64-apple-darwin.app.zip` — the macOS Intel `cmote.app`, zipped.
- `SHA256SUMS` — a SHA-256 for each, so a download can be verified (`sha256sum -c
  SHA256SUMS`, or `shasum -a 256 -c SHA256SUMS` on macOS).

The release is left as a **draft** to review before publishing. The artifacts are **not**
code-signed or notarized, and this is a decision rather than a pending task (§16): an ordinary
certificate would not silence SmartScreen anyway (it warns on reputation, not on the absence of
a signature), and a signing key in CI is a secret worth more than it saves. So `SHA256SUMS` is
**the** integrity check, not a stand-in for one — verify the download against it. The cost is
that a fresh download trips SmartScreen ("More info" → "Run anyway") and, on macOS, Gatekeeper
(right-click → **Open** the first time). There is no auto-update for the same reason: the update
path is to download the next release.

### Cutting a release

The build is driven **entirely by the tag push** — creating a release by hand in the web UI
builds nothing, and GitHub does not create the git tag for such a draft until you publish it.
So always start from the tag:

1. Make sure `main` is green and `version` in `Cargo.toml` matches the tag you are about to cut.
2. *(optional)* Dry-run first, no tag: **Actions → Release → Run workflow** (or `gh workflow run
   release.yml --ref main`). Both targets build and package; nothing is published — this is the
   `workflow_dispatch` path, there to exercise the pipeline before a real tag.
3. Tag the release commit and push — **this is what fires the workflow**:

   ```sh
   git tag -a 4.0.0 -m "cmote 4.0.0"
   git push origin 4.0.0
   ```

4. The workflow builds both targets and opens a **draft** Release with the three assets above.
5. Review the draft — confirm the assets are attached and edit the notes — then **Publish**.

Because the git tag is created by the push (not by saving a draft), the workflow and the release
have a single owner: the tag. Do **not** also hand-create a release for the same tag, or the two
collide (GitHub allows only one release per tag).

## Data and portability

cmote writes up to four files — `known_hosts` (pinned host keys), `targets.json` (saved
connection profiles plus where each session left off: the two directories, the `.*` toggle
and the panel sizes — **no secrets**), `settings.json` (the app-wide window size, remembered
across restarts — §31), and, only once you opt in to remembering a secret,
`secrets.age` (the encrypted credential vault, §16 — a master-passphrase-sealed `age` blob,
the sole place any secret is stored). All live in the same directory, resolved at
runtime (§11, `paths::data_dir`):

1. **Portable mode (preferred):** `cmote-data/` beside the binary, when that directory
   is writable. This keeps the data travelling with the app — on macOS the binary lives
   in `cmote.app/Contents/MacOS/`, so the store sits inside the bundle.
2. **Fallback (Windows):** `%LOCALAPPDATA%\cmote\` when the exe sits in a read-only
   location (e.g. `Program Files`); on macOS `~/Library/Application Support/cmote/`.

To reset trust for a host, delete the offending line (or the whole file) from
`known_hosts`. To drop a saved target, use right-click → Delete in the app and confirm
the prompt (or delete its entry from `targets.json`) — deleting a target also forgets its
vault secret when the vault is unlocked. To forget every remembered secret at once, delete
`secrets.age`; a forgotten master passphrase is unrecoverable, so this is also the only way
back in if you lose it (you keep the profiles, just re-enter the secrets).

## Testing

Pure logic is unit-tested; anything needing a live server is manual (§13). No test
framework is pulled in — everything uses Rust's built-in `#[test]` / `#[cfg(test)]`.

```sh
cargo test          # run the unit tests
cargo fmt           # format (rustfmt, hard tabs — see rustfmt.toml)
cargo clippy --all-targets -- -D warnings
```

**CI** (`.github/workflows/ci.yml`) runs these same gates on every push and pull
request to `main`, on **both** targets — `cargo fmt --check` plus `cargo clippy -D
warnings` and `cargo test` on Windows (`x86_64-pc-windows-msvc`) and macOS
(clippy against the Intel target `x86_64-apple-darwin`, tests native on the runner).
It also audits the dependency tree: `cargo audit` for RustSec advisories and
`cargo deny` (see `deny.toml`) for the license allow-list, banned crates (no
`aws-lc-*` — keeps the NASM-free portable build, §12), and trusted sources.

Automated coverage: key parsing (the `.ppk` header sniff, Ed25519 `.ppk` loaded plain and
encrypted, the encrypted-key passphrase re-ask paths, and a non-`.ppk` blob routed to the
OpenSSH loader — the `.ppk` fixtures are Ed25519, the format `from_ppk` also reads RSA / ECDSA /
DSA §7), certificate loading (an Ed25519 `-cert.pub` parsed, a non-certificate file refused,
and the `<key>-cert.pub` sibling derivation §7), host-key match/unknown/mismatch decisions and
fingerprint formatting, terminal byte-stream → grid, key-event → byte-sequence
mapping (including application-cursor-mode arrow keys, CSI vs SS3, every F1-F12
against the terminfo entry, the modified named keys — Ctrl/Shift/Alt + arrows /
navigation / F-keys and F13-F24 — and modifyOtherKeys, both the stream scanner that
detects the mode and the `CSI 27;mod;code~` encoding it switches on, and the kitty keyboard
protocol per flag — disambiguate, event types incl. release, report-all, associated text and
alternate keys — with the seam reading the pushed flags back and the engine answering the
`CSI ? u` query, §25), the terminal engine's
wiring end to end (an `f`-spelling move
lands in its own cell, a wide glyph reserves two columns, and the engine's query replies are
drained and sent back — device status, device attributes, a live cursor-position report, the
save/jump/report/restore size-probe reporting the clamped corner, and a query split across
two chunks answered on completion), inline sixel images (the decoder per command, per colour space —
RGB percentages, DEC's blue-origin HLS — and per memory cap, the stream scanner across chunk
boundaries, the cells a picture reserves and erases, its anchor surviving the scroll into history,
the erase and reset rules, the alternate-screen refusal, and the two capability answers §41),
pointer-event → mouse-report encoding (each encoding, each
mode's gating, the classic form's 223-column ceiling, the wheel, the modifier bits), the
grid's run packing and the geometry of the glyphs it draws itself (a braille
cell read back as its dot pattern, a rounded corner's arc and tails measured against a real
cell), the OSC 8 hyperlink surfaced on its cells and the link scheme allow-list (http/https/
mailto through, `file:` / `vscode:` / `javascript:` and a scheme-less URI refused, §24), the grid-resize
math, mouse-selection geometry and text extraction (wide
glyphs, trailing-blank trimming, multi-row joins, and — §42 — the multi-click tally, the word rule over
paths / URLs / endpoints / separators, a word and a copy carried across a line wrap, and the whole
logical line a triple click takes, plus §43's reflow clean-up: the selection dropped, the find bar
re-scanned and the click tally started over, and §44's live find bar: output marking the match list
stale rather than scanning per chunk, the deferred scan picking the new hit up, and a re-scan moving
neither the viewport nor the selection), §45's account machinery, which outlived the UI that drove it (the command built for
`sudo` and `su`, an account name that could be read as a flag or as shell punctuation refused before it
reaches a command line, a credential prompt told from a shell prompt from a finished line and from a
coloured one, a refusal read from the program's own line rather than from a question repeating, a whole
view swapped and restored on a switch, a parked account's output filling its own scrollback and its
queries answered on its own channel, an account's greeting kept whichever order it arrives in, and an
elevated shell exiting falling back to the login account), §46's file layer behind it (a password
written only after sudo has been refused for the want of one and never on a guess, the `sftp-server`
path taken from the remote's own configuration only when it whitelists as a program, `internal-sftp`
and a doctored path refused, the login account's commands still going out byte for byte, both panes
re-read on a switch with the other account's names dropped and the folder kept, an account that needed a
second factor having its files refused up front with a reason instead of two dead handshakes, and a file
opened as root still saved as root after switching back), paste encoding (bracketed-paste
wrapping and the injection-terminator scrub), the remote-cwd scanner (OSC 7 and
OSC 9;9, split across chunks, percent-escapes, Windows paths, oversized payloads), and
the folder tree's model (row flattening and indentation, the hidden-folder filter,
subtree collapse, `cd` reveal and its no-op on a repeat, rename validation and the
post-rename refresh, relative-path arithmetic, shell quoting, and the panel's width
clamps), and the files pane's model (batch accumulation and the dropping of batches for a
directory already left, the cwd-follow rule that a repeated announcement is not a move,
the folders-first sort, icon categories from kind and extension, rename validation, and
the pane's height clamps). The keyboard and selection work adds: the arrow walk across both
panels (clamping at both ends, skipping hidden entries, and not panicking on an empty
directory), the keep-it-visible scroll rule (including an item taller than its viewport),
MIME types from extensions and their `application/octet-stream` fallback, mtime rendering in
a server timezone (the epoch, a leap day and both sides of Greenwich), the `date +'%z %Z'`
and `ls -l` `longname` parsers with their half-answer fallbacks, the link target belonging
to the selection that asked for it, the selection gestures (range from an anchor, toggle,
plain and additive band), the rubber band's hit-testing against the grid geometry (scrolled,
past the end of the listing, and in the gap between two rows), and — through the app's own
handlers rather than the model's — Shift+click and Shift+arrow, which is what proves the
modifier state reaches a mouse press. The upload, path-eliding and resume work adds: the
upload batch planner (every file queued under its own name when nothing clashes, and each
collision answer — Replace, Skip, Keep both — deciding what happens to each clashing file,
all without an App or a server), the middle-ellipsis cut (a short string left alone, a long
one keeping both ends inside its budget, and the cut never landing inside a glyph) and the
grid cell's two-line version of it, the short mtime that keeps the server's wall-clock shift
but drops the seconds and the zone tag, the session snapshot's round trip through
`targets.json` (including a pre-v2.2 file
with no session fields at all), and — again through the app's own handlers — a reconnect
that resumes both paths and pins the pane until the shell has caught up.

### Manual smoke test (live SSH)

There is no CI SSH server in v1, so the end-to-end path is verified by hand against a
local `sshd`. Any reachable server works; the steps below use Docker for a disposable
one.

**1. Start a throwaway server** (creates user `tester` / password `testpass` on port
`2222`):

```sh
docker run --rm -d --name cmote-sshd -p 2222:22 \
  -e USER_NAME=tester -e USER_PASSWORD=testpass -e PASSWORD_ACCESS=true \
  linuxserver/openssh-server
```

(Or use WSL / any host you control. On a native Windows OpenSSH server, connect to
`localhost:22`.)

**2. Password auth + first-contact host key.** Run `cargo run`, enter `localhost`,
port `2222`, user `tester`, choose **Password**, type `testpass`, connect. **Tab** /
**Shift+Tab** should move focus across every control — the fields, the four auth radios, and
the Connect button (the active radio/button shows a highlight ring); **Enter/Space**
activates the focused radio or button. Expect:

- The **Unknown host key** dialog appears once, showing a SHA-256 fingerprint. You can
  drag it by its header, select the fingerprint and copy it (`Ctrl+C`), and closing (✕)
  rejects. Accept → the shell opens; the fingerprint is now pinned in `known_hosts`.
- Reconnecting no longer prompts (the key matches the pinned one).

**3. Terminal behaviour.** In the shell: run `ls`, `echo hi`, an interactive program
(`top`, then `q`), and **Ctrl-C** to interrupt. Print bold text
(`printf '\033[1mBOLD\033[0m normal\n'`) and confirm the bold run is visibly heavier
than the normal one (both weights are bundled — §9). Print the other styles
(`printf '\033[2mfaint\033[0m \033[3mitalic\033[0m \033[9mstruck\033[0m \033[4munder\033[0m \033[4:3mcurly\033[0m\n'`)
and confirm faint reads dimmer, italic slants (in IBM Plex Mono, §23), struck has a line
through it, and the two underlines differ — one straight, one wavy (§23). Ask the terminal its
background colour (`printf '\033]11;?\033\\'`): it replies on the input channel, so at a bash
prompt the answer `rgb:1e1e/1e1e/1e1e` appears as if typed — proof it reports what it draws (§23).
Change the cursor's shape (`printf '\033[6 q'` for a bar, `\033[4 q` an underline, `\033[2 q`
back to a block) and confirm it redraws each time — the shape a program like vim would pick (§23).
Turn on focus reporting (`printf '\033[?1004h'`), then click to another window and back: each
switch types a short `^[[O` / `^[[I` at the prompt, and tabbing the keyboard to the file tree
and back does the same — proof the remote is told (turn it off again with `printf '\033[?1004l'`,
§23). Fill the screen with history (`seq 1 200`), then **scroll back**: the mouse wheel and
**Shift+PageUp/PageDown** move up through the run, **Shift+Home** jumps to the oldest line and
**Shift+End** back to the bottom, and typing any key snaps you back to the prompt (§23). As you
scroll up a thin bar appears at the right edge showing your place in the history — it is longer for
a shallow history and shorter for a deep one — and vanishes the instant you are back at the live
bottom. Scroll up,
then run something that prints (`sleep 2; echo done` in another split, or just wait for a clock) —
the view stays where you are reading rather than jumping to the new output. Open `less /etc/services`
or `vim`, scroll with the wheel, and confirm it pages the *program* (its own alternate screen has no
cmote scrollback), then quit and confirm the wheel scrolls cmote's history again (§23). Print wide glyphs over aligned
columns (e.g. `printf '12\n世b\n'`) and confirm the character after a CJK/emoji glyph
stays in its column — a wide glyph reserves two cells (§9). Resize the window and run
`tput cols; tput lines` (or `stty size`) — the reported size should track the window.
With **NumLock on**, type a command using the **numpad** digits (e.g. `echo 2` /
`pm2 ls`) and confirm the digits appear; with **NumLock off**, the numpad arrows
(2/4/6/8) should move the cursor instead of typing digits (§9). Click **Disconnect**
→ you return to the form immediately.

**4. Key auth.** Generate a test key and authorize it:

```sh
ssh-keygen -t ed25519 -f ./smoke_key -N ""                 # unencrypted
ssh-keygen -t ed25519 -f ./smoke_key_enc -N "hunter2"      # encrypted
# copy the .pub of each into the server's ~tester/.ssh/authorized_keys
```

- **Unencrypted key:** choose **Key**, browse to `smoke_key`, connect → shell opens
  with no passphrase prompt.
- **Encrypted key:** browse to `smoke_key_enc`, connect → the **Encrypted key**
  screen appears with the field already focused; type `hunter2` → shell opens. Enter
  a **wrong** passphrase first to confirm the prompt simply re-appears (bounded
  re-ask) before the correct one succeeds.
- **PuTTY `.ppk`:** convert a key with PuTTYgen and repeat — both encrypted and
  unencrypted `.ppk` should behave like the OpenSSH cases.
- **OpenSSH certificate:** sign the key with a CA and trust that CA on the server:
  ```sh
  ssh-keygen -t ed25519 -f ./smoke_ca -N ""                        # a throwaway CA
  ssh-keygen -s ./smoke_ca -I tester -n tester ./smoke_key.pub     # writes smoke_key-cert.pub
  # on the server: echo "@cert-authority *  $(cat smoke_ca.pub)" >> ~tester/.ssh/authorized_keys  (or TrustedUserCAKeys)
  ```
  Choose **Key** and browse to `smoke_key` — the **Certificate** field should auto-fill with
  `smoke_key-cert.pub` (the sibling) — then connect → shell opens. Click **Clear** and connect
  again to confirm it falls back to plain key auth. Reopen the saved target and confirm the
  certificate path comes back pre-filled.
- **SSH agent / Pageant:** load `smoke_key` into an agent (`ssh-add ./smoke_key`, or add it
  in Pageant on Windows), choose **Agent** — no fields appear — and connect → shell opens with
  no file to pick and no passphrase. Stop the agent (or empty it) and retry: expect a clear
  "no SSH agent found" / "no keys to offer" message, not a hang.

**5. Host-key mismatch (override dialog).** Delete the server container and start a fresh
one (new host key) on the same port, then reconnect. Expect the loud **Host key has CHANGED**
dialog: a red possible-MITM line and **both** SHA-256 fingerprints (stored vs presented,
selectable/copyable), with **Reject** / **Trust once** / **Replace key**. Closing (✕) or a
backdrop click rejects. **Trust once** connects without touching `known_hosts` (reconnect and it
warns again); **Replace key** pins the new key (reconnect and it is silent). A changed key is
never trusted without an explicit click (§8, §28).

**6. Selection, copy, and paste.** In the shell, run `echo hello world`, then drag
across the output to select it — the selection should highlight and **Copy** (status
bar or right-click menu) should enable. Copy, then **Paste**: the text lands at the
shell's cursor. Paste into a bracketed-paste-aware shell (bash/zsh with readline) and
confirm a multi-line clipboard does **not** auto-run each line (bracketed paste frames
it). Right-click anywhere to confirm the context menu opens at the cursor and dismisses
on a click away. Copy is disabled with nothing selected; pasting keeps the highlight.

**7. Remote directory + upload.** On a shell that announces its directory (fish, a Windows
OSC 9;9 prompt, or a bash/zsh with the OSC 7 prompt hook in its own config) the window title
should read `cmote — tester@localhost:2222 — /config` (or wherever the shell starts), and `cd /tmp`
should update it within a prompt — cmote types **nothing** into the shell, so a plain bash/zsh with
no such hook shows no directory in the title and leaves the command history untouched (§17). Set a title from a program
(`printf '\033]2;my title\033\\'`) and the bar should switch to `cmote — tester@localhost:2222
— my title`; clearing it (`printf '\033]2;\033\\'`) brings the directory back (§23). Then:

- Click **Files…**, pick a local file — its name appears next to the buttons and **Upload**
  becomes enabled. Click **Upload**: the dialog lists the file under an editable destination
  folder of `/tmp`. Confirm → a progress bar with the byte count runs in the status bar, then
  `Uploaded to /tmp/<name>`, and the pick is cleared (Upload disabled again). `ls -l /tmp` on
  the remote should show it, byte-for-byte identical (`sha256sum` both ends for a binary
  file).
- Pick **several files at once** and upload them → they go one at a time, each with its own
  progress bar, and the closing notice reads `Uploaded N files`.
- Upload the **same batch again** → the collision dialog names the files that are already
  there: **Cancel** sends nothing (check the remote `mtime`s), **Skip** leaves them alone,
  **Keep both** writes `name-1.ext` beside them, **Replace** overwrites.
- Try the other three ways in — right-click the **terminal** → **Upload…** (destination is
  the shell's directory), right-click the files pane's **empty space** → **Upload… here**
  (the pane's directory, so point the pane elsewhere with the tree first and confirm the
  destination follows the *pane*, not the shell), and right-click a **folder in the tree** →
  **Upload…** (that folder).
- **Drag files in.** Point the pane at a folder (a tree click, or `cd` in the shell), then drag
  files from the OS file manager over the window — the pane rings **green** — and drop them. They
  upload into the pane's folder with no destination dialog, and **the pane re-lists on its own** so
  the new files appear in the grid without a manual Refresh (`ls` there confirms it). Drop several
  at once → one batch, one collision question. Drop a file whose name is already there → the same
  **Overwrite / Keep both / Skip / Cancel** dialog. Drop **folders** → each uploads tree-and-all,
  exactly as **Upload folder…** does, one after the other. Drop files and folders **together** →
  the files go first, then the folders; the closing notice says how many of each landed. Drop while
  a transfer is running → declined with the busy note. (Dragging a file *out* of the pane onto the
  desktop is not offered — use **Download…**.)
- Edit the destination in the dialog to a directory you cannot write (`/etc/x`) → the
  status bar shows the failure and the shell stays open.
- Start a shell that does **not** announce its directory (a plain `bash --norc`, or
  `docker exec … sh`) → the title drops the directory and the upload dialog offers an empty
  folder, which lands in the login directory.
- **Sort the grid.** Point the pane at a mixed directory (`/usr/bin` is a good crowded one, or
  make one: `mkdir /tmp/s && cd /tmp/s && mkdir b_dir a_dir && head -c 3000 /dev/zero > big.log
  && echo hi > small.txt && echo x > mid.md`). Click the header's **sort** button → the menu
  drops **below** the toolbar, right beside the button, with four keys and — by default —
  **neither** direction ticked. Pick **Name** with no direction → the button lights up, folders
  stay grouped at the top, and each group runs A→Z (an unset direction sorts ascending). Pick
  **Size**, then **Descending** — the files reorder biggest-first; the menu stays open so you can
  set both halves without reopening it. Pick the **lit** **Descending** again → it unsets, back to
  ascending. Pick **Extension** → files group by their `.log`/`.md`/`.txt` ending. Pick the **lit**
  key again → the sort clears, the button dims, and the grid returns to the default
  folders-first-by-name order. Click away → the menu closes. The chosen sort **sticks across
  directories** (browse elsewhere and back) until you clear it, and re-listing (**F5**) keeps it.
- **The sort is remembered per target.** With a sort set (say **Size**, **Descending**),
  **Disconnect** and reconnect to the same target → the pane reopens already sorted that way, the
  button lit. Clearing the sort before disconnecting reopens in the default order. (A target you
  never sorted, and a `targets.json` from before this existed, reopen unsorted.)

**8. Remote folder tree.** The panel on the right should list `/` on connect. Then:

- Click folders to expand and collapse them; a slow directory shows `·` until its listing
  arrives. Expand a few levels, collapse the top one, re-open it — it shows exactly one clean
  level again. Opening always re-lists, so a shell-side change under a collapsed folder is
  caught: with a folder open, `mv one-of-its-children ../elsewhere/` in the terminal, collapse
  that folder, then click it open again → the moved child is gone (the cached rows draw at once,
  then the fresh listing replaces them).
- `cd /etc/ssh` in the shell → the tree opens `/` → `/etc` → `/etc/ssh` on its own and
  highlights it. `cd` back and forth: the tree only ever expands, never closes what you
  opened.
- Toggle the `.*` checkbox in the panel header → dot-folders (`.ssh`, `.config`) disappear and
  reappear with no round trip.
- Right-click a folder: **Open in terminal** should run a quoted `cd` in the shell (make a
  folder with a space and a quote in its name — `mkdir "/tmp/it's here"` — and confirm the
  `cd` still lands in it). **Copy full path** / **Copy relative path** / **Copy name**
  should put the right text on the clipboard (paste into the shell to check; the relative
  item is greyed out on a shell that never announces its directory).
- **Rename…** turns the row into a field: Esc abandons, Enter commits and the row
  reappears sorted under its new name. Rename onto an existing name → the notice line
  under the tree says it already exists and nothing changed. Rename a folder you cannot
  write → the notice shows the refusal and the shell stays open.
- **Refresh a change made from the shell.** With a folder expanded, run `mkdir a/new` in the
  terminal → the tree does not update on its own. Right-click the folder → **Refresh** re-lists it;
  the new child appears. Now `mv a a2` in the shell and **Refresh** the (now stale) `a` row → it
  disappears and `a2` shows in its place: Refresh re-checks the folder's own name and existence
  (via its parent), not just its contents. Or press the header **↻** button (or **F5** with the
  tree focused) → every open folder re-lists at once. A collapsed branch is left closed —
  re-opening it shows the fresh listing.
- **Collapse all.** Expand several levels, then press the header **collapse-all** button → the
  tree snaps back to the root's own children, and re-opening any branch draws its cached rows
  instantly (then re-lists in the background to catch any change).
- Drag the splitter left and right: the grid reflows (`tput cols` should follow) and the
  panel stops at its minimum and at 60% of the window. The **Folders** button hides the
  panel and gives its columns back to the grid.
- Against a server with the sftp subsystem disabled (`Subsystem sftp` commented out in
  `sshd_config`), the tree should still list folders — the `ls` fallback (§18).

**9. Remote files pane.** The grid across the bottom should fill with `/` on connect,
then follow the shell. Then:

- `cd /etc` → the grid shows every entry in `/etc` with an icon per type; the header names
  the directory and counts the entries. Create a directory with thousands of files
  (`mkdir /tmp/many && cd /tmp/many && seq 1 5000 | xargs touch`) and re-enter it: the
  count should climb in steps of 1000 as the batches land, and the window stays responsive
  throughout.
- Each cell should read as a row: icon, name, and under it the size, the modified date
  on the server's wall clock but with **no zone tag**, then the permission word (`ls -l`
  style, `-rw-r--r--`) and the `owner:group` (a folder shows only the date, the mode and the
  owner), each **framed by a thin border** so the grid reads as distinct tiles. Compare a few
  against `ls -l` on the remote — the clock, the size, the mode and the owner should
  match; the zone tag lives in the details popup. A very long
  name should be cut in the middle (`report-fin…-draft.pdf`), never at the extension.
- Double-click a folder in the grid → the grid enters it and **the shell stays put** (the
  prompt's directory does not change, and the pane must NOT snap back on the next prompt).
  Same for the header's **up** button and **Enter**.
- Click a folder in the **tree** → the grid shows that folder while the shell stays where
  it is, and it must NOT snap back on the next prompt. Click **Sync** in the status bar →
  now the shell `cd`s there, the tree reveals it and the title follows; Sync greys out once
  the two agree.
- The other direction: `cd /var/log` in the shell, then click a different folder in the tree
  and collapse the branch the shell is in → **Reveal** lights up. Press it → the pane lists
  `/var/log`, the tree opens the chain down to it and selects it, **no `cd` is typed** (the
  prompt does not move and the shell prints nothing), and Reveal greys out. Press **Enter** at
  the prompt → the pane must stay on `/var/log` rather than jump, since the shell has not moved.
  Hide the pane with the **Files** button → Reveal greys out with it.
- Toggle the `.*` checkbox → dot-files disappear from the grid and the tree together.
- Right-click a file → **Download…** opens the save dialog; pick a path and the status bar
  runs a progress bar, then reports where it landed. Downloading onto an existing local
  file goes through the OS dialog's own replace prompt. **Open in terminal** is greyed out
  on a file, **Download…** on a folder.
- **Rename…** edits the label in place; Enter commits and the grid re-lists so the entry
  lands in its new sort position. **Refresh** (the menu item, the header **↻** button, or
  **F5** with the pane focused) picks up a file created from the shell.
- Drag the horizontal splitter: the grid reflows (`tput lines` should follow) and stops at
  the minimum and at 60% of the window. The **Files** button hides the pane and gives its
  rows back to the terminal.

**10. Keyboard focus and the details popup.** Press **Ctrl+Tab** repeatedly: the focus ring
should go shell → tree → files pane → shell. Hide one panel with its status-bar button and
cycle again — the hidden stop is skipped. Then, in the files pane:

- Walk with the **arrows**: left/right move one cell, up/down a whole row, and both ends
  clamp instead of wrapping. Keep going past the bottom of the pane — the grid should scroll
  only when the selection reaches an edge, never re-centre. **Tab** / **Shift+Tab** step
  next and previous. **Esc** hands the keyboard back to the shell (type at the prompt to
  confirm).
- With an entry selected, the popup beside it should name the entry in full, and show the
  MIME type (`text/x-python` on a `.py`, `application/octet-stream` on something unknown),
  the time, the size, the permission word and `owner:group`. Compare the time, the mode and
  the owner against `ls -l` on
  the remote — they should agree, including the timezone. Select a symlink
  (`ln -s /etc /tmp/link-to-etc`) → the popup adds `→ /etc` a moment later.
- **Enter** on a folder enters it; **F2** renames in place.
- **Type while the pane (or the tree) has the ring**: `whoami` → the ring jumps to the shell on
  the `w` and the whole word lands at the prompt, `w` included. Then **Ctrl+A** with the pane
  focused again → it still selects the whole listing rather than typing an `a` (§50).
- With a panel focused, right-click the terminal and pick **Paste** (put something on the
  clipboard first) → the text lands at the prompt and the ring is back on the shell, so **Enter**
  runs it. **Ctrl+V** with the panel focused should do exactly the same. Right-click and press
  **Esc** instead → the menu closes and the panel keeps the keyboard.

**11. Selecting many entries.** In a directory with a dozen or so entries:

- Drag from **empty space** across several cells — a translucent rectangle follows the
  pointer and everything it touches highlights. Release outside the pane (over the terminal)
  and the band should end there, not keep selecting when the pointer comes back.
- **Ctrl+drag** a second band: it adds to the first selection instead of replacing it.
  **Ctrl+click** toggles one entry, **Shift+click** takes the run between two, **Shift+←/→/↑/↓**
  extends from the anchor, **Ctrl+A** takes them all. The popup should switch to
  `N items selected` with the folders/files split and the total size.
- Right-click **inside** the selection → the copy items carry the count; **Copy full path**
  should paste one path per line into the shell. **Rename…** and **Open in terminal** are
  greyed out. Right-click **outside** the selection → it collapses to that one entry first.
- Select several files (a folder among them is fine) → **Download… (N)** asks for a
  destination folder, downloads them one at a time with the progress bar, skips the folder,
  and finishes with `Saved N files`. Run it again into the same folder → the
  **Some of these files are already there** dialog lists the names: **Cancel** downloads
  nothing, **Skip them** leaves the local copies untouched, **Save alongside** writes
  `name-1.ext`, **Replace** overwrites. Check the results with `ls -l` locally.

**12. Full-screen apps.** Run `vim` (or `less` on a long file). The file should render, and
the **arrow keys** should move the cursor — this exercises application cursor mode (DECCKM):
the app enables it and cmote switches its arrow keys to the SS3 form so they register. In
`vim`, `:q!` to exit. Then the query answering and the harder cases:

- **Cursor-position probe.** At the shell, run
  `printf '\033[6n'; read -rsdR r; echo "cursor: ${r#*[}"`. It should print the cursor's
  row;col at once and return to a prompt — if cmote did not answer, `read` would hang until
  you press Enter. Then measure the screen:
  `printf '\0337\033[999;999H\033[6n\0338'; read -rsdR r; echo "size: ${r#*[}"` should report
  the terminal's actual rows;cols (resize the window and repeat — it should track). A program
  like `vim` or `tmux` should now open without the ~1s startup pause its DA probe used to
  cost.

- Run **btop** (`brew install btop` on a mac remote). Every panel should sit in its own box
  where it belongs — no line running on into the next, no frame drawn twice down the screen.
  btop positions its whole UI with cursor moves the previous engine could not follow; the
  VT engine (§23) interprets them, so the layout lands where it belongs.
- Its **graphs** should be dot patterns, evenly spaced inside their cells, and its **box
  corners** should be rounded and meet the straight lines cleanly. Both are drawn from
  geometry, not shaped from a font — no monospace font we could bundle has braille at all.
- Press **F2**: btop's options menu should open. **Esc** closes it. (F1-F12 are mapped to
  the `xterm-256color` terminfo entry.)
- **Click** a process row — btop selects it. **Scroll** over the process list. Drag one of
  its sliders. Then hold **Shift** and drag across the screen: you should get cmote's own
  text selection instead, and Shift+right-click should open cmote's menu. Release Shift and
  the pointer belongs to btop again. Quit with `q`.
- Run **htop** and **mc** (midnight commander) for a second opinion on both — mc lives on
  F1-F10 and is entirely mouse-driven.
- **Modified keys.** At the shell (bash/zsh), hold **Ctrl** and press **Left** / **Right**:
  the cursor should jump a whole word, not one character — this is the modified named-key
  encoding (`ESC[1;5C`), which was silently dropped before. In `vim`, **Shift+arrow** should
  extend a visual selection. (Bare **Shift+PageUp/PageDown/Home/End** still page cmote's own
  scrollback, §23 — that binding wins over the shell on purpose; the Ctrl/Alt variants reach
  the shell.)
- **modifyOtherKeys.** In **neovim** (or vim with `:set modifyOtherKeys=2`), the Ctrl-combos a
  plain terminal cannot send now arrive: try mapping one, e.g. `:nnoremap <C-,> :echo "got it"<CR>`
  then press **Ctrl+,** — it should fire, where in a stock terminal it does nothing. Ctrl+letter
  bindings keep working too. Back at the bash prompt (mode off), **Ctrl+C** must still interrupt
  as always — the mode is the editor's to turn on, and off by default.
- **Clickable links (OSC 8).** At the shell, emit a link:
  `printf '\e]8;;https://example.com\e\\click me\e]8;;\e\\\n'`. Hold **Ctrl** and move over *click
  me* — the whole link underlines under the pointer and the underline follows it, appearing and
  vanishing as you press and release Ctrl. **Ctrl+click** the words *click
  me* — your browser should open example.com; **right-click** them for **Open link / Copy link**
  (Copy link should paste back `https://example.com`). Then emit a refused one —
  `printf '\e]8;;file:///c:/windows\e\\nope\e]8;;\e\\\n'` — and Ctrl+click it: nothing opens and
  a toast says the link was blocked (only http/https/mailto open, since the address is the
  remote's, §24). Plain text with no link is unaffected — Ctrl+click there just selects.
- **Kitty keyboard protocol.** In **neovim** on a recent build (it enables the protocol by
  default over a capable terminal) map a combo the legacy alphabet cannot spell, e.g.
  `:nnoremap <C-i> :echo "ctrl-i"<CR>` — Ctrl+I should now fire *without* also triggering Tab,
  which a stock terminal cannot tell apart. Esc should feel instant (no Alt-combo wait). To see
  it end to end without an editor: run `printf '\e[>1u'` to push the disambiguate flag, press
  **Esc** — the shell shows `^[[27u` instead of a bare escape — then `printf '\e[<u'` to pop it
  back. Back at the bash prompt (no flag pushed) everything types as before; the mode is the
  program's to turn on, off by default (§25).

**13. Copying, confirmed.** Click the copy button in the **files pane header**, then in the
**folder-tree header**, then the one on a selected entry's **details popup**. Each should
raise a toast at the bottom of the window that fades on its own after about three seconds,
and each should paste back what it promised — the pane's directory, the tree's folder, and
the whole details card (name, target, type, time, size, owner). The context menus' **Copy…**
items should raise the same toast.

**14. Resuming where you left off.** With a session open, `cd /etc/ssh` in the shell, point
the **pane** at a different directory (`/tmp` via the tree), toggle `.*` on, and drag both
splitters to unusual sizes. Then **Disconnect** and reconnect to the same target from the
home screen. Expect: the shell replays a visible `cd /etc/ssh`, the pane reopens on `/tmp`
(and does **not** get dragged to `/etc/ssh` by the shell's first announcement), the tree has
revealed the chain down to it, `.*` is still on, and both panels are the size you left them.
Kill the connection the hard way too (`docker stop cmote-sshd`, or `pkill sshd` on the
remote) and reconnect — the snapshot should survive a hangup, not just a clean disconnect.
Finally, connect to a target saved by an older build (or delete the session fields from
`targets.json` by hand): it should open at the login directory with default panels, no error.

**15. Tabs.** Connect a session, then click **"+"** on the strip: a new **Home** tab opens and
takes over the window while the first shell keeps running behind it. Connect a second target,
then click back to the first tab — its shell should be exactly where you left it (run a slow
command like `sleep 5; date` in the background tab, switch away and back, and its output should
have arrived). Resize the window with two live tabs and switch between them: each grid should
fit the window (no missing bottom row). Click a tab's **"×"**: an idle Home/Connect tab closes at
once, a **live** shell asks to confirm first. A rename or delete on one tab's home list
should be visible on another tab's home list (the target store is shared).

With two or more tabs open, check the **hand cursor**: move onto a chip → an open hand (not the
four-arrow move cursor, and not the pointing finger); press and HOLD without moving → it closes
immediately; drag onto another chip → still closed, and the target chip is outlined; release → it
opens again while the pointer is still on a chip. Slide off the strip onto the terminal → the hand
goes and the usual cursor is back. Move quickly back and forth across two chips (right to left as
well as left to right): the hand must not flicker off while the pointer is still on a chip. Close a
tab while the pointer sits on it, then move off the strip and back: the hand should be correct
again.

**16. Quitting cleanly.** With one tab left, click its **"×"** (or, from the home screen, press
**Ctrl+D**): instead of reopening a blank tab, cmote asks **Quit cmote?** — **Cancel** keeps the
window, **Quit** exits. With a **live** session, the dialog says how many will disconnect; on Quit
the shell should be torn down cleanly (`who` / the server's log shows a normal logout, not a reset)
before the window closes. Now the title-bar **×**: it too asks **Quit cmote?** — even with no live
session — and on confirm disconnects everything before exiting. While the **Quit cmote?** card is up,
**drag it by its header** — like every other dialog it moves and stays where you drop it (clamped so
the header never leaves the window); the same works on the per-tab **live-shell close** card. The
header should wear the **open hand** on hover and the **closed** one from the press to the release,
exactly as a tab chip does — including when the card slides out from under the pointer mid-drag,
which must not open the hand early. The body text keeps its I-beam and the ✕ its arrow. On a
**live** shell, **Ctrl+D** should reach the remote as EOF (it logs you out, landing back on the home
screen), *not* close the tab; a second **Ctrl+D** there then closes it.

**17. Port forwards.** In a live session click **Tunnels** on the status bar. Add a **Local**
forward — listen `8080`, to `localhost:22` (or any service the remote can reach) — and expect the
row to go from `○` to a green `●`; from your machine, `curl localhost:8080` (or `ssh -p 8080
localhost`) should reach it through the tunnel — and watch the row's **gauge** move as you connect:
`0 open · 0 total` becomes `1 open · 1 total` while a connection is live, then `0 open · 1 total`
once it closes. Add a **Dynamic** forward — listen `1080` — and
point a browser or `curl --socks5 localhost:1080 https://example.com` at it. Add a **Remote**
forward — listen `9090` on the server, to `localhost:<something-local>` — and connect to it *on the
remote*. Add another **Remote** with listen `0` (`-R 0`): the server picks a free port and the row
shows it (e.g. `R  127.0.0.1:38217 → …`); connect to that port on the remote to reach the local
service. Add a Local forward on a **bracketed IPv6** loopback — listen `[::1]:8081`, to
`localhost:22` — and reach it with `curl -g 'http://[::1]:8081'`; an *unbracketed* `::1:8081` is
refused inline with a message naming the bracket form. Try a duplicate bind (two locals on `8080`)
and a taken port: the first is refused inline,
the second shows the row **failed** without dropping the shell. Remove a forward with its **✕** — the
tunnel stops. Then **Disconnect** and reconnect to the same target: the forwards you left should be
re-established automatically (they are saved in `targets.json`).

**Cleanup:**

```sh
docker rm -f cmote-sshd
rm -f smoke_key smoke_key.pub smoke_key_enc smoke_key_enc.pub
```

## License

MIT — see [LICENSE](LICENSE).

Bundled fonts keep their own licenses (redistributed under them): **Fira Mono** and
**IBM Plex Mono** under the SIL Open Font License 1.1, and **Material Icons** under
Apache-2.0 — each with its license text in [assets/](assets/).
