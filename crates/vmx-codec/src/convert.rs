//! Conversion between [`Frame`] layouts and the internal 4:2:2(:4) planes.

use crate::frame::{Frame, PixelFormat};
use crate::layout::Layout;

/// Internal planes (Y, U, V, A), each `aligned_height` rows of `strides[p]`
/// samples.
pub(crate) struct Planes<T> {
    pub data: [Vec<T>; 4],
    pub strides: [usize; 4],
}

impl<T: Copy> Planes<T> {
    /// Allocates planes with the fill values libvmx uses (`memset` of the
    /// byte buffers with 0 / 128 / 128 / 255).
    pub(crate) fn new(layout: &Layout, fill: [T; 4]) -> Self {
        let rows = layout.aligned_height;
        let s = layout.strides;
        Self {
            data: [vec![fill[0]; s[0] * rows], vec![fill[1]; s[1] * rows], vec![fill[2]; s[2] * rows], vec![fill[3]; s[3] * rows]],
            strides: s,
        }
    }

    #[inline]
    fn row_mut(&mut self, p: usize, r: usize, len: usize) -> &mut [T] {
        let s = self.strides[p];
        &mut self.data[p][r * s..r * s + len]
    }

    #[inline]
    fn row(&self, p: usize, r: usize, len: usize) -> &[T] {
        let s = self.strides[p];
        &self.data[p][r * s..r * s + len]
    }
}

pub(crate) const FILL8: [u8; 4] = [0, 128, 128, 255];
pub(crate) const FILL16: [u16; 4] = [0, 0x8080, 0x8080, 0xFFFF];

#[inline]
fn le16(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[2 * i], b[2 * i + 1]])
}

/// Copies an 8-bit frame into the planes. Padding rows and columns keep
/// whatever the planes held before, as in libvmx.
pub(crate) fn frame_to_planes8(f: &Frame, l: &Layout, interlaced: bool, pl: &mut Planes<u8>) {
    let (w, cw) = (l.width, l.width / 2);
    for r in 0..l.aligned_height {
        let Some(ir) = l.image_row(r, interlaced) else { continue };
        match f.format {
            PixelFormat::Uyvy | PixelFormat::Yuy2 | PixelFormat::Uyva => {
                let src = f.row(0, ir, 2 * w);
                let (yo, uo, vo) = if f.format == PixelFormat::Yuy2 { (0, 1, 3) } else { (1, 0, 2) };
                {
                    let y = pl.row_mut(0, r, w);
                    for i in 0..cw {
                        y[2 * i] = src[4 * i + yo];
                        y[2 * i + 1] = src[4 * i + yo + 2];
                    }
                }
                {
                    let u = pl.row_mut(1, r, cw);
                    for i in 0..cw {
                        u[i] = src[4 * i + uo];
                    }
                }
                {
                    let v = pl.row_mut(2, r, cw);
                    for i in 0..cw {
                        v[i] = src[4 * i + vo];
                    }
                }
                if f.format == PixelFormat::Uyva {
                    pl.row_mut(3, r, w).copy_from_slice(f.row(1, ir, w));
                }
            }
            PixelFormat::Yuv422p | PixelFormat::Yuva422p => {
                pl.row_mut(0, r, w).copy_from_slice(f.row(0, ir, w));
                pl.row_mut(1, r, cw).copy_from_slice(f.row(1, ir, cw));
                pl.row_mut(2, r, cw).copy_from_slice(f.row(2, ir, cw));
                if f.format == PixelFormat::Yuva422p {
                    pl.row_mut(3, r, w).copy_from_slice(f.row(3, ir, w));
                }
            }
            PixelFormat::Nv12 => {
                pl.row_mut(0, r, w).copy_from_slice(f.row(0, ir, w));
                let src = f.row(1, ir / 2, w);
                {
                    let u = pl.row_mut(1, r, cw);
                    for i in 0..cw {
                        u[i] = src[2 * i];
                    }
                }
                let v = pl.row_mut(2, r, cw);
                for i in 0..cw {
                    v[i] = src[2 * i + 1];
                }
            }
            PixelFormat::I420 => {
                pl.row_mut(0, r, w).copy_from_slice(f.row(0, ir, w));
                pl.row_mut(1, r, cw).copy_from_slice(f.row(1, ir / 2, cw));
                pl.row_mut(2, r, cw).copy_from_slice(f.row(2, ir / 2, cw));
            }
            PixelFormat::P216 | PixelFormat::Pa16 => unreachable!("10-bit format on the 8-bit path"),
        }
    }
}

/// Copies a 10-bit (`P216` / `Pa16`) frame into the planes.
pub(crate) fn frame_to_planes16(f: &Frame, l: &Layout, interlaced: bool, pl: &mut Planes<u16>) {
    let (w, cw) = (l.width, l.width / 2);
    for r in 0..l.aligned_height {
        let Some(ir) = l.image_row(r, interlaced) else { continue };
        let ys = f.row(0, ir, 2 * w);
        let y = pl.row_mut(0, r, w);
        for (i, d) in y.iter_mut().enumerate() {
            *d = le16(ys, i);
        }
        let uv = f.row(1, ir, 2 * w);
        {
            let u = pl.row_mut(1, r, cw);
            for (i, d) in u.iter_mut().enumerate() {
                *d = le16(uv, 2 * i);
            }
        }
        {
            let v = pl.row_mut(2, r, cw);
            for (i, d) in v.iter_mut().enumerate() {
                *d = le16(uv, 2 * i + 1);
            }
        }
        if f.format == PixelFormat::Pa16 {
            let a_src = f.row(2, ir, 2 * w);
            let a = pl.row_mut(3, r, w);
            for (i, d) in a.iter_mut().enumerate() {
                *d = le16(a_src, i);
            }
        }
    }
}

/// Writes decoded 8-bit planes into `f` (which must be allocated for its format).
pub(crate) fn planes8_to_frame(pl: &Planes<u8>, l: &Layout, interlaced: bool, f: &mut Frame) {
    let (w, cw) = (l.width, l.width / 2);
    let fmt = f.format;
    for r in 0..l.aligned_height {
        let Some(ir) = l.image_row(r, interlaced) else { continue };
        match fmt {
            PixelFormat::Uyvy | PixelFormat::Yuy2 | PixelFormat::Uyva => {
                let (yo, uo, vo) = if fmt == PixelFormat::Yuy2 { (0, 1, 3) } else { (1, 0, 2) };
                let (y, u, v) = (pl.row(0, r, w), pl.row(1, r, cw), pl.row(2, r, cw));
                let s = f.planes[0].stride;
                let dst = &mut f.planes[0].data[ir * s..ir * s + 2 * w];
                for i in 0..cw {
                    dst[4 * i + yo] = y[2 * i];
                    dst[4 * i + yo + 2] = y[2 * i + 1];
                    dst[4 * i + uo] = u[i];
                    dst[4 * i + vo] = v[i];
                }
                if fmt == PixelFormat::Uyva {
                    let s = f.planes[1].stride;
                    f.planes[1].data[ir * s..ir * s + w].copy_from_slice(pl.row(3, r, w));
                }
            }
            PixelFormat::Yuv422p | PixelFormat::Yuva422p => {
                let n = if fmt == PixelFormat::Yuva422p { 4 } else { 3 };
                for p in 0..n {
                    let len = if p == 1 || p == 2 { cw } else { w };
                    let s = f.planes[p].stride;
                    f.planes[p].data[ir * s..ir * s + len].copy_from_slice(pl.row(p, r, len));
                }
            }
            _ => unreachable!("not an 8-bit decode format"),
        }
    }
}

/// Writes decoded 10-bit planes into a `P216` / `Pa16` frame.
pub(crate) fn planes16_to_frame(pl: &Planes<u16>, l: &Layout, interlaced: bool, f: &mut Frame) {
    let (w, cw) = (l.width, l.width / 2);
    for r in 0..l.aligned_height {
        let Some(ir) = l.image_row(r, interlaced) else { continue };
        let s = f.planes[0].stride;
        let dst = &mut f.planes[0].data[ir * s..ir * s + 2 * w];
        for (i, v) in pl.row(0, r, w).iter().enumerate() {
            dst[2 * i..2 * i + 2].copy_from_slice(&v.to_le_bytes());
        }
        let (u, v) = (pl.row(1, r, cw), pl.row(2, r, cw));
        let s = f.planes[1].stride;
        let dst = &mut f.planes[1].data[ir * s..ir * s + 2 * w];
        for i in 0..cw {
            dst[4 * i..4 * i + 2].copy_from_slice(&u[i].to_le_bytes());
            dst[4 * i + 2..4 * i + 4].copy_from_slice(&v[i].to_le_bytes());
        }
        if f.format == PixelFormat::Pa16 {
            let s = f.planes[2].stride;
            let dst = &mut f.planes[2].data[ir * s..ir * s + 2 * w];
            for (i, v) in pl.row(3, r, w).iter().enumerate() {
                dst[2 * i..2 * i + 2].copy_from_slice(&v.to_le_bytes());
            }
        }
    }
}
