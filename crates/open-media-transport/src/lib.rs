//! # open-media-transport
//!
//! A pure-Rust implementation of the
//! [Open Media Transport](https://www.openmediatransport.org/) (OMT) protocol,
//! the open, MIT-licensed alternative to NDI made by the vMix team.
//!
//! The protocol is implemented from `docs/PROTOCOL.md` in this repository,
//! which describes what the reference implementation,
//! [libomtnet](https://github.com/openmediatransport/libomtnet) (MIT,
//! Copyright (c) 2025 Open Media Transport Contributors), actually does, with
//! a `file:line` citation for every statement. Comments here refer to that
//! document's statement IDs (`M2`, `R1`, ...).
//!
//! ## Status
//!
//! Only the wire format exists so far: frame headers ([`frame`]), protocol
//! commands ([`command`]) and an incremental frame parser ([`Deframer`]).
//! There is no networking or discovery yet, and **nothing here has been
//! tested against another implementation**.
//!
//! ## The wire format in one paragraph
//!
//! Everything travels over TCP as frames: a 16-byte little-endian header
//! (version, type, 100 ns timestamp, per-frame metadata length, payload
//! length), then for video a 32-byte and for audio a 24-byte extended header,
//! then the payload, with any per-frame XML metadata at its end. Control
//! messages are metadata frames whose XML must match fixed strings byte for
//! byte, without a terminating NUL.
//!
//! ```
//! use open_media_transport::{command::Command, frame, Deframer, Limits};
//!
//! // What a receiver sends first on its video connection.
//! let mut wire = Vec::new();
//! frame::write_metadata(0, Command::SubscribeMetadata.as_bytes(), &mut wire);
//!
//! // What a sender does with it.
//! let mut deframer = Deframer::new(Limits::VIDEO);
//! deframer.push(&wire);
//! let f = deframer.next_frame()?.expect("one complete frame");
//! assert_eq!(Command::recognize(&f.data), Some(Command::SubscribeMetadata));
//! # Ok::<(), open_media_transport::Error>(())
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod command;
mod deframer;
pub mod frame;

pub use deframer::{Deframer, Limits, OwnedFrame};

use std::fmt;

/// Errors from parsing frames off the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Header version byte other than 1. libomtnet stalls on this (R1);
    /// we report it so the caller can drop the connection.
    UnsupportedVersion(u8),
    /// Frame type other than metadata (1), video (2) or audio (4) (R2).
    UnknownFrameType(u8),
    /// `DataLength` is negative.
    NegativeDataLength(i32),
    /// `DataLength` is too small for the extended header plus the
    /// per-frame metadata it claims to contain.
    DataLengthTooSmall {
        /// `DataLength` from the header.
        data_length: usize,
        /// Extended header length plus `MetadataLength`.
        needed: usize,
    },
    /// The whole frame would exceed the configured limit. libomtnet stalls
    /// when a frame does not fit its receive buffer (R3).
    FrameTooLarge {
        /// Header plus `DataLength`.
        length: usize,
        /// The limit in force.
        max: usize,
    },
    /// Fewer bytes than a complete frame.
    Truncated,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::UnsupportedVersion(v) => write!(f, "unsupported OMT frame version {v}"),
            Error::UnknownFrameType(t) => write!(f, "unknown OMT frame type {t}"),
            Error::NegativeDataLength(n) => write!(f, "negative frame data length {n}"),
            Error::DataLengthTooSmall {
                data_length,
                needed,
            } => {
                write!(f, "frame data length {data_length} is less than the {needed} bytes its headers need")
            }
            Error::FrameTooLarge { length, max } => {
                write!(f, "frame of {length} bytes exceeds the {max}-byte limit")
            }
            Error::Truncated => write!(f, "incomplete frame"),
        }
    }
}

impl std::error::Error for Error {}
