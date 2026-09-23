//! VMX decoder.

use crate::bits::{BitReader, Corrupt};
use crate::convert::{planes16_to_frame, planes8_to_frame, Planes, FILL16, FILL8};
use crate::dct::{Depth, QuantTables};
use crate::frame::{Frame, Plane, PixelFormat};
use crate::layout::{parse, quality_preset, Layout};
use crate::simd::Native;
use crate::slice::{decode_plane, decode_plane_preview, level_shift, DecodeOut, Sample};
use crate::tables::{QUALITY_COUNT, SLICE_HEIGHT};
use crate::Error;

/// Facts about a compressed frame read from its header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameInfo {
    /// Quality the frame was coded with.
    pub quality: i32,
    /// DC precision reduction (0 = full 11-bit DC).
    pub dc_shift: u32,
    /// Field-coded frame.
    pub interlaced: bool,
    /// `false` for a DC-only (preview) frame.
    pub has_ac: bool,
}

/// Decompresses VMX frames of one fixed size.
///
/// The VMX bitstream records neither the frame size, the bit depth nor
/// whether an alpha plane is present; the transport (for example the OMT
/// frame header) carries those. Pick the output [`PixelFormat`] accordingly:
/// `P216`/`Pa16` select the 10-bit path, formats with alpha decode the fourth
/// plane.
pub struct Decoder {
    layout: Layout,
    presets: Vec<QuantTables>,
    threads: usize,
    planes8: Option<Planes<u8>>,
    planes16: Option<Planes<u16>>,
}

impl Decoder {
    /// Creates a decoder for `width` x `height` frames.
    pub fn new(width: usize, height: usize) -> Result<Self, Error> {
        Ok(Self {
            layout: Layout::new(width, height)?,
            presets: (0..QUALITY_COUNT).map(QuantTables::new).collect(),
            threads: 1,
            planes8: None,
            planes16: None,
        })
    }

    /// Sets the number of worker threads for slice-parallel decoding.
    pub fn set_threads(&mut self, threads: usize) {
        self.threads = threads.max(1);
    }

    /// Reads the header of a compressed frame without decoding it.
    pub fn info(&self, data: &[u8]) -> Result<FrameInfo, Error> {
        let c = parse(&self.layout, data)?;
        Ok(FrameInfo {
            quality: quality_preset(c.quality).0,
            dc_shift: c.dc_shift,
            interlaced: c.interlaced,
            has_ac: !c.ac.is_empty(),
        })
    }

    /// Length of the DC-only prefix of `data` that [`Decoder::decode_preview`]
    /// needs (`VMX_GetEncodedPreviewLength`).
    ///
    /// libvmx sizes the header from the DC shift (5 bytes if non-zero, else
    /// 3), which assumes the extended header is written only with a non-zero
    /// shift — true of every stream libvmx writes. This uses the header the
    /// stream actually has, so an extended header with a shift of 0 (valid
    /// input, found by fuzzing) gets a prefix that still decodes.
    pub fn preview_len(&self, data: &[u8]) -> Result<usize, Error> {
        let c = parse(&self.layout, data)?;
        Ok(c.header_len + c.dc.iter().map(|s| s.len() + 4).sum::<usize>())
    }

    /// Decodes a frame into a newly allocated, tightly packed frame.
    pub fn decode(&mut self, data: &[u8], format: PixelFormat) -> Result<Frame, Error> {
        let mut f = Frame::new(self.layout.width, self.layout.height, format);
        self.decode_into(data, &mut f)?;
        Ok(f)
    }

    /// Decodes a frame into `out`, whose size and format must already be set
    /// (its strides are respected). Sets `out.interlaced` from the header.
    pub fn decode_into(&mut self, data: &[u8], out: &mut Frame) -> Result<(), Error> {
        let l = self.layout;
        if out.width != l.width || out.height != l.height {
            return Err(Error::FrameSizeMismatch);
        }
        if !out.format.can_decode() {
            return Err(Error::Unsupported("decoding to 4:2:0"));
        }
        out.validate()?;
        let c = parse(&l, data)?;
        if c.ac.is_empty() {
            return Err(Error::InvalidBitstream("frame has no AC data (preview only)"));
        }
        let (_, idx) = quality_preset(c.quality);
        let matrix = &self.presets[idx].decode;
        let nplanes = if out.format.has_alpha() { 4 } else { 3 };
        let streams: Vec<(&[u8], &[u8])> = c.dc.iter().copied().zip(c.ac.iter().copied()).collect();
        if out.format.is_10bit() {
            let pl = self.planes16.get_or_insert_with(|| Planes::new(&l, FILL16));
            decode_slices(pl, &streams, nplanes, matrix, c.dc_shift, self.threads)?;
            planes16_to_frame(pl, &l, c.interlaced, out);
        } else {
            let pl = self.planes8.get_or_insert_with(|| Planes::new(&l, FILL8));
            decode_slices(pl, &streams, nplanes, matrix, c.dc_shift, self.threads)?;
            planes8_to_frame(pl, &l, c.interlaced, out);
        }
        out.interlaced = c.interlaced;
        Ok(())
    }

    /// Decodes the DC-only preview at 1/8 scale (`VMX_DecodePreview*`) as
    /// `Yuv422p` (or `Yuva422p` when `alpha` is set). Only the DC part of
    /// the frame is read, so a truncated frame of
    /// [`Decoder::preview_len`] bytes is enough.
    ///
    /// Preview width is `width / 8` rounded up to even, height `height / 8`
    /// (made even for interlaced frames).
    pub fn decode_preview(&mut self, data: &[u8], alpha: bool) -> Result<Frame, Error> {
        let l = self.layout;
        let c = parse(&l, data)?;
        let nplanes = if alpha { 4 } else { 3 };
        // Preview planes: two rows of stride/8 samples per slice.
        let prow = l.slices * 2;
        let pstride: Vec<usize> = l.strides.iter().map(|s| s >> 3).collect();
        let mut pp: Vec<Vec<u8>> = (0..4).map(|p| vec![FILL8[p]; pstride[p].max(1) * prow]).collect();
        for (s, dc) in c.dc.iter().enumerate() {
            let mut r = BitReader::new(dc);
            for p in 0..nplanes {
                let ps = pstride[p];
                let (a, b) = pp[p][s * 2 * ps..(s * 2 + 2) * ps].split_at_mut(ps);
                decode_plane_preview(a, b, l.strides[p], level_shift(p, Depth::Eight), c.dc_shift, &mut r)
                    .map_err(|Corrupt| Error::InvalidBitstream("corrupt DC stream"))?;
            }
        }
        let (pw, ph) = l.preview_size(c.interlaced);
        let format = if alpha { PixelFormat::Yuva422p } else { PixelFormat::Yuv422p };
        let mut f = Frame::new(pw, ph, format);
        f.interlaced = c.interlaced;
        for p in 0..nplanes {
            let w = if p == 1 || p == 2 { pw / 2 } else { pw };
            let ps = pstride[p];
            let Plane { data, stride } = &mut f.planes[p];
            for y in 0..ph {
                let src_row = if c.interlaced {
                    let half = l.aligned_height >> 4;
                    if y % 2 == 0 {
                        y / 2
                    } else {
                        half + y / 2
                    }
                } else {
                    y
                };
                let src = &pp[p][src_row * ps..];
                // libvmx reads past the preview row when the preview is
                // wider than stride/8 (it pads the width to even); use the
                // fill value there.
                for x in 0..w {
                    data[y * *stride + x] = src.get(x).copied().filter(|_| x < ps).unwrap_or(FILL8[p]);
                }
            }
        }
        Ok(f)
    }
}

type SliceStreams<'a> = (&'a [u8], &'a [u8]);

fn decode_one_slice<T: Sample + DecodeOut>(
    rows: &mut [&mut [T]],
    strides: &[usize; 4],
    (dc, ac): SliceStreams,
    matrix: &[u16; 64],
    dc_shift: u32,
) -> Result<(), Error> {
    let mut dc = BitReader::new(dc);
    let mut ac = BitReader::new(ac);
    for (p, r) in rows.iter_mut().enumerate() {
        decode_plane::<T, Native>(r, strides[p], level_shift(p, T::DEPTH), matrix, dc_shift, &mut dc, &mut ac)
            .map_err(|Corrupt| Error::InvalidBitstream("corrupt slice stream"))?;
    }
    Ok(())
}

fn decode_slices<T: Sample + DecodeOut>(
    pl: &mut Planes<T>,
    streams: &[SliceStreams],
    nplanes: usize,
    matrix: &[u16; 64],
    dc_shift: u32,
    threads: usize,
) -> Result<(), Error> {
    let strides = pl.strides;
    // One entry per slice: the slice's rows of each coded plane.
    let mut per_slice: Vec<Vec<&mut [T]>> = (0..streams.len()).map(|_| Vec::with_capacity(4)).collect();
    for (p, data) in pl.data.iter_mut().enumerate().take(nplanes) {
        for (s, chunk) in data.chunks_mut(strides[p] * SLICE_HEIGHT).enumerate() {
            if let Some(v) = per_slice.get_mut(s) {
                v.push(chunk);
            }
        }
    }
    let n = per_slice.len();
    if threads <= 1 || n < 2 {
        for (rows, st) in per_slice.iter_mut().zip(streams) {
            decode_one_slice(rows, &strides, *st, matrix, dc_shift)?;
        }
        return Ok(());
    }
    let per = n.div_ceil(threads.min(n));
    std::thread::scope(|scope| {
        let handles: Vec<_> = per_slice
            .chunks_mut(per)
            .zip(streams.chunks(per))
            .map(|(rows, st)| {
                scope.spawn(move || {
                    for (r, s) in rows.iter_mut().zip(st) {
                        decode_one_slice(r, &strides, *s, matrix, dc_shift)?;
                    }
                    Ok::<(), Error>(())
                })
            })
            .collect();
        handles.into_iter().try_for_each(|h| h.join().expect("decoder worker panicked"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Found by the `vmx_decode` fuzz target (docs/evidence/2026-09-23-m12-prereqs):
    /// an extended header with a DC shift of 0. The preview prefix must cover
    /// the 5-byte header, or it decodes differently from the whole frame.
    #[test]
    fn preview_len_counts_extended_header_with_zero_shift() {
        let stream = [0x03, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00];
        let mut d = Decoder::new(32, 16).unwrap();
        let n = d.preview_len(&stream).unwrap();
        assert!(n <= stream.len());
        let full = d.decode_preview(&stream, false);
        let prefix = d.decode_preview(&stream[..n], false);
        assert_eq!(full.is_ok(), prefix.is_ok(), "{full:?} / {prefix:?}");
        if let (Ok(a), Ok(b)) = (full, prefix) {
            assert_eq!(a.planes, b.planes);
        }
    }
}
