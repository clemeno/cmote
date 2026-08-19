// ssh/integration.rs — read and write the remote's shell config, so it announces its cwd (§17).
//
// The block itself, the marker that finds it again and the two string edits live in
// `crate::integration`, which is pure and knows nothing about a server. This is the errand: find
// the account's home directory and which shell it logs into, read the config file, and write the
// edited contents back. Nothing here decides WHAT to write.
//
// It runs on its own channel, like the editor's loads and saves (§32) and for the same reason: the
// shell channel is for keystrokes, and a config file has no business travelling down it. The write
// is `edit`'s atomic one — temp sibling, then rename — because a connection that drops mid-write
// must never leave a user with half a `.bashrc`, which is a login they cannot get back in through.
//
// Both file backends are supported (§46). The login account almost always has SFTP, but a remote
// with no `sftp-server` at all is exactly the kind of minimal box whose bash announces nothing, so
// refusing it here would turn away the case that needs this most.

use anyhow::{Context, Result, bail};
use tokio::sync::mpsc;

use crate::bridge::SshEvent;
use crate::explorer::shell_quote;
use crate::integration::{self, Shell};
use crate::ssh::asuser::AsuserFiles;
use crate::ssh::{edit, shellfs};

/// Where the accounts are recorded on a Unix host. Read to learn the login shell — the one answer
/// that is authoritative rather than guessed, and one cmote can get without typing at a prompt.
const PASSWD: &str = "/etc/passwd";

/// The config files probed for, in order, when `/etc/passwd` cannot answer (an LDAP/SSSD account
/// is not in the file). A file that EXISTS is decent evidence of the shell that reads it; zsh
/// comes first because a machine with a `.zshrc` almost certainly uses zsh, while a `.bashrc` is
/// left behind on accounts that never open bash.
const FALLBACKS: [(&str, Shell); 2] = [(".zshrc", Shell::Zsh), (".bashrc", Shell::Bash)];

/// Look at the login account's shell config and report what could be done to it (§17). Reads only:
/// the dialog shows the block and the file, and nothing is written until the user says so.
pub async fn probe(backend: AsuserFiles, events: &mpsc::Sender<SshEvent>, user: String) {
	let events = events.clone();
	tokio::spawn(async move {
		let outcome = look(&backend, &user).await;
		close(backend).await;
		match outcome {
			Ok((shell, path, installed)) => {
				let _ = events
					.send(SshEvent::IntegrationProbed {
						shell,
						path,
						installed,
					})
					.await;
			}
			Err(error) => {
				let _ = events
					.send(SshEvent::IntegrationFailed(format!("{error:#}")))
					.await;
			}
		}
	});
}

/// Put cmote's block into `path`, or cut it back out (§17). Reports the file's state AFTER the
/// write, so the dialog says what is now true rather than what was asked for.
pub async fn write(
	backend: AsuserFiles,
	events: &mpsc::Sender<SshEvent>,
	path: String,
	shell: Shell,
	install: bool,
) {
	let events = events.clone();
	tokio::spawn(async move {
		let outcome = edit_file(&backend, &path, shell, install).await;
		close(backend).await;
		match outcome {
			Ok(()) => {
				let _ = events
					.send(SshEvent::IntegrationWritten {
						path,
						installed: install,
					})
					.await;
			}
			Err(error) => {
				let _ = events
					.send(SshEvent::IntegrationFailed(format!("{error:#}")))
					.await;
			}
		}
	});
}

/// Work out which shell the account logs into, which file to write, and whether the block is
/// already there. The shell is `None` when nothing could establish it — a shell cmote has no block
/// for, or an account it cannot find — and the path then still names the file that WOULD be
/// written, so the dialog can say what it was looking at.
async fn look(backend: &AsuserFiles, user: &str) -> Result<(Option<Shell>, String, bool)> {
	let home = home(backend).await?;

	// The authoritative answer first: what `/etc/passwd` says this account logs into. A remote that
	// will not let us read it (or does not have it) is not an error — it is one source that could
	// not answer, so the fallback below gets its turn.
	let named = read_text(backend, PASSWD).await.ok().and_then(|passwd| {
		integration::login_shell(&passwd, user).and_then(Shell::from_login_shell)
	});

	// Failing that, the config files themselves are the evidence: a file that exists is read by the
	// shell that reads it.
	let shell = match named {
		Some(shell) => Some(shell),
		None => {
			let mut found = None;
			for (file, shell) in FALLBACKS {
				if exists(backend, &join(&home, file)).await {
					found = Some(shell);
					break;
				}
			}
			found
		}
	};

	// With no shell established there is no config file either, so the path names the home
	// directory: the dialog says what cmote was looking at and offers nothing.
	let Some(shell) = shell else {
		return Ok((None, home, false));
	};
	let path = join(&home, shell.rc_file());
	// A file that is not there yet is not installed — and is not an error either: installing simply
	// creates it.
	let installed = read_text(backend, &path)
		.await
		.is_ok_and(|contents| integration::installed(&contents));
	Ok((Some(shell), path, installed))
}

/// Read the file, apply the edit, write it back — the whole of an install or a removal. A file
/// that is already in the asked-for state is reported as an error rather than rewritten
/// needlessly, so the dialog never claims to have changed something it did not.
async fn edit_file(backend: &AsuserFiles, path: &str, shell: Shell, install: bool) -> Result<()> {
	// A missing file is empty, not a failure: installing into an account with no `.bashrc` yet
	// creates one, which is what the shell would read if it existed.
	let existing = read_text(backend, path).await.unwrap_or_default();
	let edited = if install {
		integration::install(&existing, shell)
			.with_context(|| format!("{path} already has cmote's block"))?
	} else {
		integration::remove(&existing).with_context(|| format!("cmote's block is not in {path}"))?
	};
	write_text(backend, path, &edited).await
}

/// The login account's home directory — the folder the config file sits in.
///
/// SFTP answers by resolving `.`, which for a freshly opened session IS the home directory: the
/// server starts every sftp session there. The shell backend asks the shell for `$HOME` instead,
/// which is the same answer by the other road.
async fn home(backend: &AsuserFiles) -> Result<String> {
	match backend {
		AsuserFiles::Sftp(sftp) => sftp
			.canonicalize(".".to_owned())
			.await
			.context("could not work out the home directory on the server"),
		AsuserFiles::Shell(runner) => {
			let home = runner
				.stdout("printf %s \"$HOME\"")
				.await
				.context("could not work out the home directory on the server")?;
			let home = home.trim();
			if home.is_empty() {
				bail!("the server reported no home directory for this account");
			}
			Ok(home.to_owned())
		}
		AsuserFiles::Denied(reason) => bail!("{reason}"),
	}
}

/// Read a whole file as text. The bytes are the editor's read — same channel discipline, same size
/// cap, which for a config file is generous past any real one. Invalid UTF-8 is refused rather than
/// lossily converted: a config file is written back out, and mangling bytes we did not understand
/// would corrupt the user's own lines.
async fn read_text(backend: &AsuserFiles, path: &str) -> Result<String> {
	let bytes = match backend {
		// A shell config file is text, so it is read under the TEXT ceiling (§53) — the same one the
		// editor opens it with, since Install writes back what this read showed.
		AsuserFiles::Sftp(sftp) => edit::read_file(sftp, path, edit::MAX_SIZE).await?,
		AsuserFiles::Shell(runner) => shellfs::read_all(runner, path, edit::MAX_SIZE).await?,
		AsuserFiles::Denied(reason) => bail!("{reason}"),
	};
	String::from_utf8(bytes).with_context(|| format!("{path} is not text cmote can edit safely"))
}

/// Write the file back atomically — a temp sibling, then a rename over the target. A config file is
/// the one file on a server where a half-written copy locks the user out, so the commit point
/// matters more here than anywhere else cmote writes.
async fn write_text(backend: &AsuserFiles, path: &str, contents: &str) -> Result<()> {
	match backend {
		AsuserFiles::Sftp(sftp) => edit::write_atomic(sftp, path, contents.as_bytes()).await,
		AsuserFiles::Shell(runner) => shellfs::write_all(runner, path, contents.as_bytes()).await,
		AsuserFiles::Denied(reason) => bail!("{reason}"),
	}
}

/// Whether a path is there at all. Only ever asked about the candidate config files, where "could
/// not ask" and "not there" lead to the same next step.
async fn exists(backend: &AsuserFiles, path: &str) -> bool {
	match backend {
		AsuserFiles::Sftp(sftp) => sftp.metadata(path.to_owned()).await.is_ok(),
		AsuserFiles::Shell(runner) => {
			runner
				.succeeds(&format!("[ -e {} ]", shell_quote(path)))
				.await
		}
		AsuserFiles::Denied(_) => false,
	}
}

/// Give the sftp channel back when the errand is done. The shell backend has nothing to close —
/// each command it runs opens and ends its own channel.
async fn close(backend: AsuserFiles) {
	if let AsuserFiles::Sftp(sftp) = backend {
		let _ = sftp.close().await;
	}
}

/// Join a directory and a relative name into a remote path. The explorer's `join` is for absolute
/// remote paths and a config file may sit two levels down (`.config/fish/config.fish`), so this
/// small local one keeps the separator rule in one place instead of formatting it at each call.
fn join(dir: &str, name: &str) -> String {
	format!("{}/{name}", dir.trim_end_matches('/'))
}
