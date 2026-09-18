//! Uncompressed frames and their memory layouts.

use crate::Error;

/// Memory layout of an uncompressed frame.
///
/// All 16-bit formats store little-endian samples with the 10 significant
/// bits in the most significant position (`value << 6`), the convention of
/// Windows/FFmpeg `P216` and of libvmx. The low 6 bits are ignored by the
/// encoder and written as zero by the decoder.
///
/// Plane layouts (`w` = width, `h` = height, all rows of a plane share one
/// stride in bytes, which may be larger than the row size shown):
///
/// | format      | planes (bytes per row x rows)                                  | depth  | encode | decode |
/// |-------------|----------------------------------------------------------------|--------|--------|--------|
/// | `Uyvy`      | `U Y V Y` packed: `2w x h`                                     | 8-bit  | yes    | yes    |
/// | `Yuy2`      | `Y U Y V` packed: `2w x h`                                     | 8-bit  | yes    | yes    |
/// | `Uyva`      | UYVY `2w x h`, then alpha `w x h`                              | 8-bit  | yes    | yes    |
/// | `Yuv422p`   | Y `w x h`, U `w/2 x h`, V `w/2 x h`                            | 8-bit  | yes    | yes    |
/// | `Yuva422p`  | Y `w x h`, U `w/2 x h`, V `w/2 x h`, A `w x h`                 | 8-bit  | yes    | yes    |
/// | `P216`      | Y `2w x h` (u16), interleaved UV `2w x h` (u16 U, u16 V)       | 10-bit | yes    | yes    |
/// | `Pa16`      | as `P216`, then alpha `2w x h` (u16)                           | 10-bit | yes    | yes    |
/// | `Nv12`      | Y `w x h`, interleaved UV `w x h/2`                            | 8-bit  | yes    | no     |
/// | `I420`      | Y `w x h`, U `w/2 x h/2`, V `w/2 x h/2`                        | 8-bit  | yes    | no     |
///
/// 4:2:0 input is converted to 4:2:2 by repeating each chroma line, exactly
/// as libvmx does; VMX itself is always 4:2:2 (or 4:2:2:4 with alpha).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    /// Packed 4:2:2, `U0 Y0 V0 Y1`. FFmpeg `uyvy422`.
    Uyvy,
    /// Packed 4:2:2, `Y0 U0 Y1 V0`. FFmpeg `yuyv422`.
    Yuy2,
    /// UYVY followed by an 8-bit alpha plane.
    Uyva,
    /// Planar 4:2:2, 8-bit. FFmpeg `yuv422p`.
    Yuv422p,
    /// Planar 4:2:2:4, 8-bit. FFmpeg `yuva422p`.
    Yuva422p,
    /// 16-bit Y plane + interleaved 16-bit UV plane, 4:2:2, 10 significant bits.
    P216,
    /// `P216` followed by a 16-bit alpha plane.
    Pa16,
    /// 8-bit Y plane + interleaved UV plane, 4:2:0 (encode only).
    Nv12,
    /// 8-bit planar 4:2:0, also known as YV12 with U/V swapped (encode only).
    I420,
}

impl PixelFormat {
    /// Whether the format carries an alpha channel.
    pub fn has_alpha(self) -> bool {
        matches!(self, PixelFormat::Uyva | PixelFormat::Yuva422p | PixelFormat::Pa16)
    }

    /// Whether the format selects the 10-bit coding path.
    pub fn is_10bit(self) -> bool {
        matches!(self, PixelFormat::P216 | PixelFormat::Pa16)
    }

    /// Whether [`crate::Decoder`] can produce this format.
    pub fn can_decode(self) -> bool {
        !matches!(self, PixelFormat::Nv12 | PixelFormat::I420)
    }

    /// Minimum `(bytes per row, rows)` of each plane for a `width` x `height` frame.
    pub fn plane_sizes(self, width: usize, height: usize) -> Vec<(usize, usize)> {
        let (w, h) = (width, height);
        match self {
            PixelFormat::Uyvy | PixelFormat::Yuy2 => vec![(2 * w, h)],
            PixelFormat::Uyva => vec![(2 * w, h), (w, h)],
            PixelFormat::Yuv422p => vec![(w, h), (w / 2, h), (w / 2, h)],
            PixelFormat::Yuva422p => vec![(w, h), (w / 2, h), (w / 2, h), (w, h)],
            PixelFormat::P216 => vec![(2 * w, h), (2 * w, h)],
            PixelFormat::Pa16 => vec![(2 * w, h), (2 * w, h), (2 * w, h)],
            PixelFormat::Nv12 => vec![(w, h), (w, h / 2)],
            PixelFormat::I420 => vec![(w, h), (w / 2, h / 2), (w / 2, h / 2)],
        }
    }
}

/// One plane of an uncompressed frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plane {
    /// Sample bytes; row `r` starts at `r * stride`.
    pub data: Vec<u8>,
    /// Distance between rows in bytes.
    pub stride: usize,
}

/// An uncompressed video frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// Width in pixels (even).
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// Memory layout of [`Frame::planes`].
    pub format: PixelFormat,
    /// Interlaced content (two fields, top field on even lines). Only honoured
    /// by VMX for heights 480, 576 and 1080; other heights are coded as
    /// progressive, as in libvmx.
    pub interlaced: bool,
    /// Planes in the order given by [`PixelFormat`].
    pub planes: Vec<Plane>,
}

impl Frame {
    /// Allocates a zeroed, tightly packed frame.
    pub fn new(width: usize, height: usize, format: PixelFormat) -> Self {
        let planes = format
            .plane_sizes(width, height)
            .into_iter()
            .map(|(row, rows)| Plane { data: vec![0; row * rows], stride: row })
            .collect();
        Self { width, height, format, interlaced: false, planes }
    }

    /// Wraps existing planes, checking that they are large enough.
    pub fn from_planes(width: usize, height: usize, format: PixelFormat, planes: Vec<Plane>) -> Result<Self, Error> {
        let f = Self { width, height, format, interlaced: false, planes };
        f.validate()?;
        Ok(f)
    }

    /// Checks plane count, strides and buffer sizes against the format.
    pub fn validate(&self) -> Result<(), Error> {
        let sizes = self.format.plane_sizes(self.width, self.height);
        if self.planes.len() != sizes.len() {
            return Err(Error::InvalidFrame("wrong number of planes for the pixel format"));
        }
        for (p, (row, rows)) in self.planes.iter().zip(sizes) {
            if p.stride < row {
                return Err(Error::InvalidFrame("plane stride smaller than a row"));
            }
            if rows > 0 && p.data.len() < p.stride * (rows - 1) + row {
                return Err(Error::InvalidFrame("plane buffer too small"));
            }
        }
        Ok(())
    }

    /// Row `r` of plane `p` (`len` bytes).
    #[inline]
    pub(crate) fn row(&self, p: usize, r: usize, len: usize) -> &[u8] {
        let pl = &self.planes[p];
        &pl.data[r * pl.stride..r * pl.stride + len]
    }
}
