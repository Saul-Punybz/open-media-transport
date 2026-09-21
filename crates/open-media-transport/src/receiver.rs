//! A blocking receiver: connects to a sender's TCP port the way libomtnet
//! does and delivers raw frames (`docs/PROTOCOL.md` §1, §4.3, §8).
//!
//! Like libomtnet, it opens one connection for video + metadata and a second
//! one for audio (T5), and sends the same commands in the same order on each
//! (§4.3). Frames are returned as they arrive, protocol commands included;
//! decoding is left to the caller.
//!
//! If a connection drops, both are closed and reopened, at most once a
//! second, re-sending the current preview, quality and tally — libomtnet's
//! behaviour (N5, `OMTReceive.cs:328-381`), except that libomtnet retries
//! only when the application calls `Receive`, while this retries on its own.

use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::command::{Command, Quality, Tally};
use crate::{frame, Deframer, Error, Limits, OwnedFrame};

/// Minimum time between connection attempts (`OMTReceive.cs:330`).
const RETRY_INTERVAL: Duration = Duration::from_secs(1);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// What to ask the sender for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReceiverConfig {
    /// Subscribe to video.
    pub video: bool,
    /// Subscribe to audio (on a second connection).
    pub audio: bool,
    /// Ask for 1/8 preview video instead of full frames (§6.2).
    pub preview: bool,
    /// Suggested encoder quality, sent with the video subscription.
    pub quality: Quality,
    /// Initial tally.
    pub tally: Tally,
    /// Reconnect automatically when a connection drops.
    pub reconnect: bool,
}

impl Default for ReceiverConfig {
    fn default() -> Self {
        ReceiverConfig {
            video: true,
            audio: true,
            preview: false,
            quality: Quality::Default,
            tally: Tally::default(),
            reconnect: true,
        }
    }
}

/// Which of the receiver's connections an event came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Channel {
    /// The video + metadata connection (or the only one, if metadata-only).
    Video,
    /// The audio connection.
    Audio,
}

/// Something that happened on a connection.
#[derive(Debug)]
pub enum Event {
    /// A connection was (re)established and subscribed.
    Connected(Channel),
    /// A complete frame.
    Frame(Channel, OwnedFrame),
    /// The connection closed: `None` for a clean close by the peer, otherwise
    /// the I/O or protocol error that ended it.
    Closed(Channel, Option<ReceiveError>),
}

/// Why a connection ended.
#[derive(Debug)]
pub enum ReceiveError {
    /// The socket failed.
    Io(io::Error),
    /// The peer sent something that is not a valid OMT stream.
    Protocol(Error),
}

struct Connection {
    stream: TcpStream,
    alive: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
}

impl Connection {
    fn close(mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
        if let Some(h) = self.reader.take() {
            let _ = h.join();
        }
    }
}

#[derive(Default)]
struct Connections {
    video: Option<Connection>,
    audio: Option<Connection>,
}

struct Inner {
    addr: SocketAddr,
    config: Mutex<ReceiverConfig>,
    conns: Mutex<Connections>,
    events: mpsc::Sender<Event>,
    closing: Mutex<bool>,
    wake: Condvar,
}

/// A connection to one sender.
pub struct Receiver {
    inner: Arc<Inner>,
    events: mpsc::Receiver<Event>,
    supervisor: Option<JoinHandle<()>>,
}

impl Receiver {
    /// Connects to a sender at `addr` and subscribes as `config` says. The
    /// first attempt is made here and its error returned; later ones happen
    /// in the background if `config.reconnect` is set.
    pub fn connect(addr: SocketAddr, config: ReceiverConfig) -> io::Result<Receiver> {
        let (tx, events) = mpsc::channel();
        let inner = Arc::new(Inner {
            addr,
            config: Mutex::new(config),
            conns: Mutex::new(Connections::default()),
            events: tx,
            closing: Mutex::new(false),
            wake: Condvar::new(),
        });
        inner.open_all()?;
        let supervisor = if config.reconnect {
            let i = inner.clone();
            Some(
                std::thread::Builder::new()
                    .name("omt-recv-supervisor".into())
                    .spawn(move || i.supervise())?,
            )
        } else {
            None
        };
        Ok(Receiver {
            inner,
            events,
            supervisor,
        })
    }

    /// Waits up to `timeout` for the next event.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Event> {
        self.events.recv_timeout(timeout).ok()
    }

    /// Sends a command to the sender, e.g. a tally or quality change. Tally,
    /// quality and preview are remembered and re-sent after a reconnect.
    pub fn send(&self, command: Command) -> io::Result<()> {
        {
            let mut c = self.inner.config.lock().unwrap();
            match command {
                Command::Tally(t) => c.tally = t,
                Command::Quality(q) => c.quality = q,
                Command::Preview(p) => c.preview = p,
                _ => {}
            }
        }
        self.send_metadata(command.as_bytes())
    }

    /// Sends application metadata. Include a trailing NUL if the far end
    /// expects one; libomtnet passes on whatever it gets (M4).
    pub fn send_metadata(&self, xml: &[u8]) -> io::Result<()> {
        let mut out = Vec::new();
        frame::write_metadata(0, xml, &mut out);
        // The video connection, or the audio one if there is none
        // (`OMTReceive.cs:715-731`).
        let conns = self.inner.conns.lock().unwrap();
        let c = conns
            .video
            .as_ref()
            .or(conns.audio.as_ref())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "not connected"))?;
        (&c.stream).write_all(&out)
    }

    /// Whether every connection this receiver needs is open.
    pub fn is_connected(&self) -> bool {
        self.inner.connected()
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        *self.inner.closing.lock().unwrap() = true;
        self.inner.wake.notify_all();
        if let Some(h) = self.supervisor.take() {
            let _ = h.join();
        }
        self.inner.close_all();
    }
}

impl Inner {
    fn connected(&self) -> bool {
        let cfg = *self.config.lock().unwrap();
        let conns = self.conns.lock().unwrap();
        let ok =
            |c: &Option<Connection>| c.as_ref().is_some_and(|c| c.alive.load(Ordering::SeqCst));
        let need_video = cfg.video || !cfg.audio;
        (!need_video || ok(&conns.video)) && (!cfg.audio || ok(&conns.audio))
    }

    fn close_all(&self) {
        let old = std::mem::take(&mut *self.conns.lock().unwrap());
        for c in [old.video, old.audio].into_iter().flatten() {
            c.close();
        }
    }

    /// Opens every connection the config asks for, with its §4.3 sequence.
    fn open_all(&self) -> io::Result<()> {
        let cfg = *self.config.lock().unwrap();
        let mut conns = Connections::default();
        if cfg.video || !cfg.audio {
            let mut cmds = vec![Command::SubscribeMetadata];
            if cfg.video {
                if cfg.preview {
                    cmds.push(Command::Preview(true));
                }
                cmds.push(Command::SubscribeVideo);
                cmds.push(Command::Quality(cfg.quality));
            }
            cmds.push(Command::Tally(cfg.tally));
            conns.video = Some(self.open(&cmds, Channel::Video, Limits::VIDEO)?);
        }
        if cfg.audio {
            let mut cmds = Vec::new();
            if !cfg.video {
                cmds.push(Command::SubscribeMetadata);
            }
            cmds.push(Command::SubscribeAudio);
            match self.open(&cmds, Channel::Audio, Limits::AUDIO_OR_METADATA) {
                Ok(c) => conns.audio = Some(c),
                Err(e) => {
                    if let Some(v) = conns.video.take() {
                        v.close();
                    }
                    return Err(e);
                }
            }
        }
        *self.conns.lock().unwrap() = conns;
        Ok(())
    }

    fn open(
        &self,
        commands: &[Command],
        channel: Channel,
        limits: Limits,
    ) -> io::Result<Connection> {
        let mut stream = TcpStream::connect_timeout(&self.addr, CONNECT_TIMEOUT)?;
        // T3. libomtnet also enables TCP keepalive; std has no portable API for it.
        stream.set_nodelay(true)?;
        let mut out = Vec::new();
        for c in commands {
            frame::write_metadata(0, c.as_bytes(), &mut out);
        }
        stream.write_all(&out)?;
        let alive = Arc::new(AtomicBool::new(true));
        let (rs, ra, tx) = (stream.try_clone()?, alive.clone(), self.events.clone());
        let reader = std::thread::Builder::new()
            .name(format!("omt-recv-{channel:?}"))
            .spawn(move || read_loop(rs, channel, limits, tx, ra))?;
        let _ = self.events.send(Event::Connected(channel));
        Ok(Connection {
            stream,
            alive,
            reader: Some(reader),
        })
    }

    /// Checks once a second; when a needed connection is down, closes all
    /// and reconnects (`OMTReceive.cs:662-673,350-382`).
    fn supervise(&self) {
        let mut closing = self.closing.lock().unwrap();
        loop {
            closing = self.wake.wait_timeout(closing, RETRY_INTERVAL).unwrap().0;
            if *closing {
                return;
            }
            drop(closing);
            if !self.connected() {
                self.close_all();
                let _ = self.open_all();
            }
            closing = self.closing.lock().unwrap();
        }
    }
}

fn read_loop(
    mut stream: TcpStream,
    channel: Channel,
    limits: Limits,
    tx: mpsc::Sender<Event>,
    alive: Arc<AtomicBool>,
) {
    let mut deframer = Deframer::new(limits);
    // libomtnet reads at most 128 KiB per call (`OMTConstants.cs:44`).
    let mut buf = vec![0u8; 128 * 1024];
    let reason = 'read: loop {
        let n = match stream.read(&mut buf) {
            Ok(0) => break None,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => break Some(ReceiveError::Io(e)),
        };
        deframer.push(&buf[..n]);
        loop {
            match deframer.next_frame() {
                Ok(Some(f)) => {
                    if tx.send(Event::Frame(channel, f)).is_err() {
                        break 'read None; // receiver dropped
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    let _ = stream.shutdown(Shutdown::Both);
                    break 'read Some(ReceiveError::Protocol(e));
                }
            }
        }
    };
    alive.store(false, Ordering::SeqCst);
    let _ = tx.send(Event::Closed(channel, reason));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::classify;
    use crate::command::Message;
    use std::net::TcpListener;

    /// Accepts connections on a local port and returns what each one sent
    /// before the peer closed it.
    fn capture_connections(n: usize) -> (SocketAddr, std::thread::JoinHandle<Vec<Vec<u8>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let h = std::thread::spawn(move || {
            let mut out = Vec::new();
            for _ in 0..n {
                let (mut s, _) = listener.accept().unwrap();
                s.set_read_timeout(Some(Duration::from_millis(300)))
                    .unwrap();
                let mut got = Vec::new();
                let mut buf = [0u8; 4096];
                while let Ok(k) = s.read(&mut buf) {
                    if k == 0 {
                        break;
                    }
                    got.extend_from_slice(&buf[..k]);
                }
                out.push(got);
            }
            out
        });
        (addr, h)
    }

    fn commands_in(bytes: &[u8]) -> Vec<Command> {
        let mut d = Deframer::new(Limits::VIDEO);
        d.push(bytes);
        let mut out = Vec::new();
        while let Some(f) = d.next_frame().unwrap() {
            match classify(&f.data) {
                Message::Command(c) => out.push(c),
                other => panic!("not a command: {other:?}"),
            }
        }
        out
    }

    #[test]
    fn opens_two_connections_with_libomtnet_sequences() {
        let (addr, h) = capture_connections(2);
        let cfg = ReceiverConfig {
            quality: Quality::High,
            tally: Tally {
                preview: false,
                program: true,
            },
            reconnect: false,
            ..ReceiverConfig::default()
        };
        let r = Receiver::connect(addr, cfg).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        drop(r);
        let got = h.join().unwrap();
        assert_eq!(
            commands_in(&got[0]),
            [
                Command::SubscribeMetadata,
                Command::SubscribeVideo,
                Command::Quality(Quality::High),
                Command::Tally(Tally {
                    preview: false,
                    program: true
                }),
            ]
        );
        assert_eq!(commands_in(&got[1]), [Command::SubscribeAudio]);
    }

    #[test]
    fn audio_only_subscribes_metadata_on_the_audio_connection() {
        let (addr, h) = capture_connections(1);
        let cfg = ReceiverConfig {
            video: false,
            reconnect: false,
            ..ReceiverConfig::default()
        };
        drop(Receiver::connect(addr, cfg).unwrap());
        let got = h.join().unwrap();
        assert_eq!(
            commands_in(&got[0]),
            [Command::SubscribeMetadata, Command::SubscribeAudio]
        );
    }

    #[test]
    fn reconnects_and_resends_current_state() {
        // First session is cut by the server; the second must carry the tally
        // and quality set in between.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let cfg = ReceiverConfig {
            audio: false,
            ..ReceiverConfig::default()
        };
        let r = Receiver::connect(addr, cfg).unwrap();
        let (first, _) = listener.accept().unwrap();
        r.send(Command::Tally(Tally {
            preview: true,
            program: true,
        }))
        .unwrap();
        r.send(Command::Quality(Quality::Low)).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        drop(first); // server closes

        let (mut second, _) = listener.accept().unwrap();
        second
            .set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        while let Ok(k) = second.read(&mut buf) {
            if k == 0 {
                break;
            }
            got.extend_from_slice(&buf[..k]);
        }
        assert_eq!(
            commands_in(&got),
            [
                Command::SubscribeMetadata,
                Command::SubscribeVideo,
                Command::Quality(Quality::Low),
                Command::Tally(Tally {
                    preview: true,
                    program: true
                }),
            ]
        );
        let mut saw = (false, false);
        while let Some(e) = r.recv_timeout(Duration::from_millis(100)) {
            match e {
                Event::Closed(Channel::Video, _) => saw.0 = true,
                Event::Connected(Channel::Video) if saw.0 => saw.1 = true,
                _ => {}
            }
        }
        assert_eq!(saw, (true, true), "closed then connected events");
    }
}
