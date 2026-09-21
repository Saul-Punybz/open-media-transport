//! Test pattern generation and snapshot writing, in 8-bit UYVY.

use std::io::{self, Write};

/// 75% colour bars, BT.709 limited range, as (Y, Cb, Cr): white, yellow,
/// cyan, green, magenta, red, blue, black.
const BARS: [(u8, u8, u8); 8] = [
    (180, 128, 128),
    (168, 44, 136),
    (145, 147, 44),
    (133, 63, 52),
    (63, 193, 204),
    (51, 109, 212),
    (28, 212, 120),
    (16, 128, 128),
];

/// Fills a tightly packed UYVY buffer with colour bars and a box that moves
/// across once every two seconds. During the first 100 ms of every second
/// the box is white (and the audio beeps), so A/V sync can be checked by eye.
pub fn fill_test_pattern(buf: &mut [u8], w: usize, h: usize, frame: u64, fps: f64) {
    let t = frame as f64 / fps;
    let flash = t.fract() < 0.1;
    let box_w = (w / 12).max(2) & !1;
    let box_h = (h / 6).max(2);
    let x0 = (((t / 2.0).fract() * (w - box_w) as f64) as usize) & !1;
    let y0 = h * 3 / 4 - box_h / 2;
    for y in 0..h {
        let row = &mut buf[y * w * 2..(y + 1) * w * 2];
        for x in (0..w).step_by(2) {
            let in_box = y >= y0 && y < y0 + box_h && x >= x0 && x < x0 + box_w;
            let (yy, cb, cr) = if in_box {
                if flash {
                    (235, 128, 128)
                } else {
                    (16, 128, 128)
                }
            } else if y < h * 2 / 3 {
                BARS[x * 8 / w]
            } else {
                // Lower third: a grey ramp, useful for spotting banding.
                ((16 + x * 219 / w) as u8, 128, 128)
            };
            row[x * 2..x * 2 + 4].copy_from_slice(&[cb, yy, cr, yy]);
        }
    }
}

/// Writes a UYVY frame as a 24-bit BMP, converting with BT.709 (or BT.601
/// when `bt601`) limited-range coefficients.
pub fn write_bmp(
    out: &mut impl Write,
    uyvy: &[u8],
    stride: usize,
    w: usize,
    h: usize,
    bt601: bool,
) -> io::Result<()> {
    let row_len = (w * 3 + 3) & !3;
    let size = 54 + row_len * h;
    let mut hdr = Vec::with_capacity(54);
    hdr.extend_from_slice(b"BM");
    hdr.extend_from_slice(&(size as u32).to_le_bytes());
    hdr.extend_from_slice(&[0; 4]);
    hdr.extend_from_slice(&54u32.to_le_bytes());
    hdr.extend_from_slice(&40u32.to_le_bytes());
    hdr.extend_from_slice(&(w as i32).to_le_bytes());
    hdr.extend_from_slice(&(h as i32).to_le_bytes()); // bottom-up
    hdr.extend_from_slice(&1u16.to_le_bytes());
    hdr.extend_from_slice(&24u16.to_le_bytes());
    hdr.extend_from_slice(&[0; 24]);
    out.write_all(&hdr)?;

    let (rv, gu, gv, bu) = if bt601 {
        (1.596, 0.392, 0.813, 2.017)
    } else {
        (1.793, 0.213, 0.533, 2.112)
    };
    let mut row = vec![0u8; row_len];
    for y in (0..h).rev() {
        let src = &uyvy[y * stride..y * stride + w * 2];
        for x in 0..w {
            let pair = &src[(x & !1) * 2..(x & !1) * 2 + 4];
            let (u, yy, v) = (pair[0], pair[1 + (x & 1) * 2], pair[2]);
            let c = 1.164 * (yy as f32 - 16.0);
            let (d, e) = (u as f32 - 128.0, v as f32 - 128.0);
            let clamp = |f: f32| f.round().clamp(0.0, 255.0) as u8;
            row[x * 3] = clamp(c + bu * d);
            row[x * 3 + 1] = clamp(c - gu * d - gv * e);
            row[x * 3 + 2] = clamp(c + rv * e);
        }
        out.write_all(&row)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bmp_of_white_is_white() {
        let (w, h) = (4, 2);
        let uyvy: Vec<u8> = [128, 235, 128, 235].repeat(w / 2 * h);
        let mut out = Vec::new();
        write_bmp(&mut out, &uyvy, w * 2, w, h, false).unwrap();
        assert_eq!(&out[..2], b"BM");
        assert_eq!(out.len(), 54 + 12 * 2);
        assert!(out[54..54 + 12].iter().all(|&b| b == 255));
    }

    #[test]
    fn pattern_has_bars_and_a_moving_box() {
        let (w, h) = (320, 180);
        let mut a = vec![0u8; w * 2 * h];
        let mut b = a.clone();
        fill_test_pattern(&mut a, w, h, 5, 30.0);
        fill_test_pattern(&mut b, w, h, 20, 30.0);
        assert_eq!(a[1], 180, "white bar at top left");
        assert_ne!(a, b, "the box moves");
    }
}
