//! SCP file transfer over an existing SSH session — the same thing
//! Tera Term's "SSH SCP..." menu item does: reuse the already-
//! authenticated session as a second channel, rather than opening a new
//! connection just to move a file. `transport::Connection::clone_ssh_session`
//! hands over that reusable handle; this module does the actual transfer.
//!
//! Single file only (no recursive directory copy, matching Tera Term's own
//! basic SCP feature, not e.g. `scp -r`). Run `run()` from a background
//! thread — like the connection it reuses, it blocks.

use std::fs::{self, File};
use std::io::{self, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use ssh2::Session;

/// Read/write chunk size for both directions. Small enough that the
/// shared SSH session (also used by the live interactive channel) isn't
/// monopolized for long between chances for that other channel to run.
const CHUNK: usize = 32 * 1024;

/// How many `WouldBlock` retries to allow for one call before giving up —
/// the session may be in non-blocking mode (the interactive terminal
/// channel puts it there), so every call here may need to retry, the same
/// way `SshConnection::write_all`/`resize` already do.
const MAX_SPINS: u32 = 20_000;
const SPIN_DELAY: Duration = Duration::from_millis(1);

pub enum Direction {
    Upload { local: PathBuf, remote: PathBuf },
    Download { remote: PathBuf, local: PathBuf },
}

/// Progress reported back to the caller — typically sent across an
/// `mpsc::channel` to a UI thread polling it once per frame.
pub enum Event {
    Progress { transferred: u64, total: u64 },
    Done,
    Failed(String),
}

/// Run the transfer described by `direction` over `session`, calling
/// `on_event` from this thread as it progresses. Always ends with exactly
/// one `Event::Done` or `Event::Failed`.
///
/// Takes `FnMut` rather than `Fn` — the real caller only needs to send
/// down an `mpsc::Sender`, which either would satisfy, but `FnMut` also
/// allows accumulating into a captured local, which a plain `Fn` doesn't.
pub fn run(session: &Session, direction: &Direction, mut on_event: impl FnMut(Event)) {
    let result = match direction {
        Direction::Upload { local, remote } => upload(session, local, remote, &mut on_event),
        Direction::Download { remote, local } => download(session, remote, local, &mut on_event),
    };
    match result {
        Ok(()) => on_event(Event::Done),
        Err(e) => on_event(Event::Failed(format!("{e:#}"))),
    }
}

fn upload(session: &Session, local: &Path, remote: &Path, on_event: &mut impl FnMut(Event)) -> Result<()> {
    let mut file =
        File::open(local).with_context(|| format!("failed to open local file '{}'", local.display()))?;
    let metadata = file.metadata().context("failed to read local file metadata")?;
    if metadata.is_dir() {
        bail!("'{}' is a directory; only single-file transfers are supported", local.display());
    }
    let size = metadata.len();
    let mode = file_mode(&metadata);

    let mut channel = retry_ssh(|| session.scp_send(remote, mode, size, None))
        .with_context(|| format!("failed to start SCP upload to '{}'", remote.display()))?;

    let mut buf = [0u8; CHUNK];
    let mut sent: u64 = 0;
    on_event(Event::Progress { transferred: 0, total: size });
    loop {
        let n = file.read(&mut buf).context("failed to read local file")?;
        if n == 0 {
            break;
        }
        write_all_retrying(&mut channel, &buf[..n]).context("SCP write failed")?;
        sent += n as u64;
        on_event(Event::Progress { transferred: sent, total: size });
    }
    close_channel(&mut channel).context("failed to close SCP channel")?;
    Ok(())
}

fn download(session: &Session, remote: &Path, local: &Path, on_event: &mut impl FnMut(Event)) -> Result<()> {
    let (mut channel, stat) = retry_ssh(|| session.scp_recv(remote))
        .with_context(|| format!("failed to start SCP download of '{}'", remote.display()))?;
    if stat.is_dir() {
        bail!("'{}' is a directory; only single-file transfers are supported", remote.display());
    }
    let size = stat.size();

    let mut file =
        File::create(local).with_context(|| format!("failed to create local file '{}'", local.display()))?;

    let mut buf = [0u8; CHUNK];
    let mut received: u64 = 0;
    on_event(Event::Progress { transferred: 0, total: size });
    while received < size {
        let n = read_retrying(&mut channel, &mut buf).context("SCP read failed")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).context("failed to write local file")?;
        received += n as u64;
        on_event(Event::Progress { transferred: received, total: size });
    }
    close_channel(&mut channel).context("failed to close SCP channel")?;
    Ok(())
}

#[cfg(unix)]
fn file_mode(metadata: &fs::Metadata) -> i32 {
    use std::os::unix::fs::PermissionsExt;
    (metadata.permissions().mode() & 0o777) as i32
}

#[cfg(not(unix))]
fn file_mode(_metadata: &fs::Metadata) -> i32 {
    0o644
}

/// Retries `f` while it reports `WouldBlock`, for calls (channel/session
/// setup, teardown) whose error type is `ssh2::Error` rather than
/// `io::Error` — `From<ssh2::Error> for io::Error` is what maps libssh2's
/// EAGAIN to `ErrorKind::WouldBlock` for us. Mirrors
/// `SshConnection::resize`.
fn retry_ssh<T>(mut f: impl FnMut() -> Result<T, ssh2::Error>) -> io::Result<T> {
    let mut spins = 0;
    loop {
        match f().map_err(io::Error::from) {
            Ok(v) => return Ok(v),
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                spins += 1;
                if spins > MAX_SPINS {
                    return Err(io::Error::new(ErrorKind::TimedOut, "SCP operation timed out"));
                }
                std::thread::sleep(SPIN_DELAY);
            }
            Err(e) => return Err(e),
        }
    }
}

/// `Channel::read`/`write` implement `std::io::{Read, Write}` directly and
/// already return `io::Error` (unlike `retry_ssh`'s targets above), so
/// these two just retry on `WouldBlock` without the error-type conversion.
fn write_all_retrying(channel: &mut ssh2::Channel, mut data: &[u8]) -> io::Result<()> {
    let mut spins = 0;
    while !data.is_empty() {
        match channel.write(data) {
            Ok(n) => data = &data[n..],
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                spins += 1;
                if spins > MAX_SPINS {
                    return Err(io::Error::new(ErrorKind::TimedOut, "SCP write timed out"));
                }
                std::thread::sleep(SPIN_DELAY);
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn read_retrying(channel: &mut ssh2::Channel, buf: &mut [u8]) -> io::Result<usize> {
    let mut spins = 0;
    loop {
        match channel.read(buf) {
            Ok(n) => return Ok(n),
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                spins += 1;
                if spins > MAX_SPINS {
                    return Err(io::Error::new(ErrorKind::TimedOut, "SCP read timed out"));
                }
                std::thread::sleep(SPIN_DELAY);
            }
            Err(e) => return Err(e),
        }
    }
}

fn close_channel(channel: &mut ssh2::Channel) -> io::Result<()> {
    retry_ssh(|| channel.send_eof())?;
    let _ = retry_ssh(|| channel.wait_eof());
    let _ = retry_ssh(|| channel.close());
    let _ = retry_ssh(|| channel.wait_close());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_and_download_reject_directories_before_touching_the_network() {
        // No live session needed for either: a directory is rejected
        // before `upload`/`download` would otherwise reach the network
        // (uploading) or need the remote's response (downloading, where
        // `is_dir()` comes from the SCP header `scp_recv` already parsed).
        let dir = std::env::temp_dir().join(format!("rtc-scp-dir-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);

        // `download`'s directory check happens after `scp_recv`, which
        // needs a live session, so it isn't reachable from here; this
        // covers the half that's pure local filesystem logic.
        let err = upload(
            &Session::new().unwrap(),
            &dir,
            Path::new("irrelevant"),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(err.to_string().contains("directory"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn file_mode_preserves_unix_permission_bits() {
        use std::os::unix::fs::PermissionsExt;

        let path = std::env::temp_dir().join(format!("rtc-scp-mode-{}", std::process::id()));
        fs::write(&path, b"x").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

        let mode = file_mode(&fs::metadata(&path).unwrap());
        assert_eq!(mode & 0o777, 0o640);

        let _ = fs::remove_file(&path);
    }
}
