# Interop matrix

Stage 5 of [`PROTOCOL_PLAN.md`](PROTOCOL_PLAN.md). One row per pairing actually run,
with versions, date and evidence. A pairing not in this table has not been tested,
whatever the code suggests.

| Date | Our side | Their side | Their version | OS / link | Result | Evidence |
|---|---|---|---|---|---|---|
| 2026-09-21 | receiver (`omt-recv`, literal address) | libomtnet sender | 1.0.0.19 (`029ef4e`), libvmx `544bcfb` | macOS 26.5.1, loopback | **works**: handshake byte-identical to libomtnet's receiver; 210 video + 209 audio frames; decoded pixels identical to libomtnet's receiver on every compared frame | [`evidence/2026-09-21-our-receiver-vs-libomtnet`](evidence/2026-09-21-our-receiver-vs-libomtnet/README.md) |
| 2026-09-21 | discovery browse + receiver by name | libomtnet sender | 1.0.0.19 | macOS 26.5.1, loopback | **works**: found `SAULS-MACBOOK-PRO.LOCAL (Disc A)`, connected, 121 video + 120 audio frames decoded | [`evidence/2026-09-21-our-discovery`](evidence/2026-09-21-our-discovery/README.md) (A) |
| 2026-09-21 | discovery announce | libomtnet discovery | 1.0.0.19 | macOS 26.5.1, one host | **works**: libomtnet lists `SAULS-MACBOOK-PRO.LOCAL (Rust Source)`; no connection attempted (no sender yet) | [`evidence/2026-09-21-our-discovery`](evidence/2026-09-21-our-discovery/README.md) (B) |
| 2026-09-21 | sender (`omt-send`) + discovery announce | libomtnet receiver, found by name | 1.0.0.19 | macOS 26.5.1, one host (LAN address) | **works**: libomtnet discovered, connected, received 180 video + 179 audio; its decoded pixels identical to ours; our output pixel-identical to libomtnet's own sender for the same input; 0 drops | [`evidence/2026-09-21-our-sender-vs-libomtnet`](evidence/2026-09-21-our-sender-vs-libomtnet/README.md) |
| 2026-09-21 | receiver in preview | libomtnet sender | 1.0.0.19 | macOS 26.5.1, loopback | **works**: 80x45 preview, pixels identical to libomtnet's receiver; upstream's garbage per-frame metadata (U2) seen by both | [`evidence/2026-09-21-preview`](evidence/2026-09-21-preview/README.md) (P1) |
| 2026-09-21 | sender serving preview | libomtnet receiver in preview | 1.0.0.19 | macOS 26.5.1, LAN address | **works**: preview pixels identical on both receivers and to libomtnet's sender; correct per-frame metadata | [`evidence/2026-09-21-preview`](evidence/2026-09-21-preview/README.md) (P2) |
| 2026-09-21 | receiver reconnect | libomtnet sender, restarted | 1.0.0.19 | macOS 26.5.1, loopback | **works**: closed on reset, reconnected within 1 s, frames resumed from the new sender | [`evidence/2026-09-21-reconnect`](evidence/2026-09-21-reconnect/README.md) |
| 2026-09-21 | **released binary** `omt` v0.1.0 (aarch64-apple-darwin, downloaded from the draft GitHub release) — send and recv | libomtnet receiver / sender | 1.0.0.19 | macOS 26.5.1, one host | **works** both ways: 120 video frames at 1280x720 received by libomtnet; 30 fps, 0 decode errors, snapshot saved from libomtnet's stream | this table row; archive SHA-256 prefix `24b46152c6545d5c` |
| 2026-09-28 | discovery browse + announce | our `omt` on the other machine | 0.1.0 (`48c3984`) | macOS 26.5.1 arm64 ↔ Linux Mint 22.3 x86_64, wired LAN | **works** both ways across the network: macOS mDNSResponder and Linux avahi each list the other's source with the right host, IP and port | [`evidence/2026-09-28-cross-machine`](evidence/2026-09-28-cross-machine/README.md) §1 |
| 2026-09-28 | sender (Mac, arm64) | our receiver (NUC, x86_64) | 0.1.0 (`48c3984`) | macOS 26.5.1 ↔ Linux Mint 22.3, wired LAN | **works**: 1280x720 30 fps, ~5.9 Mbit/s, 48 kHz stereo, 0 decode errors over 8 s; snapshot decoded correctly on x86_64 from an arm64 VMX1 stream | [`evidence/2026-09-28-cross-machine`](evidence/2026-09-28-cross-machine/README.md) §2 |
| 2026-09-28 | receiver (Mac, arm64) | our sender (NUC, x86_64) | 0.1.0 (`48c3984`) | macOS 26.5.1 ↔ Linux Mint 22.3, wired LAN | **works**: 1280x720 30 fps, 0 decode errors; snapshot decoded correctly on arm64 from an x86_64 VMX1 stream | [`evidence/2026-09-28-cross-machine`](evidence/2026-09-28-cross-machine/README.md) §3 |
| 2026-09-28 | receiver reconnect by name | our sender (NUC, x86_64), restarted | 0.1.0 (`48c3984`) | macOS 26.5.1 ↔ Linux Mint 22.3, wired LAN | **works**: Mac receiver lost the NUC sender, retried, re-resolved the name over mDNS and reconnected on its own, resuming at 30 fps with 0 decode errors | [`evidence/2026-09-28-cross-machine`](evidence/2026-09-28-cross-machine/README.md) §4 |

Rows dated 2026-09-28 are `omt`-to-`omt` between two machines — the first runs on
a real network, on Linux, and on x86_64. They do not involve a third-party product.

## Not yet tested

- Redirect between separate machines, the discovery server between separate machines.
- vMix, OBS with the OMT plugin, SIENNA tools, the Raspberry Pi encoder/decoder.
- Windows (the v0.1.0 Windows binary was built by CI but never run), and Wi-Fi /
  cross-subnet networks (the 2026-09-28 tests were on a single wired subnet).
