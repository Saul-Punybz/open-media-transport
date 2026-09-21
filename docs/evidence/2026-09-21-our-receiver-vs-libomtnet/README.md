# Our receiver against a libomtnet sender

Date: 21 Sep 2026. Host: macOS 26.5.1, Apple Silicon. Tools outside Claude: `tshark`.
First time code from this repository has talked to another OMT implementation.

## What ran, at the same time

- **Sender**: upstream libomtnet `029ef4e` (v1.0.0.19) via `interop/libomtnet-harness send Harness 13`:
  640x360 UYVY at 30 fps (libomtnet encodes it to VMX1), stereo FPA1 with a silent right
  channel, `<HarnessFrame N="n" />\0` per-frame metadata every 30th frame.
- **libomtnet receiver**: `libomtnet-harness recv "SAULS-MACBOOK-PRO.LOCAL (Harness)" 7`, found by DNS-SD.
- **Our receiver**: `cargo run --release -p open-media-transport --example omt-recv -- 127.0.0.1:6400 7`
  (`crates/open-media-transport/src/receiver.rs` + `vmx-codec`). Both receivers ask for
  video, audio and metadata with quality High and tally program.
- `tshark -i lo0 -f "tcp portrange 6400-6600" -a duration:17`

## Results

| Check | Result | File |
|---|---|---|
| Connections | 4 TCP connections to port 6400: two per receiver (T5) | `tcp-conversations.txt` |
| Our handshake on the wire | video connection 199 bytes, audio connection 45 bytes; **byte-identical** to libomtnet's receiver in the same capture (SHA-256 prefixes `a4d36cc990024b9b`, `9a0a24b268f28d36`). Only segmentation differs: we send one TCP segment, libomtnet one per command | `control-packets.pcapng` |
| Frames received and parsed | 210 video, 209 audio, all metadata; no parse or decode error | `omt-recv.txt` |
| Decoded pixels vs libomtnet's | **identical** FNV-1a 64 hash on all 6 frames both receivers decoded (N=150…300); N=120 arrived before the libomtnet receiver connected | `pixel-hashes.txt` |
| Decoded pixels vs the source pattern | PSNR 58.0–58.9 dB | `omt-recv.txt` |
| Audio | channel 0 RMS 0.1770 (a 0.25-amplitude sine gives 0.1768); channel 1 absent from the data and reconstructed as silence (A2, A3) | `omt-recv.txt` |
| Sender reaction | libomtnet's sender reported tally program while we were connected | `libomtnet-send.txt` |

`control-packets.pcapng` keeps only small packets (all upstream traffic and downstream
packets under 200 bytes) of the 2,848 captured, so it holds every handshake and
control message but not the video and audio.

## What this shows and what it does not

- **Shows**: our receiver can connect to libomtnet, subscribe, and get the same decoded
  video libomtnet's own receiver gets.
- The pixel match covers our VMX1 decoding end to end over the network. `vmx-codec`
  was already byte-identical to libvmx in unit tests; this confirms it on frames
  libomtnet chose to encode.
- **Does not show**: anything about vMix, OBS or a Pi; discovery by our code (we
  connected to a literal address); preview mode; behaviour on another machine or OS.
