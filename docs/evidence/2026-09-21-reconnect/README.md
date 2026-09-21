# Receiver reconnect against a restarted libomtnet sender

Date: 21 Sep 2026. Host: macOS 26.5.1.

1. `libomtnet-harness send Harness 5` (libomtnet 1.0.0.19) started on port 6400.
2. `omt-recv 127.0.0.1:6400 14` connected (video + audio).
3. The sender exited; ~3 s later a new `libomtnet-harness send Harness 5` started, again on 6400.

`omt-recv.txt` shows, in order: `connected Video/Audio`, frames N=120 and 150 from the first
sender, `closed Video/Audio` (connection reset by peer), `connected Video/Audio` again within
the one-second retry, then frames N=30…150 from the second sender with the same PSNR
(58.4–58.9 dB). 201 video and 201 audio frames in total.

This is the behaviour of `src/receiver.rs`: when a needed connection drops, close both and
reopen them at most once a second, re-sending the current preview, quality and tally
(`docs/PROTOCOL.md` N5). Tally/quality re-sending is unit-tested
(`receiver::tests::reconnects_and_resends_current_state`), not shown here.

Not shown: a sender that comes back on a different port (a receiver given a full name would
need to re-resolve it; ours reconnects to the same address), or network loss rather than a
process restart.
