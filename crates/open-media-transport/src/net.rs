//! Socket reads another thread can stop, on every platform.
//!
//! Every connection here has a thread blocked in `read`, and closing the
//! connection from elsewhere must end that thread. On Linux and macOS,
//! `shutdown` on any handle of the socket wakes the blocked read with
//! `Ok(0)`. On Windows it does not: the read stays blocked until the peer
//! sends something or closes, which a stalled or vanished peer never does
//! (measured on the GitHub `windows-latest` runner,
//! `docs/evidence/2026-09-23-windows-ci`). So on Windows a reader's socket
//! gets a read timeout, and [`read`] wakes every [`POLL`] to check whether
//! it has been told to stop. Elsewhere nothing changes.

use std::io::{self, Read};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(windows)]
use std::time::Duration;

/// How often a blocked read wakes on Windows to look at its stop flag.
#[cfg(windows)]
const POLL: Duration = Duration::from_millis(100);

/// Prepares `stream` for [`read`]. Call once, before the reading thread
/// starts.
pub(crate) fn stoppable(stream: &TcpStream) -> io::Result<()> {
    #[cfg(windows)]
    stream.set_read_timeout(Some(POLL))?;
    #[cfg(not(windows))]
    let _ = stream;
    Ok(())
}

/// `stream.read(buf)` that returns `Ok(0)`, as for a closed connection, once
/// `stop` is set, even if nothing arrives. On Linux and macOS it is a plain
/// read: whoever sets `stop` also shuts the socket down, which wakes it with
/// `Ok(0)`.
pub(crate) fn read(stream: &mut TcpStream, buf: &mut [u8], stop: &AtomicBool) -> io::Result<usize> {
    loop {
        match stream.read(buf) {
            Err(e)
                if cfg!(windows)
                    && matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
            {
                if stop.load(Ordering::SeqCst) {
                    return Ok(0);
                }
            }
            r => return r,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Shutdown, TcpListener};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// The peer stays connected and silent; setting the flag and shutting
    /// the socket down, as every closer here does, ends the read promptly.
    #[test]
    fn a_blocked_read_stops() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let ours = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        let _peer = l.accept().unwrap();
        stoppable(&ours).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let (mut rs, st) = (ours.try_clone().unwrap(), stop.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut b = [0u8; 16];
            let _ = tx.send(read(&mut rs, &mut b, &st).map_err(|e| e.kind()));
        });
        std::thread::sleep(Duration::from_millis(300));
        let start = Instant::now();
        stop.store(true, Ordering::SeqCst);
        let _ = ours.shutdown(Shutdown::Both);
        let r = rx.recv_timeout(Duration::from_secs(2));
        assert_eq!(r, Ok(Ok(0)), "the read returned as closed");
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
