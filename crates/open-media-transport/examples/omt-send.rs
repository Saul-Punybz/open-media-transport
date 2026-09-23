//! Sends a test source, the same one `interop/libomtnet-harness send` makes.
//!
//! ```sh
//! cargo run --release -p open-media-transport --example omt-send -- "Rust Harness" 10
//! cargo run --release -p open-media-transport --example omt-send -- "Rust Harness" 20 "3=HOST (Other)" 10=
//! ```
//!
//! Arguments after the duration are a redirect schedule, `SECONDS=ADDRESS`:
//! at that many seconds after start, call `set_redirect` with the address;
//! an empty address clears the redirect (§9).
//!
//! 640x360 UYVY at 30 fps (a moving ramp), stereo 48 kHz audio with a 1 kHz
//! sine on the left and silence on the right, and `<HarnessFrame N="n" />\0`
//! per-frame metadata every 30th frame, so receivers can check pixels
//! (`omt-recv` computes PSNR against this pattern).

use std::time::{Duration, Instant};

use open_media_transport::clock::Clock;
use open_media_transport::sender::{Sender, SenderConfig, SenderInfo, VideoParams};
use vmx_codec::{Frame, PixelFormat};

const W: usize = 640;
const H: usize = 360;
const FPS: i64 = 30;
const RATE: usize = 48_000;

fn main() {
    let mut args = std::env::args().skip(1);
    let name = args.next().unwrap_or_else(|| "Rust Harness".into());
    let seconds: u64 = args.next().map_or(10, |s| s.parse().unwrap());
    let mut schedule: Vec<(Duration, String)> = args
        .map(|a| {
            let (t, addr) = a
                .split_once('=')
                .expect("schedule entries are SECONDS=ADDRESS");
            (
                Duration::from_secs_f64(t.parse().expect("seconds")),
                addr.to_owned(),
            )
        })
        .collect();
    schedule.sort_by_key(|e| e.0);
    schedule.reverse();

    let mut config = SenderConfig::new(name);
    config.info = Some(SenderInfo {
        product_name: "omt-send".into(),
        manufacturer: "open-media-transport".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    });
    config.connection_metadata = vec![br#"<HarnessHello Value="1" />"#.to_vec()];
    let tx = Sender::new(config).expect("start sender");
    println!(
        "send name=\"{}\" port={}",
        tx.full_name().unwrap_or("-"),
        tx.port()
    );

    let params = VideoParams {
        frame_rate_n: FPS as i32,
        frame_rate_d: 1,
        aspect_ratio: 16.0 / 9.0,
        color_space: 709,
        premultiplied: false,
    };
    let samples = RATE / FPS as usize;
    let mut frame = Frame::new(W, H, PixelFormat::Uyvy);
    let mut audio = vec![0.0f32; samples * 2];
    // One clock per stream, like libomtnet's timestamp -1 mode (C2, C3).
    let (mut video_clock, mut audio_clock) = (Clock::new(), Clock::new());
    let start = Instant::now();
    let end = start + Duration::from_secs(seconds);
    let mut last_tally = tx.tally();
    let mut last_redirect = None;

    for n in 0.. {
        if Instant::now() >= end {
            break;
        }
        while schedule.last().is_some_and(|e| start.elapsed() >= e.0) {
            let (_, addr) = schedule.pop().unwrap();
            tx.set_redirect(Some(&addr));
            println!(
                "send redirect t={:.1} set=\"{addr}\" effective={:?}",
                start.elapsed().as_secs_f64(),
                tx.redirect()
            );
        }
        let t = tx.redirect();
        if t != last_redirect {
            println!("send redirect now={t:?}");
            last_redirect = t;
        }
        fill_pattern(&mut frame.planes[0].data, n);
        let meta = if n % 30 == 0 {
            format!("<HarnessFrame N=\"{n}\" />\0").into_bytes()
        } else {
            Vec::new()
        };
        let ts = video_clock.video(FPS as i32, 1);
        tx.send_video(&frame, params, ts, &meta).expect("encode");

        for (i, s) in audio[..samples].iter_mut().enumerate() {
            let t = (n * samples + i) as f64 / RATE as f64;
            *s = ((2.0 * std::f64::consts::PI * 1000.0 * t).sin() * 0.25) as f32;
        }
        let ts = audio_clock.audio(RATE as i32, samples as i32);
        tx.send_audio(&audio, 2, RATE as i32, ts, b"");

        let t = tx.tally();
        if t != last_tally {
            println!("send tally preview={} program={}", t.preview, t.program);
            last_tally = t;
        }
    }
    let s = tx.stats();
    println!(
        "send done connections={} queued={} dropped={}",
        tx.connections(),
        s.frames_queued,
        s.frames_dropped
    );
}

/// Same bytes as `FillUyvy` in the harness: U, V = 128; Y ramps with x, y and n.
fn fill_pattern(buf: &mut [u8], n: usize) {
    for y in 0..H {
        let row = &mut buf[y * W * 2..(y + 1) * W * 2];
        for x in (0..W).step_by(2) {
            let i = x * 2;
            row[i] = 128;
            row[i + 1] = ((x + n) & 255) as u8;
            row[i + 2] = 128;
            row[i + 3] = ((y + n) & 255) as u8;
        }
    }
}
