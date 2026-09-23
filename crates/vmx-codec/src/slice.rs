//! Entropy coding of one plane within one 16-line slice.
//!
//! Port of `VMX_EncodePlaneInternal128*` / `VMX_DecodePlaneInternal128*` from
//! libvmx (MIT, Open Media Transport Contributors).
//!
//! Each plane of a slice is split into 8x8 blocks, left to right, top row of
//! blocks first, across the whole (8-aligned) stride. Per block:
//!
//! * the DC coefficient, optionally reduced by `dc_shift`, is coded as the
//!   difference to the previous block's DC in the **DC stream**: `11` for no
//!   change, otherwise a value code of the mapped difference;
//! * the 63 AC coefficients, in zig-zag order, go to the **AC stream**, which
//!   treats the plane as one long coefficient sequence (the DC slot of every
//!   block counts as a zero): each maximal run of zeros is a run code, each
//!   non-zero coefficient a value code. Runs span block boundaries.
//!
//! Both streams are padded with zero bits to a byte boundary after each plane.

use crate::bits::{from_code, to_code, AcSymbol, BitReader, BitWriter, Corrupt};
use crate::dct::{broadcast_dc8, fdct_quant_zig, idct16, idct8, Depth};
use crate::lanes::Isa;
use crate::tables::ZIGZAG;

/// A sample type the planes can hold.
pub(crate) trait Sample: Copy + Send + Sync {
    const DEPTH: Depth;
    /// Widens to the value the transform sees (10-bit samples drop the 6 low bits).
    fn lane(self) -> i16;
}

impl Sample for u8 {
    const DEPTH: Depth = Depth::Eight;
    #[inline(always)]
    fn lane(self) -> i16 {
        self as i16
    }
}

impl Sample for u16 {
    const DEPTH: Depth = Depth::Ten;
    #[inline(always)]
    fn lane(self) -> i16 {
        (self >> 6) as i16
    }
}

/// Level shift for plane `p`: luma and alpha are centred, chroma is not.
pub(crate) fn level_shift(p: usize, depth: Depth) -> i16 {
    if p == 0 || p == 3 {
        match depth {
            Depth::Eight => 128,
            Depth::Ten => 512,
        }
    } else {
        0
    }
}

/// Encodes one plane of one slice. `rows` holds the 16 slice rows of the
/// plane (`16 * stride` samples).
pub(crate) fn encode_plane<T: Sample, S: Isa>(
    rows: &[T],
    stride: usize,
    shift: i16,
    matrix: &[u16; 192],
    dc_shift: u32,
    dc: &mut BitWriter,
    ac: &mut BitWriter,
) {
    let dc_round: i16 = if dc_shift > 0 { 1 << (dc_shift - 1) } else { 0 };
    let mut dc_pred: i16 = 0;
    let mut run: u32 = 0;
    for by in 0..2 {
        let base = by * 8 * stride;
        for bx in (0..stride).step_by(8) {
            let mut block = [[0i16; 8]; 8];
            for (k, row) in block.iter_mut().enumerate() {
                let src = &rows[base + k * stride + bx..base + k * stride + bx + 8];
                for (d, s) in row.iter_mut().zip(src) {
                    *d = s.lane();
                }
            }
            let zz = fdct_quant_zig::<S>(&block, T::DEPTH, matrix, -shift);

            let d = zz[0].wrapping_add(dc_round) >> dc_shift;
            let diff = d as i32 - dc_pred as i32;
            if diff == 0 {
                dc.put(0b11, 2);
            } else {
                dc.put_value(to_code(diff));
            }
            dc_pred = d;

            // Walk the non-zero coefficients only, as libvmx does with its
            // movemask + tzcnt loop. The DC slot (bit 0) counts as a zero in
            // the AC sequence.
            let mut nz = nonzero_mask(&zz) & !1;
            let mut pos = 0;
            while nz != 0 {
                let i = nz.trailing_zeros();
                run += i - pos;
                ac.put_run_value(run, to_code(zz[i as usize] as i32));
                run = 0;
                pos = i + 1;
                nz &= nz - 1;
            }
            run += 64 - pos;
        }
    }
    ac.put_run(run);
    ac.align();
    dc.align();
}

/// Bit `i` is set when `zz[i]` is non-zero.
#[inline(always)]
fn nonzero_mask(zz: &[i16; 64]) -> u64 {
    let mut m = 0u64;
    for (i, &c) in zz.iter().enumerate() {
        m |= ((c != 0) as u64) << i;
    }
    m
}

/// Decodes one plane of one slice into `rows` (`16 * stride` samples).
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_plane<T: Sample + DecodeOut, S: Isa>(
    rows: &mut [T],
    stride: usize,
    shift: i16,
    matrix: &[u16; 64],
    dc_shift: u32,
    dc: &mut BitReader,
    ac: &mut BitReader,
) -> Result<(), Corrupt> {
    let mut dc_pred: i16 = 0;
    let mut pending: u64 = 0;
    for by in 0..2 {
        let base = by * 8 * stride;
        for bx in (0..stride).step_by(8) {
            let mut block = [0i16; 64];
            let has_ac = pending < 64;
            // Coefficients go straight to their natural (de-zig-zagged) slot.
            while pending < 64 {
                match ac.ac_symbol()? {
                    AcSymbol::Run(n) => pending += n,
                    AcSymbol::Value(v) => {
                        block[ZIGZAG[pending as usize]] = from_code(v);
                        pending += 1;
                    }
                }
            }
            pending -= 64;

            if dc.bit() == 1 {
                dc.bit();
            } else {
                let v = dc.code_tail()?;
                block[0] = ((from_code(v) as i32) << dc_shift) as i16;
            }
            block[0] = block[0].wrapping_add(dc_pred);
            dc_pred = block[0];

            T::write_block::<S>(&block, has_ac, matrix, &mut rows[base + bx..], stride, shift);
        }
    }
    ac.align();
    dc.align();
    Ok(())
}

/// Writes one reconstructed block (depth-specific).
pub(crate) trait DecodeOut: Sized {
    fn write_block<S: Isa>(
        block: &[i16; 64],
        has_ac: bool,
        matrix: &[u16; 64],
        dst: &mut [Self],
        stride: usize,
        shift: i16,
    );
}

impl DecodeOut for u8 {
    #[inline(always)]
    fn write_block<S: Isa>(
        block: &[i16; 64],
        has_ac: bool,
        matrix: &[u16; 64],
        dst: &mut [u8],
        stride: usize,
        shift: i16,
    ) {
        if has_ac {
            idct8::<S>(block, matrix, dst, stride, shift);
        } else {
            broadcast_dc8(block[0], dst, stride, shift);
        }
    }
}

impl DecodeOut for u16 {
    #[inline(always)]
    fn write_block<S: Isa>(
        block: &[i16; 64],
        _has_ac: bool,
        matrix: &[u16; 64],
        dst: &mut [u16],
        stride: usize,
        shift: i16,
    ) {
        // libvmx always runs the full inverse transform on 10-bit planes.
        idct16::<S>(block, matrix, dst, stride, shift);
    }
}

/// Decodes the DC-only 1/8 preview of one plane of one slice: two rows of
/// `stride / 8` samples written to `out0` and `out1`.
pub(crate) fn decode_plane_preview(
    out0: &mut [u8],
    out1: &mut [u8],
    stride: usize,
    shift: i16,
    dc_shift: u32,
    dc: &mut BitReader,
) -> Result<(), Corrupt> {
    let mut dc_pred: i16 = 0;
    for out in [out0, out1] {
        for o in out.iter_mut().take(stride >> 3) {
            let mut d: i16 = 0;
            if dc.bit() == 1 {
                dc.bit();
            } else {
                let v = dc.code_tail()?;
                d = ((from_code(v) as i32) << dc_shift) as i16;
            }
            d = d.wrapping_add(dc_pred);
            dc_pred = d;
            let p = (d.wrapping_add(4) >> 3).wrapping_add(shift);
            *o = p as u8;
        }
    }
    dc.align();
    Ok(())
}
