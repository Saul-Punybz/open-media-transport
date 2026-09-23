//! `omt` — list, send and receive Open Media Transport sources.
//!
//! ```text
//! omt list [--seconds N]
//! omt send [--name NAME] [--size WxH] [--fps F] [--seconds N] [--redirect SOURCE] [--10bit]
//! omt recv SOURCE [--seconds N] [--snapshot FILE.bmp|FILE.png] [--preview]
//! omt discovery-server [--port N] [--seconds N]
//! ```
//!
//! `SOURCE` is a name as `omt list` shows it, e.g. `"MY-PC (Camera 1)"`, a URL
//! `omt://host:port`, or `host:port`. A name is looked up again every time
//! the receiver reconnects, so a source that restarts on another port is
//! found again. `list`, `send` and `recv` take `--discovery-server URL` to
//! use a discovery server, and `list` and `recv` take `--no-mdns` to use
//! nothing else.

mod image;

use std::fs::File;
use std::io::{BufWriter, Write};
use std::net::ToSocketAddrs;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use open_media_transport::address::{Address, Directory};
use open_media_transport::clock::Clock;
use open_media_transport::command::{classify, Message, Quality};
use open_media_transport::discovery::{Discovery, SourceEvent};
use open_media_transport::discovery_server::{self, Server, ServerEvent};
use open_media_transport::frame::{ExtendedHeader, VideoFlags};
use open_media_transport::media::{MediaDecoder, PreferredVideoFormat, VideoFrame};
use open_media_transport::receiver::{Event, Receiver, ReceiverConfig};
use open_media_transport::sender::{Sender, SenderConfig, SenderInfo, VideoParams};
use vmx_codec::{Frame, PixelFormat};

const USAGE: &str = "\
omt — Open Media Transport test tool (open-media-transport for Rust)

USAGE:
  omt list [--seconds N]
      Show the OMT sources on the network (default 5 s).
  omt send [--name NAME] [--size WxH] [--fps F] [--seconds N] [--redirect SOURCE] [--10bit]
      Send colour bars with a moving box and a 1 kHz beep once a second.
      Defaults: --name \"Test Pattern\" --size 1280x720 --fps 30, until Ctrl-C.
      --redirect tells receivers to use SOURCE instead (a virtual source).
      --10bit sends a 10-bit (P216) source instead of 8-bit UYVY.
  omt recv SOURCE [--seconds N] [--snapshot FILE.bmp|FILE.png] [--preview]
      Connect to SOURCE (a name from `omt list`, omt://host:port, or
      host:port), print statistics every second, and optionally save the
      last frame: .png keeps 10-bit sources at 16 bits per sample and keeps
      alpha; .bmp is 8-bit RGB. Follows redirects.
  omt discovery-server [--port N] [--seconds N]
      Run a discovery server for networks without multicast (default port
      6399), printing each client and source as it comes and goes.
  omt version

  list, send and recv also take --discovery-server omt://HOST[:PORT]: send
  then registers with that server instead of announcing over mDNS; list and
  recv ask the server as well as mDNS, or only the server with --no-mdns.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("list") => list(&args[1..]),
        Some("send") => send(&args[1..]),
        Some("recv") => recv(&args[1..]),
        Some("discovery-server") => serve_discovery(&args[1..]),
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

/// mDNS, or the discovery server given with `--discovery-server`.
fn discovery(args: &[String]) -> Result<Discovery> {
    match opt(args, "--discovery-server") {
        Some(url) => {
            let mdns = !args.iter().any(|a| a == "--no-mdns");
            Discovery::with_server(url, mdns)
        }
        None => Discovery::new(),
    }
    .map_err(|e| e.to_string())
}

fn list(args: &[String]) -> Result<()> {
    let secs = seconds(args)?.unwrap_or(5);
    let d = discovery(args)?;
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
    let ten_bit = args.iter().any(|a| a == "--10bit");

    let mut config = SenderConfig::new(name);
    config.discovery_server = opt(args, "--discovery-server").map(str::to_owned);
    config.info = Some(SenderInfo {
        product_name: "omt".into(),
        manufacturer: "open-media-transport".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    });
    let tx = Sender::new(config).map_err(|e| format!("cannot start sender: {e}"))?;
    println!(
        "sending \"{}\" on port {} — {w}x{h}{} at {fps:.2} fps. Ctrl-C to stop.",
        tx.full_name().unwrap_or(name),
        tx.port(),
        if ten_bit { " 10-bit" } else { "" }
    );
    if let Some(to) = opt(args, "--redirect") {
        tx.set_redirect(Some(to));
        match tx.redirect() {
            Some(r) => println!("redirecting receivers to {r}"),
            None => println!("--redirect {to} is this sender itself; not redirecting"),
        }
    }

    let params = VideoParams {
        frame_rate_n: fps_n,
        frame_rate_d: fps_d,
        aspect_ratio: w as f32 / h as f32,
        color_space: 709,
        premultiplied: false,
    };
    let rate = 48_000usize;
    let format = if ten_bit {
        PixelFormat::P216
    } else {
        PixelFormat::Uyvy
    };
    let mut frame = Frame::new(w, h, format);
    let (mut vclock, mut aclock) = (Clock::new(), Clock::new());
    let start = Instant::now();
    let mut last_report = Instant::now();
    let mut samples_sent = 0usize;
    let mut last_tally = tx.tally();
    for n in 0u64.. {
        if limit.is_some_and(|l| start.elapsed() >= l) {
            break;
        }
        if ten_bit {
            let (luma, chroma) = frame.planes.split_at_mut(1);
            image::fill_test_pattern_p216(&mut luma[0].data, &mut chroma[0].data, w, h, n, fps);
        } else {
            image::fill_test_pattern(&mut frame.planes[0].data, w, h, n, fps);
        }
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

/// Connects by name or `omt://` URL through the library, which resolves the
/// address again on every reconnect (§8); names are looked up with mDNS
/// and/or the `--discovery-server`. `host:port` is a fixed address.
fn connect(source: &str, config: ReceiverConfig, args: &[String]) -> Result<Receiver> {
    let by_name = source.contains('(') || source.to_ascii_lowercase().starts_with("omt://");
    if by_name {
        let address = Address::parse(source).map_err(|e| format!("{source}: {e}"))?;
        let directory = match address {
            Address::Name(_) => Some(Arc::new(
                Directory::with_discovery(discovery(args)?).map_err(|e| e.to_string())?,
            )),
            _ => None,
        };
        return Receiver::connect_address(address, config, directory).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                format!("\"{source}\" not found in 5 s (check the exact name with `omt list`)")
            }
            _ => format!("cannot connect to {source}: {e}"),
        });
    }
    let addr = source
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve {source}: {e}"))?
        .next()
        .ok_or(format!("no address for {source}"))?;
    Receiver::connect(addr, config).map_err(|e| format!("cannot connect to {addr}: {e}"))
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
    println!("connecting to {source}");
    let rx = connect(
        source,
        ReceiverConfig {
            preview,
            quality: Quality::Default,
            ..ReceiverConfig::default()
        },
        args,
    )?;

    let start = Instant::now();
    let mut window = Window::default();
    let mut tick = Instant::now();
    let mut last_video = String::from("-");
    let mut last_audio = String::from("-");
    // Keeps alpha and 10-bit depth for the snapshot (`OMTReceive.cs:839-887`).
    let mut decoder = MediaDecoder::new(PreferredVideoFormat::UyvyOrUyvaOrP216OrPa16);
    let mut last_frame = VideoFrame::default();
    let mut have_frame = false;
    let mut decode_errors = 0u32;
    let mut decode_due = true;

    while limit.map_or(true, |l| start.elapsed() < l) {
        if let Some(event) = rx.recv_timeout(Duration::from_millis(100)) {
            match event {
                Event::Connected(c) => match rx.peer_addr() {
                    Some(a) => println!("connected ({c:?} channel) to {a}"),
                    None => println!("connected ({c:?} channel)"),
                },
                Event::Redirect(Some(to)) => println!("redirected to {to}"),
                Event::Redirect(None) => println!("redirect cleared; back to {source}"),
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
                            println!("sender says: {}", String::from_utf8_lossy(x))
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
                            match decoder.decode_video(&f, &mut last_frame) {
                                Ok(()) => have_frame = true,
                                Err(_) => decode_errors += 1,
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
        if !have_frame {
            println!("no video frame decoded; no snapshot written");
            return Ok(());
        }
        let img = image::to_rgb(&last_frame);
        let mut file = BufWriter::new(File::create(path).map_err(|e| format!("{path}: {e}"))?);
        let png = path.to_ascii_lowercase().ends_with(".png");
        if png {
            image::write_png(&mut file, &img).map_err(|e| format!("{path}: {e}"))?;
        } else {
            image::write_bmp(&mut file, &img).map_err(|e| format!("{path}: {e}"))?;
        }
        file.flush().map_err(|e| format!("{path}: {e}"))?;
        let (w, h) = (last_frame.width, last_frame.height);
        let depth = match (png, img.high_bit_depth) {
            (true, true) => "16-bit PNG from a 10-bit source",
            (true, false) => "8-bit PNG",
            (false, true) => "8-bit BMP from a 10-bit source (use .png to keep the depth)",
            (false, false) => "8-bit BMP",
        };
        println!("saved {w}x{h} snapshot to {path} ({depth})");
    }
    Ok(())
}

fn serve_discovery(args: &[String]) -> Result<()> {
    let port = match opt(args, "--port") {
        Some(p) => p.parse().map_err(|_| format!("bad --port {p}"))?,
        None => discovery_server::DEFAULT_PORT,
    };
    let limit = seconds(args)?.map(Duration::from_secs);
    let server = Server::bind(port).map_err(|e| format!("cannot listen on port {port}: {e}"))?;
    println!(
        "discovery server on port {}. Ctrl-C to stop.",
        server.port()
    );
    let start = Instant::now();
    while limit.map_or(true, |l| start.elapsed() < l) {
        // Lines as libomtnet's server prints them (`server/OMTDiscoveryServer.cs:123,134,157,170`).
        match server.recv_event(Duration::from_millis(200)) {
            Some(ServerEvent::Connected(a)) => println!("Connected: {a}"),
            Some(ServerEvent::Disconnected(a)) => println!("Disconnected: {a}"),
            Some(ServerEvent::Added(a, m)) => {
                println!(
                    "{a} ADDED {} port {} {:?}",
                    m.full_name, m.port, m.addresses
                )
            }
            Some(ServerEvent::Removed(a, m)) => println!("{a} REMOVED {}", m.full_name),
            None => {}
        }
    }
    Ok(())
}
