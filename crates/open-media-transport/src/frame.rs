//! Frame headers: the 16-byte header and the video and audio extended headers
//! (`docs/PROTOCOL.md` §2–3).
//!
//! All fields are little-endian (B1). Fields keep libomtnet's signed widths so
//! that any header round-trips exactly; range checks belong to whoever uses
//! the values.

use crate::Error;

/// Size of [`FrameHeader`] on the wire.
pub const HEADER_LEN: usize = 16;
/// Size of [`VideoHeader`] on the wire.
pub const VIDEO_HEADER_LEN: usize = 32;
/// Size of [`AudioHeader`] on the wire.
pub const AUDIO_HEADER_LEN: usize = 24;
/// The only version libomtnet writes or accepts.
pub const VERSION: u8 = 1;
/// Timestamp units per second (100 ns ticks).
pub const TICKS_PER_SECOND: i64 = 10_000_000;

/// The kind of frame, from the header's `FrameType` byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FrameType {
    /// XML: protocol commands or application metadata.
    Metadata,
    /// Video, with a [`VideoHeader`].
    Video,
    /// Audio, with an [`AudioHeader`].
    Audio,
}

impl FrameType {
    /// The byte on the wire.
    pub fn to_byte(self) -> u8 {
        match self {
            FrameType::Metadata => 1,
            FrameType::Video => 2,
            FrameType::Audio => 4,
        }
    }

    /// Parses the header byte; anything but 1, 2 or 4 is an error (R2).
    pub fn from_byte(b: u8) -> Result<Self, Error> {
        match b {
            1 => Ok(FrameType::Metadata),
            2 => Ok(FrameType::Video),
            4 => Ok(FrameType::Audio),
            _ => Err(Error::UnknownFrameType(b)),
        }
    }

    /// Length of the extended header that follows the frame header.
    pub fn extended_header_len(self) -> usize {
        match self {
            FrameType::Metadata => 0,
            FrameType::Video => VIDEO_HEADER_LEN,
            FrameType::Audio => AUDIO_HEADER_LEN,
        }
    }
}

/// A FourCC as libomtnet stores it: the first character in the low byte.
pub const fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*code)
}

/// FourCC `VMX1`, the only video codec a sender puts on the wire (§3.3).
pub const CODEC_VMX1: u32 = fourcc(b"VMX1");
/// FourCC `FPA1`, 32-bit float planar audio (§3.4).
pub const CODEC_FPA1: u32 = fourcc(b"FPA1");

/// The 16-byte frame header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    /// Kind of frame.
    pub frame_type: FrameType,
    /// In [`TICKS_PER_SECOND`] units.
    pub timestamp: i64,
    /// Bytes of per-frame metadata at the end of the payload.
    pub metadata_length: u16,
    /// Bytes after this header: extended header, data and per-frame metadata.
    pub data_length: i32,
}

impl FrameHeader {
    /// Serialises the header.
    pub fn to_bytes(&self) -> [u8; HEADER_LEN] {
        let mut b = [0u8; HEADER_LEN];
        b[0] = VERSION;
        b[1] = self.frame_type.to_byte();
        b[2..10].copy_from_slice(&self.timestamp.to_le_bytes());
        b[10..12].copy_from_slice(&self.metadata_length.to_le_bytes());
        b[12..16].copy_from_slice(&self.data_length.to_le_bytes());
        b
    }

    /// Parses a header from the first [`HEADER_LEN`] bytes of `b`.
    pub fn parse(b: &[u8]) -> Result<Self, Error> {
        let b: &[u8; HEADER_LEN] = b
            .get(..HEADER_LEN)
            .ok_or(Error::Truncated)?
            .try_into()
            .unwrap();
        if b[0] != VERSION {
            return Err(Error::UnsupportedVersion(b[0]));
        }
        Ok(FrameHeader {
            frame_type: FrameType::from_byte(b[1])?,
            timestamp: i64::from_le_bytes(b[2..10].try_into().unwrap()),
            metadata_length: u16::from_le_bytes(b[10..12].try_into().unwrap()),
            data_length: i32::from_le_bytes(b[12..16].try_into().unwrap()),
        })
    }

    /// Header plus payload, after checking that `DataLength` is non-negative
    /// and large enough for the extended header and the per-frame metadata.
    pub fn frame_len(&self) -> Result<usize, Error> {
        let data_length = usize::try_from(self.data_length)
            .map_err(|_| Error::NegativeDataLength(self.data_length))?;
        let needed = self.frame_type.extended_header_len() + self.metadata_length as usize;
        if data_length < needed {
            return Err(Error::DataLengthTooSmall {
                data_length,
                needed,
            });
        }
        Ok(HEADER_LEN + data_length)
    }
}

/// Video frame flags (§3.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct VideoFlags(pub u32);

impl VideoFlags {
    /// Interlaced frame.
    pub const INTERLACED: u32 = 1;
    /// Frame carries an alpha channel.
    pub const ALPHA: u32 = 2;
    /// Alpha is premultiplied (with [`Self::ALPHA`]).
    pub const PREMULTIPLIED: u32 = 4;
    /// 1/8-scale preview frame (§6.2).
    pub const PREVIEW: u32 = 8;
    /// Encoded from a 16-bit (P216/PA16) source.
    pub const HIGH_BIT_DEPTH: u32 = 16;

    /// Whether every bit of `flag` is set.
    pub fn contains(self, flag: u32) -> bool {
        self.0 & flag == flag
    }
}

/// The 32-byte video extended header.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VideoHeader {
    /// FourCC; [`CODEC_VMX1`] on the wire.
    pub codec: u32,
    /// Full-frame width in pixels, also for preview frames.
    pub width: i32,
    /// Full-frame height in pixels.
    pub height: i32,
    /// Frame rate numerator.
    pub frame_rate_n: i32,
    /// Frame rate denominator.
    pub frame_rate_d: i32,
    /// Display aspect ratio, width / height.
    pub aspect_ratio: f32,
    /// See [`VideoFlags`].
    pub flags: VideoFlags,
    /// 0 undefined, 601 or 709.
    pub color_space: i32,
}

impl VideoHeader {
    /// Serialises the header.
    pub fn to_bytes(&self) -> [u8; VIDEO_HEADER_LEN] {
        let mut b = [0u8; VIDEO_HEADER_LEN];
        let fields = [
            self.codec,
            self.width as u32,
            self.height as u32,
            self.frame_rate_n as u32,
            self.frame_rate_d as u32,
            self.aspect_ratio.to_bits(),
            self.flags.0,
            self.color_space as u32,
        ];
        for (chunk, v) in b.chunks_exact_mut(4).zip(fields) {
            chunk.copy_from_slice(&v.to_le_bytes());
        }
        b
    }

    /// Parses the header from the first [`VIDEO_HEADER_LEN`] bytes of `b`.
    pub fn parse(b: &[u8]) -> Result<Self, Error> {
        let w = words::<8>(b)?;
        Ok(VideoHeader {
            codec: w[0],
            width: w[1] as i32,
            height: w[2] as i32,
            frame_rate_n: w[3] as i32,
            frame_rate_d: w[4] as i32,
            aspect_ratio: f32::from_bits(w[5]),
            flags: VideoFlags(w[6]),
            color_space: w[7] as i32,
        })
    }
}

/// The 24-byte audio extended header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioHeader {
    /// FourCC; [`CODEC_FPA1`] on the wire.
    pub codec: u32,
    /// Samples per second.
    pub sample_rate: i32,
    /// Samples in each channel of this frame.
    pub samples_per_channel: i32,
    /// Channels described, 1..=32.
    pub channels: i32,
    /// Bit `i` set when channel `i` is present in the data; silent channels
    /// are left out (A2).
    pub active_channels: u32,
    /// Written as 0 by libomtnet.
    pub reserved: i32,
}

impl AudioHeader {
    /// Serialises the header.
    pub fn to_bytes(&self) -> [u8; AUDIO_HEADER_LEN] {
        let mut b = [0u8; AUDIO_HEADER_LEN];
        let fields = [
            self.codec,
            self.sample_rate as u32,
            self.samples_per_channel as u32,
            self.channels as u32,
            self.active_channels,
            self.reserved as u32,
        ];
        for (chunk, v) in b.chunks_exact_mut(4).zip(fields) {
            chunk.copy_from_slice(&v.to_le_bytes());
        }
        b
    }

    /// Parses the header from the first [`AUDIO_HEADER_LEN`] bytes of `b`.
    pub fn parse(b: &[u8]) -> Result<Self, Error> {
        let w = words::<6>(b)?;
        Ok(AudioHeader {
            codec: w[0],
            sample_rate: w[1] as i32,
            samples_per_channel: w[2] as i32,
            channels: w[3] as i32,
            active_channels: w[4],
            reserved: w[5] as i32,
        })
    }
}

/// The extended header, by frame type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExtendedHeader {
    /// Metadata frames have none.
    None,
    /// Video frames.
    Video(VideoHeader),
    /// Audio frames.
    Audio(AudioHeader),
}

impl ExtendedHeader {
    /// The frame type this extended header belongs to.
    pub fn frame_type(&self) -> FrameType {
        match self {
            ExtendedHeader::None => FrameType::Metadata,
            ExtendedHeader::Video(_) => FrameType::Video,
            ExtendedHeader::Audio(_) => FrameType::Audio,
        }
    }
}

/// Appends one complete frame to `out`: header, extended header, `data`,
/// then `metadata` (per-frame XML, §3.2).
///
/// # Panics
///
/// If `metadata` exceeds 65535 bytes or the frame exceeds `i32::MAX`.
pub fn write(
    timestamp: i64,
    ext: &ExtendedHeader,
    data: &[u8],
    metadata: &[u8],
    out: &mut Vec<u8>,
) {
    let frame_type = ext.frame_type();
    let metadata_length =
        u16::try_from(metadata.len()).expect("per-frame metadata over 65535 bytes");
    let data_length = frame_type.extended_header_len() + data.len() + metadata.len();
    let header = FrameHeader {
        frame_type,
        timestamp,
        metadata_length,
        data_length: i32::try_from(data_length).expect("frame over i32::MAX bytes"),
    };
    out.reserve(HEADER_LEN + data_length);
    out.extend_from_slice(&header.to_bytes());
    match ext {
        ExtendedHeader::None => {}
        ExtendedHeader::Video(v) => out.extend_from_slice(&v.to_bytes()),
        ExtendedHeader::Audio(a) => out.extend_from_slice(&a.to_bytes()),
    }
    out.extend_from_slice(data);
    out.extend_from_slice(metadata);
}

/// Appends a metadata frame carrying `xml` exactly as given. Protocol
/// commands must not include a NUL (M2); use [`crate::command::Command::as_bytes`].
pub fn write_metadata(timestamp: i64, xml: &[u8], out: &mut Vec<u8>) {
    write(timestamp, &ExtendedHeader::None, xml, &[], out);
}

fn words<const N: usize>(b: &[u8]) -> Result<[u32; N], Error> {
    let b = b.get(..N * 4).ok_or(Error::Truncated)?;
    let mut w = [0u32; N];
    for (v, chunk) in w.iter_mut().zip(b.chunks_exact(4)) {
        *v = u32::from_le_bytes(chunk.try_into().unwrap());
    }
    Ok(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fourccs_match_libomtnet() {
        // OMTPublicTypes.cs:96-97.
        assert_eq!(CODEC_VMX1, 0x3158_4D56);
        assert_eq!(CODEC_FPA1, 0x3141_5046);
    }

    #[test]
    fn header_layout() {
        // Hand-built from §3.1 (OMTFrame.cs:186-201), not from a capture.
        let h = FrameHeader {
            frame_type: FrameType::Video,
            timestamp: 0x0102_0304_0506_0708,
            metadata_length: 0x0A0B,
            data_length: 0x0C0D_0E0F,
        };
        let expected = [
            1, 2, //
            0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, //
            0x0B, 0x0A, //
            0x0F, 0x0E, 0x0D, 0x0C,
        ];
        assert_eq!(h.to_bytes(), expected);
        assert_eq!(FrameHeader::parse(&expected).unwrap(), h);
    }

    #[test]
    fn header_rejects_bad_version_and_type() {
        let mut b = FrameHeader {
            frame_type: FrameType::Metadata,
            timestamp: 0,
            metadata_length: 0,
            data_length: 0,
        }
        .to_bytes();
        b[0] = 2;
        assert_eq!(FrameHeader::parse(&b), Err(Error::UnsupportedVersion(2)));
        b[0] = 1;
        b[1] = 3;
        assert_eq!(FrameHeader::parse(&b), Err(Error::UnknownFrameType(3)));
        assert_eq!(FrameHeader::parse(&b[..15]), Err(Error::Truncated));
    }

    #[test]
    fn frame_len_checks() {
        let mut h = FrameHeader {
            frame_type: FrameType::Video,
            timestamp: 0,
            metadata_length: 10,
            data_length: 41,
        };
        assert_eq!(
            h.frame_len(),
            Err(Error::DataLengthTooSmall {
                data_length: 41,
                needed: 42
            })
        );
        h.data_length = 42;
        assert_eq!(h.frame_len(), Ok(58));
        h.data_length = -1;
        assert_eq!(h.frame_len(), Err(Error::NegativeDataLength(-1)));
    }

    #[test]
    fn video_header_layout() {
        // §3.3 order: codec, width, height, rate n, rate d, aspect, flags, colour space.
        let v = VideoHeader {
            codec: CODEC_VMX1,
            width: 1920,
            height: 1080,
            frame_rate_n: 60000,
            frame_rate_d: 1001,
            aspect_ratio: 16.0 / 9.0,
            flags: VideoFlags(VideoFlags::INTERLACED | VideoFlags::PREVIEW),
            color_space: 709,
        };
        let b = v.to_bytes();
        assert_eq!(&b[0..4], b"VMX1");
        assert_eq!(&b[4..8], &1920u32.to_le_bytes());
        assert_eq!(&b[16..20], &1001u32.to_le_bytes());
        assert_eq!(&b[20..24], &(16.0f32 / 9.0).to_bits().to_le_bytes());
        assert_eq!(&b[24..28], &9u32.to_le_bytes());
        assert_eq!(&b[28..32], &709u32.to_le_bytes());
        assert_eq!(VideoHeader::parse(&b).unwrap(), v);
    }

    #[test]
    fn audio_header_layout() {
        let a = AudioHeader {
            codec: CODEC_FPA1,
            sample_rate: 48000,
            samples_per_channel: 800,
            channels: 32,
            active_channels: 1 << 31 | 1,
            reserved: 0,
        };
        let b = a.to_bytes();
        assert_eq!(&b[0..4], b"FPA1");
        assert_eq!(&b[16..20], &[1, 0, 0, 0x80]);
        assert_eq!(AudioHeader::parse(&b).unwrap(), a);
    }

    #[test]
    fn write_puts_metadata_last_and_counts_it() {
        let v = VideoHeader {
            codec: CODEC_VMX1,
            width: 16,
            height: 16,
            frame_rate_n: 30,
            frame_rate_d: 1,
            aspect_ratio: 1.0,
            flags: VideoFlags::default(),
            color_space: 0,
        };
        let mut out = Vec::new();
        write(5, &ExtendedHeader::Video(v), b"pixels", b"<x/>\0", &mut out);
        let h = FrameHeader::parse(&out).unwrap();
        assert_eq!(h.metadata_length, 5);
        assert_eq!(h.data_length as usize, VIDEO_HEADER_LEN + 6 + 5);
        assert_eq!(out.len(), h.frame_len().unwrap());
        assert!(out.ends_with(b"pixels<x/>\0"));
    }
}
