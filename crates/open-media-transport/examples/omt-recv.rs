//! Receives from an OMT sender and decodes what arrives with
//! `open_media_transport::media`.
//!
//! ```sh
//! cargo run -p open-media-transport --example omt-recv -- 127.0.0.1:6400 5
//! cargo run -p open-media-transport --example omt-recv -- "MY-MAC.LOCAL (Camera)" 5 --format bgra
//! cargo run -p open-media-transport --example omt-recv -- omt://my-mac.local:6400 5
//! ```
//!
//! A full name is looked up with DNS-SD, and looked up again whenever the
//! receiver reconnects; an `omt://` URL is resolved with DNS (§8). Redirects
//! are followed and printed (§9). A third argument
//! `preview` asks for 1/8 preview video (§6.2). `--format` picks the decoded
//! layout with libomtnet's names: `uyvy` (default), `uyvyorbgra`, `bgra`,
//! `uyvyoruyva`, `uyvyoruyvaorp216orpa16` (or `hbd`), `p216`.
//!
//! For frames that carry per-frame metadata it prints a `pixels` line in the
//! same format as `interop/libomtnet-harness recv`, hashing the whole decoded
//! buffer as libomtnet delivers it, so the two receivers' output can be diffed
//! when both watch the same sender with the same format. Audio frames print an
//! `audio` line with a hash of the decoded planar samples, likewise. If the
//! metadata is the harness's `<HarnessFrame N="n" />` and the layout is UYVY,
//! it also compares the pixels with the pattern the harness sent and prints
//! the PSNR.

use std::net::ToSocketAddrs;
use std::time::{Duration, Instant};

use open_media_transport::command::{classify, Message, Quality, Tally};
use open_media_transport::frame::ExtendedHeader;
use open_media_transport::media::{Media, MediaDecoder, PreferredVideoFormat, VideoFormat};
use open_media_transport::receiver::{Event, Receiver, ReceiverConfig};

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let format = match args.iter().position(|a| a == "--format") {
        Some(i) => {
            let name = args.get(i + 1).expect("--format needs a value").clone();
            args.drain(i..i + 2);
            parse_format(&name)
        }
        None => PreferredVideoFormat::Uyvy,
    };
    let mut args = args.into_iter();
    let target = args.next().expect(
        "usage: omt-recv HOST:PORT|\"MACHINE (Name)\"|omt://HOST:PORT [SECONDS] [preview] [--format F]",
    );
    println!("connecting to {target}");
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
    let rx = if target.contains('(') || target.to_ascii_lowercase().starts_with("omt://") {
        Receiver::connect_to(&target, config).expect("connect")
    } else {
        let addr = target
            .to_socket_addrs()
            .expect("resolve address")
            .next()
            .expect("no address");
        Receiver::connect(addr, config).expect("connect")
    };
    let mut decoder = MediaDecoder::new(format);
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
                match rx.peer_addr() {
                    Some(a) => println!("connected {c:?} {a}"),
                    None => println!("connected {c:?}"),
                }
                continue;
            }
            Event::Redirect(to) => {
                println!("redirect to {}", to.as_deref().unwrap_or("(original)"));
                continue;
            }
            Event::Closed(c, why) => {
                // The receiver reconnects on its own.
                println!("closed {c:?} {why:?}");
                continue;
            }
        };
        if let ExtendedHeader::None = f.ext {
            let kind = match classify(&f.data) {
                Message::Command(c) => format!("command {c:?}"),
                Message::SenderInfo(_) => "sender-info".into(),
                Message::Redirect(_) => "redirect".into(),
                Message::QualityOther(_) => "quality".into(),
                Message::Application(_) => "application".into(),
            };
            println!("metadata {channel:?} {kind} xml=\"{}\"", printable(&f.data));
            continue;
        }
        let media = match decoder.decode(&f) {
            Ok(m) => m,
            Err(e) => {
                println!("decode error: {e}");
                continue;
            }
        };
        match media {
            Some(Media::Video(v)) => {
                video += 1;
                let fm = printable(&v.metadata);
                if video == 1 || !v.metadata.is_empty() {
                    println!(
                        "video ts={} {}x{} codec={} flags={} cs={} rate={}/{} meta=\"{fm}\"",
                        v.timestamp,
                        v.width,
                        v.height,
                        fourcc(v.format.fourcc()),
                        v.flags.0,
                        v.color_space,
                        v.frame_rate_n,
                        v.frame_rate_d,
                    );
                }
                if !v.metadata.is_empty() {
                    println!(
                        "pixels {fm} fnv1a64={:016x} stride={}",
                        fnv1a64(&v.data),
                        v.stride
                    );
                    if let (Some(n), VideoFormat::Uyvy, false) =
                        (harness_frame_number(&v.metadata), v.format, v.is_preview())
                    {
                        let psnr = psnr(&v.data, &harness_pattern(v.width, v.height, n));
                        println!("psnr N={n} {psnr:.2} dB");
                    }
                }
            }
            Some(Media::Audio(a)) => {
                audio += 1;
                let bytes: Vec<u8> = a.samples.iter().flat_map(|s| s.to_le_bytes()).collect();
                println!(
                    "audio ts={} rate={} ch={} spc={} fnv1a64={:016x}",
                    a.timestamp,
                    a.sample_rate,
                    a.channels,
                    a.samples_per_channel,
                    fnv1a64(&bytes)
                );
                let ch0 = a.channel(0);
                let sum: f64 = ch0.iter().map(|&s| (s as f64) * (s as f64)).sum();
                audio_rms = (sum / ch0.len().max(1) as f64).sqrt();
                if a.channels > 1 && a.channel(1).iter().any(|&s| s != 0.0) {
                    silent_ch1 = false;
                }
            }
            Some(Media::Metadata(_)) | None => {}
        }
    }
    println!("done video={video} audio={audio} ch0_rms={audio_rms:.4} ch1_silent={silent_ch1}");
}

fn parse_format(name: &str) -> PreferredVideoFormat {
    match name.to_ascii_lowercase().as_str() {
        "uyvy" => PreferredVideoFormat::Uyvy,
        "uyvyorbgra" => PreferredVideoFormat::UyvyOrBgra,
        "bgra" => PreferredVideoFormat::Bgra,
        "uyvyoruyva" => PreferredVideoFormat::UyvyOrUyva,
        "uyvyoruyvaorp216orpa16" | "hbd" => PreferredVideoFormat::UyvyOrUyvaOrP216OrPa16,
        "p216" => PreferredVideoFormat::P216,
        other => panic!("unknown --format {other}"),
    }
}

fn fourcc(c: u32) -> String {
    String::from_utf8_lossy(&c.to_le_bytes()).into_owned()
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
