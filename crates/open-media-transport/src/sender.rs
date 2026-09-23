//! A sender: listens for receivers, answers their commands and sends them
//! video, audio and metadata (`docs/PROTOCOL.md` §1, §4, §6).
//!
//! Behaviour follows libomtnet's `OMTSend`:
//!
//! - listens on the first free port of 6400–6600 on a dual-stack socket (T1, T2);
//! - on each new connection sends sender info, connection metadata and the
//!   combined tally, before any subscription arrives (§4.2, §4.3);
//! - sends video and audio only to connections that subscribed to them, and
//!   application metadata only to those that subscribed to metadata (M6);
//! - combines receivers' tallies with OR and broadcasts changes (§4.2);
//! - picks the encoder profile from the highest suggested quality among video
//!   connections when its own quality is `Default` (V3);
//! - leaves silent audio channels out (A2);
//! - never blocks on a slow receiver: each connection has at most 4 video or
//!   audio frames and 64 metadata frames queued, and drops beyond that, as
//!   libomtnet's send pools do (`OMTChannel.cs:207-218`, `OMTConstants.cs:46,51`).
//!
//! One deliberate difference: libomtnet's preview frames carry VMX bytes where
//! per-frame metadata should be (U2). Here the metadata follows the preview
//! prefix, so receivers that take the last `MetadataLength` bytes get it right.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::ops::RangeInclusive;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use socket2::{Domain, Protocol, Socket, Type};
use vmx_codec::{Decoder, Encoder, EncoderConfig, Profile};

use crate::command::{classify, Command, Message, Quality, Tally};
use crate::discovery::Discovery;
use crate::frame::{
    self, AudioHeader, ExtendedHeader, VideoFlags, VideoHeader, CODEC_FPA1, CODEC_VMX1,
    VIDEO_HEADER_LEN,
};
use crate::{Deframer, Limits};

/// libomtnet's default port range (`OMTConstants.cs:64-65`).
pub const DEFAULT_PORTS: RangeInclusive<u16> = 6400..=6600;
/// Frames larger than this are dropped, not sent (R4).
pub const MAX_FRAME_LEN: usize = 10_485_760;
const MAX_QUEUED_AV: usize = 4;
const MAX_QUEUED_METADATA: usize = 64;
const MAX_UNREAD_METADATA: usize = 60;

/// Describes the sending product to receivers (`<OMTInfo …/>`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SenderInfo {
    /// Product name.
    pub product_name: String,
    /// Manufacturer.
    pub manufacturer: String,
    /// Version.
    pub version: String,
}

impl SenderInfo {
    /// The XML libomtnet produces, byte for byte for plain ASCII values
    /// (captured, `docs/evidence/2026-09-21-libomtnet-loopback`). How .NET
    /// escapes special characters has not been captured; `&`, `<`, `>` and
    /// `"` are escaped here.
    pub fn to_xml(&self) -> String {
        format!(
            r#"<OMTInfo ProductName="{}" Manufacturer="{}" Version="{}" />"#,
            escape_attr(&self.product_name),
            escape_attr(&self.manufacturer),
            escape_attr(&self.version)
        )
    }
}

/// How to create a sender.
#[derive(Clone, Debug)]
pub struct SenderConfig {
    /// Source name; the full name becomes `MACHINE (name)`.
    pub name: String,
    /// Encoder quality. `Default` follows receivers' suggestions.
    pub quality: Quality,
    /// Sent to every receiver on connect, if set.
    pub info: Option<SenderInfo>,
    /// Extra metadata strings sent to every receiver on connect.
    pub connection_metadata: Vec<Vec<u8>>,
    /// Announce over DNS-SD, or to `discovery_server` if one is set.
    pub announce: bool,
    /// Announce to this discovery server (`omt://host[:port]`) instead of
    /// over DNS-SD, as libomtnet does when one is configured (S1).
    pub discovery_server: Option<String>,
    /// Ports to try, in order.
    pub ports: RangeInclusive<u16>,
}

impl SenderConfig {
    /// A sender called `name` with libomtnet's defaults.
    pub fn new(name: impl Into<String>) -> Self {
        SenderConfig {
            name: name.into(),
            quality: Quality::Default,
            info: None,
            connection_metadata: Vec::new(),
            announce: true,
            discovery_server: None,
            ports: DEFAULT_PORTS,
        }
    }
}

/// Video frame properties that travel in the header (§3.3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VideoParams {
    /// Frame rate numerator.
    pub frame_rate_n: i32,
    /// Frame rate denominator.
    pub frame_rate_d: i32,
    /// Display aspect ratio, width / height.
    pub aspect_ratio: f32,
    /// 0 undefined, 601 or 709.
    pub color_space: i32,
    /// Premultiplied alpha (only meaningful for formats with alpha).
    pub premultiplied: bool,
}

/// Counters since the sender started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SenderStats {
    /// Frames queued to a connection.
    pub frames_queued: u64,
    /// Frames dropped because a connection's queue was full or the frame was too large.
    pub frames_dropped: u64,
}

/// An OMT source.
pub struct Sender {
    shared: Arc<Shared>,
    port: u16,
    full_name: Option<String>,
    discovery: Option<Discovery>,
    accept: Option<JoinHandle<()>>,
    video: Mutex<VideoState>,
    metadata_rx: Mutex<mpsc::Receiver<(SocketAddr, Vec<u8>)>>,
}

struct VideoState {
    encoder: Option<(usize, usize, Profile, Encoder)>,
    decoder: Option<(usize, usize, Decoder)>,
    buf: Vec<u8>,
}

impl Sender {
    /// Binds a port, starts accepting receivers and, if configured, announces.
    pub fn new(config: SenderConfig) -> io::Result<Sender> {
        let (listener, port) = bind_first_free(config.ports.clone())?;
        let mut on_connect = Vec::new();
        if let Some(info) = &config.info {
            on_connect.push(info.to_xml().into_bytes());
        }
        on_connect.extend(config.connection_metadata.iter().cloned());
        let (metadata_tx, metadata_rx) = mpsc::sync_channel(MAX_UNREAD_METADATA);
        let shared = Arc::new(Shared {
            peers: Mutex::new(Vec::new()),
            on_connect,
            quality: config.quality,
            tally: Mutex::new(Tally::default()),
            metadata_tx,
            closing: AtomicBool::new(false),
            queued: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        });
        // Announce before starting the accept thread, so a failure leaves nothing running.
        let (discovery, full_name) = if config.announce {
            let d = match &config.discovery_server {
                Some(url) => Discovery::with_server(url, false),
                None => Discovery::new(),
            }
            .map_err(io::Error::other)?;
            let full = d.announce(&config.name, port).map_err(io::Error::other)?;
            (Some(d), Some(full))
        } else {
            (None, None)
        };
        let s = shared.clone();
        let accept = std::thread::Builder::new()
            .name("omt-send-accept".into())
            .spawn(move || accept_loop(listener, s))?;
        Ok(Sender {
            shared,
            port,
            full_name,
            discovery,
            accept: Some(accept),
            video: Mutex::new(VideoState {
                encoder: None,
                decoder: None,
                buf: Vec::new(),
            }),
            metadata_rx: Mutex::new(metadata_rx),
        })
    }

    /// The TCP port receivers connect to.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// `MACHINE (Name)` if announced.
    pub fn full_name(&self) -> Option<&str> {
        self.full_name.as_deref()
    }

    /// Open connections (a typical receiver uses two, T5).
    pub fn connections(&self) -> usize {
        self.shared.peers.lock().unwrap().len()
    }

    /// Connections currently subscribed to video.
    pub fn video_receivers(&self) -> usize {
        self.shared
            .snapshot()
            .iter()
            .filter(|p| p.state.lock().unwrap().video)
            .count()
    }

    /// Combined tally of all receivers.
    pub fn tally(&self) -> Tally {
        *self.shared.tally.lock().unwrap()
    }

    /// Frame counters.
    pub fn stats(&self) -> SenderStats {
        SenderStats {
            frames_queued: self.shared.queued.load(Ordering::Relaxed),
            frames_dropped: self.shared.dropped.load(Ordering::Relaxed),
        }
    }

    /// Encodes `frame` with VMX1 and sends it to every video subscriber.
    /// `metadata` is per-frame XML (include a trailing NUL if receivers expect
    /// one). Returns the number of connections it was queued for.
    pub fn send_video(
        &self,
        frame: &vmx_codec::Frame,
        params: VideoParams,
        timestamp: i64,
        metadata: &[u8],
    ) -> Result<usize, vmx_codec::Error> {
        let peers = self.shared.snapshot();
        let video_peers: Vec<&Arc<Peer>> = peers
            .iter()
            .filter(|p| p.state.lock().unwrap().video)
            .collect();
        let profile = profile_for(self.shared.quality, &video_peers);
        let (w, h) = (frame.width, frame.height);

        let mut v = self.video.lock().unwrap();
        let v = &mut *v;
        match &mut v.encoder {
            Some((ew, eh, ep, enc)) if (*ew, *eh) == (w, h) => {
                if *ep != profile {
                    // Keep the running quality across a profile change, as
                    // libomtnet does (`OMTSend.cs:524-530`).
                    let q = enc.quality();
                    let mut cfg = EncoderConfig::new(w, h);
                    cfg.profile = profile;
                    *enc = Encoder::new(cfg)?;
                    enc.set_quality(q);
                    *ep = profile;
                }
            }
            slot => {
                let mut cfg = EncoderConfig::new(w, h);
                cfg.profile = profile;
                *slot = Some((w, h, profile, Encoder::new(cfg)?));
            }
        }
        let enc = &mut v.encoder.as_mut().unwrap().3;
        v.buf.clear(); // encode_into appends
        enc.encode_into(frame, &mut v.buf)?;

        let mut flags = 0;
        if frame.interlaced {
            flags |= VideoFlags::INTERLACED;
        }
        if frame.format.has_alpha() {
            flags |= VideoFlags::ALPHA;
            if params.premultiplied {
                flags |= VideoFlags::PREMULTIPLIED;
            }
        }
        if frame.format.is_10bit() {
            flags |= VideoFlags::HIGH_BIT_DEPTH; // OMTSend.cs:725,736
        }
        let header = VideoHeader {
            codec: CODEC_VMX1,
            width: w as i32,
            height: h as i32,
            frame_rate_n: params.frame_rate_n,
            frame_rate_d: params.frame_rate_d,
            aspect_ratio: params.aspect_ratio,
            flags: VideoFlags(flags),
            color_space: params.color_space,
        };
        let mut full = Vec::new();
        frame::write(
            timestamp,
            &ExtendedHeader::Video(header),
            &v.buf,
            metadata,
            &mut full,
        );
        let full = Arc::new(full);

        let wants_preview = video_peers.iter().any(|p| p.state.lock().unwrap().preview);
        let preview = if wants_preview {
            if v.decoder.as_ref().map(|d| (d.0, d.1)) != Some((w, h)) {
                v.decoder = Some((w, h, Decoder::new(w, h)?));
            }
            let len = v.decoder.as_ref().unwrap().2.preview_len(&v.buf)?;
            let mut ph = header;
            ph.flags = VideoFlags(flags | VideoFlags::PREVIEW);
            let mut out = Vec::new();
            frame::write(
                timestamp,
                &ExtendedHeader::Video(ph),
                &v.buf[..len],
                metadata,
                &mut out,
            );
            debug_assert_eq!(
                out.len(),
                frame::HEADER_LEN + VIDEO_HEADER_LEN + len + metadata.len()
            );
            Some(Arc::new(out))
        } else {
            None
        };

        let mut n = 0;
        for p in video_peers {
            let bytes = match (&preview, p.state.lock().unwrap().preview) {
                (Some(pv), true) => pv.clone(),
                _ => full.clone(),
            };
            n += self.shared.queue(p, bytes, false) as usize;
        }
        Ok(n)
    }

    /// Sends planar 32-bit float audio: `samples` holds `channels` planes of
    /// `samples.len() / channels` samples each. Returns the number of
    /// connections it was queued for.
    ///
    /// # Panics
    ///
    /// If `channels` is not 1..=32 or does not divide `samples.len()`.
    pub fn send_audio(
        &self,
        samples: &[f32],
        channels: usize,
        sample_rate: i32,
        timestamp: i64,
        metadata: &[u8],
    ) -> usize {
        assert!((1..=32).contains(&channels), "1..=32 channels");
        assert_eq!(
            samples.len() % channels,
            0,
            "samples not a multiple of channels"
        );
        let spc = samples.len() / channels;
        let mut active = 0u32;
        let mut data = Vec::with_capacity(samples.len() * 4);
        for (ch, plane) in samples.chunks_exact(spc.max(1)).enumerate() {
            // A2: a channel is left out when every byte is zero, so -0.0 counts as sound.
            if plane.iter().any(|s| s.to_bits() != 0) {
                active |= 1 << ch;
                for s in plane {
                    data.extend_from_slice(&s.to_le_bytes());
                }
            }
        }
        let header = AudioHeader {
            codec: CODEC_FPA1,
            sample_rate,
            samples_per_channel: spc as i32,
            channels: channels as i32,
            active_channels: active,
            reserved: 0,
        };
        let mut out = Vec::new();
        frame::write(
            timestamp,
            &ExtendedHeader::Audio(header),
            &data,
            metadata,
            &mut out,
        );
        let out = Arc::new(out);
        let mut n = 0;
        for p in self.shared.snapshot() {
            if p.state.lock().unwrap().audio {
                n += self.shared.queue(&p, out.clone(), false) as usize;
            }
        }
        n
    }

    /// Sends application metadata to every connection that subscribed to
    /// metadata. Returns the number of connections it was queued for.
    pub fn send_metadata(&self, xml: &[u8], timestamp: i64) -> usize {
        let mut out = Vec::new();
        frame::write_metadata(timestamp, xml, &mut out);
        self.shared.broadcast_metadata(Arc::new(out))
    }

    /// Waits up to `timeout` for application metadata sent by a receiver.
    pub fn recv_metadata(&self, timeout: Duration) -> Option<(SocketAddr, Vec<u8>)> {
        self.metadata_rx.lock().unwrap().recv_timeout(timeout).ok()
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        if let (Some(d), Some(full)) = (&self.discovery, &self.full_name) {
            let _ = d.withdraw(full);
        }
        self.shared.closing.store(true, Ordering::SeqCst);
        // Wake the blocking accept.
        let _ = TcpStream::connect_timeout(
            &SocketAddr::from(([127, 0, 0, 1], self.port)),
            Duration::from_millis(200),
        );
        if let Some(h) = self.accept.take() {
            let _ = h.join();
        }
        let peers: Vec<Arc<Peer>> = std::mem::take(&mut *self.shared.peers.lock().unwrap());
        for p in peers {
            p.close();
            p.join();
        }
    }
}

struct Shared {
    peers: Mutex<Vec<Arc<Peer>>>,
    on_connect: Vec<Vec<u8>>,
    quality: Quality,
    tally: Mutex<Tally>,
    metadata_tx: mpsc::SyncSender<(SocketAddr, Vec<u8>)>,
    closing: AtomicBool,
    queued: AtomicU64,
    dropped: AtomicU64,
}

impl Shared {
    fn snapshot(&self) -> Vec<Arc<Peer>> {
        self.peers.lock().unwrap().clone()
    }

    fn queue(&self, peer: &Peer, bytes: Arc<Vec<u8>>, metadata: bool) -> bool {
        let ok = bytes.len() <= MAX_FRAME_LEN && peer.outbox.push(bytes, metadata);
        let counter = if ok { &self.queued } else { &self.dropped };
        counter.fetch_add(1, Ordering::Relaxed);
        ok
    }

    fn broadcast_metadata(&self, bytes: Arc<Vec<u8>>) -> usize {
        let mut n = 0;
        for p in self.snapshot() {
            if p.state.lock().unwrap().metadata {
                n += self.queue(&p, bytes.clone(), true) as usize;
            }
        }
        n
    }

    /// Recomputes the combined tally and broadcasts it if it changed
    /// (`OMTSendReceiveBase.cs:117-129`, `OMTSend.cs:464-467`).
    fn update_tally(&self) {
        let combined = self.snapshot().iter().fold(Tally::default(), |t, p| {
            t.union(p.state.lock().unwrap().tally)
        });
        let changed = {
            let mut t = self.tally.lock().unwrap();
            let changed = *t != combined;
            *t = combined;
            changed
        };
        if changed {
            let mut out = Vec::new();
            frame::write_metadata(0, Command::Tally(combined).as_bytes(), &mut out);
            self.broadcast_metadata(Arc::new(out));
        }
    }

    fn remove(&self, id: u64) {
        let removed = {
            let mut peers = self.peers.lock().unwrap();
            let before = peers.len();
            peers.retain(|p| p.id != id);
            before != peers.len()
        };
        if removed {
            self.update_tally();
        }
    }
}

#[derive(Default)]
struct PeerState {
    video: bool,
    audio: bool,
    metadata: bool,
    preview: bool,
    tally: Tally,
    quality: Quality,
}

struct Peer {
    id: u64,
    addr: SocketAddr,
    stream: TcpStream,
    state: Mutex<PeerState>,
    outbox: Outbox,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl Peer {
    fn close(&self) {
        self.outbox.close();
        let _ = self.stream.shutdown(Shutdown::Both);
    }

    fn join(&self) {
        let handles = std::mem::take(&mut *self.threads.lock().unwrap());
        for h in handles {
            if h.thread().id() != std::thread::current().id() {
                let _ = h.join();
            }
        }
    }
}

/// Bounded per-connection send queue, written by one thread.
struct Outbox {
    q: Mutex<OutQueue>,
    ready: Condvar,
}

#[derive(Default)]
struct OutQueue {
    items: VecDeque<(Arc<Vec<u8>>, bool)>,
    av: usize,
    metadata: usize,
    closed: bool,
}

impl Outbox {
    fn new() -> Self {
        Outbox {
            q: Mutex::new(OutQueue::default()),
            ready: Condvar::new(),
        }
    }

    fn push(&self, bytes: Arc<Vec<u8>>, metadata: bool) -> bool {
        let mut q = self.q.lock().unwrap();
        if q.closed {
            return false;
        }
        let (count, max) = if metadata {
            (&mut q.metadata, MAX_QUEUED_METADATA)
        } else {
            (&mut q.av, MAX_QUEUED_AV)
        };
        if *count >= max {
            return false;
        }
        *count += 1;
        q.items.push_back((bytes, metadata));
        self.ready.notify_one();
        true
    }

    /// Blocks until there is something to write; `None` once closed.
    fn pop(&self) -> Option<(Arc<Vec<u8>>, bool)> {
        let mut q = self.q.lock().unwrap();
        loop {
            if q.closed {
                return None;
            }
            if let Some(item) = q.items.pop_front() {
                return Some(item);
            }
            q = self.ready.wait(q).unwrap();
        }
    }

    /// Marks an item as written, freeing its slot.
    fn done(&self, metadata: bool) {
        let mut q = self.q.lock().unwrap();
        if metadata {
            q.metadata -= 1;
        } else {
            q.av -= 1;
        }
    }

    fn close(&self) {
        self.q.lock().unwrap().closed = true;
        self.ready.notify_all();
    }
}

fn bind_first_free(ports: RangeInclusive<u16>) -> io::Result<(TcpListener, u16)> {
    let mut last = io::Error::new(io::ErrorKind::AddrInUse, "no free port in range");
    for port in ports {
        match bind_dual_stack(port) {
            Ok(l) => return Ok((l, port)),
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => last = e,
            Err(e) => return Err(e),
        }
    }
    Err(last)
}

/// `[::]:port` with IPv6-only off, like libomtnet (T1); IPv4 only if the
/// host has no IPv6.
pub(crate) fn bind_dual_stack(port: u16) -> io::Result<TcpListener> {
    let v6 = (|| {
        let s = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?;
        s.set_only_v6(false)?;
        // Lets a restarted sender take its port back while old connections
        // sit in TIME_WAIT. Not on Windows, where it would allow stealing.
        #[cfg(unix)]
        s.set_reuse_address(true)?;
        s.bind(&SocketAddr::from(([0u16; 8], port)).into())?;
        s.listen(5)?;
        Ok::<_, io::Error>(s)
    })();
    let s = match v6 {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => return Err(e),
        Err(_) => {
            let s = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
            #[cfg(unix)]
            s.set_reuse_address(true)?;
            s.bind(&SocketAddr::from(([0u8; 4], port)).into())?;
            s.listen(5)?;
            s
        }
    };
    Ok(s.into())
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>) {
    let mut next_id = 0u64;
    for conn in listener.incoming() {
        if shared.closing.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = conn else { continue };
        next_id += 1;
        if let Err(e) = start_peer(stream, next_id, &shared) {
            // A connection that fails during setup is simply dropped.
            let _ = e;
        }
    }
}

fn start_peer(stream: TcpStream, id: u64, shared: &Arc<Shared>) -> io::Result<()> {
    stream.set_nodelay(true)?; // T3
    let addr = stream.peer_addr()?;
    let peer = Arc::new(Peer {
        id,
        addr,
        stream: stream.try_clone()?,
        state: Mutex::new(PeerState::default()),
        outbox: Outbox::new(),
        threads: Mutex::new(Vec::new()),
    });

    // §4.2: sender info, connection metadata, then the combined tally, sent
    // whatever the connection later subscribes to (M6).
    for xml in &shared.on_connect {
        let mut out = Vec::new();
        frame::write_metadata(0, xml, &mut out);
        shared.queue(&peer, Arc::new(out), true);
    }
    let mut out = Vec::new();
    let tally = *shared.tally.lock().unwrap();
    frame::write_metadata(0, Command::Tally(tally).as_bytes(), &mut out);
    shared.queue(&peer, Arc::new(out), true);

    let writer_peer = peer.clone();
    let writer_shared = shared.clone();
    let writer_stream = stream.try_clone()?;
    let writer = std::thread::Builder::new()
        .name(format!("omt-send-w{id}"))
        .spawn(move || write_loop(writer_stream, writer_peer, writer_shared))?;
    let reader_peer = peer.clone();
    let reader_shared = shared.clone();
    let reader = std::thread::Builder::new()
        .name(format!("omt-send-r{id}"))
        .spawn(move || read_loop(stream, reader_peer, reader_shared))?;
    peer.threads.lock().unwrap().extend([writer, reader]);
    shared.peers.lock().unwrap().push(peer);
    shared.update_tally();
    Ok(())
}

fn write_loop(mut stream: TcpStream, peer: Arc<Peer>, shared: Arc<Shared>) {
    while let Some((bytes, metadata)) = peer.outbox.pop() {
        let result = stream.write_all(&bytes);
        peer.outbox.done(metadata);
        if result.is_err() {
            break;
        }
    }
    peer.close();
    shared.remove(peer.id);
}

fn read_loop(mut stream: TcpStream, peer: Arc<Peer>, shared: Arc<Shared>) {
    // Accepted connections are metadata channels with a 1 MiB receive
    // buffer in libomtnet (`OMTChannel.cs:111-114`, `OMTSend.cs:364`).
    let mut deframer = Deframer::new(Limits::AUDIO_OR_METADATA);
    let mut buf = vec![0u8; 64 * 1024];
    'read: loop {
        let n = match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        deframer.push(&buf[..n]);
        loop {
            match deframer.next_frame() {
                Ok(Some(f)) if f.ext == ExtendedHeader::None => {
                    handle_metadata(&f.data, &peer, &shared)
                }
                Ok(Some(_)) => {} // receivers send only metadata; ignore the rest
                Ok(None) => break,
                Err(_) => break 'read,
            }
        }
    }
    peer.close();
    shared.remove(peer.id);
}

fn handle_metadata(data: &[u8], peer: &Peer, shared: &Shared) {
    let mut tally_changed = false;
    match classify(data) {
        Message::Command(c) => {
            let mut s = peer.state.lock().unwrap();
            match c {
                Command::SubscribeVideo => s.video = true,
                Command::SubscribeAudio => s.audio = true,
                Command::SubscribeMetadata => s.metadata = true,
                Command::Preview(on) => s.preview = on,
                Command::Tally(t) => {
                    tally_changed = s.tally != t;
                    s.tally = t;
                }
                Command::Quality(q) => s.quality = q,
            }
        }
        // Consumed by libomtnet; its XML parsing is not implemented here.
        Message::QualityOther(_) | Message::Redirect(_) => {}
        Message::SenderInfo(x) | Message::Application(x) => {
            // Dropped when the application is not reading (`OMTChannel.cs:400-410`).
            let _ = shared.metadata_tx.try_send((peer.addr, x.to_vec()));
        }
    }
    if tally_changed {
        shared.update_tally();
    }
}

/// V3: `Default` defers to the highest suggestion among video connections;
/// `Default` everywhere means `OMT_SQ` (`codecs/OMTVMX1Codec.cs:105`,
/// `OMTSend.cs:511-519`).
fn profile_for(own: Quality, video_peers: &[&Arc<Peer>]) -> Profile {
    let q = if own != Quality::Default {
        own
    } else {
        video_peers
            .iter()
            .map(|p| p.state.lock().unwrap().quality)
            .max()
            .unwrap_or_default()
    };
    match q {
        Quality::Low => Profile::OmtLq,
        Quality::Default | Quality::Medium => Profile::OmtSq,
        Quality::High => Profile::OmtHq,
    }
}

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::receiver::{Event, Receiver, ReceiverConfig};
    use std::time::Instant;
    use vmx_codec::{Frame, PixelFormat};

    fn quiet(name: &str) -> SenderConfig {
        SenderConfig {
            announce: false,
            ports: 16400..=16600,
            info: Some(SenderInfo {
                product_name: "test".into(),
                manufacturer: "open-media-transport".into(),
                version: "0".into(),
            }),
            ..SenderConfig::new(name)
        }
    }

    fn wait_for(mut cond: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !cond() {
            assert!(Instant::now() < deadline, "timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn sender_info_xml_matches_capture() {
        let info = SenderInfo {
            product_name: "omt-harness".into(),
            manufacturer: "open-media-transport".into(),
            version: "0.1".into(),
        };
        assert_eq!(
            info.to_xml(),
            r#"<OMTInfo ProductName="omt-harness" Manufacturer="open-media-transport" Version="0.1" />"#
        );
    }

    #[test]
    fn our_receiver_gets_video_audio_and_tally() {
        let tx = Sender::new(quiet("loop")).unwrap();
        let addr = SocketAddr::from(([127, 0, 0, 1], tx.port()));
        let cfg = ReceiverConfig {
            quality: Quality::High,
            tally: Tally {
                preview: true,
                program: false,
            },
            ..ReceiverConfig::default()
        };
        let rx = Receiver::connect(addr, cfg).unwrap();
        wait_for(|| {
            tx.connections() == 2
                && tx.tally().preview
                && tx.send_video(
                    &Frame::new(64, 32, PixelFormat::Uyvy),
                    VideoParams {
                        frame_rate_n: 30,
                        frame_rate_d: 1,
                        aspect_ratio: 2.0,
                        color_space: 709,
                        premultiplied: false,
                    },
                    0,
                    b"",
                ) == Ok(1)
        });
        let mut audio = vec![0.0f32; 2 * 100];
        audio[..100].fill(0.5);
        assert_eq!(tx.send_audio(&audio, 2, 48000, 7, b""), 1);

        let (mut info, mut video, mut audio_hdr) = (false, None, None);
        let deadline = Instant::now() + Duration::from_secs(3);
        while (video.is_none() || audio_hdr.is_none()) && Instant::now() < deadline {
            if let Some(Event::Frame(_, f)) = rx.recv_timeout(Duration::from_millis(100)) {
                match f.ext {
                    ExtendedHeader::None => {
                        info |= matches!(classify(&f.data), Message::SenderInfo(_))
                    }
                    ExtendedHeader::Video(v) => video = Some((v, f.data)),
                    ExtendedHeader::Audio(a) => audio_hdr = Some((a, f.data)),
                }
            }
        }
        assert!(info, "sender info on connect");
        let (v, data) = video.expect("video frame");
        assert_eq!(
            (v.codec, v.width, v.height, v.color_space),
            (CODEC_VMX1, 64, 32, 709)
        );
        let px = Decoder::new(64, 32)
            .unwrap()
            .decode(&data, PixelFormat::Uyvy)
            .unwrap();
        assert_eq!(px.planes[0].data.len(), 64 * 2 * 32);
        let (a, data) = audio_hdr.expect("audio frame");
        assert_eq!(
            (a.channels, a.active_channels, a.samples_per_channel),
            (2, 1, 100)
        );
        assert_eq!(data.len(), 400, "silent channel left out");
    }

    #[test]
    fn preview_connections_get_the_prefix_and_the_metadata() {
        let tx = Sender::new(quiet("preview")).unwrap();
        let addr = SocketAddr::from(([127, 0, 0, 1], tx.port()));
        let cfg = ReceiverConfig {
            audio: false,
            preview: true,
            ..ReceiverConfig::default()
        };
        let rx = Receiver::connect(addr, cfg).unwrap();
        let mut frame = Frame::new(256, 64, PixelFormat::Uyvy);
        for (i, b) in frame.planes[0].data.iter_mut().enumerate() {
            *b = (i * 7 % 251) as u8;
        }
        let params = VideoParams {
            frame_rate_n: 25,
            frame_rate_d: 1,
            aspect_ratio: 4.0,
            color_space: 0,
            premultiplied: false,
        };
        wait_for(|| tx.send_video(&frame, params, 0, b"<m/>\0") == Ok(1));
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if let Some(Event::Frame(_, f)) = rx.recv_timeout(Duration::from_millis(100)) {
                if let ExtendedHeader::Video(v) = f.ext {
                    assert!(v.flags.contains(VideoFlags::PREVIEW));
                    assert_eq!(f.metadata, b"<m/>\0");
                    let dec = Decoder::new(256, 64).unwrap();
                    assert_eq!(dec.preview_len(&f.data).unwrap(), f.data.len());
                    return;
                }
            }
        }
        panic!("no preview frame");
    }

    #[test]
    fn repeated_frames_do_not_grow() {
        // Regression: the encode buffer was reused without clearing, so each
        // frame carried every previous one (found against libomtnet, 21 Sep 2026).
        let tx = Sender::new(quiet("repeat")).unwrap();
        let addr = SocketAddr::from(([127, 0, 0, 1], tx.port()));
        let cfg = ReceiverConfig {
            audio: false,
            ..ReceiverConfig::default()
        };
        let rx = Receiver::connect(addr, cfg).unwrap();
        let mut frame = Frame::new(128, 64, PixelFormat::Uyvy);
        for (i, b) in frame.planes[0].data.iter_mut().enumerate() {
            *b = (i % 200) as u8 + 20;
        }
        let params = VideoParams {
            frame_rate_n: 30,
            frame_rate_d: 1,
            aspect_ratio: 2.0,
            color_space: 709,
            premultiplied: false,
        };
        wait_for(|| tx.video_receivers() == 1);
        assert_eq!(tx.send_video(&frame, params, 0, b""), Ok(1));
        for ts in 1..5 {
            assert_eq!(tx.send_video(&frame, params, ts, b""), Ok(1));
        }
        let mut sizes = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        while sizes.len() < 5 && Instant::now() < deadline {
            if let Some(Event::Frame(_, f)) = rx.recv_timeout(Duration::from_millis(100)) {
                if let ExtendedHeader::Video(_) = f.ext {
                    if sizes.is_empty() {
                        // First frame: exactly what a fresh OMT_SQ encoder makes.
                        let mut cfg = EncoderConfig::new(128, 64);
                        cfg.profile = Profile::OmtSq;
                        let expected = Encoder::new(cfg).unwrap().encode(&frame).unwrap();
                        assert_eq!(f.data, expected);
                    }
                    sizes.push(f.data.len());
                }
            }
        }
        assert_eq!(sizes.len(), 5);
        assert!(sizes.iter().all(|&n| n < 128 * 64 * 2), "{sizes:?}");
        assert!(
            sizes.iter().max().unwrap() - sizes.iter().min().unwrap() < sizes[0] / 2,
            "{sizes:?}"
        );
    }

    #[test]
    fn outbox_limits() {
        let o = Outbox::new();
        for _ in 0..MAX_QUEUED_AV {
            assert!(o.push(Arc::new(vec![]), false));
        }
        assert!(!o.push(Arc::new(vec![]), false));
        assert!(o.push(Arc::new(vec![]), true));
        let (_, m) = o.pop().unwrap();
        o.done(m);
        assert!(o.push(Arc::new(vec![]), false));
    }
}
