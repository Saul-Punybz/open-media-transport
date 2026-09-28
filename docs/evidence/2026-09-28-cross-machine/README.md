# 2026-09-28 — `omt` across two machines, two OSes, two architectures

The first time this project has run over a real network between separate
computers, and the first time its code has run on Linux or on x86_64 at all.
Every pairing before this was `omt` (or libomtnet) against libomtnet on **one
Mac**, over loopback or a single host's LAN address.

## Machines

| Role | Host | OS | Arch | `omt` |
|---|---|---|---|---|
| A | `Sauls-MacBook-Pro` (172.16.80.64) | macOS 26.5.1 (25F80) | arm64 (Apple M4) | v0.1.0, commit `48c3984`, built with the repo's toolchain |
| B | `punybz-NUC7i3BNHXF` (172.16.80.73) | Linux Mint 22.3, kernel 7.0.0-34-generic | x86_64 (Intel NUC i3) | v0.1.0, commit `48c3984`, built with rustc 1.90.0 |

Same source commit on both. Release binaries:

- Mac: `sha256 b8b7baf133dc6758c7798bbb068dd6e32d3de0fc247d356b5f12f70774df104e`
- NUC: `sha256 86aaf7070efaa70632cb0941d9c53581aec2d6f8fe948c3ff81ebc8bb4751bbe`

(Binaries differ by architecture, as expected; both from `48c3984`.)

Wi-Fi, both machines on the same network and subnet `172.16.80.0/22` (the NUC on
`wlp58s0`, whose Ethernet port had no cable, and the Mac on `en0`; corrected 28 Sep 2026
from "wired LAN", see `2026-09-28-fix-library-two-machines`), no client isolation. macOS application
firewall off. Discovery is mDNS only: macOS uses its own `mDNSResponder`, the
NUC uses `avahi-daemon 0.8`. No discovery server was used.

Both sides are **our** `omt`. This is not a test against vMix, OBS, SIENNA or a
Raspberry Pi — those remain untested. What is new here is: a real network, two
machines, and the codec running on x86_64 (encode and decode) for the first time.

## 1. Discovery, both directions (macOS mDNSResponder ↔ Linux avahi)

**B sends, A lists** — `omt list` on the Mac:

```
"PUNYBZ-NUC7I3BNHXF (NUC Bars)"    punybz-NUC7i3BNHXF-omt.local:6400  [172.16.80.73]
```

**A sends, B lists** — `omt list` on the NUC:

```
"SAULS-MACBOOK-PRO.LOCAL (Mac Bars)"    Sauls-MacBook-Pro-omt.local:6400  [172.16.80.64]
```

Each machine's browser saw the other's announcement across the two mDNS stacks,
resolved the `<host>-omt.local` SRV target to the right IP and port, and showed
the source name. **Works both ways.**

## 2. Video + audio, A → B (arm64 encode → x86_64 decode)

Mac `omt send --size 1280x720 --fps 30`; NUC `omt recv … --snapshot mac_to_nuc.png`.
Eight seconds, every second reported:

```
video 1280x720 30.00 fps | ~30 fps received | ~5.9 Mbit/s | audio 48000 Hz 2 ch | decode errors 0
```

`OMTInfo` metadata and tally received on both the video and audio channels.
0 decode errors throughout. Snapshot `mac-to-nuc.png` (1280×720): the SMPTE-style
colour bars with the moving box and the grey-ramp bar, decoded correctly on x86_64
from an arm64 VMX1 stream.

## 3. Video + audio, B → A (x86_64 encode → arm64 decode)

NUC `omt send --size 1280x720 --fps 30`; Mac `omt recv … --snapshot nuc_to_mac.png`.

```
video 1280x720 30.00 fps | ~29 fps received (22–34 jitter over the LAN) | ~5.8 Mbit/s | audio 48000 Hz 2 ch | decode errors 0
```

0 decode errors. Snapshot `nuc-to-mac.png` (1280×720): the same pattern, correct,
this time decoded on arm64 from an x86_64 stream. (The box sits at a different x
than in test 2 because it moves over time; the snapshots are from different moments.)

## 4. Reconnect by name across the network

Mac `omt recv "PUNYBZ-NUC7I3BNHXF (Recon)"` for 22 s. The NUC sender ran 0–5 s,
stopped, and started again around 10 s. Full log in `reconnect-mac-receiver.log`.
The receiver connected, received at 30 fps with 0 decode errors, saw the sender
vanish (`disconnected … retrying`, fps → 0), and when the NUC sender came back it
**re-resolved the name over mDNS and reconnected on its own**, resuming at 30 fps
with 0 decode errors. Both senders happened to bind port 6400 (each was free by the
time the next started), so this shows browse-again-and-reconnect; the
different-port re-resolution case is covered in `2026-09-23-addressing` against
libomtnet.

## What this does and does not prove

- **Does:** `omt` discovers, sends and receives across a real LAN between two
  separate machines; macOS mDNSResponder and Linux avahi interoperate for OMT
  discovery; the VMX codec encodes and decodes correctly on x86_64 as well as
  arm64, each decoding the other's stream with no errors; the receiver reconnects
  by name across the network.
- **Does not:** say anything about vMix, OBS, SIENNA or the Raspberry Pi tools
  (still untested), and does not compare pixels byte-for-byte between the two
  machines (the box moves, so the snapshots are from different frames — codec
  byte-equality is established separately by the conformance tests and the
  libomtnet interop rows). No Windows machine was involved.
