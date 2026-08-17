// local/mod.rs — a session on THIS machine (PLAN §103).
//
// cmote is an SSH client, and every screen it has was built for a session on another machine: a
// terminal fed by a channel, a folder tree and a files pane fed by SFTP, an editor and a picture
// preview that read through the same connection. §103 asks for that same view of the LOCAL machine —
// a shell in the grid, the local filesystem in the panes — reached from a row of buttons on the home
// screen rather than through a connect form.
//
// The whole design is one observation: **the GUI never talks to SSH.** It talks to `bridge`, in
// `SshCommand`s out and `SshEvent`s in, and it is `ssh::client::run` that turns those into a
// connection. That command loop forwards almost everything to a `SessionMsg` channel without looking
// at it — so a session that consumes the same `SessionMsg`s and answers in the same `SshEvent`s is
// indistinguishable from a connection, as far as the thousands of lines above it are concerned. That
// is what this module is:
//
//   shells   which shells this machine can start, and where they live — the Local bar's contents
//   pty      a pseudo-terminal here, so `term` gets bytes exactly as it does off a channel
//   path     the one translation needed: the panes speak POSIX, Windows does not
//   fs       the panes' answers, from `std::fs` instead of SFTP
//   copy     the transfer queue's work when both ends are this machine
//   session  the `SessionMsg` loop — the twin of `ssh::client::stream`
//
// Nothing in `app`, `ui`, `term`, `files`, `explorer`, `editor`, `preview` or `transfer` knows a local
// session from a remote one, with two exceptions, both of which are honest rather than incidental: the
// status bar says `local — pwsh` where it would say `user@host:port`, and the features that are
// meaningless without a remote (another account, a port forward, shell integration) are refused with
// the reason rather than offered and left to fail.
//
// What is deliberately NOT reused: the SSH file layer's account machinery (§46). Not because it could
// not be bent to fit, but because it answers a question that does not exist here — a local session
// runs as the user, full stop, and there is no second account for the panes to be reading as.
// What IS reused is everything about copying that is not about the network: the tree walk, the resume
// arithmetic, the progress ticker, the collision answers (`ssh::transfer`, `ssh::upload::walk_local`).

pub mod copy;
pub mod fs;
pub mod path;
pub mod pty;
pub mod session;
pub mod shells;
