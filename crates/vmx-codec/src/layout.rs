//! Frame geometry, quality presets and the compressed frame container.

use crate::tables::{QUALITY, QUALITY_COUNT, SLICE_HEIGHT};
use crate::Error;

pub(crate) const MIN_WIDTH: usize = 16;
pub(crate) const MIN_HEIGHT: usize = 16;
pub(crate) const MAX_WIDTH: usize = 7680;
pub(crate) const MAX_HEIGHT: usize = 4320;

/// Container format byte values (`VMX_CODEC_FORMAT`).
pub(crate) const FORMAT_PROGRESSIVE: u8 = 1;
pub(crate) const FORMAT_INTERLACED: u8 = 2;
pub(crate) const FORMAT_EXTENDED: u8 = 3;

fn align(v: usize, a: usize) -> usize {
    v.div_ceil(a) * a
}

/// Internal plane geometry for one frame size.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Layout {
    pub width: usize,
    pub height: usize,
    pub aligned_height: usize,
    pub slices: usize,
    /// Stride (in samples) of planes 0..4: Y, U, V, A.
    pub strides: [usize; 4],
}

impl Layout {
    pub(crate) fn new(width: usize, height: usize) -> Result<Self, Error> {
        if !(MIN_WIDTH..=MAX_WIDTH).contains(&width)
            || !(MIN_HEIGHT..=MAX_HEIGHT).contains(&height)
            || width % 2 != 0
        {
            return Err(Error::InvalidDimensions { width, height });
        }
        let sy = align(width, 8);
        let sc = align(width / 2, 8);
        let aligned_height = align(height, SLICE_HEIGHT);
        Ok(Self { width, height, aligned_height, slices: aligned_height / SLICE_HEIGHT, strides: [sy, sc, sc, sy] })
    }

    /// Whether libvmx codes a frame of this height as interlaced when asked.
    pub(crate) fn interlace_capable(&self) -> bool {
        matches!(self.height, 480 | 576 | 1080)
    }

    /// Image row stored in internal plane row `r`, or `None` for padding rows.
    ///
    /// Interlaced frames store the top field (even lines) in the upper half of
    /// the slices and the bottom field in the lower half.
    #[inline]
    pub(crate) fn image_row(&self, r: usize, interlaced: bool) -> Option<usize> {
        if !interlaced {
            return (r < self.height).then_some(r);
        }
        let half = self.aligned_height / 2;
        let field_rows = self.height / 2;
        let (f, parity) = if r < half { (r, 0) } else { (r - half, 1) };
        (f < field_rows).then_some(2 * f + parity)
    }

    /// Largest DC stream libvmx accepts for one slice.
    pub(crate) fn max_dc_len(&self) -> usize {
        self.strides[0] * SLICE_HEIGHT * 2
    }

    /// Largest AC stream libvmx accepts for one slice.
    pub(crate) fn max_ac_len(&self) -> usize {
        let ac = self.strides[0] * SLICE_HEIGHT * 4;
        ac - (ac >> 3)
    }

    /// Preview (1/8 scale) dimensions, as `VMX_Create` computes them.
    pub(crate) fn preview_size(&self, interlaced: bool) -> (usize, usize) {
        let w = align(self.width >> 3, 2);
        let mut h = self.height >> 3;
        if interlaced && h % 2 == 1 {
            h -= 1;
        }
        (w, h)
    }
}

/// Resolves a quality value to (effective quality, preset index), exactly as
/// libvmx `VMX_SetQualityInternal` (including its fall-through to preset 0
/// for qualities below 36).
pub(crate) fn quality_preset(q: i32) -> (i32, usize) {
    for (i, &m) in QUALITY.iter().enumerate().take(QUALITY_COUNT) {
        if m as i32 >= 100 - q {
            return (100 - m as i32, i);
        }
    }
    (q, 0)
}

/// Parsed container header plus the per-slice stream ranges.
pub(crate) struct Container<'a> {
    pub interlaced: bool,
    pub quality: i32,
    pub dc_shift: u32,
    pub dc: Vec<&'a [u8]>,
    /// Empty when the frame carries only the DC (preview) part.
    pub ac: Vec<&'a [u8]>,
}

fn read_u32(data: &[u8], pos: &mut usize) -> Result<usize, Error> {
    let b = data.get(*pos..*pos + 4).ok_or(Error::Truncated)?;
    *pos += 4;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

fn read_streams<'a>(data: &'a [u8], pos: &mut usize, slices: usize, max: usize) -> Result<Vec<&'a [u8]>, Error> {
    let mut v = Vec::with_capacity(slices);
    for _ in 0..slices {
        let len = read_u32(data, pos)?;
        if len > max {
            return Err(Error::InvalidBitstream("slice stream larger than allowed"));
        }
        let s = data.get(*pos..*pos + len).ok_or(Error::Truncated)?;
        *pos += len;
        v.push(s);
    }
    Ok(v)
}

/// Parses a compressed frame (port of `VMX_LoadFrom`).
pub(crate) fn parse<'a>(layout: &Layout, data: &'a [u8]) -> Result<Container<'a>, Error> {
    if data.len() < 5 {
        return Err(Error::Truncated);
    }
    let (offset, dc_shift) = match data[0] {
        FORMAT_PROGRESSIVE | FORMAT_INTERLACED => (0, 0u32),
        FORMAT_EXTENDED => (2, data[1] as u32),
        _ => return Err(Error::InvalidBitstream("unknown codec format byte")),
    };
    if dc_shift > 15 {
        return Err(Error::InvalidBitstream("DC shift out of range"));
    }
    let format = data[offset] as i32 - 1;
    let quality = data[offset + 1] as i32;
    let slices = data[offset + 2] as usize;
    // The slice count is stored in one byte: 8K (270 slices) wraps to 14.
    if slices != layout.slices & 0xFF {
        return Err(Error::SliceCountMismatch { expected: layout.slices, found: slices });
    }
    let mut pos = 3 + offset;
    let dc = read_streams(data, &mut pos, layout.slices, layout.max_dc_len())?;
    let ac = if pos < data.len() {
        read_streams(data, &mut pos, layout.slices, layout.max_ac_len())?
    } else {
        Vec::new()
    };
    Ok(Container { interlaced: format != 0 && layout.interlace_capable(), quality, dc_shift, dc, ac })
}

/// Writes a compressed frame (port of `VMX_SaveTo`).
pub(crate) fn write(
    out: &mut Vec<u8>,
    layout: &Layout,
    interlaced: bool,
    quality: i32,
    dc_shift: u32,
    dc: &[Vec<u8>],
    ac: &[Vec<u8>],
) {
    let total: usize = dc.iter().chain(ac).map(|s| s.len() + 4).sum();
    out.reserve(total + 5);
    let fmt = if interlaced { FORMAT_INTERLACED } else { FORMAT_PROGRESSIVE };
    if dc_shift > 0 {
        out.extend_from_slice(&[FORMAT_EXTENDED, dc_shift as u8]);
    }
    out.extend_from_slice(&[fmt, quality as u8, (layout.slices & 0xFF) as u8]);
    for s in dc.iter().chain(ac) {
        out.extend_from_slice(&(s.len() as u32).to_le_bytes());
        out.extend_from_slice(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_presets_match_libvmx() {
        assert_eq!(quality_preset(80), (80, 13));
        assert_eq!(quality_preset(99), (99, 0));
        assert_eq!(quality_preset(98), (98, 1));
        assert_eq!(quality_preset(81), (80, 13));
        assert_eq!(quality_preset(52), (52, 21));
        assert_eq!(quality_preset(30), (30, 0));
    }

    #[test]
    fn interlaced_rows() {
        let l = Layout::new(1920, 1080).unwrap();
        assert_eq!(l.aligned_height, 1088);
        assert_eq!(l.image_row(0, true), Some(0));
        assert_eq!(l.image_row(539, true), Some(1078));
        assert_eq!(l.image_row(540, true), None);
        assert_eq!(l.image_row(544, true), Some(1));
        assert_eq!(l.image_row(1083, true), Some(1079));
        assert_eq!(l.image_row(1084, true), None);
    }
}
