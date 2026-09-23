//! Test pattern generation (8-bit UYVY or 10-bit P216) and snapshot writing
//! (8-bit BMP, or 8/16-bit PNG).

use std::io::{self, Write};

use open_media_transport::media::{VideoFormat, VideoFrame};

/// 75% colour bars, BT.709 limited range, 10-bit, as (Y, Cb, Cr): white,
/// yellow, cyan, green, magenta, red, blue, black. The 8-bit pattern uses
/// these divided by 4.
const BARS: [(u16, u16, u16); 8] = [
    (720, 512, 512),
    (672, 176, 544),
    (580, 588, 176),
    (532, 252, 208),
    (252, 772, 816),
    (204, 436, 848),
    (112, 848, 480),
    (64, 512, 512),
];

/// The 10-bit (Y, Cb, Cr) of each 2-pixel pair of row `y`: colour bars, a
/// grey ramp in the lower third, and a box that moves across once every two
/// seconds. During the first 100 ms of every second the box is white (and
/// the audio beeps), so A/V sync can be checked by eye. The ramp runs over
/// all 877 10-bit levels, so an 8-bit path shows banding a 10-bit one does not.
fn pattern_row(
    w: usize,
    h: usize,
    y: usize,
    frame: u64,
    fps: f64,
    mut put: impl FnMut(usize, u16, u16, u16),
) {
    let t = frame as f64 / fps;
    let flash = t.fract() < 0.1;
    let box_w = (w / 12).max(2) & !1;
    let box_h = (h / 6).max(2);
    let x0 = (((t / 2.0).fract() * (w - box_w) as f64) as usize) & !1;
    let y0 = h * 3 / 4 - box_h / 2;
    for x in (0..w).step_by(2) {
        let in_box = y >= y0 && y < y0 + box_h && x >= x0 && x < x0 + box_w;
        let (yy, cb, cr) = if in_box {
            if flash {
                (940, 512, 512)
            } else {
                (64, 512, 512)
            }
        } else if y < h * 2 / 3 {
            BARS[x * 8 / w]
        } else {
            ((64 + x * 876 / w) as u16, 512, 512)
        };
        put(x, yy, cb, cr);
    }
}

/// Fills a tightly packed 8-bit UYVY buffer with the test pattern.
pub fn fill_test_pattern(buf: &mut [u8], w: usize, h: usize, frame: u64, fps: f64) {
    for y in 0..h {
        let row = &mut buf[y * w * 2..(y + 1) * w * 2];
        pattern_row(w, h, y, frame, fps, |x, yy, cb, cr| {
            let (yy, cb, cr) = ((yy >> 2) as u8, (cb >> 2) as u8, (cr >> 2) as u8);
            row[x * 2..x * 2 + 4].copy_from_slice(&[cb, yy, cr, yy]);
        });
    }
}

/// Fills tightly packed P216 planes (16-bit little-endian, 10 significant
/// bits at the top) with the test pattern.
pub fn fill_test_pattern_p216(
    luma: &mut [u8],
    chroma: &mut [u8],
    w: usize,
    h: usize,
    frame: u64,
    fps: f64,
) {
    let word = |v: u16| (v << 6).to_le_bytes();
    for y in 0..h {
        let yr = &mut luma[y * w * 2..(y + 1) * w * 2];
        let cr_row = &mut chroma[y * w * 2..(y + 1) * w * 2];
        pattern_row(w, h, y, frame, fps, |x, yy, cb, cr| {
            yr[x * 2..x * 2 + 2].copy_from_slice(&word(yy));
            yr[x * 2 + 2..x * 2 + 4].copy_from_slice(&word(yy));
            cr_row[x * 2..x * 2 + 2].copy_from_slice(&word(cb));
            cr_row[x * 2 + 2..x * 2 + 4].copy_from_slice(&word(cr));
        });
    }
}

/// A picture converted to RGB(A), each sample in 0.0..=1.0.
pub struct Rgb {
    pub width: usize,
    pub height: usize,
    /// 4 samples per pixel instead of 3.
    pub alpha: bool,
    /// The source had more than 8 bits per sample.
    pub high_bit_depth: bool,
    pub samples: Vec<f32>,
}

/// Converts a decoded frame to RGB with the limited-range BT.709 or BT.601
/// matrix the frame calls for ([`VideoFrame::is_bt601`]).
pub fn to_rgb(v: &VideoFrame) -> Rgb {
    let (w, h) = (v.width, v.height);
    let (kr, kb) = if v.is_bt601() {
        (0.299, 0.114)
    } else {
        (0.2126, 0.0722)
    };
    let kg = 1.0 - kr - kb;
    let planes = v.planes();
    let alpha = matches!(
        v.format,
        VideoFormat::Uyva | VideoFormat::Pa16 | VideoFormat::Bgra
    );
    let mut samples = Vec::with_capacity(w * h * if alpha { 4 } else { 3 });
    let le16 = |p: &[u8], i: usize| (u16::from_le_bytes([p[2 * i], p[2 * i + 1]]) >> 6) as f32;
    for y in 0..h {
        for x in 0..w {
            // (Y, Cb, Cr) normalised to 0..1 and -0.5..0.5, then alpha.
            let (yy, cb, cr, a) = match v.format {
                VideoFormat::Uyvy | VideoFormat::Uyva => {
                    let p = &planes[0][y * w * 2 + (x & !1) * 2..][..4];
                    let a = if v.format == VideoFormat::Uyva {
                        planes[1][y * w + x] as f32 / 255.0
                    } else {
                        1.0
                    };
                    let yy = p[1 + (x & 1) * 2] as f32;
                    (
                        (yy - 16.0) / 219.0,
                        (p[0] as f32 - 128.0) / 224.0,
                        (p[2] as f32 - 128.0) / 224.0,
                        a,
                    )
                }
                VideoFormat::P216 | VideoFormat::Pa16 => {
                    let i = y * w + x;
                    let c = y * w + (x & !1);
                    let a = if v.format == VideoFormat::Pa16 {
                        le16(planes[2], i) / 1023.0
                    } else {
                        1.0
                    };
                    (
                        (le16(planes[0], i) - 64.0) / 876.0,
                        (le16(planes[1], c) - 512.0) / 896.0,
                        (le16(planes[1], c + 1) - 512.0) / 896.0,
                        a,
                    )
                }
                VideoFormat::Bgra => {
                    let p = &planes[0][(y * w + x) * 4..][..4];
                    let f = |b: u8| b as f32 / 255.0;
                    samples.extend_from_slice(&[f(p[2]), f(p[1]), f(p[0]), f(p[3])]);
                    continue;
                }
            };
            let r = yy + 2.0 * (1.0 - kr) * cr;
            let b = yy + 2.0 * (1.0 - kb) * cb;
            let g = (yy - kr * r - kb * b) / kg;
            samples.extend_from_slice(&[r, g, b]);
            if alpha {
                samples.push(a);
            }
        }
    }
    Rgb {
        width: w,
        height: h,
        alpha,
        high_bit_depth: v.format.is_10bit(),
        samples,
    }
}

fn quantise(v: f32, max: f32) -> f32 {
    (v.clamp(0.0, 1.0) * max).round()
}

/// Writes a 24-bit BMP (alpha, if any, is dropped; depth is cut to 8 bits).
pub fn write_bmp(out: &mut impl Write, img: &Rgb) -> io::Result<()> {
    let (w, h) = (img.width, img.height);
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

    let n = if img.alpha { 4 } else { 3 };
    let mut row = vec![0u8; row_len];
    for y in (0..h).rev() {
        for x in 0..w {
            let p = &img.samples[(y * w + x) * n..][..3];
            row[x * 3] = quantise(p[2], 255.0) as u8;
            row[x * 3 + 1] = quantise(p[1], 255.0) as u8;
            row[x * 3 + 2] = quantise(p[0], 255.0) as u8;
        }
        out.write_all(&row)?;
    }
    Ok(())
}

/// Writes a PNG: 16 bits per sample for high-bit-depth sources, else 8; RGBA
/// when the source has alpha.
pub fn write_png(out: impl Write, img: &Rgb) -> Result<(), png::EncodingError> {
    let mut enc = png::Encoder::new(out, img.width as u32, img.height as u32);
    enc.set_color(if img.alpha {
        png::ColorType::Rgba
    } else {
        png::ColorType::Rgb
    });
    let data: Vec<u8> = if img.high_bit_depth {
        enc.set_depth(png::BitDepth::Sixteen);
        // PNG stores 16-bit samples big-endian.
        img.samples
            .iter()
            .flat_map(|&s| (quantise(s, 65535.0) as u16).to_be_bytes())
            .collect()
    } else {
        enc.set_depth(png::BitDepth::Eight);
        img.samples
            .iter()
            .map(|&s| quantise(s, 255.0) as u8)
            .collect()
    };
    let mut writer = enc.write_header()?;
    writer.write_image_data(&data)?;
    writer.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_media_transport::frame::VideoFlags;

    fn frame(format: VideoFormat, w: usize, h: usize, data: Vec<u8>) -> VideoFrame {
        VideoFrame {
            width: w,
            height: h,
            format,
            stride: format.stride(w),
            data,
            color_space: 709,
            flags: VideoFlags::default(),
            ..VideoFrame::default()
        }
    }

    #[test]
    fn bmp_of_white_is_white() {
        let (w, h) = (4, 2);
        let uyvy: Vec<u8> = [128, 235, 128, 235].repeat(w / 2 * h);
        let mut out = Vec::new();
        write_bmp(&mut out, &to_rgb(&frame(VideoFormat::Uyvy, w, h, uyvy))).unwrap();
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

    #[test]
    fn ten_bit_pattern_and_png_keep_more_than_eight_bits() {
        let (w, h) = (1024, 48);
        let mut luma = vec![0u8; w * 2 * h];
        let mut chroma = luma.clone();
        fill_test_pattern_p216(&mut luma, &mut chroma, w, h, 0, 30.0);
        assert_eq!(
            u16::from_le_bytes([luma[0], luma[1]]) >> 6,
            720,
            "white bar"
        );
        let mut data = luma;
        data.extend_from_slice(&chroma);
        let img = to_rgb(&frame(VideoFormat::P216, w, h, data));
        assert!(img.high_bit_depth && !img.alpha);

        let mut png_bytes = Vec::new();
        write_png(&mut png_bytes, &img).unwrap();
        let mut reader = png::Decoder::new(std::io::Cursor::new(png_bytes))
            .read_info()
            .unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (w as u32, h as u32));
        assert_eq!(
            (info.color_type, info.bit_depth),
            (png::ColorType::Rgb, png::BitDepth::Sixteen)
        );
        // The ramp in the last row: more distinct grey levels than 8 bits hold.
        let row = &buf[(h - 1) * w * 6..h * w * 6];
        let mut greens: Vec<u16> = row
            .chunks_exact(6)
            .map(|p| u16::from_be_bytes([p[2], p[3]]))
            .collect();
        greens.dedup();
        assert!(greens.len() > 256, "{} levels", greens.len());
    }

    #[test]
    fn png_is_eight_bit_rgba_for_uyva() {
        let (w, h) = (4, 2);
        let mut data: Vec<u8> = [128, 235, 128, 235].repeat(w / 2 * h);
        data.extend(std::iter::repeat(128).take(w * h));
        let mut out = Vec::new();
        write_png(&mut out, &to_rgb(&frame(VideoFormat::Uyva, w, h, data))).unwrap();
        let reader = png::Decoder::new(std::io::Cursor::new(out))
            .read_info()
            .unwrap();
        let info = reader.info();
        assert_eq!(
            (info.color_type, info.bit_depth),
            (png::ColorType::Rgba, png::BitDepth::Eight)
        );
    }
}
