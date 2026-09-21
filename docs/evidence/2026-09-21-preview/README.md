# Preview mode, both directions, and upstream bug U2

Date: 21 Sep 2026. Host: macOS 26.5.1. Tools outside Claude: `tshark` on `lo0`.
libomtnet 1.0.0.19 via `interop/libomtnet-harness` (`recv ADDRESS SECONDS preview` sets
`OMTReceiveFlags.Preview`); ours via `omt-recv ADDRESS SECONDS preview` and `omt-send`.
Same test source as the other runs (640x360, `<HarnessFrame N="n" />\0` every 30th frame).

## P1 — libomtnet sender, libomtnet and our receivers both in preview

- Both receivers got 80x45 preview frames with flag 8 (P1, P3): `p1-*-recv-preview.txt`.
- Decoded preview pixels identical between libomtnet (`VMX_DecodePreviewUYVY`) and us
  (`vmx_codec::Decoder::decode_preview`, packed to UYVY): 5 of 5 frames both saw.
- **U2 confirmed.** On frames that carry per-frame metadata, both receivers got binary
  garbage instead of `<HarnessFrame …/>`, e.g. `\x9B\x08\x00\x00\xC5\x0A\xDA\x9D…` — the
  bytes following the VMX preview prefix, since libomtnet sends the first
  `EncodedPreviewLength + MetadataLength` bytes of the full frame (§6.2). libomtnet's own
  receiver shows the same garbage, so this is upstream behaviour, not a parsing error of ours.

## P2 — our sender, both receivers in preview

- libomtnet's receiver got 80x45 preview frames **with the correct metadata**
  (`<HarnessFrame N="120" />\0`): our sender appends the real metadata after the prefix.
- Preview pixels identical between the two receivers: 5 of 5.
- Preview pixels from our sender equal those from libomtnet's sender in P1 for the same
  frames (N=120 `6ed6ddd5c2557c65`, N=150 `a7b39d5ff9c1cc45`, N=180 `12b3314f0722bf25`).
- 0 frames dropped (`p2-omt-send.txt`).

`pixel-hashes.txt` has the comparisons; the `*-control-packets.pcapng` files keep the
handshakes (including `<OMTSettings Preview="true" />`) and small frames.
