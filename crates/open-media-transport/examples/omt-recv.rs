//! Receives from an OMT sender and decodes what arrives.
//!
//! ```sh
//! cargo run -p open-media-transport --example omt-recv -- 127.0.0.1:6400 5
//! cargo run -p open-media-transport --example omt-recv -- "MY-MAC.LOCAL (Camera)" 5
//! ```
//!
//! A name containing `(` is looked up with DNS-SD first. A third argument
//! `preview` asks for 1/8 preview video (§6.2); preview frames are decoded
//! with `decode_preview` and hashed as UYVY, like libomtnet delivers them.
//!
//! Video is decoded to UYVY with `vmx-codec`. For frames that carry per-frame
//! metadata it prints a `pixels` line in the same format as
//! `interop/libomtnet-harness recv`, so the two receivers' output can be diffed
//! when both watch the same sender. If the metadata is the harness's
//! `<HarnessFrame N="n" />`, it also compares the pixels with the pattern the
//! harness sent and prints the PSNR.

use std::net::{SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant};

use open_media_transport::command::{classify, Message, Quality, Tally};
use open_media_transport::discovery::{Discovery, SourceEvent};
use open_media_transport::frame::{ExtendedHeader, VideoFlags, CODEC_FPA1, CODEC_VMX1};
use open_media_transport::receiver::{Event, Receiver, ReceiverConfig};
use vmx_codec::{Decoder, PixelFormat};

fn main() {
    let mut args = std::env::args().skip(1);
    let target = args
        .next()
        .expect("usage: omt-recv HOST:PORT|\"MACHINE (Name)\" [SECONDS]");
    let addr = if target.contains('(') {
        find(&target, Duration::from_secs(5))
    } else {
        target
            .to_socket_addrs()
            .expect("resolve address")
            .next()
            .expect("no address")
    };
    println!("connecting to {addr}");
    let seconds: u64 = args.next().map(|s| s.parse().unwrap()).unwrap_or(5);
    let preview = args.next().as_deref() == Some("preview");

    let config = ReceiverConfig {
        preview,
        quality: Quality::High,
        tally: Tally {
            preview: false,
            program: true,
        },
        ..ReceiverConfig::default()
    };
    let rx = Receiver::connect(addr, config).expect("connect");
    let mut decoder: Option<(i32, i32, Decoder)> = None;
    let (mut video, mut audio) = (0u32, 0u32);
    let (mut silent_ch1, mut audio_rms) = (true, 0.0f64);
    let deadline = Instant::now() + Duration::from_secs(seconds);

    while Instant::now() < deadline {
        let Some(event) = rx.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        let (channel, f) = match event {
            Event::Frame(c, f) => (c, f),
            Event::Connected(c) => {
                println!("connected {c:?}");
                continue;
            }
            Event::Closed(c, why) => {
                // The receiver reconnects on its own.
                println!("closed {c:?} {why:?}");
                continue;
            }
        };
        match f.ext {
            ExtendedHeader::None => {
                let kind = match classify(&f.data) {
                    Message::Command(c) => format!("command {c:?}"),
                    Message::SenderInfo(_) => "sender-info".into(),
                    Message::Redirect(_) => "redirect".into(),
                    Message::QualityOther(_) => "quality".into(),
                    Message::Application(_) => "application".into(),
                };
                println!("metadata {channel:?} {kind} xml=\"{}\"", printable(&f.data));
            }
            ExtendedHeader::Video(v) => {
                video += 1;
                assert_eq!(v.codec, CODEC_VMX1, "unexpected video codec");
                let (w, h) = (v.width, v.height);
                if decoder.as_ref().map(|d| (d.0, d.1)) != Some((w, h)) {
                    decoder = Some((w, h, Decoder::new(w as usize, h as usize).expect("decoder")));
                }
                let dec = &mut decoder.as_mut().unwrap().2;
                if v.flags.contains(VideoFlags::PREVIEW) {
                    let pv = dec.decode_preview(&f.data, false).expect("decode preview");
                    if video == 1 || !f.metadata.is_empty() {
                        let uyvy = planar_to_uyvy(&pv);
                        println!(
                            "preview {}x{} data={} meta=\"{}\"",
                            pv.width,
                            pv.height,
                            f.data.len(),
                            printable(&f.metadata)
                        );
                        if !f.metadata.is_empty() {
                            println!(
                                "pixels {} fnv1a64={:016x} stride={}",
                                printable(&f.metadata),
                                fnv1a64(&uyvy),
                                pv.width * 2
                            );
                        }
                    }
                    continue;
                }
                let px = dec.decode(&f.data, PixelFormat::Uyvy).expect("decode VMX1");
                if !f.metadata.is_empty() {
                    let fm = printable(&f.metadata);
                    let plane = &px.planes[0];
                    println!(
                        "pixels {fm} fnv1a64={:016x} stride={}",
                        fnv1a64(&plane.data),
                        plane.stride
                    );
                    if let Some(n) = harness_frame_number(&f.metadata) {
                        let psnr = psnr(&plane.data, &harness_pattern(w as usize, h as usize, n));
                        println!("psnr N={n} {psnr:.2} dB");
                    }
                }
            }
            ExtendedHeader::Audio(a) => {
                audio += 1;
                assert_eq!(a.codec, CODEC_FPA1, "unexpected audio codec");
                // FPA1: planar f32, channels whose bit is clear are omitted (A2, A3).
                let spc = a.samples_per_channel as usize;
                let mut present = f.data.chunks_exact(spc * 4);
                for ch in 0..a.channels as usize {
                    let active = a.active_channels & (1 << ch) != 0;
                    let samples: Vec<f32> = if active {
                        present
                            .next()
                            .expect("audio data shorter than active channels")
                            .chunks_exact(4)
                            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                            .collect()
                    } else {
                        vec![0.0; spc]
                    };
                    if ch == 0 {
                        let sum: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
                        audio_rms = (sum / spc as f64).sqrt();
                    } else if ch == 1 && samples.iter().any(|&s| s != 0.0) {
                        silent_ch1 = false;
                    }
                }
            }
        }
    }
    println!("done video={video} audio={audio} ch0_rms={audio_rms:.4} ch1_silent={silent_ch1}");
}

/// Browses until `full_name` resolves; returns its first address (IPv4 first).
fn find(full_name: &str, timeout: Duration) -> SocketAddr {
    let d = Discovery::new().expect("start mDNS");
    let browser = d.browse().expect("browse");
    let deadline = Instant::now() + timeout;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        if let Some(SourceEvent::Resolved(s)) = browser.recv_timeout(left) {
            if s.full_name == full_name {
                if let Some(ip) = s.addresses.first() {
                    println!(
                        "discovered \"{}\" at {}:{} ({})",
                        s.full_name, ip, s.port, s.host
                    );
                    return SocketAddr::new(*ip, s.port);
                }
            }
        }
    }
    panic!("\"{full_name}\" not found within {timeout:?}");
}

/// Packs a `Yuv422p` frame (what `decode_preview` returns) as UYVY.
fn planar_to_uyvy(f: &vmx_codec::Frame) -> Vec<u8> {
    let (yp, up, vp) = (&f.planes[0], &f.planes[1], &f.planes[2]);
    let mut out = Vec::with_capacity(f.width * 2 * f.height);
    for y in 0..f.height {
        for x in (0..f.width).step_by(2) {
            out.push(up.data[y * up.stride + x / 2]);
            out.push(yp.data[y * yp.stride + x]);
            out.push(vp.data[y * vp.stride + x / 2]);
            out.push(yp.data[y * yp.stride + x + 1]);
        }
    }
    out
}

/// The UYVY test pattern `libomtnet-harness send` generates for frame `n`.
fn harness_pattern(w: usize, h: usize, n: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * 2 * h];
    for y in 0..h {
        let row = &mut out[y * w * 2..(y + 1) * w * 2];
        for x in (0..w).step_by(2) {
            let i = x * 2;
            row[i] = 128;
            row[i + 1] = ((x + n) & 255) as u8;
            row[i + 2] = 128;
            row[i + 3] = ((y + n) & 255) as u8;
        }
    }
    out
}

fn harness_frame_number(meta: &[u8]) -> Option<usize> {
    let s = std::str::from_utf8(meta).ok()?;
    let rest = s.strip_prefix("<HarnessFrame N=\"")?;
    rest[..rest.find('"')?].parse().ok()
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let mse: f64 = a
        .iter()
        .zip(b)
        .map(|(&x, &y)| (x as f64 - y as f64).powi(2))
        .sum::<f64>()
        / a.len() as f64;
    if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0 * 255.0 / mse).log10()
    }
}

fn fnv1a64(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325, |h, &x| {
        (h ^ x as u64).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Same escaping as the harness: printable ASCII except `"`, else `\xNN`.
fn printable(b: &[u8]) -> String {
    b.iter()
        .map(|&x| {
            if (32..127).contains(&x) && x != b'"' {
                (x as char).to_string()
            } else {
                format!("\\x{x:02X}")
            }
        })
        .collect()
}
