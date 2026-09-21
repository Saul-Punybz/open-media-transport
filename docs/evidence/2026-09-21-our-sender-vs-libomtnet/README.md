# Our sender against a libomtnet receiver

Date: 21 Sep 2026. Host: macOS 26.5.1, Apple Silicon. Tools outside Claude: `tshark` on `lo0`.

## What ran, at the same time

- **Our sender**: `cargo run --release -p open-media-transport --example omt-send -- "Rust Harness" 13`
  (`src/sender.rs`, encoding with `vmx-codec`, announcing with `src/discovery.rs`). It sends the
  same input as `libomtnet-harness send`: 640x360 UYVY ramp at 30 fps, stereo FPA1 with a
  silent right channel, `<HarnessFrame N="n" />\0` every 30th frame.
- **libomtnet receiver** (1.0.0.19): `libomtnet-harness recv "SAULS-MACBOOK-PRO.LOCAL (Rust Harness)" 7`,
  so libomtnet's own DNS-SD found our source by name.
- **Our receiver**: `omt-recv 127.0.0.1:6400 7`.

## Results

| Check | Result | File |
|---|---|---|
| libomtnet discovery finds us | resolved our instance and our SRV host `Sauls-MacBook-Pro-omt.local.`, connected to `172.16.80.59:6400` | `tcp-conversations.txt` |
| libomtnet's handshake to us | its usual sequence (SubscribeMetadata, SubscribeVideo, Quality High, Tally program; SubscribeAudio) | `up-52670.dump.txt`, `up-52671.dump.txt` |
| What we send on accept | `OMTInfo`, connection metadata, combined tally — §4.2 order; then video / audio only on the subscribed connection | `down-52670.dump.txt`, `down-52671.dump.txt` (183 frames each, no parse errors) |
| libomtnet receives and decodes | 180 video, 179 audio, sender info `omt-send/open-media-transport/0.1.0` | `libomtnet-recv.txt` |
| Tally back to us | our sender saw program on, then off when receivers left | `omt-send.txt` |
| Drops | `queued=795 dropped=0` | `omt-send.txt` |
| Pixels: libomtnet's decode vs ours | identical hash on all 6 frames both decoded | `pixel-hashes.txt` |
| Pixels vs source | PSNR 58.0–58.9 dB; audio RMS 0.1764, right channel silent | `omt-recv.txt` |
| **Our sender vs libomtnet's sender** | for the same input frames N=120…300, the decoded pixels are identical to those libomtnet's sender produced in the earlier run (7 of 7); frame N=120 is 57,166 bytes in both | `pixel-hashes.txt`, `../2026-09-21-our-receiver-vs-libomtnet/` |

The last row says more than "libomtnet can decode us": profile choice (a High
suggestion selects `OMT_HQ`), rate control and the codec all behave the same as
libomtnet + libvmx for this input. Only decoded pixels and one frame size were
compared, not every bitstream byte.

## A bug this run found first

The first attempt produced 6.6 MB frames and 9 dB PSNR: `vmx_codec::Encoder::encode_into`
appends, and the sender reused its buffer without clearing it, so every frame carried all
earlier ones. The two receivers still agreed on the (wrong) pixels, which is why PSNR against
the source is part of the check. Fixed, with a regression test
(`sender::tests::repeated_frames_do_not_grow`) that also requires the first frame to be
byte-identical to a fresh `OMT_SQ` encoder's output.

## Not shown

vMix, OBS, a Pi; another machine or OS; preview mode against libomtnet (our preview path
is unit-tested only); many receivers at once; long runs.
