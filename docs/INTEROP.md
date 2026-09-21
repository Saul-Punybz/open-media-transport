# Interop matrix

Stage 5 of [`PROTOCOL_PLAN.md`](PROTOCOL_PLAN.md). One row per pairing actually run,
with versions, date and evidence. A pairing not in this table has not been tested,
whatever the code suggests.

| Date | Our side | Their side | Their version | OS / link | Result | Evidence |
|---|---|---|---|---|---|---|
| 2026-09-21 | receiver (`omt-recv`, literal address) | libomtnet sender | 1.0.0.19 (`029ef4e`), libvmx `544bcfb` | macOS 26.5.1, loopback | **works**: handshake byte-identical to libomtnet's receiver; 210 video + 209 audio frames; decoded pixels identical to libomtnet's receiver on every compared frame | [`evidence/2026-09-21-our-receiver-vs-libomtnet`](evidence/2026-09-21-our-receiver-vs-libomtnet/README.md) |

## Not yet tested

- Our sender against anything (there is no sender yet).
- Our discovery (none yet): we connected to `127.0.0.1:6400` directly.
- vMix, OBS with the OMT plugin, SIENNA tools, the Raspberry Pi encoder/decoder.
- Anything across a real network, or on Windows or Linux.
- Preview mode, redirect, the discovery server.
