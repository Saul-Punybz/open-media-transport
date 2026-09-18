//! Conformance against the upstream libvmx C++ implementation (128-bit path).
//!
//! For every case we check three things:
//! 1. the Rust encoder produces exactly the same bytes as libvmx,
//! 2. the Rust decoder reproduces libvmx's decoded pixels exactly from a
//!    libvmx-encoded frame,
//! 3. libvmx decodes the Rust-encoded frame to exactly the pixels the Rust
//!    decoder produces.
//!
//! The reference is built by the `libvmx-ref` dev-dependency from
//! `reference/libvmx` (gitignored). Without it these tests print a notice and
//! pass vacuously.
//!
//! Test widths are multiples of 16 (or of 32 where the C conversion code
//! would otherwise read uninitialised padding): for other widths libvmx's
//! packed-to-planar conversion leaves undefined bytes in the plane padding,
//! which makes its own output non-deterministic.

use libvmx_ref::{profile, RefCodec, RefFormat};
use vmx_codec::{Decoder, Encoder, EncoderConfig, Frame, PixelFormat, Plane, Profile};

fn available() -> bool {
    if !libvmx_ref::AVAILABLE {
        eprintln!("libvmx reference not built (reference/libvmx missing): skipping conformance test");
    }
    libvmx_ref::AVAILABLE
}

#[derive(Clone, Copy, Debug)]
enum Pattern {
    Gradient,
    Noise,
    Bars,
    Mixed,
}

/// 10-bit Y, U, V, A sample at (x, y) for chroma column `x / 2`.
fn sample(p: Pattern, x: usize, y: usize, w: usize, h: usize, seed: &mut u64) -> [u16; 4] {
    let mut rnd = || {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (*seed >> 33) as u32
    };
    match p {
        Pattern::Gradient => {
            let yv = (x * 1023 / w.max(1) + y * 511 / h.max(1)).min(1023);
            let u = (y * 1023 / h.max(1)) as u16;
            let v = (1023 - x * 1023 / w.max(1)) as u16;
            let a = ((x + y) * 1023 / (w + h)) as u16;
            [yv as u16, u, v, a]
        }
        Pattern::Noise => [
            (rnd() % 1024) as u16,
            (rnd() % 1024) as u16,
            (rnd() % 1024) as u16,
            (rnd() % 1024) as u16,
        ],
        Pattern::Bars => {
            // 75% colour bars (BT.709, 10-bit): white yellow cyan green magenta red blue black
            const BARS: [[u16; 3]; 8] = [
                [721, 512, 512],
                [674, 176, 543],
                [581, 589, 176],
                [534, 253, 207],
                [251, 771, 817],
                [204, 435, 848],
                [111, 848, 481],
                [64, 512, 512],
            ];
            let b = BARS[(x * 8 / w).min(7)];
            let a = if y < h / 2 { 1023 } else { 0 };
            [b[0], b[1], b[2], a]
        }
        Pattern::Mixed => {
            let checker = ((x / 3) + (y / 5)) % 2 == 0;
            let n = (rnd() % 64) as usize;
            let yv = if x % 16 < 8 { (x * 7 + y * 3 + n) % 1024 } else if checker { 1023 } else { 0 };
            [yv as u16, ((y * 13 + n) % 1024) as u16, ((x * 11) % 1024) as u16, (n * 16) as u16]
        }
    }
}

/// Builds a tightly packed buffer in the libvmx layout for `fmt`.
fn make_buffer(fmt: RefFormat, p: Pattern, w: usize, h: usize, seed: u64) -> Vec<u8> {
    let mut s = seed;
    let mut img = vec![[0u16; 4]; w * h];
    for y in 0..h {
        for x in 0..w {
            img[y * w + x] = sample(p, x, y, w, h, &mut s);
        }
    }
    let at = |x: usize, y: usize| img[y * w + x];
    let mut out = Vec::new();
    match fmt {
        RefFormat::Uyvy | RefFormat::Uyva | RefFormat::Yuy2 => {
            for y in 0..h {
                for x in (0..w).step_by(2) {
                    let a = at(x, y);
                    let b = at(x + 1, y);
                    let q = [(a[1] >> 2) as u8, (a[0] >> 2) as u8, (a[2] >> 2) as u8, (b[0] >> 2) as u8];
                    if fmt == RefFormat::Yuy2 {
                        out.extend_from_slice(&[q[1], q[0], q[3], q[2]]);
                    } else {
                        out.extend_from_slice(&q);
                    }
                }
            }
            if fmt == RefFormat::Uyva {
                for y in 0..h {
                    for x in 0..w {
                        out.push((at(x, y)[3] >> 2) as u8);
                    }
                }
            }
        }
        RefFormat::P216 | RefFormat::Pa16 => {
            for y in 0..h {
                for x in 0..w {
                    out.extend_from_slice(&(at(x, y)[0] << 6).to_le_bytes());
                }
            }
            for y in 0..h {
                for x in (0..w).step_by(2) {
                    out.extend_from_slice(&(at(x, y)[1] << 6).to_le_bytes());
                    out.extend_from_slice(&(at(x, y)[2] << 6).to_le_bytes());
                }
            }
            if fmt == RefFormat::Pa16 {
                for y in 0..h {
                    for x in 0..w {
                        out.extend_from_slice(&(at(x, y)[3] << 6).to_le_bytes());
                    }
                }
            }
        }
    }
    out
}

fn pixel_format(f: RefFormat) -> PixelFormat {
    match f {
        RefFormat::Uyvy => PixelFormat::Uyvy,
        RefFormat::Uyva => PixelFormat::Uyva,
        RefFormat::Yuy2 => PixelFormat::Yuy2,
        RefFormat::P216 => PixelFormat::P216,
        RefFormat::Pa16 => PixelFormat::Pa16,
    }
}

/// Splits a tight libvmx buffer into a [`Frame`].
fn to_frame(buf: &[u8], fmt: RefFormat, w: usize, h: usize) -> Frame {
    let pf = pixel_format(fmt);
    let mut off = 0;
    let planes = pf
        .plane_sizes(w, h)
        .into_iter()
        .map(|(row, rows)| {
            let p = Plane { data: buf[off..off + row * rows].to_vec(), stride: row };
            off += row * rows;
            p
        })
        .collect();
    Frame::from_planes(w, h, pf, planes).unwrap()
}

fn concat(f: &Frame) -> Vec<u8> {
    f.planes.iter().flat_map(|p| p.data.iter().copied()).collect()
}

fn rust_profile(p: i32) -> Profile {
    match p {
        profile::LQ => Profile::Lq,
        profile::SQ => Profile::Sq,
        profile::HQ => Profile::Hq,
        profile::OMT_LQ => Profile::OmtLq,
        profile::OMT_SQ => Profile::OmtSq,
        profile::OMT_HQ => Profile::OmtHq,
        _ => unreachable!(),
    }
}

fn first_diff(a: &[u8], b: &[u8]) -> String {
    match a.iter().zip(b).position(|(x, y)| x != y) {
        Some(i) => format!("first difference at byte {i} (C {} vs Rust {}), lengths {} / {}", a[i], b[i], a.len(), b.len()),
        None => format!("lengths differ: {} / {}", a.len(), b.len()),
    }
}

struct Case {
    w: usize,
    h: usize,
    fmt: RefFormat,
    pattern: Pattern,
    profile: i32,
    quality: i32,
    interlaced: bool,
}

fn run_case(c: &Case) {
    let label = format!(
        "{}x{} {:?} {:?} profile {} q {} interlaced {}",
        c.w, c.h, c.fmt, c.pattern, c.profile, c.quality, c.interlaced
    );
    let src = make_buffer(c.fmt, c.pattern, c.w, c.h, (c.w * 31 + c.h) as u64);

    let mut cenc = RefCodec::new(c.w, c.h, c.profile, 1, false).unwrap();
    cenc.set_quality(c.quality);
    let c_bytes = cenc.encode(c.fmt, &src, c.interlaced);

    let mut cfg = EncoderConfig::new(c.w, c.h);
    cfg.profile = rust_profile(c.profile);
    let mut renc = Encoder::new(cfg).unwrap();
    renc.set_quality(c.quality);
    let mut frame = to_frame(&src, c.fmt, c.w, c.h);
    frame.interlaced = c.interlaced;
    let r_bytes = renc.encode(&frame).unwrap();

    assert!(c_bytes == r_bytes, "{label}: bitstream differs: {}", first_diff(&c_bytes, &r_bytes));
    assert_eq!(cenc.quality(), renc.quality(), "{label}: rate control diverged");

    // C-encoded -> both decoders.
    let mut cdec = RefCodec::new(c.w, c.h, c.profile, 1, false).unwrap();
    let c_out = cdec.decode(&c_bytes, c.fmt).unwrap();
    let mut rdec = Decoder::new(c.w, c.h).unwrap();
    let r_out = rdec.decode(&c_bytes, pixel_format(c.fmt)).unwrap();
    assert_eq!(r_out.interlaced, c.interlaced && matches!(c.h, 480 | 576 | 1080));
    let r_out = concat(&r_out);
    assert!(c_out == r_out, "{label}: decode of C stream differs: {}", first_diff(&c_out, &r_out));

    // Rust-encoded -> C decoder.
    let c_out2 = cdec.decode(&r_bytes, c.fmt).unwrap();
    let r_out2 = concat(&rdec.decode(&r_bytes, pixel_format(c.fmt)).unwrap());
    assert!(c_out2 == r_out2, "{label}: C decode of Rust stream differs: {}", first_diff(&c_out2, &r_out2));
}

const SIZES: [(usize, usize); 6] = [(16, 16), (48, 32), (64, 40), (128, 72), (320, 240), (256, 144)];
const PATTERNS: [Pattern; 4] = [Pattern::Gradient, Pattern::Noise, Pattern::Bars, Pattern::Mixed];

#[test]
fn eight_bit_uyvy_matches_libvmx() {
    if !available() {
        return;
    }
    for &(w, h) in &SIZES {
        for &pattern in &PATTERNS {
            for &(profile, quality) in
                &[(profile::HQ, 80), (profile::HQ, 98), (profile::OMT_HQ, 52), (profile::OMT_SQ, 70), (profile::LQ, 60)]
            {
                run_case(&Case { w, h, fmt: RefFormat::Uyvy, pattern, profile, quality, interlaced: false });
            }
        }
    }
}

#[test]
fn eight_bit_yuy2_and_alpha_match_libvmx() {
    if !available() {
        return;
    }
    for &(w, h) in &[(64, 40), (128, 72), (320, 240)] {
        for &pattern in &PATTERNS {
            for fmt in [RefFormat::Yuy2, RefFormat::Uyva] {
                for &(profile, quality) in &[(profile::OMT_HQ, 90), (profile::SQ, 60)] {
                    run_case(&Case { w, h, fmt, pattern, profile, quality, interlaced: false });
                }
            }
        }
    }
}

#[test]
fn ten_bit_matches_libvmx() {
    if !available() {
        return;
    }
    for &(w, h) in &[(16, 16), (64, 40), (128, 72), (320, 240)] {
        for &pattern in &PATTERNS {
            for fmt in [RefFormat::P216, RefFormat::Pa16] {
                for &(profile, quality) in &[(profile::HQ, 98), (profile::OMT_HQ, 60), (profile::OMT_LQ, 75)] {
                    run_case(&Case { w, h, fmt, pattern, profile, quality, interlaced: false });
                }
            }
        }
    }
}

#[test]
fn interlaced_matches_libvmx() {
    if !available() {
        return;
    }
    for fmt in [RefFormat::Uyvy, RefFormat::P216, RefFormat::Uyva] {
        for pattern in [Pattern::Mixed, Pattern::Gradient] {
            run_case(&Case { w: 640, h: 480, fmt, pattern, profile: profile::OMT_HQ, quality: 80, interlaced: true });
        }
    }
    run_case(&Case {
        w: 1920,
        h: 1080,
        fmt: RefFormat::Uyvy,
        pattern: Pattern::Mixed,
        profile: profile::OMT_SQ,
        quality: 75,
        interlaced: true,
    });
    // Interlacing requested at an unsupported height is coded progressive.
    run_case(&Case { w: 128, h: 72, fmt: RefFormat::Uyvy, pattern: Pattern::Bars, profile: profile::HQ, quality: 80, interlaced: true });
}

#[test]
fn full_hd_matches_libvmx() {
    if !available() {
        return;
    }
    for fmt in [RefFormat::Uyvy, RefFormat::P216] {
        run_case(&Case {
            w: 1920,
            h: 1080,
            fmt,
            pattern: Pattern::Mixed,
            profile: profile::OMT_HQ,
            quality: 80,
            interlaced: false,
        });
    }
}

/// Several frames without forcing the quality: the rate control must follow
/// libvmx frame by frame.
#[test]
fn rate_control_matches_libvmx() {
    if !available() {
        return;
    }
    for &(w, h, prof) in &[(320, 240, profile::OMT_HQ), (128, 72, profile::LQ), (256, 144, profile::HQ)] {
        let mut cenc = RefCodec::new(w, h, prof, 1, false).unwrap();
        let mut cfg = EncoderConfig::new(w, h);
        cfg.profile = rust_profile(prof);
        let mut renc = Encoder::new(cfg).unwrap();
        let (fmin, fmax, minq, shift) = cenc.encoding_parameters();
        let p = renc.encoding_parameters();
        assert_eq!((fmin, fmax, minq, shift as u32), (p.frame_min, p.frame_max, p.min_quality, p.dc_shift));
        // Tight window so the quality actually moves.
        cenc.set_encoding_parameters(2000, 4000, minq, shift);
        renc.set_encoding_parameters(vmx_codec::EncodingParameters { frame_min: 2000, frame_max: 4000, ..p });
        for i in 0..12 {
            let pattern = PATTERNS[i % 4];
            let src = make_buffer(RefFormat::Uyvy, pattern, w, h, i as u64);
            let a = cenc.encode(RefFormat::Uyvy, &src, false);
            let b = renc.encode(&to_frame(&src, RefFormat::Uyvy, w, h)).unwrap();
            assert!(a == b, "{w}x{h} frame {i}: {}", first_diff(&a, &b));
            assert_eq!(cenc.quality(), renc.quality(), "{w}x{h} frame {i}");
        }
    }
}

#[test]
fn four_two_zero_input_matches_libvmx() {
    if !available() {
        return;
    }
    for &(w, h) in &[(64, 40), (320, 240)] {
        for &pattern in &PATTERNS {
            let yuyv = make_buffer(RefFormat::Uyvy, pattern, w, h, 7);
            // Derive 4:2:0 planes from the UYVY image (chroma from even rows).
            let mut y = vec![0u8; w * h];
            let mut u = vec![0u8; w / 2 * h / 2];
            let mut v = vec![0u8; w / 2 * h / 2];
            for r in 0..h {
                for i in 0..w / 2 {
                    let q = &yuyv[r * 2 * w + 4 * i..r * 2 * w + 4 * i + 4];
                    y[r * w + 2 * i] = q[1];
                    y[r * w + 2 * i + 1] = q[3];
                    if r % 2 == 0 {
                        u[r / 2 * w / 2 + i] = q[0];
                        v[r / 2 * w / 2 + i] = q[2];
                    }
                }
            }
            let uv: Vec<u8> = u.iter().zip(&v).flat_map(|(a, b)| [*a, *b]).collect();

            let mut c = RefCodec::new(w, h, profile::OMT_HQ, 1, false).unwrap();
            c.set_quality(85);
            let c_nv12 = c.encode_nv12(&y, &uv);
            c.set_quality(85);
            let c_i420 = c.encode_yv12(&y, &u, &v);

            let mut cfg = EncoderConfig::new(w, h);
            cfg.profile = Profile::OmtHq;
            let mut r = Encoder::new(cfg).unwrap();
            r.set_quality(85);
            let nv12 = Frame::from_planes(
                w,
                h,
                PixelFormat::Nv12,
                vec![Plane { data: y.clone(), stride: w }, Plane { data: uv.clone(), stride: w }],
            )
            .unwrap();
            let r_nv12 = r.encode(&nv12).unwrap();
            r.set_quality(85);
            let i420 = Frame::from_planes(
                w,
                h,
                PixelFormat::I420,
                vec![
                    Plane { data: y.clone(), stride: w },
                    Plane { data: u.clone(), stride: w / 2 },
                    Plane { data: v.clone(), stride: w / 2 },
                ],
            )
            .unwrap();
            let r_i420 = r.encode(&i420).unwrap();
            assert!(c_nv12 == r_nv12, "{w}x{h} {pattern:?} NV12: {}", first_diff(&c_nv12, &r_nv12));
            assert!(c_i420 == r_i420, "{w}x{h} {pattern:?} I420: {}", first_diff(&c_i420, &r_i420));
        }
    }
}

#[test]
fn preview_matches_libvmx() {
    if !available() {
        return;
    }
    for &(w, h) in &[(128, 72), (320, 240), (256, 144)] {
        for &pattern in &PATTERNS {
            let src = make_buffer(RefFormat::Uyvy, pattern, w, h, 3);
            let mut c = RefCodec::new(w, h, profile::OMT_SQ, 1, false).unwrap();
            let bytes = c.encode(RefFormat::Uyvy, &src, false);
            let (c_prev, pw, ph) = c.decode_preview_uyvy(&bytes).unwrap();
            let mut d = Decoder::new(w, h).unwrap();
            let plen = d.preview_len(&bytes).unwrap();
            let p = d.decode_preview(&bytes[..plen], false).unwrap();
            assert_eq!((p.width, p.height), (pw, ph));
            let mut uyvy = vec![0u8; pw * 2 * ph];
            for yy in 0..ph {
                for i in 0..pw / 2 {
                    let o = yy * pw * 2 + 4 * i;
                    uyvy[o] = p.planes[1].data[yy * p.planes[1].stride + i];
                    uyvy[o + 1] = p.planes[0].data[yy * p.planes[0].stride + 2 * i];
                    uyvy[o + 2] = p.planes[2].data[yy * p.planes[2].stride + i];
                    uyvy[o + 3] = p.planes[0].data[yy * p.planes[0].stride + 2 * i + 1];
                }
            }
            assert!(c_prev == uyvy, "{w}x{h} {pattern:?} preview: {}", first_diff(&c_prev, &uyvy));
        }
    }
}

#[test]
fn threaded_rust_is_deterministic() {
    let (w, h) = (320, 240);
    let src = make_buffer(RefFormat::Pa16, Pattern::Mixed, w, h, 9);
    let frame = to_frame(&src, RefFormat::Pa16, w, h);
    let mut one = Encoder::new(EncoderConfig::new(w, h)).unwrap();
    let mut cfg = EncoderConfig::new(w, h);
    cfg.threads = 3;
    let mut many = Encoder::new(cfg).unwrap();
    let a = one.encode(&frame).unwrap();
    let b = many.encode(&frame).unwrap();
    assert_eq!(a, b);
    let mut d1 = Decoder::new(w, h).unwrap();
    let mut d3 = Decoder::new(w, h).unwrap();
    d3.set_threads(4);
    assert_eq!(d1.decode(&a, PixelFormat::Pa16).unwrap(), d3.decode(&a, PixelFormat::Pa16).unwrap());
}
