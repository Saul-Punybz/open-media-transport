//! # vmx-codec
//!
//! A pure-Rust implementation of **VMX**, the intra-frame video codec of
//! [Open Media Transport](https://www.openmediatransport.org/) (OMT), the
//! open, MIT-licensed alternative to NDI made by the vMix team.
//!
//! This crate is a port of the reference C++ implementation,
//! [libvmx](https://github.com/openmediatransport/libvmx) (MIT, Copyright (c)
//! 2025 Open Media Transport Contributors). Its bitstream is byte-for-byte
//! identical to libvmx's 128-bit (SSE / NEON) code path, and its decoded
//! pixels are identical too; see `tests/conformance.rs`.
//!
//! ## The format in one paragraph
//!
//! Every frame is 4:2:2 (optionally with a full-resolution alpha plane),
//! 8-bit or 10-bit, cut into slices of 16 lines. Each plane of a slice is
//! transformed in 8x8 blocks with a fixed-point DCT, quantised with one of
//! 25 scaled JPEG-like matrices, and entropy coded with Exp-Golomb style
//! codes into two byte-aligned streams per slice: one for the DC
//! coefficients (differentially coded) and one for the AC coefficients
//! (zero runs + values). Slices are independent, which makes the codec
//! trivially parallel and low latency. There is no inter-frame prediction.
//!
//! ## Usage
//!
//! ```
//! use vmx_codec::{Decoder, Encoder, EncoderConfig, Frame, PixelFormat, Profile};
//!
//! let (w, h) = (128, 64);
//! let mut config = EncoderConfig::new(w, h);
//! config.profile = Profile::OmtHq;
//! let mut encoder = Encoder::new(config)?;
//!
//! let mut frame = Frame::new(w, h, PixelFormat::Uyvy);
//! for (i, b) in frame.planes[0].data.iter_mut().enumerate() {
//!     *b = (i % 251) as u8;
//! }
//! let packet: Vec<u8> = encoder.encode(&frame)?;
//!
//! let mut decoder = Decoder::new(w, h)?;
//! let decoded: Frame = decoder.decode(&packet, PixelFormat::Uyvy)?;
//! assert_eq!(decoded.planes[0].data.len(), w * 2 * h);
//! # Ok::<(), vmx_codec::Error>(())
//! ```
//!
//! The compressed frame does not record its size, bit depth or whether it
//! carries alpha: the transport does. Decode 10-bit streams to
//! [`PixelFormat::P216`] / [`PixelFormat::Pa16`] and 8-bit streams to any
//! other decodable format.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod bits;
mod convert;
mod dct;
mod decoder;
mod encoder;
mod frame;
mod lanes;
mod layout;
mod slice;
mod tables;

pub use decoder::{Decoder, FrameInfo};
pub use encoder::{Encoder, EncoderConfig, EncodingParameters, Profile};
pub use frame::{Frame, PixelFormat, Plane};

/// Errors returned by the encoder and decoder.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// Width must be even and 16..=7680, height 16..=4320.
    InvalidDimensions {
        /// Requested width.
        width: usize,
        /// Requested height.
        height: usize,
    },
    /// The frame's size differs from the codec's.
    FrameSizeMismatch,
    /// The frame's planes do not match its pixel format.
    InvalidFrame(&'static str),
    /// Not supported by this implementation (or by VMX).
    Unsupported(&'static str),
    /// The compressed frame ended early.
    Truncated,
    /// The compressed frame was made for a different height.
    SliceCountMismatch {
        /// Slices (low 8 bits) this decoder expects.
        expected: usize,
        /// Slice count byte found in the frame.
        found: usize,
    },
    /// Malformed compressed data.
    InvalidBitstream(&'static str),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::InvalidDimensions { width, height } => write!(f, "invalid VMX frame size {width}x{height}"),
            Error::FrameSizeMismatch => f.write_str("frame size does not match the codec"),
            Error::InvalidFrame(m) => write!(f, "invalid frame: {m}"),
            Error::Unsupported(m) => write!(f, "unsupported: {m}"),
            Error::Truncated => f.write_str("compressed frame is truncated"),
            Error::SliceCountMismatch { expected, found } => {
                write!(f, "compressed frame has {found} slices, expected {}", expected & 0xFF)
            }
            Error::InvalidBitstream(m) => write!(f, "invalid VMX bitstream: {m}"),
        }
    }
}

impl std::error::Error for Error {}
