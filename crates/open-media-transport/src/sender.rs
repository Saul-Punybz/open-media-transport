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
//!   libomtnet's send pools do (`OMTChannel.cs:207-218`, `OMTConstants.cs:46,51`);
//! - can redirect its receivers to another source (§9, [`Sender::set_redirect`]);
//! - can forward a frame that is already VMX1-compressed without touching it
//!   (V2, P4, [`Sender::send_encoded_video`]).
//!
//! Many senders can share one [`Discovery`] ([`SenderConfig::discovery`]),
//! so a process with many sources runs a single mDNS responder.
//!
//! Dropping a sender shuts its sockets down first and waits a bounded time
//! for its threads ([`DROP_TIMEOUT`]); a thread still stuck after that is
//! left to finish on its own rather than hang the caller.
//!
//! Deliberate differences: libomtnet's preview frames carry VMX bytes where
//! per-frame metadata should be (U2). Here the metadata follows the preview
//! prefix, so receivers that take the last `MetadataLength` bytes get it right.
//! And once a redirect has been cleared, libomtnet keeps sending
//! `<OMTRedirect NewAddress="" />` to every new connection
//! (`OMTRedirect.cs:59-63`, `OMTSend.cs:372-375`), which makes a libomtnet
//! receiver ignore all later redirects (see [`crate::receiver`]); here a new
//! connection gets a redirect only while one is active.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::ops::RangeInclusive;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use std::{error, fmt};

use socket2::{Domain, Protocol, Socket, Type};
use vmx_codec::{Decoder, Encoder, EncoderConfig, Profile};

use crate::address::Address;
use crate::command::{classify, Command, Message, Quality, Tally};
use crate::discovery::{self, Discovery};
use crate::frame::{
    self, AudioHeader, ExtendedHeader, VideoFlags, VideoHeader, CODEC_FPA1, CODEC_VMX1,
    VIDEO_HEADER_LEN,
};
use crate::redirect::{self, Watcher};
use crate::{Deframer, Limits};

/// libomtnet's default port range (`OMTConstants.cs:64-65`).
pub const DEFAULT_PORTS: RangeInclusive<u16> = 6400..=6600;
/// Frames larger than this are dropped, not sent (R4).
pub const MAX_FRAME_LEN: usize = 10_485_760;
const MAX_QUEUED_AV: usize = 4;
const MAX_QUEUED_METADATA: usize = 64;
const MAX_UNREAD_METADATA: usize = 60;
/// libomtnet refuses audio whose planar float data exceeds 1 MiB
/// (`OMTSend.cs:800-806`, `OMTConstants.cs:62`).
pub const MAX_AUDIO_DATA_LEN: usize = 1_048_576;
/// How long dropping a [`Sender`] or a [`crate::receiver::Receiver`] waits
/// for its threads after shutting its sockets down.
pub const DROP_TIMEOUT: Duration = Duration::from_secs(2);

/// Why a frame could not be sent. Frames that are valid but cannot be queued
/// (a full queue, a frame over [`MAX_FRAME_LEN`]) are not errors: they are
/// counted as dropped ([`SenderStats`]), as libomtnet does.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SendError {
    /// The VMX encoder rejected the frame.
    Codec(vmx_codec::Error),
    /// Audio channel count outside 1..=32 (`OMTSend.cs:800`).
    InvalidChannels(usize),
    /// The sample count is not a multiple of the channel count.
    SamplesNotMultipleOfChannels {
        /// Samples given.
        samples: usize,
        /// Channels given.
        channels: usize,
    },
    /// No samples, or a sample rate that is not positive (`OMTSend.cs:800`).
    EmptyAudio,
    /// Planar float data over [`MAX_AUDIO_DATA_LEN`] (`OMTSend.cs:802-806`).
    AudioTooLarge(usize),
    /// A pre-encoded frame with no data (`OMTSend.cs:768,782-785`).
    EmptyVideo,
    /// Width or height is zero or does not fit the header.
    InvalidDimensions {
        /// Width given.
        width: usize,
        /// Height given.
        height: usize,
    },
    /// Per-frame metadata over 65535 bytes, the most `MetadataLength` holds (§3.1).
    MetadataTooLarge(usize),
}

impl fmt::Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SendError::Codec(e) => write!(f, "VMX encoder: {e}"),
            SendError::InvalidChannels(n) => write!(f, "{n} audio channels; 1 to 32 allowed"),
            SendError::SamplesNotMultipleOfChannels { samples, channels } => {
                write!(
                    f,
                    "{samples} samples is not a multiple of {channels} channels"
                )
            }
            SendError::EmptyAudio => write!(f, "no audio samples or no sample rate"),
            SendError::AudioTooLarge(n) => write!(
                f,
                "{n} bytes of audio exceeds the {MAX_AUDIO_DATA_LEN}-byte limit"
            ),
            SendError::EmptyVideo => write!(f, "empty VMX1 frame"),
            SendError::InvalidDimensions { width, height } => {
                write!(f, "invalid frame size {width}x{height}")
            }
            SendError::MetadataTooLarge(n) => {
                write!(f, "{n} bytes of per-frame metadata exceeds 65535")
            }
        }
    }
}

impl error::Error for SendError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            SendError::Codec(e) => Some(e),
            _ => None,
        }
    }
}

impl From<vmx_codec::Error> for SendError {
    fn from(e: vmx_codec::Error) -> Self {
        SendError::Codec(e)
    }
}

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
    /// Announce with this [`Discovery`], shared with other senders and
    /// directories, instead of starting one for this sender. When set,
    /// `discovery_server` is not used: configure the shared one instead.
    pub discovery: Option<Arc<Discovery>>,
    /// Worker threads for the VMX encoder (1 = encode on the calling thread;
    /// 0 is taken as 1). The bitstream does not depend on it. libvmx picks a
    /// count from the frame size (`vmxcodec.cpp:280-312`) and libomtnet
    /// doubles it above 60 fps (`codecs/OMTVMX1Codec.cs:107-113`).
    pub encoder_threads: usize,
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
            discovery: None,
            encoder_threads: 1,
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

/// A frame compressed with VMX1 elsewhere, for [`Sender::send_encoded_video`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EncodedVideo<'a> {
    /// The VMX1 bitstream, sent as is.
    pub data: &'a [u8],
    /// Width in pixels the frame was encoded at.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// How it was coded: [`VideoFlags::INTERLACED`], [`VideoFlags::ALPHA`],
    /// [`VideoFlags::HIGH_BIT_DEPTH`], [`VideoFlags::PREMULTIPLIED`].
    /// [`VideoFlags::PREVIEW`] is set per connection and ignored here.
    pub flags: VideoFlags,
}

/// Frame counters for one kind of frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameCounts {
    /// Frames queued to a connection (one per connection).
    pub queued: u64,
    /// Frames written to a socket.
    pub sent: u64,
    /// Frames not queued: the connection's queue was full, or the frame was
    /// over [`MAX_FRAME_LEN`].
    pub dropped: u64,
}

/// Counters since the sender started. Frames are counted once per
/// connection; frames still queued when a connection closes are neither
/// sent nor dropped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SenderStats {
    /// Frames queued to a connection, all kinds.
    pub frames_queued: u64,
    /// Frames dropped because a connection's queue was full or the frame was too large.
    pub frames_dropped: u64,
    /// Bytes written to all connections, headers included.
    pub bytes_sent: u64,
    /// Video frames.
    pub video: FrameCounts,
    /// Audio frames.
    pub audio: FrameCounts,
    /// Metadata frames, protocol messages included.
    pub metadata: FrameCounts,
    /// Open connections now (a typical receiver uses two, T5).
    pub connections: usize,
}

/// One open connection, for [`Sender::peer_stats`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerStats {
    /// The receiver's address.
    pub addr: SocketAddr,
    /// Subscribed to video.
    pub video: bool,
    /// Subscribed to audio.
    pub audio: bool,
    /// Subscribed to metadata.
    pub metadata: bool,
    /// Asked for preview video.
    pub preview: bool,
    /// Bytes written to it.
    pub bytes_sent: u64,
    /// Frames written to it.
    pub frames_sent: u64,
    /// Frames dropped for it because its queue was full or the frame too large.
    pub frames_dropped: u64,
    /// Frames waiting in its queue now.
    pub queued: usize,
}

/// An OMT source.
pub struct Sender {
    shared: Arc<Shared>,
    port: u16,
    full_name: Option<String>,
    discovery: Option<Arc<Discovery>>,
    accept: Option<JoinHandle<()>>,
    encoder_threads: usize,
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
        // What a redirect to ourselves looks like (X4): our full name, as
        // libomtnet compares (`OMTRedirect.cs:115-118`), and our URL (N4).
        let machine = discovery::machine_name();
        let self_names = vec![
            discovery::full_name(&machine, &config.name),
            format!("{}{machine}:{port}", crate::address::URL_PREFIX),
        ];
        let shared = Arc::new(Shared {
            redirect: Mutex::new(RedirectState::default()),
            self_names,
            peers: Mutex::new(Vec::new()),
            on_connect,
            quality: config.quality,
            tally: Mutex::new(Tally::default()),
            metadata_tx,
            closing: AtomicBool::new(false),
            counts: Default::default(),
            bytes_sent: AtomicU64::new(0),
        });
        // Announce before starting the accept thread, so a failure leaves nothing running.
        let (discovery, full_name) = if config.announce {
            let d = match (&config.discovery, &config.discovery_server) {
                (Some(d), _) => d.clone(),
                (None, Some(url)) => {
                    Arc::new(Discovery::with_server(url, false).map_err(io::Error::other)?)
                }
                (None, None) => Arc::new(Discovery::new().map_err(io::Error::other)?),
            };
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
            encoder_threads: config.encoder_threads.max(1),
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

    /// `omt://MACHINE:port`, the URL form of this sender's address (N4).
    pub fn url(&self) -> &str {
        &self.shared.self_names[1]
    }

    /// Tells receivers to use the source at `address` instead — a full name
    /// or an `omt://` URL — or, with `None` or an empty string, cancels the
    /// redirect (§9). The message goes to every metadata-subscribed
    /// connection now and to each new connection while the redirect is active
    /// (X1). A redirect to this sender itself is no redirect (X4).
    ///
    /// While redirected, the sender watches the target with a metadata-only
    /// connection; if the target is itself redirected, receivers are sent
    /// that address instead, and back again when it clears (X3). This follows
    /// `OMTRedirect.SetRedirect` and `OnRedirectChanged`
    /// (`OMTRedirect.cs:110-163`), including re-sending the message when the
    /// address has not changed.
    pub fn set_redirect(&self, address: Option<&str>) {
        let mut new = address.filter(|a| !a.is_empty()).map(str::to_owned);
        if new
            .as_ref()
            .is_some_and(|n| self.shared.self_names.contains(n))
        {
            new = None;
        }
        let (xml, old) = {
            let mut r = self.shared.redirect.lock().unwrap();
            if r.address != new {
                r.upstream = None;
            }
            r.address = new.clone();
            let keep = matches!((&new, &r.watcher), (Some(n), Some(w)) if w.address() == n);
            let old = if keep { None } else { r.watcher.take() };
            if let (Some(n), None) = (&new, &r.watcher) {
                r.generation += 1;
                let (weak, generation) = (Arc::downgrade(&self.shared), r.generation);
                if let Ok(target) = Address::parse(n) {
                    r.watcher =
                        Watcher::start(target, None, move |a| upstream_heard(&weak, generation, a))
                            .ok();
                }
            }
            (r.xml(), old)
        };
        // Dropped unlocked: its thread may be waiting for the lock.
        drop(old);
        let mut out = Vec::new();
        frame::write_metadata(0, xml.as_bytes(), &mut out);
        self.shared.broadcast_metadata(Arc::new(out));
    }

    /// The address receivers are being redirected to, if any: the one set
    /// with [`Sender::set_redirect`], or the target's own redirect (X3).
    pub fn redirect(&self) -> Option<String> {
        let r = self.shared.redirect.lock().unwrap();
        r.address.as_ref()?;
        Some(r.effective())
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

    /// Counters since the sender started.
    pub fn stats(&self) -> SenderStats {
        let c = &self.shared.counts;
        let kind = |k: Kind| FrameCounts {
            queued: c[k as usize][QUEUED].load(Ordering::Relaxed),
            sent: c[k as usize][SENT].load(Ordering::Relaxed),
            dropped: c[k as usize][DROPPED].load(Ordering::Relaxed),
        };
        let (video, audio, metadata) = (kind(Kind::Video), kind(Kind::Audio), kind(Kind::Metadata));
        SenderStats {
            frames_queued: video.queued + audio.queued + metadata.queued,
            frames_dropped: video.dropped + audio.dropped + metadata.dropped,
            bytes_sent: self.shared.bytes_sent.load(Ordering::Relaxed),
            video,
            audio,
            metadata,
            connections: self.connections(),
        }
    }

    /// Counters for each open connection.
    pub fn peer_stats(&self) -> Vec<PeerStats> {
        self.shared
            .snapshot()
            .iter()
            .map(|p| {
                let s = p.state.lock().unwrap();
                PeerStats {
                    addr: p.addr,
                    video: s.video,
                    audio: s.audio,
                    metadata: s.metadata,
                    preview: s.preview,
                    bytes_sent: p.bytes_sent.load(Ordering::Relaxed),
                    frames_sent: p.frames_sent.load(Ordering::Relaxed),
                    frames_dropped: p.frames_dropped.load(Ordering::Relaxed),
                    queued: p.outbox.len(),
                }
            })
            .collect()
    }

    /// Encodes `frame` with VMX1 and sends it to every video subscriber.
    /// `metadata` is per-frame XML (include a trailing NUL if receivers expect
    /// one). Returns the number of connections it was queued for.
    ///
    /// Encoding holds only this sender's encoder: other senders, and this
    /// sender's audio and metadata, are not held up by it.
    pub fn send_video(
        &self,
        frame: &vmx_codec::Frame,
        params: VideoParams,
        timestamp: i64,
        metadata: &[u8],
    ) -> Result<usize, SendError> {
        check_metadata(metadata)?;
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
                    cfg.threads = self.encoder_threads;
                    *enc = Encoder::new(cfg)?;
                    enc.set_quality(q);
                    *ep = profile;
                }
            }
            slot => {
                let mut cfg = EncoderConfig::new(w, h);
                cfg.profile = profile;
                cfg.threads = self.encoder_threads;
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
            n += self.shared.queue(p, bytes, Kind::Video) as usize;
        }
        Ok(n)
    }

    /// Sends a frame that is already VMX1-compressed, without decoding or
    /// re-encoding it, to every video subscriber, as libomtnet does when
    /// given a `VMX1` frame (V2, `OMTSend.cs:766-786`). Returns the number of
    /// connections it was queued for.
    ///
    /// The bitstream is not checked, the sender's quality setting and
    /// receivers' quality suggestions do not apply, and connections in
    /// preview mode get the whole frame with [`VideoFlags::PREVIEW`] set,
    /// because libomtnet sets the preview length of a forwarded frame to its
    /// full length (P4, `OMTSend.cs:772`); the 1/8 preview decode reads only
    /// the frame's DC prefix, so they can still show it.
    pub fn send_encoded_video(
        &self,
        frame: EncodedVideo<'_>,
        params: VideoParams,
        timestamp: i64,
        metadata: &[u8],
    ) -> Result<usize, SendError> {
        if frame.data.is_empty() {
            return Err(SendError::EmptyVideo);
        }
        let dims = (i32::try_from(frame.width), i32::try_from(frame.height));
        let (Ok(width @ 1..), Ok(height @ 1..)) = dims else {
            return Err(SendError::InvalidDimensions {
                width: frame.width,
                height: frame.height,
            });
        };
        check_metadata(metadata)?;
        let mut flags = frame.flags.0 & !VideoFlags::PREVIEW;
        if params.premultiplied && flags & VideoFlags::ALPHA != 0 {
            flags |= VideoFlags::PREMULTIPLIED;
        }
        let header = VideoHeader {
            codec: CODEC_VMX1,
            width,
            height,
            frame_rate_n: params.frame_rate_n,
            frame_rate_d: params.frame_rate_d,
            aspect_ratio: params.aspect_ratio,
            flags: VideoFlags(flags),
            color_space: params.color_space,
        };
        let peers = self.shared.snapshot();
        let (mut full, mut preview) = (None, None);
        let mut n = 0;
        for p in peers {
            let wants = {
                let s = p.state.lock().unwrap();
                s.video.then_some(s.preview)
            };
            let Some(wants_preview) = wants else { continue };
            let slot = if wants_preview {
                &mut preview
            } else {
                &mut full
            };
            let bytes = slot
                .get_or_insert_with(|| {
                    let mut h = header;
                    if wants_preview {
                        h.flags = VideoFlags(flags | VideoFlags::PREVIEW);
                    }
                    let mut out = Vec::new();
                    frame::write(
                        timestamp,
                        &ExtendedHeader::Video(h),
                        frame.data,
                        metadata,
                        &mut out,
                    );
                    Arc::new(out)
                })
                .clone();
            n += self.shared.queue(&p, bytes, Kind::Video) as usize;
        }
        Ok(n)
    }

    /// Sends planar 32-bit float audio: `samples` holds `channels` planes of
    /// `samples.len() / channels` samples each. Returns the number of
    /// connections it was queued for.
    ///
    /// Refused, as libomtnet refuses it (`OMTSend.cs:800-806`), when there
    /// are no samples, `sample_rate` is not positive, `channels` is not
    /// 1..=32 or the data exceeds [`MAX_AUDIO_DATA_LEN`]; also when
    /// `channels` does not divide `samples.len()`.
    pub fn send_audio(
        &self,
        samples: &[f32],
        channels: usize,
        sample_rate: i32,
        timestamp: i64,
        metadata: &[u8],
    ) -> Result<usize, SendError> {
        if !(1..=32).contains(&channels) {
            return Err(SendError::InvalidChannels(channels));
        }
        if samples.is_empty() || sample_rate <= 0 {
            return Err(SendError::EmptyAudio);
        }
        if samples.len() % channels != 0 {
            return Err(SendError::SamplesNotMultipleOfChannels {
                samples: samples.len(),
                channels,
            });
        }
        if samples.len() * 4 > MAX_AUDIO_DATA_LEN {
            return Err(SendError::AudioTooLarge(samples.len() * 4));
        }
        check_metadata(metadata)?;
        let spc = samples.len() / channels;
        let mut active = 0u32;
        let mut data = Vec::with_capacity(samples.len() * 4);
        for (ch, plane) in samples.chunks_exact(spc).enumerate() {
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
                n += self.shared.queue(&p, out.clone(), Kind::Audio) as usize;
            }
        }
        Ok(n)
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
        // First, so its thread never holds the last reference to `Shared`.
        let watcher = self.shared.redirect.lock().unwrap().watcher.take();
        drop(watcher);
        if let (Some(d), Some(full)) = (&self.discovery, &self.full_name) {
            let _ = d.withdraw(full);
        }
        self.shared.closing.store(true, Ordering::SeqCst);
        // Sockets first: a writer blocked on a stalled receiver fails at once.
        for p in self.shared.snapshot() {
            p.close();
        }
        // Wake the blocking accept.
        let _ = TcpStream::connect_timeout(
            &SocketAddr::from(([127, 0, 0, 1], self.port)),
            Duration::from_millis(200),
        );
        let deadline = Instant::now() + DROP_TIMEOUT;
        if let Some(h) = self.accept.take() {
            join_bounded(h, deadline);
        }
        let peers: Vec<Arc<Peer>> = std::mem::take(&mut *self.shared.peers.lock().unwrap());
        for p in &peers {
            p.close();
        }
        for p in peers {
            p.join(deadline);
        }
    }
}

/// Joins `h` if it finishes by `deadline`; otherwise leaves it running
/// detached. Returns whether it was joined.
pub(crate) fn join_bounded(h: JoinHandle<()>, deadline: Instant) -> bool {
    if h.thread().id() == std::thread::current().id() {
        return false;
    }
    while !h.is_finished() {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ = h.join();
    true
}

fn check_metadata(metadata: &[u8]) -> Result<(), SendError> {
    if metadata.len() > u16::MAX as usize {
        return Err(SendError::MetadataTooLarge(metadata.len()));
    }
    Ok(())
}

/// Redirect state, as in libomtnet's `OMTRedirect` for a sender.
#[derive(Default)]
struct RedirectState {
    /// Set by the application.
    address: Option<String>,
    /// What the target itself redirects to (`redirectAddressUpstream`).
    upstream: Option<String>,
    /// Metadata-only connection to `address`.
    watcher: Option<Watcher>,
    /// Tells a replaced watcher's late callbacks apart.
    generation: u64,
}

impl RedirectState {
    /// `OMTRedirect.cs:40-49`: the upstream address if there is one.
    fn effective(&self) -> String {
        match &self.upstream {
            Some(u) if !u.is_empty() => u.clone(),
            _ => self.address.clone().unwrap_or_default(),
        }
    }

    fn xml(&self) -> String {
        redirect::to_xml(&self.effective())
    }
}

/// The redirect target announced a redirect of its own (`OMTRedirect.cs:128-163`).
fn upstream_heard(shared: &Weak<Shared>, generation: u64, address: String) {
    let Some(shared) = shared.upgrade() else {
        return;
    };
    let xml = {
        let mut r = shared.redirect.lock().unwrap();
        if r.generation != generation
            || shared.self_names[0] == address
            || r.address.as_ref() == Some(&address)
            || r.upstream.as_ref() == Some(&address)
        {
            return;
        }
        r.upstream = Some(address);
        r.xml()
    };
    let mut out = Vec::new();
    frame::write_metadata(0, xml.as_bytes(), &mut out);
    shared.broadcast_metadata(Arc::new(out));
}

struct Shared {
    redirect: Mutex<RedirectState>,
    /// Our full name and URL (X4).
    self_names: Vec<String>,
    peers: Mutex<Vec<Arc<Peer>>>,
    on_connect: Vec<Vec<u8>>,
    quality: Quality,
    tally: Mutex<Tally>,
    metadata_tx: mpsc::SyncSender<(SocketAddr, Vec<u8>)>,
    closing: AtomicBool,
    /// Queued, sent and dropped frames, by [`Kind`].
    counts: [[AtomicU64; 3]; 3],
    bytes_sent: AtomicU64,
}

const QUEUED: usize = 0;
const SENT: usize = 1;
const DROPPED: usize = 2;

/// What a queued frame is, for the queue limits and the counters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Video = 0,
    Audio = 1,
    Metadata = 2,
}

impl Shared {
    fn snapshot(&self) -> Vec<Arc<Peer>> {
        self.peers.lock().unwrap().clone()
    }

    fn queue(&self, peer: &Peer, bytes: Arc<Vec<u8>>, kind: Kind) -> bool {
        let ok = bytes.len() <= MAX_FRAME_LEN && peer.outbox.push(bytes, kind);
        let which = if ok { QUEUED } else { DROPPED };
        self.counts[kind as usize][which].fetch_add(1, Ordering::Relaxed);
        if !ok {
            peer.frames_dropped.fetch_add(1, Ordering::Relaxed);
        }
        ok
    }

    fn sent(&self, peer: &Peer, len: usize, kind: Kind) {
        self.counts[kind as usize][SENT].fetch_add(1, Ordering::Relaxed);
        self.bytes_sent.fetch_add(len as u64, Ordering::Relaxed);
        peer.frames_sent.fetch_add(1, Ordering::Relaxed);
        peer.bytes_sent.fetch_add(len as u64, Ordering::Relaxed);
    }

    fn broadcast_metadata(&self, bytes: Arc<Vec<u8>>) -> usize {
        let mut n = 0;
        for p in self.snapshot() {
            if p.state.lock().unwrap().metadata {
                n += self.queue(&p, bytes.clone(), Kind::Metadata) as usize;
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
    bytes_sent: AtomicU64,
    frames_sent: AtomicU64,
    frames_dropped: AtomicU64,
}

impl Peer {
    fn close(&self) {
        self.outbox.close();
        let _ = self.stream.shutdown(Shutdown::Both);
    }

    fn join(&self, deadline: Instant) {
        let handles = std::mem::take(&mut *self.threads.lock().unwrap());
        for h in handles {
            join_bounded(h, deadline);
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
    items: VecDeque<(Arc<Vec<u8>>, Kind)>,
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

    fn push(&self, bytes: Arc<Vec<u8>>, kind: Kind) -> bool {
        let mut q = self.q.lock().unwrap();
        if q.closed {
            return false;
        }
        let (count, max) = if kind == Kind::Metadata {
            (&mut q.metadata, MAX_QUEUED_METADATA)
        } else {
            (&mut q.av, MAX_QUEUED_AV)
        };
        if *count >= max {
            return false;
        }
        *count += 1;
        q.items.push_back((bytes, kind));
        self.ready.notify_one();
        true
    }

    /// Blocks until there is something to write; `None` once closed.
    fn pop(&self) -> Option<(Arc<Vec<u8>>, Kind)> {
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
    fn done(&self, kind: Kind) {
        let mut q = self.q.lock().unwrap();
        if kind == Kind::Metadata {
            q.metadata -= 1;
        } else {
            q.av -= 1;
        }
    }

    fn close(&self) {
        self.q.lock().unwrap().closed = true;
        self.ready.notify_all();
    }

    /// Items waiting to be written.
    fn len(&self) -> usize {
        self.q.lock().unwrap().items.len()
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
        bytes_sent: AtomicU64::new(0),
        frames_sent: AtomicU64::new(0),
        frames_dropped: AtomicU64::new(0),
    });

    // §4.2: sender info, connection metadata, then the combined tally, sent
    // whatever the connection later subscribes to (M6).
    for xml in &shared.on_connect {
        let mut out = Vec::new();
        frame::write_metadata(0, xml, &mut out);
        shared.queue(&peer, Arc::new(out), Kind::Metadata);
    }
    let mut out = Vec::new();
    let tally = *shared.tally.lock().unwrap();
    frame::write_metadata(0, Command::Tally(tally).as_bytes(), &mut out);
    shared.queue(&peer, Arc::new(out), Kind::Metadata);
    // Then the redirect, if active (`OMTSend.cs:372-375`, X1).
    let redirect = {
        let r = shared.redirect.lock().unwrap();
        r.address.is_some().then(|| r.xml())
    };
    if let Some(xml) = redirect {
        let mut out = Vec::new();
        frame::write_metadata(0, xml.as_bytes(), &mut out);
        shared.queue(&peer, Arc::new(out), Kind::Metadata);
    }

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
    while let Some((bytes, kind)) = peer.outbox.pop() {
        let result = stream.write_all(&bytes);
        peer.outbox.done(kind);
        if result.is_err() {
            break;
        }
        shared.sent(&peer, bytes.len(), kind);
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
    use crate::OwnedFrame;
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
        // The audio connection counts as connected before its subscription is
        // processed; until then nothing is sent (seen on a CI runner).
        wait_for(|| tx.send_audio(&audio, 2, 48000, 7, b"") == Ok(1));

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
        // No more than the outbox holds: on a slow machine the writer may not
        // have taken any frame yet, and a fifth would be dropped by design.
        let n = MAX_QUEUED_AV as i64;
        for ts in 1..n {
            assert_eq!(tx.send_video(&frame, params, ts, b""), Ok(1));
        }
        let mut sizes = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        while sizes.len() < MAX_QUEUED_AV && Instant::now() < deadline {
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
        assert_eq!(sizes.len(), MAX_QUEUED_AV);
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
            assert!(o.push(Arc::new(vec![]), Kind::Video));
        }
        assert!(!o.push(Arc::new(vec![]), Kind::Audio));
        assert!(o.push(Arc::new(vec![]), Kind::Metadata));
        assert_eq!(o.len(), MAX_QUEUED_AV + 1);
        let (_, m) = o.pop().unwrap();
        o.done(m);
        assert!(o.push(Arc::new(vec![]), Kind::Video));
    }

    fn pattern(w: usize, h: usize) -> Frame {
        let mut frame = Frame::new(w, h, PixelFormat::Uyvy);
        for (i, b) in frame.planes[0].data.iter_mut().enumerate() {
            *b = (i * 7 % 251) as u8;
        }
        frame
    }

    const PARAMS: VideoParams = VideoParams {
        frame_rate_n: 30,
        frame_rate_d: 1,
        aspect_ratio: 2.0,
        color_space: 709,
        premultiplied: false,
    };

    /// The first video frame `rx` delivers.
    fn next_video(rx: &Receiver) -> (VideoHeader, OwnedFrame) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if let Some(Event::Frame(_, f)) = rx.recv_timeout(Duration::from_millis(100)) {
                if let ExtendedHeader::Video(v) = f.ext {
                    return (v, f);
                }
            }
        }
        panic!("no video frame");
    }

    #[test]
    fn encoded_frames_are_forwarded_untouched() {
        // V2, P4: a VMX1 frame goes out as given; a preview connection gets
        // the whole of it with flag 8 (`OMTSend.cs:766-786,772`).
        let tx = Sender::new(quiet("encoded")).unwrap();
        let addr = SocketAddr::from(([127, 0, 0, 1], tx.port()));
        let full_cfg = ReceiverConfig {
            audio: false,
            ..ReceiverConfig::default()
        };
        let preview_cfg = ReceiverConfig {
            preview: true,
            ..full_cfg
        };
        let full_rx = Receiver::connect(addr, full_cfg).unwrap();
        let preview_rx = Receiver::connect(addr, preview_cfg).unwrap();
        wait_for(|| tx.video_receivers() == 2);

        let frame = pattern(64, 32);
        let mut cfg = EncoderConfig::new(64, 32);
        cfg.profile = Profile::OmtLq; // not what the sender would pick (V3)
        let bits = Encoder::new(cfg).unwrap().encode(&frame).unwrap();
        let encoded = EncodedVideo {
            data: &bits,
            width: 64,
            height: 32,
            flags: VideoFlags(VideoFlags::INTERLACED | VideoFlags::PREVIEW),
        };
        let params = VideoParams {
            premultiplied: true, // no alpha: ignored
            ..PARAMS
        };
        assert_eq!(tx.send_encoded_video(encoded, params, 42, b"<m/>\0"), Ok(2));

        let (v, f) = next_video(&full_rx);
        assert_eq!(f.data, bits, "bitstream untouched");
        assert_eq!(f.metadata, b"<m/>\0");
        assert_eq!(f.header.timestamp, 42);
        assert_eq!(
            (v.codec, v.width, v.height, v.frame_rate_n, v.color_space),
            (CODEC_VMX1, 64, 32, 30, 709)
        );
        assert_eq!(v.flags, VideoFlags(VideoFlags::INTERLACED));
        let (pv, pf) = next_video(&preview_rx);
        assert_eq!(
            pf.data, bits,
            "preview connections get the whole frame (P4)"
        );
        assert_eq!(
            pv.flags,
            VideoFlags(VideoFlags::INTERLACED | VideoFlags::PREVIEW)
        );
        assert!(Decoder::new(64, 32)
            .unwrap()
            .decode_preview(&pf.data, false)
            .is_ok());
        assert_eq!(tx.stats().video.queued, 2);
    }

    #[test]
    fn bad_input_is_an_error_not_a_panic() {
        let tx = Sender::new(quiet("errors")).unwrap();
        let one = [0.5f32; 4];
        assert_eq!(
            tx.send_audio(&one, 0, 48000, 0, b""),
            Err(SendError::InvalidChannels(0))
        );
        assert_eq!(
            tx.send_audio(&one, 33, 48000, 0, b""),
            Err(SendError::InvalidChannels(33))
        );
        assert_eq!(
            tx.send_audio(&one, 3, 48000, 0, b""),
            Err(SendError::SamplesNotMultipleOfChannels {
                samples: 4,
                channels: 3
            })
        );
        assert_eq!(
            tx.send_audio(&[], 2, 48000, 0, b""),
            Err(SendError::EmptyAudio)
        );
        assert_eq!(
            tx.send_audio(&one, 2, 0, 0, b""),
            Err(SendError::EmptyAudio)
        );
        let big = vec![0.0f32; MAX_AUDIO_DATA_LEN / 4 + 2];
        assert_eq!(
            tx.send_audio(&big, 2, 48000, 0, b""),
            Err(SendError::AudioTooLarge(big.len() * 4))
        );
        let meta = vec![b'x'; 65536];
        assert_eq!(
            tx.send_audio(&one, 2, 48000, 0, &meta),
            Err(SendError::MetadataTooLarge(65536))
        );
        assert_eq!(tx.send_audio(&one, 2, 48000, 0, b""), Ok(0));
        assert_eq!(
            tx.send_video(&pattern(64, 32), PARAMS, 0, &meta),
            Err(SendError::MetadataTooLarge(65536))
        );
        assert!(matches!(
            tx.send_video(&Frame::new(8, 8, PixelFormat::Uyvy), PARAMS, 0, b""),
            Err(SendError::Codec(_))
        ));
        let encoded = |data, width, height| EncodedVideo {
            data,
            width,
            height,
            flags: VideoFlags::default(),
        };
        assert_eq!(
            tx.send_encoded_video(encoded(&[], 64, 32), PARAMS, 0, b""),
            Err(SendError::EmptyVideo)
        );
        assert_eq!(
            tx.send_encoded_video(encoded(&[1], 0, 32), PARAMS, 0, b""),
            Err(SendError::InvalidDimensions {
                width: 0,
                height: 32
            })
        );
        assert_eq!(
            tx.send_encoded_video(encoded(&[1], 64, 1 << 40), PARAMS, 0, b""),
            Err(SendError::InvalidDimensions {
                width: 64,
                height: 1 << 40
            })
        );
        assert!(SendError::EmptyAudio.to_string().contains("audio"));
    }

    #[test]
    fn encoder_threads_do_not_change_the_bitstream() {
        let tx = Sender::new(SenderConfig {
            encoder_threads: 4,
            ..quiet("threads")
        })
        .unwrap();
        let addr = SocketAddr::from(([127, 0, 0, 1], tx.port()));
        let cfg = ReceiverConfig {
            audio: false,
            ..ReceiverConfig::default()
        };
        let rx = Receiver::connect(addr, cfg).unwrap();
        wait_for(|| tx.video_receivers() == 1);
        let frame = pattern(256, 128);
        assert_eq!(tx.send_video(&frame, PARAMS, 0, b""), Ok(1));
        let mut cfg = EncoderConfig::new(256, 128);
        cfg.profile = Profile::OmtSq;
        let expected = Encoder::new(cfg).unwrap().encode(&frame).unwrap();
        assert_eq!(next_video(&rx).1.data, expected);
    }

    #[test]
    fn stats_count_frames_bytes_and_connections() {
        let tx = Sender::new(quiet("stats")).unwrap();
        let addr = SocketAddr::from(([127, 0, 0, 1], tx.port()));
        let rx = Receiver::connect(addr, ReceiverConfig::default()).unwrap();
        wait_for(|| tx.connections() == 2 && tx.video_receivers() == 1);
        wait_for(|| tx.send_audio(&[0.25; 200], 2, 48000, 0, b"") == Ok(1));
        assert_eq!(tx.send_video(&pattern(64, 32), PARAMS, 0, b""), Ok(1));
        next_video(&rx);
        wait_for(|| {
            let s = tx.stats();
            s.video.sent == 1 && s.audio.sent == 1
        });
        let s = tx.stats();
        assert_eq!(s.connections, 2);
        assert_eq!((s.video.queued, s.video.dropped), (1, 0));
        // Sender info and a tally on each connection, at least.
        assert!(s.metadata.sent >= 4, "{s:?}");
        assert_eq!(
            s.frames_queued,
            s.video.queued + s.audio.queued + s.metadata.queued
        );
        let peers = tx.peer_stats();
        assert_eq!(peers.len(), 2);
        assert_eq!(
            peers.iter().map(|p| p.bytes_sent).sum::<u64>(),
            s.bytes_sent
        );
        assert_eq!(peers.iter().filter(|p| p.video).count(), 1);
        assert_eq!(peers.iter().filter(|p| p.audio).count(), 1);
        let video_peer = peers.iter().find(|p| p.video).unwrap();
        assert!(video_peer.metadata && !video_peer.preview);

        // The receiver's side of the same traffic.
        wait_for(|| {
            let r = rx.stats();
            r.video.bytes + r.audio.bytes == s.bytes_sent
        });
        let r = rx.stats();
        assert!(r.video.frames >= 3 && r.audio.frames >= 3, "{r:?}");
        assert_eq!((r.reconnects, r.redirects), (0, 0));
    }

    #[test]
    fn drop_is_bounded_with_a_stalled_receiver() {
        // A receiver that subscribes and never reads: the writer ends up
        // blocked in `write_all`, and the queue overflows.
        let tx = Sender::new(quiet("stalled")).unwrap();
        let mut stalled = TcpStream::connect(("127.0.0.1", tx.port())).unwrap();
        let mut out = Vec::new();
        for c in [Command::SubscribeMetadata, Command::SubscribeVideo] {
            frame::write_metadata(0, c.as_bytes(), &mut out);
        }
        stalled.write_all(&out).unwrap();
        wait_for(|| tx.video_receivers() == 1);
        let mut frame = Frame::new(1280, 720, PixelFormat::Uyvy);
        for (i, b) in frame.planes[0].data.iter_mut().enumerate() {
            *b = (i.wrapping_mul(2_654_435_761) >> 13) as u8; // noise: large frames
        }
        wait_for(|| {
            let _ = tx.send_video(&frame, PARAMS, 0, b"");
            tx.peer_stats()[0].frames_dropped > 0
        });
        let peer = &tx.peer_stats()[0];
        // A full queue, less the frame being written.
        assert!(peer.queued >= MAX_QUEUED_AV - 1, "{peer:?}");
        assert!(tx.stats().video.dropped > 0);

        let start = Instant::now();
        drop(tx);
        assert!(
            start.elapsed() < DROP_TIMEOUT + Duration::from_secs(1),
            "drop took {:?}",
            start.elapsed()
        );
        drop(stalled);
    }

    /// Needs working multicast; run with `--ignored` and watch with
    /// `dns-sd -B _omt._tcp`: both names come from one responder.
    #[test]
    #[ignore = "uses the network's mDNS"]
    fn senders_share_one_mdns_responder() {
        let shared = Arc::new(Discovery::new().unwrap());
        let config = |name: &str| SenderConfig {
            announce: true,
            discovery: Some(shared.clone()),
            ..quiet(name)
        };
        let a = Sender::new(config("m12-prereqs-mdns-a")).unwrap();
        let b = Sender::new(config("m12-prereqs-mdns-b")).unwrap();
        let browser = shared.browse().unwrap();
        let mut seen = std::collections::HashSet::new();
        let deadline = Instant::now() + Duration::from_secs(8);
        while seen.len() < 2 && Instant::now() < deadline {
            if let Some(crate::discovery::SourceEvent::Resolved(s)) =
                browser.recv_timeout(Duration::from_millis(200))
            {
                if [a.full_name(), b.full_name()].contains(&Some(s.full_name.as_str())) {
                    seen.insert(s.full_name);
                }
            }
        }
        assert_eq!(seen.len(), 2, "{seen:?}");
        std::thread::sleep(Duration::from_secs(3));
        drop(a);
        std::thread::sleep(Duration::from_secs(3));
    }

    #[test]
    fn senders_share_one_discovery() {
        use crate::address::Directory;
        use crate::discovery_server::Server;
        let server = Server::bind(0).unwrap();
        let url = format!("omt://127.0.0.1:{}", server.port());
        let shared = Arc::new(Discovery::with_server(&url, false).unwrap());
        let config = |name: &str| SenderConfig {
            announce: true,
            discovery: Some(shared.clone()),
            discovery_server: Some("omt://unused.invalid".into()), // the shared one wins
            ..quiet(name)
        };
        let a = Sender::new(config("m12-prereqs-share-a")).unwrap();
        let b = Sender::new(config("m12-prereqs-share-b")).unwrap();
        let dir = Directory::with_shared(shared.clone()).unwrap();
        let (fa, fb) = (a.full_name().unwrap(), b.full_name().unwrap());
        assert_eq!(
            dir.wait_for(fa, Duration::from_secs(3)).unwrap().port,
            a.port()
        );
        assert_eq!(
            dir.wait_for(fb, Duration::from_secs(3)).unwrap().port,
            b.port()
        );
        assert_eq!(server.connections(), 1, "one client for all of them");
        let fa = fa.to_owned();
        drop(a);
        wait_for(|| dir.get(&fa).is_none());
        assert!(dir.get(b.full_name().unwrap()).is_some());
        assert_eq!(server.entries().len(), 1);
    }
}
