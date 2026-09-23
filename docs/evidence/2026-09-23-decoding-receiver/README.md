# Decoding receiver (`media`) against libomtnet's own decoding

Date: 23 Sep 2026. Host: macOS 26.5.1, one Mac, loopback. libomtnet 1.0.0.19 via
`interop/libomtnet-harness` built with `libvmx.dylib` from `reference/libvmx`
(STATUS.md "Rebuild recipe"). Ours: `examples/omt-recv` and `omt`, both release builds of
branch `feat/decode`, decoding with `open_media_transport::media::MediaDecoder`.

## D1: every layout, libomtnet's sender, both receivers side by side

`run.sh BIN SOURCE CASES` starts `libomtnet-harness send decode-SOURCE N SOURCE` (640x360,
30 fps, BT.709, `<HarnessFrame N="n" />\0` on every 30th frame, stereo 48 kHz with a
silent right channel), then for each case runs at the same time, for 5 s:

- libomtnet: `libomtnet-harness recv omt://127.0.0.1:PORT 5 MODE FORMAT`, which opens
  `OMTReceive` with that `OMTPreferredVideoFormat` and prints the FNV-1a 64 of all
  `DataLength` bytes of each decoded frame (`pixels` lines) and of decoded audio.
- ours: `omt-recv 127.0.0.1:PORT 5 MODE --format FORMAT`, same hashes of `VideoFrame::data`
  and of `AudioFrame::samples` as little-endian f32.

`compare.py` joins the video hashes on the per-frame metadata and the audio hashes on the
timestamp. Result (`comparison.txt`): **23 cases, every frame both receivers saw is
byte-identical, video and audio; libomtnet and we chose the same layout in every case.**

| harness source (flags) | preferred formats checked (full frames) | preview |
|---|---|---|
| UYVY (0) | UYVY, BGRA, UYVYorBGRA, P216 | UYVY, BGRA |
| UYVA (alpha, 2) | UYVYorUYVA, BGRA, UYVYorBGRA, UYVYorUYVAorP216orPA16, UYVY | UYVYorUYVA, BGRA |
| P216 (10-bit, 16) | UYVYorUYVAorP216orPA16, P216, UYVY, BGRA | UYVYorUYVAorP216orPA16 |
| PA16 (10-bit + alpha, 18) | UYVYorUYVAorP216orPA16, UYVYorUYVA, UYVYorBGRA, P216 | BGRA |

So these decoded layouts are verified against libomtnet: UYVY, UYVA, BGRA (with alpha and
as BGRX), P216 (from 10-bit and from 8-bit streams), PA16, and the preview UYVY, UYVA and
BGRA/BGRX; FPA1 audio with a silent channel re-inserted (A3). The layout table in
`PreferredVideoFormat`'s docs matches what libomtnet picked (V6). Files:
`SOURCE-MODE-FORMAT-libomtnet.txt` / `-ours.txt`, `send-SOURCE.txt`.

Not covered: interlaced sources, BT.601 (the harness sends BT.709; the BT.601 and
interlaced BGRA paths are checked against the libvmx C++ reference instead, in
`crates/open-media-transport/tests/media_libvmx.rs`), sizes other than 640x360, and the
`PreferredVideoFormat::P216` + preview case (libomtnet decodes nothing; we return
`NoMatchingFormat`; not run live).

## D2: 10-bit end to end with the CLI

`snapshots.sh BIN`:

- `omt send --10bit` (our sender, P216 source): libomtnet's receiver, preferring
  `UYVYorUYVAorP216orPA16`, delivered `codec=P216 flags=16`
  (`omt-send-10bit-libomtnet-recv.txt`).
- `omt recv --snapshot snap-omt-send-10bit.png` of that stream wrote a 16-bit RGB PNG
  (`file`, `sips`: `snapshot-file-info.txt`). `png_levels.py` (standard library only)
  decodes it: the bottom row, a grey ramp, has **557 distinct levels** (`png-levels.txt`);
  an 8-bit limited-range source cannot have more than 220 grey levels.
- The same stream saved as `.bmp` gives an 8-bit BMP and says so.
- libomtnet's P216 sender, snapshotted by `omt recv`: 16-bit PNG
  (`snap-libomtnet-p216.png`); its UYVY sender: 8-bit BMP. The BMPs were not kept (700 KB
  each); their `file`/`sips` output is in `snapshot-file-info.txt`.

All processes ended with their `--seconds` limits; none were left running.
