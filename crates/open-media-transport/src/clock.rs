//! Timestamp generation and pacing for senders that have no capture clock,
//! as libomtnet does for frames sent with timestamp −1
//! (`docs/PROTOCOL.md` §5, `OMTClock.cs`).
//!
//! The first frame gets timestamp 0; each later one gets the previous plus one
//! frame interval. The clock then sleeps until wall time catches up (C3), and
//! if the caller has fallen more than one interval behind, it skips timestamps
//! forward instead of bursting. Changing the frame or sample rate restarts the
//! wall-clock reference (C4). Use one `Clock` for video and another for audio,
//! like libomtnet (`OMTSend.cs:85-86`).

use std::time::{Duration, Instant};

use crate::frame::TICKS_PER_SECOND;

/// Paces frames and stamps them in 100 ns ticks.
#[derive(Debug)]
pub struct Clock {
    rate: Option<(i64, i64)>,
    interval: i64,
    start: Instant,
    clock_ts: i64,
    last: Option<i64>,
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock {
    /// A clock that has not stamped anything yet.
    pub fn new() -> Self {
        Clock {
            rate: None,
            interval: 0,
            start: Instant::now(),
            clock_ts: 0,
            last: None,
        }
    }

    /// Timestamp for the next video frame at `n / d` frames per second,
    /// after sleeping as needed to hold that rate.
    pub fn video(&mut self, frame_rate_n: i32, frame_rate_d: i32) -> i64 {
        let key = (frame_rate_n as i64, frame_rate_d as i64);
        let interval = video_interval(frame_rate_n, frame_rate_d);
        self.next(key, interval)
    }

    /// Timestamp for the next audio frame of `samples` per channel at
    /// `sample_rate`, after sleeping as needed.
    pub fn audio(&mut self, sample_rate: i32, samples: i32) -> i64 {
        let interval = if sample_rate > 0 && samples > 0 {
            TICKS_PER_SECOND * samples as i64 / sample_rate as i64 // OMTClock.cs:66-69
        } else {
            0
        };
        self.next((sample_rate as i64, 0), interval)
    }

    fn next(&mut self, key: (i64, i64), interval: i64) -> i64 {
        if self.rate != Some(key) {
            self.reset(key, interval);
        }
        let ts = match self.last {
            None => {
                self.reset(key, interval);
                0
            }
            Some(last) => {
                self.interval = interval;
                let mut ts = last + interval;
                self.clock_ts += interval;
                let mut diff = self.clock_ts - self.elapsed_ticks();
                // Behind by more than a frame: skip ahead rather than burst.
                while interval > 0 && diff < -interval {
                    ts += interval;
                    self.clock_ts += interval;
                    diff += interval;
                }
                let ahead = self.clock_ts - self.elapsed_ticks();
                if ahead > 0 {
                    std::thread::sleep(Duration::from_nanos(ahead as u64 * 100));
                }
                ts
            }
        };
        self.last = Some(ts);
        ts
    }

    fn reset(&mut self, key: (i64, i64), interval: i64) {
        self.rate = Some(key);
        self.interval = interval;
        self.start = Instant::now();
        self.clock_ts = 0;
    }

    /// libomtnet measures wall time in whole milliseconds
    /// (`clock.ElapsedMilliseconds * 10000`, `OMTClock.cs:73,79`).
    fn elapsed_ticks(&self) -> i64 {
        self.start.elapsed().as_millis() as i64 * 10_000
    }
}

/// `10^7 / fps` where fps is `n / d` rounded to two decimals and held in an
/// `f32`, then truncated — libomtnet's arithmetic (`OMTUtils.cs:168-174`,
/// `OMTClock.cs:96-99`). 30000/1001 gives 333667, not 333666.
pub fn video_interval(frame_rate_n: i32, frame_rate_d: i32) -> i64 {
    if frame_rate_d == 0 {
        return 0;
    }
    let fps = ((frame_rate_n as f64 / frame_rate_d as f64) * 100.0).round() / 100.0;
    let fps = fps as f32;
    if fps <= 0.0 {
        return 0;
    }
    (TICKS_PER_SECOND as f32 / fps) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intervals_match_libomtnet_arithmetic() {
        assert_eq!(video_interval(30, 1), 333_333); // seen on the wire, C2
        assert_eq!(video_interval(30000, 1001), 333_667);
        assert_eq!(video_interval(60000, 1001), 166_833);
        assert_eq!(video_interval(25, 1), 400_000);
        assert_eq!(video_interval(1, 0), 0);
    }

    #[test]
    fn stamps_and_paces() {
        // What holds however loaded the machine is: stamps start at 0, are
        // whole intervals, strictly increase, and are never handed out before
        // wall time has reached them. (A late sleep makes the clock skip
        // ahead, which a loaded CI runner does; see skips_ahead_when_late.)
        let mut c = Clock::new();
        let t0 = Instant::now();
        let mut last = -1;
        for i in 0..6 {
            let ts = c.video(20, 1); // 50 ms interval
            if i == 0 {
                assert_eq!(ts, 0);
            }
            assert_eq!(ts % 500_000, 0, "{ts}");
            assert!(ts > last, "{ts} after {last}");
            let elapsed_ticks = t0.elapsed().as_millis() as i64 * 10_000;
            assert!(
                elapsed_ticks >= ts,
                "stamp {ts} handed out at {elapsed_ticks}"
            );
            last = ts;
        }
        assert!(last >= 2_500_000, "five intervals paced, got {last}");
    }

    #[test]
    fn skips_ahead_when_late() {
        let mut c = Clock::new();
        assert_eq!(c.video(100, 1), 0);
        std::thread::sleep(Duration::from_millis(55));
        let ts = c.video(100, 1);
        assert!(ts >= 400_000, "should have skipped missed frames, got {ts}");
        assert_eq!(ts % 100_000, 0);
    }

    #[test]
    fn audio_interval() {
        let mut c = Clock::new();
        assert_eq!(c.audio(48000, 1600), 0);
        assert_eq!(c.audio(48000, 1600), 333_333);
    }
}
