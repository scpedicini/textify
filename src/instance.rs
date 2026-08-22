//! Single-instance coordination.
//!
//! The first Textify process binds a local endpoint inside the data directory
//! and listens for later launches. A later launch connects, hands over any
//! paths it was asked to open (possibly none, which simply means "bring the
//! running window to the front"), and exits without ever starting the UI. This
//! is what keeps a Raycast/Spotlight launch from spawning a second process
//! that would fight the first one over the persisted session state.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
#[cfg(windows)]
use std::time::Duration;

/// Held by the primary instance for the lifetime of the process. Dropping it
/// stops the listener and removes the endpoint marker so the next launch
/// starts cleanly.
pub struct PrimaryInstance {
    endpoint: Option<PathBuf>,
    shutdown: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl Drop for PrimaryInstance {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some(endpoint) = self.endpoint.take() {
            let _ = std::fs::remove_file(endpoint);
        }
    }
}

/// Claim the single-instance endpoint for this user.
///
/// Returns `Some` when this process is (or must act as) the primary instance.
/// Returns `None` when a running instance accepted the hand-off of
/// `launch_paths`; the caller should exit immediately in that case.
///
/// `sender` receives one `Vec<PathBuf>` per later launch. An empty batch means
/// that launch carried no paths and only wants the running window activated.
pub fn acquire(
    data_dir: &Path,
    launch_paths: &[PathBuf],
    sender: Sender<Vec<PathBuf>>,
) -> Option<PrimaryInstance> {
    if let Err(error) = std::fs::create_dir_all(data_dir) {
        tracing::warn!(%error, "could not prepare the data directory; running without single-instance protection");
        return Some(PrimaryInstance { endpoint: None, shutdown: None });
    }
    platform::acquire(data_dir, launch_paths, sender)
}

fn encode_paths(paths: &[PathBuf]) -> String {
    let mut payload = String::new();
    for path in paths {
        // Paths are sent one per line; a path containing a newline cannot be
        // represented and is dropped rather than corrupting the stream.
        let text = path.to_string_lossy();
        if text.contains('\n') {
            tracing::warn!(?path, "skipping path with embedded newline in instance hand-off");
            continue;
        }
        payload.push_str(&text);
        payload.push('\n');
    }
    payload
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::io::{BufRead as _, BufReader, Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::time::Duration;

    /// First line the primary instance writes on every connection, so a client
    /// never mistakes a stale or foreign endpoint for a running Textify.
    const GREETING: &str = "textify-instance-1";
    const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);

    /// Handshake and forward paths over an established connection. Returns true
    /// when the peer identified itself as a running Textify and took the batch.
    fn hand_off_over<S: Read + Write>(stream: &mut S, payload: &str) -> bool {
        let mut greeting = String::new();
        let mut reader = BufReader::new(&mut *stream);
        if reader.read_line(&mut greeting).is_err() || greeting.trim_end() != GREETING {
            return false;
        }
        stream.write_all(payload.as_bytes()).is_ok() && stream.flush().is_ok()
    }

    /// Serve one accepted connection: identify ourselves, collect the batch,
    /// and queue it for the workspace. Returns false when the workspace side is
    /// gone.
    fn serve_connection<S: Read + Write>(stream: &mut S, sender: &Sender<Vec<PathBuf>>) -> bool {
        if stream.write_all(GREETING.as_bytes()).is_err()
            || stream.write_all(b"\n").is_err()
            || stream.flush().is_err()
        {
            return true;
        }
        let mut paths = Vec::new();
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else {
                break;
            };
            if !line.is_empty() {
                paths.push(PathBuf::from(line));
            }
        }
        // An empty batch still signals "activate the running window".
        sender.send(paths).is_ok()
    }

    pub(super) fn acquire(
        data_dir: &Path,
        launch_paths: &[PathBuf],
        sender: Sender<Vec<PathBuf>>,
    ) -> Option<PrimaryInstance> {
        let socket_path = data_dir.join("instance.sock");

        if let Ok(mut stream) = UnixStream::connect(&socket_path) {
            let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
            let _ = stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT));
            if hand_off_over(&mut stream, &encode_paths(launch_paths)) {
                return None;
            }
            tracing::warn!("endpoint at instance socket is not a running Textify; reclaiming it");
        }

        // No live instance answered: any file at the socket path is stale
        // (crash, force kill) and must be removed before rebinding.
        let _ = std::fs::remove_file(&socket_path);
        let listener = match UnixListener::bind(&socket_path) {
            Ok(listener) => listener,
            Err(error) => {
                tracing::warn!(%error, "could not bind the single-instance socket; running without protection");
                return Some(PrimaryInstance { endpoint: None, shutdown: None });
            }
        };

        let spawned = std::thread::Builder::new()
            .name("textify-instance".to_owned())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else {
                        continue;
                    };
                    let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
                    let _ = stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT));
                    if !serve_connection(&mut stream, &sender) {
                        break;
                    }
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "could not start the single-instance listener thread");
        }

        Some(PrimaryInstance {
            endpoint: Some(socket_path),
            // The accept loop blocks in the kernel and dies with the process;
            // removing the socket file is all the cleanup drop needs.
            shutdown: None,
        })
    }
}

// Windows has no std Unix-domain sockets, and this crate deliberately forbids
// std::net (see tests/privacy.rs), so the primary instance advertises itself
// with a heartbeat file it refreshes continuously and later launches drop
// their batches into a mailbox directory the primary polls.
#[cfg(windows)]
mod platform {
    use super::*;
    use std::time::SystemTime;

    const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(500);
    /// A heartbeat older than this means the previous instance is gone
    /// (crashed or force-killed) and the endpoint can be reclaimed.
    const HEARTBEAT_STALE_AFTER: Duration = Duration::from_secs(5);

    fn heartbeat_is_fresh(heartbeat_path: &Path) -> bool {
        std::fs::metadata(heartbeat_path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age < HEARTBEAT_STALE_AFTER)
    }

    fn deliver_batch(mailbox_dir: &Path, launch_paths: &[PathBuf]) -> std::io::Result<()> {
        std::fs::create_dir_all(mailbox_dir)?;
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let name = format!("{stamp}-{}.batch", std::process::id());
        // Write-then-rename so the primary never reads a half-written batch.
        let staging = mailbox_dir.join(format!("{name}.tmp"));
        std::fs::write(&staging, encode_paths(launch_paths))?;
        std::fs::rename(&staging, mailbox_dir.join(name))
    }

    fn drain_mailbox(mailbox_dir: &Path, sender: &Sender<Vec<PathBuf>>) -> bool {
        let Ok(entries) = std::fs::read_dir(mailbox_dir) else {
            return true;
        };
        let mut batches: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("batch"))
            .collect();
        batches.sort();
        for batch_path in batches {
            let Ok(contents) = std::fs::read_to_string(&batch_path) else {
                continue;
            };
            let _ = std::fs::remove_file(&batch_path);
            let paths = contents
                .lines()
                .filter(|line| !line.is_empty())
                .map(PathBuf::from)
                .collect();
            // An empty batch still signals "activate the running window".
            if sender.send(paths).is_err() {
                return false;
            }
        }
        true
    }

    pub(super) fn acquire(
        data_dir: &Path,
        launch_paths: &[PathBuf],
        sender: Sender<Vec<PathBuf>>,
    ) -> Option<PrimaryInstance> {
        let heartbeat_path = data_dir.join("instance-heartbeat");
        let mailbox_dir = data_dir.join("instance-mailbox");

        if heartbeat_is_fresh(&heartbeat_path) {
            match deliver_batch(&mailbox_dir, launch_paths) {
                Ok(()) => return None,
                Err(error) => {
                    tracing::warn!(%error, "could not hand off to the running instance; continuing this launch");
                    return Some(PrimaryInstance { endpoint: None, shutdown: None });
                }
            }
        }

        if let Err(error) = std::fs::write(&heartbeat_path, b"textify") {
            tracing::warn!(%error, "could not claim the single-instance heartbeat; running without protection");
            return Some(PrimaryInstance { endpoint: None, shutdown: None });
        }
        let _ = std::fs::create_dir_all(&mailbox_dir);

        let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_shutdown = shutdown.clone();
        let thread_heartbeat = heartbeat_path.clone();
        let spawned = std::thread::Builder::new()
            .name("textify-instance".to_owned())
            .spawn(move || {
                while !thread_shutdown.load(std::sync::atomic::Ordering::Relaxed) {
                    if std::fs::write(&thread_heartbeat, b"textify").is_err() {
                        // Keep serving the mailbox even if the heartbeat cannot
                        // be refreshed; a later launch may then start a second
                        // instance, but hand-offs to this one still work.
                    }
                    if !drain_mailbox(&mailbox_dir, &sender) {
                        break;
                    }
                    std::thread::sleep(HEARTBEAT_INTERVAL);
                }
                // A loop iteration may have re-created the marker after drop
                // removed it; clear it again on the way out.
                let _ = std::fs::remove_file(&thread_heartbeat);
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "could not start the single-instance listener thread");
        }

        Some(PrimaryInstance {
            endpoint: Some(heartbeat_path),
            // Drop raises this flag so the heartbeat loop stops re-creating
            // the marker after it has been removed.
            shutdown: Some(shutdown),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn second_launch_hands_its_paths_to_the_primary_instance() {
        let directory = tempfile::tempdir().expect("instance directory");
        let (sender, receiver) = mpsc::channel();
        let primary = acquire(directory.path(), &[], sender).expect("primary claim");

        let (unused_sender, _unused_receiver) = mpsc::channel();
        let handed_off = acquire(
            directory.path(),
            &[PathBuf::from("/tmp/example-notes.txt")],
            unused_sender,
        );
        assert!(handed_off.is_none(), "second launch must not become primary");

        let batch = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("forwarded batch");
        assert_eq!(batch, vec![PathBuf::from("/tmp/example-notes.txt")]);
        drop(primary);
    }

    #[test]
    fn pathless_second_launch_still_signals_activation() {
        let directory = tempfile::tempdir().expect("instance directory");
        let (sender, receiver) = mpsc::channel();
        let primary = acquire(directory.path(), &[], sender).expect("primary claim");

        let (unused_sender, _unused_receiver) = mpsc::channel();
        assert!(acquire(directory.path(), &[], unused_sender).is_none());

        let batch = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("activation batch");
        assert!(batch.is_empty(), "a pathless launch forwards an empty batch");
        drop(primary);
    }

    #[cfg(unix)]
    #[test]
    fn stale_endpoint_from_a_killed_instance_is_reclaimed() {
        let directory = tempfile::tempdir().expect("instance directory");
        // A force-killed instance leaves the socket file behind with nothing
        // listening on it.
        std::fs::write(directory.path().join("instance.sock"), b"").expect("stale marker");

        let (sender, _receiver) = mpsc::channel();
        let primary = acquire(directory.path(), &[], sender);
        assert!(primary.is_some(), "stale endpoint must not block a new launch");
    }

    #[test]
    fn dropping_the_primary_removes_the_endpoint_marker() {
        let directory = tempfile::tempdir().expect("instance directory");
        let (sender, _receiver) = mpsc::channel();
        let primary = acquire(directory.path(), &[], sender).expect("primary claim");
        let marker = if cfg!(unix) {
            directory.path().join("instance.sock")
        } else {
            directory.path().join("instance-heartbeat")
        };
        assert!(marker.exists());
        drop(primary);
        // The Windows heartbeat loop may re-create the marker once before it
        // notices the shutdown flag; give it a few beats to clear out.
        let mut waited = Duration::ZERO;
        while marker.exists() && waited < Duration::from_secs(3) {
            let step = Duration::from_millis(50);
            std::thread::sleep(step);
            waited += step;
        }
        assert!(!marker.exists());
    }
}
