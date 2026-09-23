//! A blocking receiver: connects to a sender's TCP port the way libomtnet
//! does and delivers raw frames (`docs/PROTOCOL.md` §1, §4.3, §8, §9).
//!
//! Like libomtnet, it opens one connection for video + metadata and a second
//! one for audio (T5), and sends the same commands in the same order on each
//! (§4.3). Frames are returned as they arrive, protocol commands included;
//! decoding is left to the caller, or to [`crate::media::MediaDecoder`].
//!
//! **Addressing (§8).** A receiver is given a socket address, a full name
//! `MACHINE (Name)` or an `omt://host:port` URL ([`Address`]). A name is
//! looked up in a [`Directory`] of discovered sources and a URL is resolved
//! with DNS, again on **every** connection attempt, so a sender that
//! restarted on another port or address is found again. libomtnet does the
//! same: each attempt calls `FindByFullNameOrUrl` on the address
//! (`OMTReceive.cs:328-349`, `OMTDiscovery.cs:389-415`), and its discovery
//! table replaces a source that went away and came back
//! (`OMTDiscovery.cs:175-247`).
//!
//! **Reconnecting.** If a connection drops, both are closed and reopened, at
//! most once a second, re-sending the current preview, quality and tally —
//! libomtnet's behaviour (N5, `OMTReceive.cs:328-381`), except that libomtnet
//! retries only when the application calls `Receive`, while this retries on
//! its own.
//!
//! **Redirect (§9).** When the sender sends `<OMTRedirect NewAddress="B" />`,
//! the receiver reconnects to `B` and opens a metadata-only side connection
//! to the original address to hear later changes; the side connection lasts
//! as long as the receiver. A new address from the side connection moves the
//! receiver again, and an empty one returns it to the original
//! (`OMTReceive.cs:562-603,532-560`, `OMTRedirect.cs:65-108,128-163`). As in
//! libomtnet, once the side connection exists, redirects heard from the
//! redirect target itself are ignored: chains are resolved by the original
//! sender (X3). Where we differ: libomtnet treats an *empty* redirect heard
//! on the main connection before any real one as a first redirect — it
//! reconnects for nothing and, having created its redirect state without a
//! side connection, ignores every later redirect (`OMTReceive.cs:579-594`,
//! `OMTRedirect.cs:84-90`). Here an empty first redirect is a no-op.
//!
//! **Dropping** a receiver shuts its sockets down and waits at most
//! [`DROP_TIMEOUT`] for its threads; one still busy after that (say, in a
//! connection attempt) finishes on its own and closes what it opened.

use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::address::{Address, Directory};
use crate::command::{classify, Command, Message, Quality, Tally};
use crate::frame::ExtendedHeader;
use crate::redirect::{self, Watcher};
use crate::sender::join_bounded;
pub use crate::sender::DROP_TIMEOUT;
use crate::{frame, Deframer, Error, Limits, OwnedFrame};

/// Minimum time between connection attempts (`OMTReceive.cs:330`).
const RETRY_INTERVAL: Duration = Duration::from_secs(1);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// How long the first connection waits for a full name to be discovered.
/// libomtnet does not wait: its first attempt usually finds nothing and the
/// next `Receive` call tries again.
pub const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);

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
    /// Follow redirects (§9). libomtnet always does.
    pub follow_redirects: bool,
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
            follow_redirects: true,
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
    /// The receiver is switching source because of a redirect (§9):
    /// `Some(address)` to follow it, `None` to go back to the original.
    Redirect(Option<String>),
}

/// Bytes and frames received on one channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChannelStats {
    /// Bytes read from the socket.
    pub bytes: u64,
    /// Complete frames, protocol messages included.
    pub frames: u64,
}

/// Counters since the receiver started, for [`Receiver::stats`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReceiverStats {
    /// The video + metadata connection.
    pub video: ChannelStats,
    /// The audio connection.
    pub audio: ChannelStats,
    /// Times the connections were re-established after the first attempt,
    /// other than to follow a redirect.
    pub reconnects: u64,
    /// Times the receiver switched source for a redirect (§9).
    pub redirects: u64,
}

#[derive(Default)]
struct Counters {
    bytes: [AtomicU64; 2],
    frames: [AtomicU64; 2],
    reconnects: AtomicU64,
    redirects: AtomicU64,
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
            join_bounded(h, Instant::now() + DROP_TIMEOUT);
        }
    }
}

#[derive(Default)]
struct Connections {
    video: Option<Connection>,
    audio: Option<Connection>,
    peer: Option<SocketAddr>,
}

/// Redirect and shutdown state, guarded together so the supervisor can
/// wait on it.
#[derive(Default)]
struct Control {
    closing: bool,
    /// The address being followed; `None` means the original.
    redirect: Option<String>,
    /// A first redirect has been seen: libomtnet's `redirect != null`.
    following: bool,
    /// The target changed: reconnect now, without waiting for the retry interval.
    retarget: bool,
    /// Metadata-only connection to the original address.
    side: Option<Watcher>,
}

struct Inner {
    original: Address,
    /// What redirect addresses are compared with (`OMTReceive.cs:570`).
    original_text: String,
    directory: Mutex<Option<Arc<Directory>>>,
    config: Mutex<ReceiverConfig>,
    conns: Mutex<Connections>,
    events: mpsc::Sender<Event>,
    ctl: Mutex<Control>,
    wake: Condvar,
    me: Weak<Inner>,
    stats: Arc<Counters>,
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
        Receiver::start(Address::Socket(addr), config, None, true)
    }

    /// Connects to a full name `MACHINE (Name)` or an `omt://host:port` URL
    /// (N1). A name is looked up with a [`Directory`] this receiver starts
    /// for itself, waiting up to [`RESOLVE_TIMEOUT`] for it to appear; use
    /// [`Receiver::connect_address`] to share one directory among receivers.
    pub fn connect_to(address: &str, config: ReceiverConfig) -> io::Result<Receiver> {
        let a =
            Address::parse(address).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        Receiver::connect_address(a, config, None)
    }

    /// Connects to `address`, looking names up in `directory`. Without one, a
    /// directory is started when a name first needs resolving (the address
    /// itself, or a redirect to a name).
    pub fn connect_address(
        address: Address,
        config: ReceiverConfig,
        directory: Option<Arc<Directory>>,
    ) -> io::Result<Receiver> {
        Receiver::start(address, config, directory, true)
    }

    /// `first_must_succeed = false` keeps retrying in the background instead
    /// of returning the first attempt's error (for side connections).
    pub(crate) fn start(
        address: Address,
        config: ReceiverConfig,
        directory: Option<Arc<Directory>>,
        first_must_succeed: bool,
    ) -> io::Result<Receiver> {
        let (tx, events) = mpsc::channel();
        let inner = Arc::new_cyclic(|me| Inner {
            original_text: address.to_string(),
            original: address,
            directory: Mutex::new(directory),
            config: Mutex::new(config),
            conns: Mutex::new(Connections::default()),
            events: tx,
            ctl: Mutex::new(Control::default()),
            wake: Condvar::new(),
            me: me.clone(),
            stats: Arc::default(),
        });
        let wait = first_must_succeed.then_some(RESOLVE_TIMEOUT);
        let pending = match inner.open_all(wait) {
            Ok(()) => false,
            Err(e) if first_must_succeed => return Err(e),
            Err(_) => true,
        };
        let i = inner.clone();
        let supervisor = std::thread::Builder::new()
            .name("omt-recv-supervisor".into())
            .spawn(move || i.supervise(pending));
        let supervisor = match supervisor {
            Ok(h) => h,
            Err(e) => {
                inner.close_all();
                return Err(e);
            }
        };
        Ok(Receiver {
            inner,
            events,
            supervisor: Some(supervisor),
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

    /// The address given to this receiver.
    pub fn address(&self) -> &Address {
        &self.inner.original
    }

    /// The redirect being followed, if any (§9).
    pub fn redirect(&self) -> Option<String> {
        self.inner.ctl.lock().unwrap().redirect.clone()
    }

    /// Counters since the receiver started.
    pub fn stats(&self) -> ReceiverStats {
        let c = &self.inner.stats;
        let channel = |i: usize| ChannelStats {
            bytes: c.bytes[i].load(Ordering::Relaxed),
            frames: c.frames[i].load(Ordering::Relaxed),
        };
        ReceiverStats {
            video: channel(0),
            audio: channel(1),
            reconnects: c.reconnects.load(Ordering::Relaxed),
            redirects: c.redirects.load(Ordering::Relaxed),
        }
    }

    /// The socket address of the current connections; set before each
    /// [`Event::Connected`] is sent, cleared when they are closed.
    pub fn peer_addr(&self) -> Option<SocketAddr> {
        self.inner.conns.lock().unwrap().peer
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        let side = {
            let mut c = self.inner.ctl.lock().unwrap();
            c.closing = true;
            c.side.take()
        };
        self.inner.wake.notify_all();
        // Sockets first, so no thread stays blocked on a stalled sender.
        self.inner.close_all();
        drop(side);
        if let Some(h) = self.supervisor.take() {
            join_bounded(h, Instant::now() + DROP_TIMEOUT);
        }
        // The supervisor may have started a side connection or connected
        // while stopping.
        let side = self.inner.ctl.lock().unwrap().side.take();
        drop(side);
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

    /// The directory, started on first use.
    fn directory(&self) -> io::Result<Arc<Directory>> {
        let mut d = self.directory.lock().unwrap();
        if let Some(d) = &*d {
            return Ok(d.clone());
        }
        let dir = Arc::new(Directory::browse().map_err(io::Error::other)?);
        *d = Some(dir.clone());
        Ok(dir)
    }

    /// Where to connect now: the redirect if one is followed, otherwise the
    /// original address (`OMTReceive.cs:287-294`).
    fn target(&self) -> Address {
        let r = self.ctl.lock().unwrap().redirect.clone();
        r.and_then(|r| Address::parse(&r).ok())
            .unwrap_or_else(|| self.original.clone())
    }

    /// Resolves `target` afresh. A name may be waited for.
    fn resolve(&self, target: &Address, wait: Option<Duration>) -> io::Result<Vec<SocketAddr>> {
        match target {
            Address::Name(name) => {
                let dir = self.directory()?;
                match wait {
                    Some(w) if dir.get(name).is_none() => {
                        let _ = dir.wait_for(name, w);
                    }
                    _ => {}
                }
                target.resolve(Some(&dir))
            }
            _ => target.resolve(None),
        }
    }

    /// Opens every connection the config asks for, with its §4.3 sequence,
    /// at the current target.
    fn open_all(&self, wait: Option<Duration>) -> io::Result<()> {
        let target = self.target();
        let addrs = self.resolve(&target, wait)?;
        let cfg = *self.config.lock().unwrap();
        let mut conns = Connections::default();
        // Like `Socket.BeginConnect(IPAddress[], port)` (`OMTReceive.cs:391`),
        // try each address in turn; the second connection goes to the one
        // that answered.
        let mut last = io::Error::new(io::ErrorKind::NotFound, "no address");
        for a in &addrs {
            let result = if cfg.video || !cfg.audio {
                let mut cmds = vec![Command::SubscribeMetadata];
                if cfg.video {
                    if cfg.preview {
                        cmds.push(Command::Preview(true));
                    }
                    cmds.push(Command::SubscribeVideo);
                    cmds.push(Command::Quality(cfg.quality));
                    // A metadata-only connection sends nothing else
                    // (`OMTReceive.cs:430,443-449`).
                    cmds.push(Command::Tally(cfg.tally));
                }
                self.open(*a, &cmds, Channel::Video, Limits::VIDEO)
                    .map(|c| conns.video = Some(c))
            } else {
                // Audio only: its connection is the one that must answer.
                self.open(
                    *a,
                    &[Command::SubscribeMetadata, Command::SubscribeAudio],
                    Channel::Audio,
                    Limits::AUDIO_OR_METADATA,
                )
                .map(|c| conns.audio = Some(c))
            };
            match result {
                Ok(()) => {
                    conns.peer = Some(*a);
                    break;
                }
                Err(e) => last = e,
            }
        }
        let Some(peer) = conns.peer else {
            return Err(last);
        };
        if cfg.audio && cfg.video {
            match self.open(
                peer,
                &[Command::SubscribeAudio],
                Channel::Audio,
                Limits::AUDIO_OR_METADATA,
            ) {
                Ok(c) => conns.audio = Some(c),
                Err(e) => {
                    if let Some(v) = conns.video.take() {
                        v.close();
                    }
                    return Err(e);
                }
            }
        }
        // Held while storing, so a receiver being dropped either sees these
        // connections or they are closed here.
        let ctl = self.ctl.lock().unwrap();
        if ctl.closing {
            drop(ctl);
            for c in [conns.video, conns.audio].into_iter().flatten() {
                c.close();
            }
            return Err(io::Error::new(io::ErrorKind::Interrupted, "closing"));
        }
        *self.conns.lock().unwrap() = conns;
        Ok(())
    }

    fn open(
        &self,
        addr: SocketAddr,
        commands: &[Command],
        channel: Channel,
        limits: Limits,
    ) -> io::Result<Connection> {
        let mut stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
        // T3. libomtnet also enables TCP keepalive; std has no portable API for it.
        stream.set_nodelay(true)?;
        let mut out = Vec::new();
        for c in commands {
            frame::write_metadata(0, c.as_bytes(), &mut out);
        }
        stream.write_all(&out)?;
        let alive = Arc::new(AtomicBool::new(true));
        let (rs, ra, me) = (stream.try_clone()?, alive.clone(), self.me.clone());
        let stats = self.stats.clone();
        let reader = std::thread::Builder::new()
            .name(format!("omt-recv-{channel:?}"))
            .spawn(move || read_loop(rs, channel, limits, me, ra, stats))?;
        // Recorded before the event, so `peer_addr` agrees with it.
        self.conns.lock().unwrap().peer = Some(addr);
        let _ = self.events.send(Event::Connected(channel));
        Ok(Connection {
            stream,
            alive,
            reader: Some(reader),
        })
    }

    /// A redirect message arrived, on a main connection or (`from_side`) on
    /// the side connection to the original address.
    fn redirect_heard(&self, from_side: bool, address: String) {
        if !self.config.lock().unwrap().follow_redirects {
            return;
        }
        let mut c = self.ctl.lock().unwrap();
        // A redirect back to the original is ignored (`OMTReceive.cs:570`,
        // `OMTRedirect.cs:138`).
        if c.closing || address == self.original_text {
            return;
        }
        let new = if from_side {
            // `OMTRedirect.cs:140-154`: a change moves the receiver; an empty
            // address returns it to the original (`OMTReceive.cs:289-293`).
            if c.redirect.as_deref().unwrap_or("") == address {
                return;
            }
            (!address.is_empty()).then_some(address)
        } else {
            // `OMTReceive.cs:579-594`: only the first redirect heard on a
            // main connection counts; after that the side connection rules.
            if c.following || address.is_empty() {
                return;
            }
            c.following = true;
            Some(address)
        };
        c.redirect = new.clone();
        c.retarget = true;
        drop(c);
        self.stats.redirects.fetch_add(1, Ordering::Relaxed);
        let _ = self.events.send(Event::Redirect(new));
        self.wake.notify_all();
    }

    fn start_side(&self) {
        let me = self.me.clone();
        let directory = self.directory.lock().unwrap().clone();
        let side = Watcher::start(self.original.clone(), directory, move |a| {
            if let Some(i) = me.upgrade() {
                i.redirect_heard(true, a);
            }
        });
        let mut c = self.ctl.lock().unwrap();
        if let Ok(w) = side {
            if c.side.is_none() {
                c.side = Some(w);
            }
        }
    }

    /// Reconnects when a needed connection is down, at most once a second
    /// (`OMTReceive.cs:662-673,328-331`), and at once when a redirect changes
    /// the target (`OMTReceive.cs:543-560` resets the rate limit).
    fn supervise(&self, mut pending: bool) {
        let mut last_attempt = Instant::now();
        loop {
            let (retarget, want_side) = {
                let mut c = self.ctl.lock().unwrap();
                if !c.closing && !c.retarget {
                    let since = last_attempt.elapsed();
                    let wait = if since < RETRY_INTERVAL {
                        RETRY_INTERVAL - since
                    } else {
                        RETRY_INTERVAL
                    };
                    c = self
                        .wake
                        .wait_timeout(c, wait.max(Duration::from_millis(10)))
                        .unwrap()
                        .0;
                }
                if c.closing {
                    return;
                }
                (
                    std::mem::take(&mut c.retarget),
                    c.following && c.side.is_none(),
                )
            };
            if want_side {
                self.start_side();
            }
            let reconnect = self.config.lock().unwrap().reconnect;
            let due = last_attempt.elapsed() >= RETRY_INTERVAL;
            if retarget || ((reconnect || pending) && due && !self.connected()) {
                self.close_all();
                last_attempt = Instant::now();
                pending = self.open_all(None).is_err();
                if !pending && !retarget {
                    self.stats.reconnects.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

fn read_loop(
    mut stream: TcpStream,
    channel: Channel,
    limits: Limits,
    inner: Weak<Inner>,
    alive: Arc<AtomicBool>,
    stats: Arc<Counters>,
) {
    let slot = match channel {
        Channel::Video => 0,
        Channel::Audio => 1,
    };
    let Some(tx) = inner.upgrade().map(|i| i.events.clone()) else {
        return;
    };
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
        stats.bytes[slot].fetch_add(n as u64, Ordering::Relaxed);
        deframer.push(&buf[..n]);
        loop {
            match deframer.next_frame() {
                Ok(Some(f)) => {
                    stats.frames[slot].fetch_add(1, Ordering::Relaxed);
                    let heard = match (&f.ext, classify(&f.data)) {
                        (ExtendedHeader::None, Message::Redirect(x)) => redirect::parse(x),
                        _ => None,
                    };
                    if tx.send(Event::Frame(channel, f)).is_err() {
                        break 'read None; // receiver dropped
                    }
                    if let (Some(a), Some(i)) = (heard, inner.upgrade()) {
                        i.redirect_heard(false, a);
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
    // Let the supervisor reconnect without waiting for its next tick.
    if let Some(i) = inner.upgrade() {
        i.wake.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::classify;
    use crate::command::Message;
    use std::net::TcpListener;
    use std::time::Instant;

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
        let stats = r.stats();
        assert_eq!((stats.reconnects, stats.redirects), (1, 0));
    }

    #[test]
    fn drop_is_bounded_with_a_stalled_sender() {
        // A "sender" that accepts and then neither reads nor writes: the
        // reader thread sits in `read` until the socket is shut down.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let r = Receiver::connect(addr, ReceiverConfig::default()).unwrap();
        let held: Vec<_> = (0..2).map(|_| listener.accept().unwrap().0).collect();
        assert!(r.is_connected());
        let start = Instant::now();
        drop(r);
        assert!(
            start.elapsed() < DROP_TIMEOUT + Duration::from_secs(1),
            "drop took {:?}",
            start.elapsed()
        );
        drop(held);
    }

    #[test]
    fn drop_is_bounded_while_connecting() {
        // The sender went away; the supervisor keeps trying while we drop.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let cfg = ReceiverConfig {
            audio: false,
            ..ReceiverConfig::default()
        };
        let r = Receiver::connect(addr, cfg).unwrap();
        drop(listener);
        std::thread::sleep(Duration::from_millis(1200));
        let start = Instant::now();
        drop(r);
        assert!(start.elapsed() < DROP_TIMEOUT + Duration::from_secs(1));
    }

    #[test]
    fn metadata_only_sends_only_the_metadata_subscription() {
        // §4.3, `OMTReceive.cs:443-449`: no tally on a metadata-only connection.
        let (addr, h) = capture_connections(1);
        let cfg = ReceiverConfig {
            video: false,
            audio: false,
            reconnect: false,
            ..ReceiverConfig::default()
        };
        drop(Receiver::connect(addr, cfg).unwrap());
        let got = h.join().unwrap();
        assert_eq!(commands_in(&got[0]), [Command::SubscribeMetadata]);
    }

    #[test]
    fn connects_by_url() {
        let (addr, h) = capture_connections(1);
        let cfg = ReceiverConfig {
            audio: false,
            reconnect: false,
            ..ReceiverConfig::default()
        };
        let r = Receiver::connect_to(&format!("omt://127.0.0.1:{}", addr.port()), cfg).unwrap();
        assert_eq!(r.peer_addr(), Some(addr));
        drop(r);
        assert_eq!(
            commands_in(&h.join().unwrap()[0])[0],
            Command::SubscribeMetadata
        );
        assert!(Receiver::connect_to("omt://127.0.0.1", cfg).is_err());
    }

    #[test]
    fn a_name_is_resolved_again_after_the_sender_moves() {
        use crate::discovery::{Source, SourceEvent};
        use crate::sender::{Sender, SenderConfig};
        let quiet = || SenderConfig {
            announce: false,
            ports: 17400..=17600,
            ..SenderConfig::new("moves")
        };
        let source = |port| {
            SourceEvent::Resolved(Source {
                full_name: "TEST (moves)".into(),
                host: "test-omt.local.".into(),
                port,
                addresses: vec!["127.0.0.1".parse().unwrap()],
            })
        };
        let first = Sender::new(quiet()).unwrap();
        let dir = Arc::new(Directory::manual());
        dir.apply(source(first.port()));
        let cfg = ReceiverConfig {
            audio: false,
            ..ReceiverConfig::default()
        };
        let r = Receiver::connect_address(
            Address::parse("TEST (moves)").unwrap(),
            cfg,
            Some(dir.clone()),
        )
        .unwrap();
        assert_eq!(r.peer_addr().unwrap().port(), first.port());

        // The sender restarts on another port; discovery sees it go and come back.
        let second = Sender::new(quiet()).unwrap();
        assert_ne!(second.port(), first.port());
        dir.apply(SourceEvent::Removed("TEST (moves)".into()));
        drop(first);
        std::thread::sleep(Duration::from_millis(1500));
        assert!(!r.is_connected(), "nothing to connect to while it is gone");
        dir.apply(source(second.port()));
        let deadline = Instant::now() + Duration::from_secs(3);
        while second.video_receivers() == 0 {
            assert!(Instant::now() < deadline, "not reconnected");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(r.peer_addr().unwrap().port(), second.port());
    }
}
