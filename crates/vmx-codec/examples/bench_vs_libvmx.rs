//! Encode/decode throughput of vmx-codec (NEON / SSE2 kernels) against the
//! libvmx C++ reference (128-bit SIMD path: NEON via sse2neon on ARM, SSE on
//! x86-64), 1920x1080 UYVY 8-bit, one thread each.
//!
//! cargo run --release -p vmx-codec --example bench_vs_libvmx

use std::time::Instant;

use libvmx_ref::{profile, RefCodec, RefFormat};
use vmx_codec::{Decoder, Encoder, EncoderConfig, Frame, PixelFormat, Profile};

fn test_image(w: usize, h: usize) -> Vec<u8> {
    // Smooth gradients plus mild noise and some hard edges: closer to camera
    // content than pure noise, harder than flat colour.
    let mut seed = 12345u64;
    let mut out = vec![0u8; w * 2 * h];
    for y in 0..h {
        for x in 0..w / 2 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let n = ((seed >> 60) as i32) - 8;
            let edge = if (x / 40 + y / 40) % 2 == 0 { 30 } else { 0 };
            let yv = |xx: usize| ((xx * 200 / w + y * 40 / h) as i32 + 16 + n + edge).clamp(16, 235) as u8;
            let o = y * w * 2 + 4 * x;
            out[o] = (128 + (x * 60 / w) as i32 - 30 + n / 2).clamp(16, 240) as u8;
            out[o + 1] = yv(2 * x);
            out[o + 2] = (128 + (y * 60 / h) as i32 - 30 - n / 2).clamp(16, 240) as u8;
            out[o + 3] = yv(2 * x + 1);
        }
    }
    out
}

fn fps(frames: usize, secs: f64) -> f64 {
    frames as f64 / secs
}

fn main() {
    let (w, h) = (1920usize, 1080usize);
    let frames: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(60);
    let src = test_image(w, h);
    let frame = Frame::from_planes(w, h, PixelFormat::Uyvy, vec![vmx_codec::Plane { data: src.clone(), stride: 2 * w }])
        .unwrap();

    for (name, prof, rprof) in [("OMT HQ q80", profile::OMT_HQ, Profile::OmtHq), ("HQ q98", profile::HQ, Profile::Hq)] {
        let q = if prof == profile::HQ { 98 } else { 80 };
        println!("== {name}, {frames} frames 1920x1080 UYVY, 1 thread ==");

        let mut cfg = EncoderConfig::new(w, h);
        cfg.profile = rprof;
        let mut renc = Encoder::new(cfg).unwrap();
        renc.set_quality(q);
        let packet = renc.encode(&frame).unwrap(); // warm-up
        let t = Instant::now();
        for _ in 0..frames {
            renc.set_quality(q);
            std::hint::black_box(renc.encode(&frame).unwrap());
        }
        let r_enc = fps(frames, t.elapsed().as_secs_f64());

        let mut rdec = Decoder::new(w, h).unwrap();
        let mut out = Frame::new(w, h, PixelFormat::Uyvy);
        rdec.decode_into(&packet, &mut out).unwrap();
        let t = Instant::now();
        for _ in 0..frames {
            rdec.decode_into(&packet, &mut out).unwrap();
        }
        let r_dec = fps(frames, t.elapsed().as_secs_f64());

        if !libvmx_ref::AVAILABLE {
            println!("vmx-codec: encode {r_enc:.1} fps, decode {r_dec:.1} fps ({} bytes/frame)", packet.len());
            println!("libvmx reference not built; skipping C numbers");
            continue;
        }
        let mut cenc = RefCodec::new(w, h, prof, 1, false).unwrap();
        cenc.set_quality(q);
        let cpacket = cenc.encode(RefFormat::Uyvy, &src, false);
        assert_eq!(cpacket, packet, "bitstreams differ");
        let t = Instant::now();
        for _ in 0..frames {
            cenc.set_quality(q);
            std::hint::black_box(cenc.encode(RefFormat::Uyvy, &src, false));
        }
        let c_enc = fps(frames, t.elapsed().as_secs_f64());
        let mut cdec = RefCodec::new(w, h, prof, 1, false).unwrap();
        cdec.decode(&packet, RefFormat::Uyvy).unwrap();
        let t = Instant::now();
        for _ in 0..frames {
            std::hint::black_box(cdec.decode(&packet, RefFormat::Uyvy).unwrap());
        }
        let c_dec = fps(frames, t.elapsed().as_secs_f64());

        println!("frame size: {} bytes ({:.0} Mbit/s at 60 fps)", packet.len(), packet.len() as f64 * 8.0 * 60.0 / 1e6);
        println!("vmx-codec   : encode {r_enc:7.1} fps   decode {r_dec:7.1} fps");
        println!("libvmx SIMD : encode {c_enc:7.1} fps   decode {c_dec:7.1} fps");
        println!("ratio (C/Rust): encode {:.2}x   decode {:.2}x", c_enc / r_enc, c_dec / r_dec);
    }
}
