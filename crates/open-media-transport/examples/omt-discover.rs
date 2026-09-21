//! Browses for OMT sources, or announces one.
//!
//! ```sh
//! cargo run -p open-media-transport --example omt-discover -- browse 5
//! cargo run -p open-media-transport --example omt-discover -- announce "Test Source" 6500 10
//! ```
//!
//! `announce` only publishes the DNS-SD records; nothing listens on the port.

use std::time::{Duration, Instant};

use open_media_transport::discovery::{Discovery, SourceEvent};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let d = Discovery::new().expect("start mDNS");
    match args.first().map(String::as_str) {
        Some("browse") => {
            let seconds = args.get(1).map_or(5, |s| s.parse().unwrap());
            let browser = d.browse().expect("browse");
            let deadline = Instant::now() + Duration::from_secs(seconds);
            while let Some(left) = deadline.checked_duration_since(Instant::now()) {
                match browser.recv_timeout(left) {
                    Some(SourceEvent::Resolved(s)) => println!(
                        "resolved \"{}\" host={} port={} addresses={:?}",
                        s.full_name, s.host, s.port, s.addresses
                    ),
                    Some(SourceEvent::Removed(name)) => println!("removed \"{name}\""),
                    None => break,
                }
            }
        }
        Some("announce") if args.len() >= 3 => {
            let port: u16 = args[2].parse().expect("port");
            let seconds = args.get(3).map_or(10, |s| s.parse().unwrap());
            let full = d.announce(&args[1], port).expect("announce");
            println!("announced \"{full}\" port={port}");
            std::thread::sleep(Duration::from_secs(seconds));
            d.withdraw(&full).expect("withdraw");
            println!("withdrawn \"{full}\"");
        }
        _ => eprintln!("usage: omt-discover browse [SECONDS] | announce NAME PORT [SECONDS]"),
    }
}
