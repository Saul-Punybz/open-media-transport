//! `omt` — list, send and receive Open Media Transport sources.
//!
//! ```text
//! omt list [--seconds N]
//! omt send [--name NAME] [--size WxH] [--fps F] [--seconds N]
//! omt recv SOURCE [--seconds N] [--snapshot FILE.bmp] [--preview]
//! ```
//!
//! `SOURCE` is a name as `omt list` shows it, e.g. `"MY-PC (Camera 1)"`, or
//! `host:port`.

mod image;

use std::fs::File;
use std::io::BufWriter;
use std::net::{SocketAddr, ToSocketAddrs};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use open_media_transport::clock::Clock;
use open_media_transport::command::{classify, Message, Quality};
use open_media_transport::discovery::{Discovery, SourceEvent};
use open_media_transport::frame::{ExtendedHeader, VideoFlags};
use open_media_transport::receiver::{Event, Receiver, ReceiverConfig};
use open_media_transport::sender::{Sender, SenderConfig, SenderInfo, VideoParams};
use vmx_codec::{Decoder, Frame, PixelFormat};

const USAGE: &str = "\
omt — Open Media Transport test tool (open-media-transport for Rust)

USAGE:
  omt list [--seconds N]
      Show the OMT sources on the network (default 5 s).
  omt send [--name NAME] [--size WxH] [--fps F] [--seconds N]
      Send colour bars with a moving box and a 1 kHz beep once a second.
      Defaults: --name \"Test Pattern\" --size 1280x720 --fps 30, until Ctrl-C.
  omt recv SOURCE [--seconds N] [--snapshot FILE.bmp] [--preview]
      Connect to SOURCE (a name from `omt list`, or host:port), print
      statistics every second, and optionally save the last frame.
  omt version
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("list") => list(&args[1..]),
        Some("send") => send(&args[1..]),
        Some("recv") => recv(&args[1..]),
        Some("version" | "--version" | "-V") => {
            println!("omt {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => {
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("omt: {e}");
            ExitCode::FAILURE
        }
    }
}

type Result<T> = std::result::Result<T, String>;

/// `--flag value` lookup.
fn opt<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn seconds(args: &[String]) -> Result<Option<u64>> {
    opt(args, "--seconds")
        .map(|s| s.parse().map_err(|_| format!("bad --seconds {s}")))
        .transpose()
}

fn list(args: &[String]) -> Result<()> {
    let secs = seconds(args)?.unwrap_or(5);
    let d = Discovery::new().map_err(|e| e.to_string())?;
    let browser = d.browse().map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut seen = std::collections::BTreeMap::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match browser.recv_timeout(left) {
            Some(SourceEvent::Resolved(s)) => {
                if !seen.contains_key(&s.full_name) {
                    println!(
                        "{:<48} {}:{}  {:?}",
                        format!("\"{}\"", s.full_name),
                        s.host.trim_end_matches('.'),
                        s.port,
                        s.addresses
                    );
                }
                seen.insert(s.full_name.clone(), s);
            }
            Some(SourceEvent::Removed(name)) => {
                if seen.remove(&name).is_some() {
                    println!("removed \"{name}\"");
                }
            }
            None => break,
        }
    }
    if seen.is_empty() {
        println!("no OMT sources found in {secs} s");
    }
    Ok(())
}

fn parse_fps(s: &str) -> Result<(i32, i32)> {
    Ok(match s {
        "23.976" | "23.98" => (24000, 1001),
        "29.97" => (30000, 1001),
        "59.94" => (60000, 1001),
        _ => (s.parse().map_err(|_| format!("bad --fps {s}"))?, 1),
    })
}

fn send(args: &[String]) -> Result<()> {
    let name = opt(args, "--name").unwrap_or("Test Pattern");
    let (w, h) = match opt(args, "--size") {
        Some(s) => {
            let (a, b) = s.split_once('x').ok_or(format!("bad --size {s}"))?;
            (
                a.parse().map_err(|_| format!("bad --size {s}"))?,
                b.parse().map_err(|_| format!("bad --size {s}"))?,
            )
        }
        None => (1280usize, 720usize),
    };
    let (fps_n, fps_d) = parse_fps(opt(args, "--fps").unwrap_or("30"))?;
    let fps = fps_n as f64 / fps_d as f64;
    let limit = seconds(args)?.map(Duration::from_secs);

    let mut config = SenderConfig::new(name);
    config.info = Some(SenderInfo {
        product_name: "omt".into(),
        manufacturer: "open-media-transport".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    });
    let tx = Sender::new(config).map_err(|e| format!("cannot start sender: {e}"))?;
    println!(
        "sending \"{}\" on port {} — {w}x{h} at {fps:.2} fps. Ctrl-C to stop.",
        tx.full_name().unwrap_or(name),
        tx.port()
    );

    let params = VideoParams {
        frame_rate_n: fps_n,
        frame_rate_d: fps_d,
        aspect_ratio: w as f32 / h as f32,
        color_space: 709,
        premultiplied: false,
    };
    let rate = 48_000usize;
    let mut frame = Frame::new(w, h, PixelFormat::Uyvy);
    let (mut vclock, mut aclock) = (Clock::new(), Clock::new());
    let start = Instant::now();
    let mut last_report = Instant::now();
    let mut samples_sent = 0usize;
    let mut last_tally = tx.tally();
    for n in 0u64.. {
        if limit.is_some_and(|l| start.elapsed() >= l) {
            break;
        }
        image::fill_test_pattern(&mut frame.planes[0].data, w, h, n, fps);
        let ts = vclock.video(fps_n, fps_d);
        tx.send_video(&frame, params, ts, b"")
            .map_err(|e| format!("encode: {e}"))?;

        // Audio for this frame's duration: 1 kHz for the first 100 ms of
        // each second, both channels; silence (left out on the wire) otherwise.
        let upto = ((n + 1) as f64 * rate as f64 / fps) as usize;
        let count = upto - samples_sent;
        let mut audio = vec![0.0f32; count * 2];
        for i in 0..count {
            let s = samples_sent + i;
            if s % rate < rate / 10 {
                let v = (2.0 * std::f64::consts::PI * 1000.0 * s as f64 / rate as f64).sin() * 0.25;
                audio[i] = v as f32;
                audio[count + i] = v as f32;
            }
        }
        samples_sent = upto;
        let ats = aclock.audio(rate as i32, count as i32);
        tx.send_audio(&audio, 2, rate as i32, ats, b"");

        let t = tx.tally();
        if t != last_tally || last_report.elapsed() >= Duration::from_secs(5) {
            let st = tx.stats();
            println!(
                "{:>6.1}s  connections={} video_receivers={} tally={}{} dropped={}",
                start.elapsed().as_secs_f64(),
                tx.connections(),
                tx.video_receivers(),
                if t.program { "PGM" } else { "" },
                if t.preview { " PVW" } else { "" },
                st.frames_dropped
            );
            last_tally = t;
            last_report = Instant::now();
        }
    }
    Ok(())
}

fn resolve(source: &str) -> Result<SocketAddr> {
    if !source.contains('(') {
        return source
            .to_socket_addrs()
            .map_err(|e| format!("cannot resolve {source}: {e}"))?
            .next()
            .ok_or(format!("no address for {source}"));
    }
    let d = Discovery::new().map_err(|e| e.to_string())?;
    let browser = d.browse().map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        if let Some(SourceEvent::Resolved(s)) = browser.recv_timeout(left) {
            if s.full_name == source {
                if let Some(ip) = s.addresses.first() {
                    return Ok(SocketAddr::new(*ip, s.port));
                }
            }
        }
    }
    Err(format!(
        "\"{source}\" not found in 5 s (check the exact name with `omt list`)"
    ))
}

#[derive(Default)]
struct Window {
    video: u32,
    audio: u32,
    audio_with_sound: u32,
    bytes: u64,
}

fn recv(args: &[String]) -> Result<()> {
    let source = args
        .first()
        .filter(|a| !a.starts_with("--"))
        .ok_or("recv needs a SOURCE (see `omt list`)")?;
    let limit = seconds(args)?.map(Duration::from_secs);
    let snapshot = opt(args, "--snapshot");
    let preview = args.iter().any(|a| a == "--preview");
    let addr = resolve(source)?;
    println!("connecting to {source} at {addr}");
    let rx = Receiver::connect(
        addr,
        ReceiverConfig {
            preview,
            quality: Quality::Default,
            ..ReceiverConfig::default()
        },
    )
    .map_err(|e| format!("cannot connect to {addr}: {e}"))?;

    let start = Instant::now();
    let mut window = Window::default();
    let mut tick = Instant::now();
    let mut last_video = String::from("-");
    let mut last_audio = String::from("-");
    let mut decoder: Option<(i32, i32, Decoder)> = None;
    let mut last_frame: Option<(Vec<u8>, usize, usize, usize, bool)> = None;
    let mut decode_errors = 0u32;
    let mut decode_due = true;

    while limit.map_or(true, |l| start.elapsed() < l) {
        if let Some(event) = rx.recv_timeout(Duration::from_millis(100)) {
            match event {
                Event::Connected(c) => println!("connected ({c:?} channel)"),
                Event::Closed(c, why) => {
                    println!("disconnected ({c:?} channel): {why:?} — retrying")
                }
                Event::Frame(_, f) => match f.ext {
                    ExtendedHeader::None => match classify(&f.data) {
                        Message::SenderInfo(x) | Message::Application(x) => {
                            println!(
                                "metadata: {}",
                                String::from_utf8_lossy(x).trim_end_matches('\0')
                            )
                        }
                        Message::Command(c) => println!("sender says: {c:?}"),
                        Message::Redirect(x) => {
                            println!(
                                "redirect (not followed yet): {}",
                                String::from_utf8_lossy(x)
                            )
                        }
                        Message::QualityOther(_) => {}
                    },
                    ExtendedHeader::Video(v) => {
                        window.video += 1;
                        window.bytes += f.data.len() as u64;
                        let fl = v.flags;
                        last_video = format!(
                            "{}x{} {:.2} fps{}{}{}{}",
                            v.width,
                            v.height,
                            v.frame_rate_n as f64 / v.frame_rate_d.max(1) as f64,
                            if fl.contains(VideoFlags::INTERLACED) {
                                " interlaced"
                            } else {
                                ""
                            },
                            if fl.contains(VideoFlags::ALPHA) {
                                " alpha"
                            } else {
                                ""
                            },
                            if fl.contains(VideoFlags::HIGH_BIT_DEPTH) {
                                " 10-bit"
                            } else {
                                ""
                            },
                            if fl.contains(VideoFlags::PREVIEW) {
                                " preview"
                            } else {
                                ""
                            },
                        );
                        // Decode one frame a second: proves the stream decodes and
                        // feeds --snapshot, without the CPU cost of every frame.
                        if decode_due {
                            decode_due = false;
                            let (w, h) = (v.width, v.height);
                            if decoder.as_ref().map(|d| (d.0, d.1)) != Some((w, h)) {
                                decoder =
                                    Decoder::new(w as usize, h as usize).ok().map(|d| (w, h, d));
                            }
                            let bt601 = v.color_space == 601 || (v.color_space == 0 && h < 720);
                            match decoder.as_mut() {
                                Some((_, _, dec)) if fl.contains(VideoFlags::PREVIEW) => {
                                    match dec.decode_preview(&f.data, false) {
                                        Ok(p) => {
                                            last_frame = Some((
                                                planar_to_uyvy(&p),
                                                p.width * 2,
                                                p.width,
                                                p.height,
                                                bt601,
                                            ))
                                        }
                                        Err(_) => decode_errors += 1,
                                    }
                                }
                                // 10-bit streams: snapshot not supported yet.
                                Some(_) if fl.contains(VideoFlags::HIGH_BIT_DEPTH) => {}
                                Some((_, _, dec)) => match dec.decode(&f.data, PixelFormat::Uyvy) {
                                    Ok(p) => {
                                        let pl = &p.planes[0];
                                        last_frame = Some((
                                            pl.data.clone(),
                                            pl.stride,
                                            p.width,
                                            p.height,
                                            bt601,
                                        ));
                                    }
                                    Err(_) => decode_errors += 1,
                                },
                                None => decode_errors += 1,
                            }
                        }
                    }
                    ExtendedHeader::Audio(a) => {
                        window.audio += 1;
                        window.audio_with_sound += (a.active_channels != 0) as u32;
                        window.bytes += f.data.len() as u64;
                        last_audio = format!("{} Hz {} ch", a.sample_rate, a.channels);
                    }
                },
            }
        }
        if tick.elapsed() >= Duration::from_secs(1) {
            let secs = tick.elapsed().as_secs_f64();
            println!(
                "{:>6.1}s  video {} | {:.1} fps received | {:.1} Mbit/s | audio {}, {}/{} frames with sound | decode errors {}",
                start.elapsed().as_secs_f64(),
                last_video,
                window.video as f64 / secs,
                window.bytes as f64 * 8.0 / secs / 1e6,
                last_audio,
                window.audio_with_sound,
                window.audio,
                decode_errors
            );
            window = Window::default();
            tick = Instant::now();
            decode_due = true;
        }
    }

    if let Some(path) = snapshot {
        match last_frame {
            Some((px, stride, w, h, bt601)) => {
                let file = File::create(path).map_err(|e| format!("{path}: {e}"))?;
                image::write_bmp(&mut BufWriter::new(file), &px, stride, w, h, bt601)
                    .map_err(|e| format!("{path}: {e}"))?;
                println!("saved {w}x{h} snapshot to {path}");
            }
            None => println!("no video frame decoded; no snapshot written"),
        }
    }
    Ok(())
}

/// Packs a `Yuv422p` frame (what `decode_preview` returns) as UYVY.
fn planar_to_uyvy(f: &Frame) -> Vec<u8> {
    let (yp, up, vp) = (&f.planes[0], &f.planes[1], &f.planes[2]);
    let mut out = Vec::with_capacity(f.width * 2 * f.height);
    for y in 0..f.height {
        for x in (0..f.width).step_by(2) {
            out.extend_from_slice(&[
                up.data[y * up.stride + x / 2],
                yp.data[y * yp.stride + x],
                vp.data[y * vp.stride + x / 2],
                yp.data[y * yp.stride + x + 1],
            ]);
        }
    }
    out
}
