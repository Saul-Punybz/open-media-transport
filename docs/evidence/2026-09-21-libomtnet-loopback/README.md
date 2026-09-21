# libomtnet talking to itself, captured

Date: 21 Sep 2026. Host: macOS 26.5.1, Apple Silicon, .NET SDK 10.0.401.
Tools outside Claude: `tshark` (Wireshark CLI), Apple `dns-sd`.

## What ran

Upstream libomtnet at `029ef4e` (v1.0.0.19), built from `reference/libomtnet`
through `interop/libomtnet-harness`, with libvmx at `544bcfb` built as an arm64
dylib. Both ends are libomtnet; none of our code is on the wire.

```sh
tshark -i lo0 -f "tcp portrange 6400-6600" -a duration:16 -w omt-lo0.pcapng
dotnet libomtnet-harness.dll send Harness 11                              # sender
dotnet libomtnet-harness.dll recv "SAULS-MACBOOK-PRO.LOCAL (Harness)" 5   # receiver
```

The sender sends 640x360 UYVY at 30 fps (encoded to VMX1 by libomtnet), stereo
FPA1 audio whose right channel is silent, per-frame metadata
`<HarnessFrame N="…" />\0` every 30th frame, sender info, and one connection
metadata string. The receiver asks for video, audio and metadata, sets tally to
program and suggested quality to High before connecting.

Separately, `dns-sd -B`, `-L` and `-Z` were run while the sender announced
`Cam 1.5`.

## Files

| File | What |
|---|---|
| `libomtnet-loopback-first120.pcapng` | First 120 packets of the 883 captured (the full file is 8.3 MB): both connections' handshakes and the first frames |
| `tcp-conversations.txt` | `tshark -z conv,tcp` over the full capture: exactly two TCP connections |
| `s0-c2s.dump.txt`, `s0-s2c.dump.txt` | Connection 0 (video + metadata), each direction, parsed by our `omt-dump` |
| `s1-c2s.dump.txt`, `s1-s2c.dump.txt` | Connection 1 (audio), each direction |
| `harness-send.txt`, `harness-recv.txt` | What libomtnet's own API reported |
| `dnssd-browse.txt`, `dnssd-lookup.txt`, `dnssd-zone.txt` | The DNS-SD records libomtnet published |

The `.dump.txt` files were made by extracting each direction with
`tshark -q -z follow,tcp,raw,N`, converting hex to binary, and running
`cargo run -p open-media-transport --example omt-dump`. All four streams parsed
completely: 4 + 1 frames upstream, 124 + 123 frames downstream, no errors, no
trailing bytes.

## What it confirms (statement IDs from `docs/PROTOCOL.md`)

| ID | Observed |
|---|---|
| T5 | Two TCP connections from one receiver to port 6400 (`tcp-conversations.txt`) |
| T2 | Sender took port 6400, the first of the range |
| §3.1–3.4 | Headers parse as specified; video `VMX1` 640x360, rate 30/1, aspect 1.7778, flags 0, colour space 709; audio `FPA1`, reserved 0 |
| §3.2, M4 | Per-frame metadata is the last `MetadataLength` bytes and keeps the application's NUL: `mlen=25`, `…/>\x00`, `dlen = 32 + data + 25` |
| M1, M2, M5 | Commands and sender info are metadata frames with timestamp 0 and **no NUL**: lengths 32, 29, 30, 44, 45, 87 are exactly the strings |
| §4.1 | `SubscribeMetadata`, `SubscribeVideo`, `SubscribeAudio`, `Quality="High"`, tally with `Program==`, byte for byte |
| §4.2 | Sender info exact bytes: `<OMTInfo ProductName="omt-harness" Manufacturer="open-media-transport" Version="0.1" />` (one line, one space before `/>`) |
| §4.2 | On accept, each connection gets sender info, then connection metadata, then tally (`TALLY_NONE`) |
| §4.2, M6 | The later combined tally (program) went only to connection 0, the one that subscribed to metadata |
| §4.3 | Video connection: SubscribeMetadata, SubscribeVideo, Quality, Tally. Audio connection (video also requested): SubscribeAudio only |
| C2 | Generated timestamps step by 333333 = floor(10^7 / 30) |
| A2 | Silent right channel omitted: `active=0x1`, 6400 bytes = 1600 samples x 4 |
| D2, D3 | Instance `SAULS-MACBOOK-PRO.LOCAL (Cam 1.5)`: `gethostname()` upper-cased, `.LOCAL` included, dot in the source name kept on macOS |
| D5 | TXT record present but empty (`""`) |

SRV target is the machine's real mDNS host, `Sauls-MacBook-Pro.local.`, supplied by
Apple's DNS-SD since libomtnet passes no host.

## What it does not confirm

- M3 (a command with a NUL is not recognised) — needs a peer that sends one; our crate can.
- Anything about vMix, OBS or a Pi: this is libomtnet against itself, on one machine, over loopback.
- Preview mode, redirect, discovery server, Windows or Linux behaviour.
