//! Incremental frame parser for a TCP byte stream.
//!
//! libomtnet bounds frames only by the size of its receive buffer, and a
//! frame that does not fit, or has an unknown version, leaves the connection
//! waiting forever (R1, R3). Here every limit is explicit and every bad
//! header is an error, so the caller can drop the peer.

use crate::frame::{AudioHeader, ExtendedHeader, FrameHeader, FrameType, VideoHeader, HEADER_LEN};
use crate::Error;

/// Largest whole frame (header included) accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Header plus `DataLength`, in bytes.
    pub max_frame_len: usize,
}

impl Limits {
    /// 10 MiB: libomtnet's receive buffer on a video connection and its
    /// send-side drop threshold (R3, R4; `OMTConstants.cs:57`).
    pub const VIDEO: Limits = Limits {
        max_frame_len: 10_485_760,
    };
    /// 1 MiB: libomtnet's receive buffer on audio and metadata connections
    /// (R3; `OMTConstants.cs:62`).
    pub const AUDIO_OR_METADATA: Limits = Limits {
        max_frame_len: 1_048_576,
    };
}

/// A complete frame, copied out of the stream.
#[derive(Clone, Debug, PartialEq)]
pub struct OwnedFrame {
    /// The frame header.
    pub header: FrameHeader,
    /// The extended header matching `header.frame_type`.
    pub ext: ExtendedHeader,
    /// Payload without the per-frame metadata: compressed video, audio
    /// samples, or a metadata frame's XML.
    pub data: Vec<u8>,
    /// Per-frame metadata: the last `MetadataLength` bytes (§3.2).
    pub metadata: Vec<u8>,
}

/// Splits a byte stream into frames.
#[derive(Debug)]
pub struct Deframer {
    limits: Limits,
    buf: Vec<u8>,
    start: usize,
}

impl Deframer {
    /// A parser that rejects frames larger than `limits`.
    pub fn new(limits: Limits) -> Self {
        Deframer {
            limits,
            buf: Vec::new(),
            start: 0,
        }
    }

    /// Appends bytes read from the socket.
    pub fn push(&mut self, bytes: &[u8]) {
        if self.start > 0 && self.start == self.buf.len() {
            self.buf.clear();
            self.start = 0;
        }
        self.buf.extend_from_slice(bytes);
    }

    /// Bytes received but not yet returned as frames.
    pub fn buffered(&self) -> usize {
        self.buf.len() - self.start
    }

    /// Returns the next complete frame, `Ok(None)` if more bytes are needed,
    /// or an error once the stream is known to be bad. After an error the
    /// stream cannot be resynchronised; drop the connection.
    pub fn next_frame(&mut self) -> Result<Option<OwnedFrame>, Error> {
        let avail = &self.buf[self.start..];
        if avail.len() < HEADER_LEN {
            return Ok(None);
        }
        let header = FrameHeader::parse(avail)?;
        let len = header.frame_len()?;
        if len > self.limits.max_frame_len {
            return Err(Error::FrameTooLarge {
                length: len,
                max: self.limits.max_frame_len,
            });
        }
        if avail.len() < len {
            return Ok(None);
        }
        let body = &avail[HEADER_LEN..len];
        let ext_len = header.frame_type.extended_header_len();
        let ext = match header.frame_type {
            FrameType::Metadata => ExtendedHeader::None,
            FrameType::Video => ExtendedHeader::Video(VideoHeader::parse(body)?),
            FrameType::Audio => ExtendedHeader::Audio(AudioHeader::parse(body)?),
        };
        let split = body.len() - header.metadata_length as usize;
        let frame = OwnedFrame {
            header,
            ext,
            data: body[ext_len..split].to_vec(),
            metadata: body[split..].to_vec(),
        };
        self.start += len;
        if self.start > self.buf.len() / 2 {
            self.buf.drain(..self.start);
            self.start = 0;
        }
        Ok(Some(frame))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Command;
    use crate::frame::{self, VideoFlags, CODEC_VMX1};

    fn video() -> ExtendedHeader {
        ExtendedHeader::Video(VideoHeader {
            codec: CODEC_VMX1,
            width: 1920,
            height: 1080,
            frame_rate_n: 50,
            frame_rate_d: 1,
            aspect_ratio: 16.0 / 9.0,
            flags: VideoFlags::default(),
            color_space: 709,
        })
    }

    #[test]
    fn splits_frames_fed_one_byte_at_a_time() {
        let mut wire = Vec::new();
        frame::write_metadata(0, Command::SubscribeMetadata.as_bytes(), &mut wire);
        frame::write(123, &video(), &[7; 1000], b"<m/>\0", &mut wire);
        frame::write_metadata(0, Command::SubscribeVideo.as_bytes(), &mut wire);

        let mut d = Deframer::new(Limits::VIDEO);
        let mut frames = Vec::new();
        for b in &wire {
            d.push(std::slice::from_ref(b));
            while let Some(f) = d.next_frame().unwrap() {
                frames.push(f);
            }
        }
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].data, Command::SubscribeMetadata.as_bytes());
        assert_eq!(frames[1].header.timestamp, 123);
        assert_eq!(frames[1].ext, video());
        assert_eq!(frames[1].data, vec![7; 1000]);
        assert_eq!(frames[1].metadata, b"<m/>\0");
        assert_eq!(frames[2].data, Command::SubscribeVideo.as_bytes());
        assert_eq!(d.buffered(), 0);
    }

    #[test]
    fn many_frames_in_one_push() {
        let mut wire = Vec::new();
        for i in 0..50 {
            frame::write(i, &video(), &[i as u8; 64], &[], &mut wire);
        }
        let mut d = Deframer::new(Limits::VIDEO);
        d.push(&wire);
        for i in 0..50 {
            assert_eq!(d.next_frame().unwrap().unwrap().header.timestamp, i);
        }
        assert_eq!(d.next_frame().unwrap(), None);
    }

    #[test]
    fn oversized_frame_is_an_error_not_a_stall() {
        let mut wire = Vec::new();
        frame::write(0, &video(), &vec![0; 2_000_000], &[], &mut wire);
        let mut d = Deframer::new(Limits::AUDIO_OR_METADATA);
        d.push(&wire[..HEADER_LEN]);
        assert!(matches!(
            d.next_frame(),
            Err(Error::FrameTooLarge { max: 1_048_576, .. })
        ));
    }

    /// A cheap stand-in for the fuzz targets that runs with `cargo test`:
    /// valid frames, corrupted at random, fed in random chunks. The stream
    /// must end in frames, `Ok(None)` or an error, never a panic, and every
    /// frame returned must be self-consistent.
    #[test]
    fn corrupted_streams_never_panic() {
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..2000 {
            let mut wire = Vec::new();
            for i in 0..(next() % 4) {
                match next() % 3 {
                    0 => frame::write_metadata(0, Command::SubscribeVideo.as_bytes(), &mut wire),
                    1 => frame::write(i as i64, &video(), &[1; 40], b"<m/>", &mut wire),
                    _ => frame::write(i as i64, &ExtendedHeader::None, &[], &[], &mut wire),
                }
            }
            for _ in 0..(next() % 4) {
                if !wire.is_empty() {
                    let at = (next() as usize) % wire.len();
                    wire[at] = next() as u8;
                }
            }
            let mut d = Deframer::new(Limits {
                max_frame_len: 4096,
            });
            let chunk = (next() % 17 + 1) as usize;
            'stream: for piece in wire.chunks(chunk) {
                d.push(piece);
                loop {
                    match d.next_frame() {
                        Ok(Some(f)) => {
                            let ext = f.header.frame_type.extended_header_len();
                            assert_eq!(
                                HEADER_LEN + ext + f.data.len() + f.metadata.len(),
                                f.header.frame_len().unwrap()
                            );
                        }
                        Ok(None) => break,
                        Err(_) => break 'stream,
                    }
                }
            }
        }
    }

    #[test]
    fn bad_version_is_an_error_not_a_stall() {
        let mut wire = Vec::new();
        frame::write_metadata(0, b"<x/>", &mut wire);
        wire[0] = 9;
        let mut d = Deframer::new(Limits::VIDEO);
        d.push(&wire);
        assert_eq!(d.next_frame(), Err(Error::UnsupportedVersion(9)));
    }
}
