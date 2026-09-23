//! Turns received frames into pictures, sound and XML (`docs/PROTOCOL.md` §6).
//!
//! The [`receiver`](crate::receiver) hands back frames as they came off the
//! wire. [`MediaDecoder`] does what libomtnet's `OMTReceive.Receive` does with
//! them next:
//!
//! - **Video** (VMX1, V1) is decoded with `vmx-codec` into the layout picked
//!   by a [`PreferredVideoFormat`], with libomtnet's rules for alpha and high
//!   bit depth (V6, `OMTReceive.cs:797-887`). Preview frames (§6.2) are
//!   decoded at 1/8 scale (P3).
//! - **Audio** (FPA1, §6.3) becomes 32-bit float planar samples, with the
//!   channels the sender left out re-inserted as silence (A3).
//! - **Metadata** frames are returned as XML, except the protocol messages the
//!   connection consumes (commands, quality settings, redirects; M3).
//!
//! Every output carries what the headers said: timestamp, frame rate, aspect
//! ratio, colour space, flags (§3.3, §3.4), and any per-frame metadata (§3.2).
//!
//! Decoding runs on whichever thread calls it. [`Receiver`] events already
//! arrive on the caller's thread, not the socket reader's; to decode on yet
//! another thread, move the [`OwnedFrame`]s there (a `MediaDecoder` is
//! `Send`). Output buffers are reused: [`MediaDecoder::decode`] fills buffers
//! it owns and lends them out until the next call, and
//! [`MediaDecoder::decode_video`] / [`decode_audio`] fill buffers you own, so
//! a steady stream allocates nothing large per frame.
//!
//! ```
//! use std::net::SocketAddr;
//! use std::time::Duration;
//! use open_media_transport::media::{Media, MediaDecoder, PreferredVideoFormat, VideoFormat};
//! use open_media_transport::receiver::{Event, Receiver, ReceiverConfig};
//! use open_media_transport::sender::{Sender, SenderConfig, VideoParams};
//! use vmx_codec::{Frame, PixelFormat};
//!
//! // A sender on this machine, not announced on the network.
//! let mut config = SenderConfig::new("doc");
//! config.announce = false;
//! let tx = Sender::new(config)?;
//! let addr = SocketAddr::from(([127, 0, 0, 1], tx.port()));
//! let rx = Receiver::connect(addr, ReceiverConfig { audio: false, ..Default::default() })?;
//!
//! let params = VideoParams {
//!     frame_rate_n: 30,
//!     frame_rate_d: 1,
//!     aspect_ratio: 16.0 / 9.0,
//!     color_space: 709,
//!     premultiplied: false,
//! };
//! let picture = Frame::new(64, 32, PixelFormat::Uyvy);
//! while tx.video_receivers() == 0 {
//!     std::thread::sleep(Duration::from_millis(10));
//! }
//! tx.send_video(&picture, params, 0, b"<Hello />\0").unwrap();
//!
//! let mut decoder = MediaDecoder::new(PreferredVideoFormat::UyvyOrBgra);
//! while let Some(event) = rx.recv_timeout(Duration::from_secs(5)) {
//!     let Event::Frame(_, frame) = event else { continue }; // Connected, Closed
//!     if let Some(Media::Video(v)) = decoder.decode(&frame)? {
//!         assert_eq!((v.width, v.height, v.format), (64, 32, VideoFormat::Uyvy));
//!         assert_eq!(v.data.len(), 64 * 2 * 32);
//!         assert_eq!(v.frame_rate(), 30.0);
//!         assert_eq!(v.metadata, b"<Hello />\0");
//!         return Ok(());
//!     }
//! }
//! panic!("no video frame");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! [`Receiver`]: crate::receiver::Receiver

use std::borrow::Cow;
use std::fmt;

use vmx_codec::{Decoder, Frame, PixelFormat, Plane};

use crate::command::{classify, Message};
use crate::frame::{fourcc, ExtendedHeader, VideoFlags, VideoHeader, CODEC_FPA1, CODEC_VMX1};
use crate::OwnedFrame;

/// Largest decoded audio frame libomtnet accepts: `SamplesPerChannel ×
/// Channels × 4` (A4, `OMTConstants.cs:62`, `OMTReceive.cs:1082-1083`).
pub const MAX_AUDIO_BYTES: usize = 1_048_576;

/// Layout of a decoded [`VideoFrame`], named after libomtnet's `OMTCodec`
/// values (`OMTPublicTypes.cs:75-105`). All rows of a plane are `stride`
/// bytes and planes follow each other without gaps, exactly as libomtnet
/// delivers them (`OMTReceive.cs:913-921`).
///
/// | format | planes                                                    | stride |
/// |--------|-----------------------------------------------------------|--------|
/// | `Uyvy` | `U Y V Y` packed, 8-bit                                   | `2w`   |
/// | `Uyva` | UYVY, then an 8-bit alpha plane of `w × h`                | `2w`   |
/// | `Bgra` | `B G R A` packed, 8-bit; A is 255 when the source has no alpha | `4w` |
/// | `P216` | 16-bit Y plane, then interleaved 16-bit U/V, little-endian, 10 significant bits in the high bits | `2w` |
/// | `Pa16` | `P216`, then a 16-bit alpha plane                         | `2w`   |
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VideoFormat {
    /// 8-bit 4:2:2.
    #[default]
    Uyvy,
    /// 8-bit 4:2:2 plus alpha.
    Uyva,
    /// 8-bit RGB plus alpha.
    Bgra,
    /// 10-bit 4:2:2 in 16-bit words.
    P216,
    /// 10-bit 4:2:2 plus alpha in 16-bit words.
    Pa16,
}

impl VideoFormat {
    /// libomtnet's FourCC for this layout (`OMTPublicTypes.cs:98-105`).
    pub fn fourcc(self) -> u32 {
        match self {
            VideoFormat::Uyvy => fourcc(b"UYVY"),
            VideoFormat::Uyva => fourcc(b"UYVA"),
            VideoFormat::Bgra => fourcc(b"BGRA"),
            VideoFormat::P216 => fourcc(b"P216"),
            VideoFormat::Pa16 => fourcc(b"PA16"),
        }
    }

    /// Samples are 16-bit words carrying 10 significant bits.
    pub fn is_10bit(self) -> bool {
        matches!(self, VideoFormat::P216 | VideoFormat::Pa16)
    }

    /// Bytes of each plane for a `width` × `height` picture, in order.
    pub fn plane_sizes(self, width: usize, height: usize) -> Vec<usize> {
        let (w, h) = (width, height);
        match self {
            VideoFormat::Uyvy => vec![2 * w * h],
            VideoFormat::Uyva => vec![2 * w * h, w * h],
            VideoFormat::Bgra => vec![4 * w * h],
            VideoFormat::P216 => vec![2 * w * h, 2 * w * h],
            VideoFormat::Pa16 => vec![2 * w * h, 2 * w * h, 2 * w * h],
        }
    }

    /// Bytes per row of the first plane (`OMTReceive.cs:803-887`).
    pub fn stride(self, width: usize) -> usize {
        match self {
            VideoFormat::Bgra => 4 * width,
            _ => 2 * width,
        }
    }
}

/// Which layout to decode video into, with libomtnet's names and rules
/// (`OMTPreferredVideoFormat`, `OMTPublicTypes.cs:131-154`). What each
/// preference yields, by the frame's flags (`OMTReceive.cs:797-887`):
///
/// | preference               | opaque        | alpha        | 10-bit        | 10-bit + alpha | preview (opaque / alpha) |
/// |--------------------------|---------------|--------------|---------------|----------------|--------------------------|
/// | `Uyvy`                   | UYVY          | UYVY         | UYVY          | UYVY           | UYVY / UYVY              |
/// | `UyvyOrBgra`             | UYVY          | BGRA         | UYVY          | BGRA           | UYVY / BGRA              |
/// | `Bgra`                   | BGRA (A=255)  | BGRA         | BGRA (A=255)  | BGRA           | BGRA (A=255) / BGRA      |
/// | `UyvyOrUyva`             | UYVY          | UYVA         | UYVY          | UYVA           | UYVY / UYVA              |
/// | `UyvyOrUyvaOrP216OrPa16` | UYVY          | UYVA         | P216          | PA16           | UYVY / UYVA              |
/// | `P216`                   | P216          | P216         | P216          | P216           | none                     |
///
/// 10-bit sources decoded to an 8-bit layout go through VMX's 8-bit path,
/// and 8-bit sources decoded to `P216` through its 10-bit path, as libvmx
/// does. "None" is libomtnet's "No matching preferred format found": the
/// frame is dropped ([`DecodeError::NoMatchingFormat`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PreferredVideoFormat {
    /// Always 8-bit UYVY; alpha is discarded. The fastest.
    #[default]
    Uyvy,
    /// UYVY, or BGRA when the frame has alpha.
    UyvyOrBgra,
    /// Always BGRA.
    Bgra,
    /// UYVY, or UYVA when the frame has alpha.
    UyvyOrUyva,
    /// As `UyvyOrUyva`, but P216 / PA16 for frames the sender encoded from a
    /// high-bit-depth source.
    UyvyOrUyvaOrP216OrPa16,
    /// Always P216 (full frames only).
    P216,
}

impl PreferredVideoFormat {
    /// The layout libomtnet picks for a frame with these flags, or `None`
    /// when it decodes nothing (`OMTReceive.cs:797-887`).
    pub fn select(self, flags: VideoFlags) -> Option<VideoFormat> {
        use PreferredVideoFormat as P;
        let alpha = flags.contains(VideoFlags::ALPHA);
        let high = flags.contains(VideoFlags::HIGH_BIT_DEPTH);
        let preview = flags.contains(VideoFlags::PREVIEW);
        Some(match self {
            P::Uyvy => VideoFormat::Uyvy,
            P::UyvyOrBgra | P::UyvyOrUyva if !alpha => VideoFormat::Uyvy,
            P::UyvyOrBgra | P::Bgra => VideoFormat::Bgra,
            P::UyvyOrUyva => VideoFormat::Uyva,
            P::UyvyOrUyvaOrP216OrPa16 => match (preview, high, alpha) {
                (true, _, false) | (false, false, false) => VideoFormat::Uyvy,
                (true, _, true) | (false, false, true) => VideoFormat::Uyva,
                (false, true, false) => VideoFormat::P216,
                (false, true, true) => VideoFormat::Pa16,
            },
            P::P216 if preview => return None,
            P::P216 => VideoFormat::P216,
        })
    }
}

/// A decoded video frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VideoFrame {
    /// In 100 ns units, as sent (§3.1).
    pub timestamp: i64,
    /// Decoded width: the sender's width, or the preview width for preview
    /// frames (P3).
    pub width: usize,
    /// Decoded height.
    pub height: usize,
    /// Layout of [`VideoFrame::data`].
    pub format: VideoFormat,
    /// Bytes per row of the first plane.
    pub stride: usize,
    /// All planes, back to back (see [`VideoFormat`]).
    pub data: Vec<u8>,
    /// Frame rate numerator (§3.3).
    pub frame_rate_n: i32,
    /// Frame rate denominator.
    pub frame_rate_d: i32,
    /// Display aspect ratio, width / height.
    pub aspect_ratio: f32,
    /// As sent: 0 undefined, 601 or 709.
    pub color_space: i32,
    /// As sent: interlaced, alpha, premultiplied, preview, high bit depth.
    pub flags: VideoFlags,
    /// Per-frame XML, exactly as sent (usually NUL-terminated, M4); empty if none.
    pub metadata: Vec<u8>,
    /// The sender's frame width; differs from `width` for preview frames.
    pub source_width: usize,
    /// The sender's frame height, which also picks the colour matrix when
    /// the colour space is undefined.
    pub source_height: usize,
}

impl VideoFrame {
    /// Frames per second, or 0 if the denominator is not positive.
    pub fn frame_rate(&self) -> f64 {
        if self.frame_rate_d > 0 {
            self.frame_rate_n as f64 / self.frame_rate_d as f64
        } else {
            0.0
        }
    }

    /// The planes of [`VideoFrame::data`], in the order of [`VideoFormat`].
    pub fn planes(&self) -> Vec<&[u8]> {
        let mut rest = self.data.as_slice();
        self.format
            .plane_sizes(self.width, self.height)
            .into_iter()
            .map(|n| {
                let (p, r) = rest.split_at(n.min(rest.len()));
                rest = r;
                p
            })
            .collect()
    }

    /// Whether the YUV↔RGB matrix is BT.601 rather than BT.709. As libvmx
    /// decides for BGRA (`vmxcodec.cpp:262-268,689-691`): 601 when the
    /// sender says so, or says nothing and the full frame is under 720 lines;
    /// 709 otherwise.
    pub fn is_bt601(&self) -> bool {
        bt601(self.color_space, self.source_height)
    }

    /// Two fields in one frame.
    pub fn is_interlaced(&self) -> bool {
        self.flags.contains(VideoFlags::INTERLACED)
    }

    /// The source has an alpha channel. It survives decoding only into UYVA,
    /// BGRA or PA16.
    pub fn has_alpha(&self) -> bool {
        self.flags.contains(VideoFlags::ALPHA)
    }

    /// A 1/8-scale preview frame (§6.2).
    pub fn is_preview(&self) -> bool {
        self.flags.contains(VideoFlags::PREVIEW)
    }

    /// The sender encoded it from a 10-bit (P216 / PA16) source.
    pub fn is_high_bit_depth(&self) -> bool {
        self.flags.contains(VideoFlags::HIGH_BIT_DEPTH)
    }
}

/// A decoded audio frame: 32-bit float, planar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioFrame {
    /// In 100 ns units, as sent.
    pub timestamp: i64,
    /// Samples per second.
    pub sample_rate: i32,
    /// Number of channels, including silent ones.
    pub channels: usize,
    /// Samples in each channel.
    pub samples_per_channel: usize,
    /// `channels` planes of `samples_per_channel` samples each; channels the
    /// sender left out as silent are zeros (A3).
    pub samples: Vec<f32>,
    /// Bit `i` set when channel `i` was sent, clear when it was silent (A2).
    pub active_channels: u32,
    /// Per-frame XML, exactly as sent; empty if none.
    pub metadata: Vec<u8>,
}

impl AudioFrame {
    /// Samples of channel `ch`.
    ///
    /// # Panics
    ///
    /// If `ch >= self.channels`.
    pub fn channel(&self, ch: usize) -> &[f32] {
        assert!(ch < self.channels, "channel {ch} of {}", self.channels);
        let n = self.samples_per_channel;
        &self.samples[ch * n..(ch + 1) * n]
    }
}

/// A metadata frame meant for the application.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetadataFrame {
    /// In 100 ns units, as sent.
    pub timestamp: i64,
    /// The XML exactly as sent, including any trailing NUL (M4).
    pub xml: Vec<u8>,
    /// `<OMTInfo …/>` sender information, which libomtnet both parses and
    /// passes on (`OMTChannel.cs:322-411`).
    pub sender_info: bool,
}

impl MetadataFrame {
    /// The XML as text, without trailing NULs.
    pub fn text(&self) -> Cow<'_, str> {
        let end = self.xml.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
        String::from_utf8_lossy(&self.xml[..end])
    }
}

/// One decoded frame, borrowed from the [`MediaDecoder`] until its next call.
#[derive(Clone, Copy, Debug)]
pub enum Media<'a> {
    /// A picture.
    Video(&'a VideoFrame),
    /// Sound.
    Audio(&'a AudioFrame),
    /// Application metadata or sender information.
    Metadata(&'a MetadataFrame),
}

/// Why a frame could not be decoded. The connection is fine; drop the frame.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum DecodeError {
    /// A codec FourCC other than VMX1 (video) or FPA1 (audio).
    UnsupportedCodec(u32),
    /// The preferred format has no layout for this frame (see
    /// [`PreferredVideoFormat`]).
    NoMatchingFormat,
    /// The header's size is not one VMX can code.
    InvalidSize {
        /// Width from the header.
        width: i32,
        /// Height from the header.
        height: i32,
    },
    /// The VMX data is invalid.
    Video(vmx_codec::Error),
    /// Negative channel or sample count, or more than 32 channels.
    InvalidAudioHeader,
    /// `SamplesPerChannel × Channels × 4` exceeds [`MAX_AUDIO_BYTES`] (A4).
    AudioTooLarge(usize),
    /// Fewer audio bytes than the active channels need.
    AudioTruncated {
        /// Bytes the active channels need.
        needed: usize,
        /// Bytes present.
        got: usize,
    },
    /// Not a video, audio or metadata frame.
    NotMedia,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::UnsupportedCodec(c) => {
                let b = c.to_le_bytes();
                write!(f, "unsupported codec {}", String::from_utf8_lossy(&b))
            }
            DecodeError::NoMatchingFormat => {
                f.write_str("the preferred video format has no layout for this frame")
            }
            DecodeError::InvalidSize { width, height } => {
                write!(f, "invalid video size {width}x{height}")
            }
            DecodeError::Video(e) => write!(f, "VMX1: {e}"),
            DecodeError::InvalidAudioHeader => f.write_str("invalid audio header"),
            DecodeError::AudioTooLarge(n) => {
                write!(f, "audio frame of {n} bytes exceeds {MAX_AUDIO_BYTES}")
            }
            DecodeError::AudioTruncated { needed, got } => {
                write!(
                    f,
                    "audio data has {got} bytes, active channels need {needed}"
                )
            }
            DecodeError::NotMedia => f.write_str("not a media frame"),
        }
    }
}

impl std::error::Error for DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DecodeError::Video(e) => Some(e),
            _ => None,
        }
    }
}

impl From<vmx_codec::Error> for DecodeError {
    fn from(e: vmx_codec::Error) -> Self {
        DecodeError::Video(e)
    }
}

/// Decodes received frames, keeping the codec and buffers between calls.
pub struct MediaDecoder {
    preferred: PreferredVideoFormat,
    threads: usize,
    vmx: Option<(usize, usize, Decoder)>,
    scratch: Option<Frame>,
    video: VideoFrame,
    audio: AudioFrame,
    metadata: MetadataFrame,
}

impl MediaDecoder {
    /// A decoder that delivers video in the `preferred` layout, using one
    /// thread for VMX.
    pub fn new(preferred: PreferredVideoFormat) -> Self {
        MediaDecoder {
            preferred,
            threads: 1,
            vmx: None,
            scratch: None,
            video: VideoFrame::default(),
            audio: AudioFrame::default(),
            metadata: MetadataFrame::default(),
        }
    }

    /// The video layout preference.
    pub fn preferred(&self) -> PreferredVideoFormat {
        self.preferred
    }

    /// Changes the video layout preference from the next frame on.
    pub fn set_preferred(&mut self, preferred: PreferredVideoFormat) {
        self.preferred = preferred;
    }

    /// Worker threads for slice-parallel VMX decoding (default 1).
    pub fn set_threads(&mut self, threads: usize) {
        self.threads = threads.max(1);
        if let Some((_, _, d)) = &mut self.vmx {
            d.set_threads(self.threads);
        }
    }

    /// Decodes any frame into buffers owned by the decoder. Returns
    /// `Ok(None)` for metadata the connection consumes itself (commands,
    /// quality settings, redirects).
    pub fn decode(&mut self, frame: &OwnedFrame) -> Result<Option<Media<'_>>, DecodeError> {
        match frame.ext {
            ExtendedHeader::Video(_) => {
                let mut out = std::mem::take(&mut self.video);
                let r = self.decode_video(frame, &mut out);
                self.video = out;
                r.map(|()| Some(Media::Video(&self.video)))
            }
            ExtendedHeader::Audio(_) => {
                decode_audio(frame, &mut self.audio)?;
                Ok(Some(Media::Audio(&self.audio)))
            }
            ExtendedHeader::None => {
                let sender_info = match classify(&frame.data) {
                    Message::SenderInfo(_) => true,
                    Message::Application(_) => false,
                    _ => return Ok(None),
                };
                let m = &mut self.metadata;
                m.timestamp = frame.header.timestamp;
                m.sender_info = sender_info;
                m.xml.clear();
                m.xml.extend_from_slice(&frame.data);
                Ok(Some(Media::Metadata(&self.metadata)))
            }
        }
    }

    /// Decodes a video frame into `out`, reusing its buffers.
    pub fn decode_video(
        &mut self,
        frame: &OwnedFrame,
        out: &mut VideoFrame,
    ) -> Result<(), DecodeError> {
        let ExtendedHeader::Video(v) = frame.ext else {
            return Err(DecodeError::NotMedia);
        };
        if v.codec != CODEC_VMX1 {
            return Err(DecodeError::UnsupportedCodec(v.codec));
        }
        let format = self
            .preferred
            .select(v.flags)
            .ok_or(DecodeError::NoMatchingFormat)?;
        let (w, h) = match (usize::try_from(v.width), usize::try_from(v.height)) {
            (Ok(w), Ok(h)) => (w, h),
            _ => return Err(invalid_size(&v)),
        };
        if self.vmx.as_ref().map(|d| (d.0, d.1)) != Some((w, h)) {
            let mut d = Decoder::new(w, h).map_err(|_| invalid_size(&v))?;
            d.set_threads(self.threads);
            self.vmx = Some((w, h, d));
        }
        let dec = &mut self.vmx.as_mut().unwrap().2;
        let alpha = v.flags.contains(VideoFlags::ALPHA);
        let table = if bt601(v.color_space, h) {
            &YUV_RGB_601
        } else {
            &YUV_RGB_709
        };

        let (ow, oh) = if v.flags.contains(VideoFlags::PREVIEW) {
            // P3: libvmx's DC-only preview. Decoded as planar and packed here,
            // as VMX_DecodePreview* does (`vmxcodec.cpp:994-1120`).
            let keep_alpha = alpha && format != VideoFormat::Uyvy;
            let pv = dec.decode_preview(&frame.data, keep_alpha)?;
            out.data.clear();
            match format {
                VideoFormat::Uyvy | VideoFormat::Uyva => {
                    pack_uyvy(&pv, &mut out.data);
                    if format == VideoFormat::Uyva {
                        copy_plane(&pv.planes[3], pv.width, pv.height, &mut out.data);
                    }
                }
                VideoFormat::Bgra => to_bgra(&pv, table, &mut out.data),
                VideoFormat::P216 | VideoFormat::Pa16 => unreachable!("no 10-bit preview"),
            }
            (pv.width, pv.height)
        } else {
            let pixel = match format {
                VideoFormat::Uyvy => PixelFormat::Uyvy,
                VideoFormat::Uyva => PixelFormat::Uyva,
                VideoFormat::P216 => PixelFormat::P216,
                VideoFormat::Pa16 => PixelFormat::Pa16,
                VideoFormat::Bgra if alpha => PixelFormat::Yuva422p,
                VideoFormat::Bgra => PixelFormat::Yuv422p,
            };
            if pixel == PixelFormat::Uyvy {
                // One plane: decode straight into the caller's buffer.
                let mut data = std::mem::take(&mut out.data);
                data.clear();
                data.resize(2 * w * h, 0);
                let mut f = Frame {
                    width: w,
                    height: h,
                    format: pixel,
                    interlaced: false,
                    planes: vec![Plane {
                        data,
                        stride: 2 * w,
                    }],
                };
                let r = dec.decode_into(&frame.data, &mut f);
                out.data = f.planes.pop().unwrap().data;
                r?;
            } else {
                let scratch = match &mut self.scratch {
                    Some(f) if (f.width, f.height, f.format) == (w, h, pixel) => f,
                    slot => slot.insert(Frame::new(w, h, pixel)),
                };
                dec.decode_into(&frame.data, scratch)?;
                out.data.clear();
                if format == VideoFormat::Bgra {
                    to_bgra(scratch, table, &mut out.data);
                } else {
                    for p in &scratch.planes {
                        out.data.extend_from_slice(&p.data);
                    }
                }
            }
            (w, h)
        };

        out.timestamp = frame.header.timestamp;
        out.width = ow;
        out.height = oh;
        out.format = format;
        out.stride = format.stride(ow);
        out.frame_rate_n = v.frame_rate_n;
        out.frame_rate_d = v.frame_rate_d;
        out.aspect_ratio = v.aspect_ratio;
        out.color_space = v.color_space;
        out.flags = v.flags;
        out.source_width = w;
        out.source_height = h;
        out.metadata.clear();
        out.metadata.extend_from_slice(&frame.metadata);
        Ok(())
    }
}

fn invalid_size(v: &VideoHeader) -> DecodeError {
    DecodeError::InvalidSize {
        width: v.width,
        height: v.height,
    }
}

/// Decodes an FPA1 audio frame into `out`, reusing its buffers (§6.3).
pub fn decode_audio(frame: &OwnedFrame, out: &mut AudioFrame) -> Result<(), DecodeError> {
    let ExtendedHeader::Audio(a) = frame.ext else {
        return Err(DecodeError::NotMedia);
    };
    if a.codec != CODEC_FPA1 {
        return Err(DecodeError::UnsupportedCodec(a.codec));
    }
    // libomtnet builds channel masks with `1 << i` on an int, which wraps
    // past 32 (`codecs/OMTFPA1Codec.cs:45`); more than 32 channels is refused here.
    let (Ok(channels), Ok(spc)) = (
        usize::try_from(a.channels),
        usize::try_from(a.samples_per_channel),
    ) else {
        return Err(DecodeError::InvalidAudioHeader);
    };
    if channels > 32 {
        return Err(DecodeError::InvalidAudioHeader);
    }
    let bytes = channels * spc * 4;
    if bytes > MAX_AUDIO_BYTES {
        return Err(DecodeError::AudioTooLarge(bytes));
    }
    let active = (0..channels)
        .filter(|&ch| a.active_channels & (1 << ch) != 0)
        .count();
    let needed = active * spc * 4;
    if frame.data.len() < needed {
        return Err(DecodeError::AudioTruncated {
            needed,
            got: frame.data.len(),
        });
    }
    out.samples.clear();
    out.samples.reserve(channels * spc);
    // A1, A3: present channels in order, silent ones as zeros
    // (`codecs/OMTFPA1Codec.cs:39-59`).
    let mut present = frame.data.chunks_exact((spc * 4).max(1));
    for ch in 0..channels {
        if a.active_channels & (1 << ch) != 0 {
            let plane = present.next().unwrap_or(&[]);
            out.samples.extend(
                plane
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            );
        } else {
            out.samples.resize(out.samples.len() + spc, 0.0);
        }
    }
    out.timestamp = frame.header.timestamp;
    out.sample_rate = a.sample_rate;
    out.channels = channels;
    out.samples_per_channel = spc;
    out.active_channels = a.active_channels;
    out.metadata.clear();
    out.metadata.extend_from_slice(&frame.metadata);
    Ok(())
}

fn bt601(color_space: i32, height: usize) -> bool {
    color_space == 601 || (color_space == 0 && height < 720)
}

/// libvmx's YUV→RGB constants (`vmxcodec_common.h:194-246`): Y, R from V,
/// G from U, G from V, B from U.
const YUV_RGB_709: [i16; 5] = [19077, 29372, 3494, 8731, 17305];
const YUV_RGB_601: [i16; 5] = [19077, 26149, 6419, 13320, 16525];

/// `_mm_mulhi_epi16`: the high half of a signed 16×16 product.
#[inline]
fn mulhi(a: i16, b: i16) -> i16 {
    ((a as i32 * b as i32) >> 16) as i16
}

/// One pixel of `VMX_YUV4224ToBGRA` (`vmxcodec_arm.cpp:3584-3731`; the x86
/// path is the same arithmetic), lane by lane so the result is bit-exact.
#[inline]
fn yuv_to_bgra(y: u8, u: u8, v: u8, a: u8, t: &[i16; 5]) -> [u8; 4] {
    let y = mulhi((y.saturating_sub(16) as i16) << 6, t[0]);
    let (u, v) = (u as i16 - 128, v as i16 - 128);
    let r = mulhi(v << 6, t[1]).saturating_add(y);
    let b = mulhi(u << 7, t[4]).saturating_add(y);
    let g = y
        .saturating_sub(mulhi(u << 6, t[2]))
        .saturating_sub(mulhi(v << 6, t[3]));
    let px = |c: i16| (c.saturating_add(8) >> 4).clamp(0, 255) as u8;
    [px(b), px(g), px(r), a]
}

/// Converts a `Yuv422p` / `Yuva422p` frame to packed BGRA, appending to
/// `out`. Without an alpha plane, alpha is 255 (libvmx's BGRX).
fn to_bgra(f: &Frame, t: &[i16; 5], out: &mut Vec<u8>) {
    let (w, h) = (f.width, f.height);
    out.reserve(4 * w * h);
    let (yp, up, vp) = (&f.planes[0], &f.planes[1], &f.planes[2]);
    let ap = f.planes.get(3);
    for r in 0..h {
        let yr = &yp.data[r * yp.stride..][..w];
        let ur = &up.data[r * up.stride..][..w / 2];
        let vr = &vp.data[r * vp.stride..][..w / 2];
        for x in 0..w {
            let a = ap.map_or(255, |p| p.data[r * p.stride + x]);
            out.extend_from_slice(&yuv_to_bgra(yr[x], ur[x / 2], vr[x / 2], a, t));
        }
    }
}

/// Packs a `Yuv422p` / `Yuva422p` frame as UYVY, appending to `out`.
fn pack_uyvy(f: &Frame, out: &mut Vec<u8>) {
    let (w, h) = (f.width, f.height);
    out.reserve(2 * w * h);
    let (yp, up, vp) = (&f.planes[0], &f.planes[1], &f.planes[2]);
    for r in 0..h {
        let yr = &yp.data[r * yp.stride..][..w];
        let ur = &up.data[r * up.stride..][..w / 2];
        let vr = &vp.data[r * vp.stride..][..w / 2];
        for i in 0..w / 2 {
            out.extend_from_slice(&[ur[i], yr[2 * i], vr[i], yr[2 * i + 1]]);
        }
    }
}

fn copy_plane(p: &Plane, w: usize, h: usize, out: &mut Vec<u8>) {
    for r in 0..h {
        out.extend_from_slice(&p.data[r * p.stride..][..w]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{AudioHeader, FrameHeader, FrameType};
    use vmx_codec::{Encoder, EncoderConfig, Profile};

    fn owned(ext: ExtendedHeader, data: Vec<u8>, metadata: &[u8]) -> OwnedFrame {
        OwnedFrame {
            header: FrameHeader {
                frame_type: ext.frame_type(),
                timestamp: 1234,
                metadata_length: metadata.len() as u16,
                data_length: 0,
            },
            ext,
            data,
            metadata: metadata.to_vec(),
        }
    }

    fn video_header(w: i32, h: i32, flags: u32) -> VideoHeader {
        VideoHeader {
            codec: CODEC_VMX1,
            width: w,
            height: h,
            frame_rate_n: 60000,
            frame_rate_d: 1001,
            aspect_ratio: 16.0 / 9.0,
            flags: VideoFlags(flags),
            color_space: 709,
        }
    }

    fn encode(f: &Frame) -> Vec<u8> {
        let mut cfg = EncoderConfig::new(f.width, f.height);
        cfg.profile = Profile::OmtHq;
        Encoder::new(cfg).unwrap().encode(f).unwrap()
    }

    fn ramp(w: usize, h: usize, format: PixelFormat) -> Frame {
        let mut f = Frame::new(w, h, format);
        for (p, plane) in f.planes.iter_mut().enumerate() {
            for (i, b) in plane.data.iter_mut().enumerate() {
                *b = ((i * (3 + p) / 5 + 20) % 200) as u8 + 16;
            }
        }
        f
    }

    #[test]
    fn select_follows_libomtnet() {
        use PreferredVideoFormat as P;
        use VideoFormat as F;
        let (a, hi, pv) = (
            VideoFlags::ALPHA,
            VideoFlags::HIGH_BIT_DEPTH,
            VideoFlags::PREVIEW,
        );
        let s = |p: P, f: u32| p.select(VideoFlags(f));
        // OMTReceive.cs:839-887, full frames.
        assert_eq!(s(P::Uyvy, a | hi), Some(F::Uyvy));
        assert_eq!(s(P::UyvyOrBgra, hi), Some(F::Uyvy));
        assert_eq!(s(P::UyvyOrBgra, a), Some(F::Bgra));
        assert_eq!(s(P::Bgra, 0), Some(F::Bgra));
        assert_eq!(s(P::UyvyOrUyva, a | hi), Some(F::Uyva));
        assert_eq!(s(P::UyvyOrUyvaOrP216OrPa16, 0), Some(F::Uyvy));
        assert_eq!(s(P::UyvyOrUyvaOrP216OrPa16, a), Some(F::Uyva));
        assert_eq!(s(P::UyvyOrUyvaOrP216OrPa16, hi), Some(F::P216));
        assert_eq!(s(P::UyvyOrUyvaOrP216OrPa16, a | hi), Some(F::Pa16));
        assert_eq!(s(P::P216, a), Some(F::P216));
        // OMTReceive.cs:803-836, previews: never 10-bit.
        assert_eq!(s(P::UyvyOrUyvaOrP216OrPa16, pv | hi), Some(F::Uyvy));
        assert_eq!(s(P::UyvyOrUyvaOrP216OrPa16, pv | hi | a), Some(F::Uyva));
        assert_eq!(s(P::UyvyOrBgra, pv | a), Some(F::Bgra));
        assert_eq!(s(P::P216, pv), None);
    }

    #[test]
    fn uyvy_and_p216_match_vmx_codec() {
        let src = ramp(64, 32, PixelFormat::P216);
        let packet = encode(&src);
        let mut vmx = Decoder::new(64, 32).unwrap();
        let mut d = MediaDecoder::new(PreferredVideoFormat::UyvyOrUyvaOrP216OrPa16);
        let flags = VideoFlags::HIGH_BIT_DEPTH;
        let f = owned(
            ExtendedHeader::Video(video_header(64, 32, flags)),
            packet.clone(),
            b"<m/>\0",
        );
        let Some(Media::Video(v)) = d.decode(&f).unwrap() else {
            panic!("video")
        };
        let expected = vmx.decode(&packet, PixelFormat::P216).unwrap();
        assert_eq!(v.format, VideoFormat::P216);
        assert_eq!(
            v.planes(),
            [&expected.planes[0].data[..], &expected.planes[1].data[..]]
        );
        assert_eq!((v.timestamp, v.stride, v.frame_rate_n), (1234, 128, 60000));
        assert_eq!(v.metadata, b"<m/>\0");
        assert!(v.is_high_bit_depth() && !v.is_bt601());

        // The same frame for a UYVY receiver goes through the 8-bit path.
        d.set_preferred(PreferredVideoFormat::Uyvy);
        let mut out = VideoFrame::default();
        d.decode_video(&f, &mut out).unwrap();
        let expected = vmx.decode(&packet, PixelFormat::Uyvy).unwrap();
        assert_eq!(out.data, expected.planes[0].data);
        // And the buffer is reused.
        let ptr = out.data.as_ptr();
        d.decode_video(&f, &mut out).unwrap();
        assert_eq!(out.data.as_ptr(), ptr);
    }

    #[test]
    fn uyva_and_pa16_are_planes_back_to_back() {
        let src = ramp(32, 16, PixelFormat::Uyva);
        let packet = encode(&src);
        let mut d = MediaDecoder::new(PreferredVideoFormat::UyvyOrUyva);
        let f = owned(
            ExtendedHeader::Video(video_header(32, 16, VideoFlags::ALPHA)),
            packet.clone(),
            b"",
        );
        let Some(Media::Video(v)) = d.decode(&f).unwrap() else {
            panic!()
        };
        let e = Decoder::new(32, 16)
            .unwrap()
            .decode(&packet, PixelFormat::Uyva)
            .unwrap();
        assert_eq!(v.data.len(), 32 * 2 * 16 + 32 * 16);
        assert_eq!(v.planes(), [&e.planes[0].data[..], &e.planes[1].data[..]]);
    }

    #[test]
    fn bgra_white_black_and_alpha() {
        let t = &YUV_RGB_709;
        assert_eq!(yuv_to_bgra(235, 128, 128, 7, t), [255, 255, 255, 7]);
        assert_eq!(yuv_to_bgra(16, 128, 128, 255, t), [0, 0, 0, 255]);
        let red = yuv_to_bgra(63, 102, 240, 255, t);
        assert!(red[2] > 250 && red[0] < 5 && red[1] < 5, "{red:?}");
    }

    #[test]
    fn previews_have_the_preview_size() {
        let src = ramp(128, 64, PixelFormat::Uyva);
        let packet = encode(&src);
        let flags = VideoFlags::PREVIEW | VideoFlags::ALPHA;
        let f = owned(
            ExtendedHeader::Video(video_header(128, 64, flags)),
            packet,
            b"",
        );
        for (pref, format, len) in [
            (PreferredVideoFormat::Uyvy, VideoFormat::Uyvy, 16 * 2 * 8),
            (
                PreferredVideoFormat::UyvyOrUyva,
                VideoFormat::Uyva,
                16 * 3 * 8,
            ),
            (PreferredVideoFormat::Bgra, VideoFormat::Bgra, 16 * 4 * 8),
        ] {
            let mut d = MediaDecoder::new(pref);
            let Some(Media::Video(v)) = d.decode(&f).unwrap() else {
                panic!()
            };
            assert_eq!((v.width, v.height, v.format), (16, 8, format));
            assert_eq!(v.data.len(), len);
        }
        let mut d = MediaDecoder::new(PreferredVideoFormat::P216);
        assert_eq!(d.decode(&f).unwrap_err(), DecodeError::NoMatchingFormat);
    }

    #[test]
    fn bad_video_is_an_error_not_a_panic() {
        let mut d = MediaDecoder::new(PreferredVideoFormat::Uyvy);
        let f = owned(
            ExtendedHeader::Video(video_header(64, 32, 0)),
            vec![1, 2, 3],
            b"",
        );
        assert!(matches!(d.decode(&f), Err(DecodeError::Video(_))));
        let f = owned(ExtendedHeader::Video(video_header(-1, 32, 0)), vec![], b"");
        assert!(matches!(d.decode(&f), Err(DecodeError::InvalidSize { .. })));
        let mut h = video_header(64, 32, 0);
        h.codec = fourcc(b"UYVY");
        let f = owned(ExtendedHeader::Video(h), vec![], b"");
        assert_eq!(
            d.decode(&f).unwrap_err(),
            DecodeError::UnsupportedCodec(fourcc(b"UYVY"))
        );
    }

    fn audio(channels: i32, spc: i32, active: u32, data: &[f32]) -> OwnedFrame {
        let bytes = data.iter().flat_map(|s| s.to_le_bytes()).collect();
        owned(
            ExtendedHeader::Audio(AudioHeader {
                codec: CODEC_FPA1,
                sample_rate: 48000,
                samples_per_channel: spc,
                channels,
                active_channels: active,
                reserved: 0,
            }),
            bytes,
            b"<a/>",
        )
    }

    #[test]
    fn audio_reinserts_silent_channels() {
        // Channels 0 and 2 of 3 sent; channel 1 silent (A2, A3).
        let f = audio(3, 2, 0b101, &[1.0, 2.0, 5.0, 6.0]);
        let mut out = AudioFrame::default();
        decode_audio(&f, &mut out).unwrap();
        assert_eq!(out.samples, [1.0, 2.0, 0.0, 0.0, 5.0, 6.0]);
        assert_eq!(out.channel(2), [5.0, 6.0]);
        assert_eq!((out.sample_rate, out.timestamp), (48000, 1234));
        assert_eq!(out.metadata, b"<a/>");
    }

    #[test]
    fn audio_limits() {
        let mut out = AudioFrame::default();
        let f = audio(2, 2, 0b11, &[1.0, 2.0, 3.0]);
        assert_eq!(
            decode_audio(&f, &mut out),
            Err(DecodeError::AudioTruncated {
                needed: 16,
                got: 12
            })
        );
        // A4: exactly 1 MiB is accepted, one more sample per channel is not.
        let f = audio(2, 131072, 0, &[]);
        assert!(decode_audio(&f, &mut out).is_ok());
        let f = audio(2, 131073, 0, &[]);
        assert_eq!(
            decode_audio(&f, &mut out),
            Err(DecodeError::AudioTooLarge(1_048_584))
        );
        let f = audio(33, 1, 0, &[]);
        assert_eq!(
            decode_audio(&f, &mut out),
            Err(DecodeError::InvalidAudioHeader)
        );
    }

    #[test]
    fn metadata_passes_application_xml_and_sender_info_only() {
        use crate::command::Command;
        let mut d = MediaDecoder::new(PreferredVideoFormat::Uyvy);
        let meta = |xml: &[u8]| {
            let mut f = owned(ExtendedHeader::None, xml.to_vec(), b"");
            f.header.frame_type = FrameType::Metadata;
            f
        };
        let f = meta(b"<Hello />\0");
        let Some(Media::Metadata(m)) = d.decode(&f).unwrap() else {
            panic!()
        };
        assert_eq!((m.text().as_ref(), m.sender_info), ("<Hello />", false));
        let f = meta(br#"<OMTInfo ProductName="x" />"#);
        assert!(matches!(d.decode(&f).unwrap(), Some(Media::Metadata(m)) if m.sender_info));
        let f = meta(Command::SubscribeVideo.as_bytes());
        assert!(d.decode(&f).unwrap().is_none());
    }
}
