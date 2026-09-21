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

## Not yet tested

- Preview mode against libomtnet, redirect, the discovery server.
- vMix, OBS with the OMT plugin, SIENNA tools, the Raspberry Pi encoder/decoder.
- Anything across a real network, or on Windows or Linux.
