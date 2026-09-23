# vmx-codec

A pure-Rust implementation of **VMX**, the video codec of
[Open Media Transport](https://www.openmediatransport.org/) (OMT).

**OMT** is an open protocol for sending live video, audio and metadata over a
local network with low latency, the same job NDI does, but under the MIT
license. It was created by the team behind vMix. **VMX** is its codec. It
started out as the vMix Video Codec used for vMix Instant Replay, and is built
for very fast software encoding and decoding:

- intra-frame only (no motion prediction), so latency stays low and any frame can be cut or decoded on its own
- 4:2:2, or 4:2:2:4 with an alpha plane
- 8-bit and 10-bit
- 8x8 fixed-point DCT, 25 quality presets, Exp-Golomb style entropy coding
- slices of 16 lines, each coded on its own, so work splits cleanly across threads

This crate is a **port of [libvmx](https://github.com/openmediatransport/libvmx)**,
the MIT-licensed C++ reference implementation. It has no dependencies and
matches libvmx byte for byte (see [Conformance](#conformance)). It is safe
Rust except for one module, `simd`, which implements the transform's 128-bit
operations with `std::arch` NEON (aarch64) or SSE2 (x86-64) intrinsics
(`#![deny(unsafe_code)]` everywhere else). The same operations also exist as
portable Rust, used on other targets and as the reference the SIMD versions
are tested against.

## Usage

```rust
use vmx_codec::{Decoder, Encoder, EncoderConfig, Frame, PixelFormat, Profile};

fn main() -> Result<(), vmx_codec::Error> {
    let (w, h) = (1920, 1080);
    let mut config = EncoderConfig::new(w, h);
    config.profile = Profile::OmtHq; // the profile OMT senders use
    config.threads = 4;              // slices are coded in parallel; output is the same
    let mut encoder = Encoder::new(config)?;

    let frame = Frame::new(w, h, PixelFormat::Uyvy); // fill frame.planes[0].data
    let packet: Vec<u8> = encoder.encode(&frame)?;

    let mut decoder = Decoder::new(w, h)?;
    let out: Frame = decoder.decode(&packet, PixelFormat::Uyvy)?;
    assert_eq!(out.width, w);
    Ok(())
}
```

### API

| Item | Purpose |
|---|---|
| `Encoder::new(EncoderConfig { width, height, profile, threads })` | Encoder for one frame size. `Profile::{Lq, Sq, Hq, OmtLq, OmtSq, OmtHq}` set the bitrate target, the lowest allowed quality and the DC precision. |
| `encoder.encode(&Frame) -> Result<Vec<u8>>` | Compresses one frame. After each frame the quality moves toward the profile's bitrate window, the same way libvmx does it. |
| `encoder.set_quality(q)` / `quality()` | Forces the quality (0 to 100, clamped to `min_quality..=98`) for the next frame. |
| `encoder.set_encoding_parameters(..)` | Sets the bitrate window, the lowest allowed quality and the DC shift. |
| `Decoder::new(width, height)` | Decoder for one frame size. The bitstream does not store the frame size. |
| `decoder.decode(&[u8], PixelFormat) -> Result<Frame>` | Decompresses one frame into the layout you ask for. |
| `decoder.decode_preview(&[u8], alpha)` | 1/8-scale preview built from the DC coefficients only. It needs only the first `decoder.preview_len(..)` bytes. |
| `decoder.info(&[u8])` | Reads the header: quality, DC shift, interlaced flag. |

### Pixel formats (explicit plane layouts)

| `PixelFormat` | Planes (bytes per row × rows) | Depth | Encode | Decode |
|---|---|---|---|---|
| `Uyvy` | `U Y V Y`: `2w × h` | 8 | ✓ | ✓ |
| `Yuy2` | `Y U Y V`: `2w × h` | 8 | ✓ | ✓ |
| `Uyva` | UYVY `2w × h`, then alpha `w × h` | 8 | ✓ | ✓ |
| `Yuv422p` | Y `w × h`, U `w/2 × h`, V `w/2 × h` | 8 | ✓ | ✓ |
| `Yuva422p` | as `Yuv422p`, then A `w × h` | 8 | ✓ | ✓ |
| `P216` | Y `2w × h` (u16 LE), interleaved UV `2w × h` (u16 LE) | 10 | ✓ | ✓ |
| `Pa16` | as `P216`, then A `2w × h` (u16 LE) | 10 | ✓ | ✓ |
| `Nv12` | Y `w × h`, interleaved UV `w × h/2` | 8 | ✓ | – |
| `I420` | Y `w × h`, U `w/2 × h/2`, V `w/2 × h/2` | 8 | ✓ | – |

Each `Plane` has its own `stride` in bytes. 16-bit samples keep their 10
significant bits at the top of the word (`value << 6`). 4:2:0 input becomes
4:2:2 by repeating each chroma line, the same way libvmx does it.

A VMX frame does not say how wide it is, whether it is 8-bit or 10-bit, or
whether it has alpha. The transport carries those facts (OMT puts them in its
frame header), so the decoder needs them from you: `P216`/`Pa16` pick the
10-bit path, and formats with alpha decode the fourth plane.

## Bitstream

```
[3 if dc_shift > 0][dc_shift]   optional extended header
[1 = progressive | 2 = interlaced][quality][slice count (low 8 bits)]
slice count × { u32 LE length, DC stream }
slice count × { u32 LE length, AC stream }   (left out in a DC-only preview frame)
```

Each slice stream holds the Y, U, V (and A) planes of 16 lines. Each plane
is padded to a whole byte. DC values are coded as the difference from the
previous DC. AC coefficients use zig-zag order, with zero runs that can
continue into the next block. `src/slice.rs` describes the codes.

## Conformance

`tests/conformance.rs` builds libvmx from source (only as a test dependency,
in the unpublished `libvmx-ref` crate; `vmx-codec` itself never links C) and
checks the following for gradients, noise, colour bars and a mixed
high-frequency pattern, at sizes from 16×16 to 1920×1080, across all six
profiles, several quality settings, 8-bit and 10-bit, with and without alpha,
progressive and interlaced:

1. **Encoder:** the Rust bitstream is **byte-identical** to libvmx's.
2. **Decoder:** decoding a libvmx frame gives **identical pixels** to libvmx.
3. **Cross-decoding:** libvmx decodes Rust frames to identical pixels.
4. **Rate control:** the quality chosen frame by frame over a sequence matches libvmx.
5. **4:2:0 input and the DC preview:** both match libvmx.

The reference is libvmx's 128-bit SIMD path (SSE on x86-64, NEON via
sse2neon on ARM). libvmx has no plain scalar C path. Its AVX2 path is not
tested here. The unit tests also check this crate's NEON / SSE2 kernels
against its portable ones on random and extreme inputs.

To run the tests, place a checkout of libvmx at `reference/libvmx` (or set
`LIBVMX_SRC` to its `src` directory). Without it, the conformance tests skip
themselves.

## Status

- **Ported:** the container format, the forward and inverse DCT, quantisation,
  entropy coding, 8-bit and 10-bit, alpha, interlaced frames, rate control,
  DC-only preview decoding, and UYVY / YUY2 / UYVA / P216 / PA16 / planar 4:2:2,
  plus NV12 / I420 input.
- **Not yet:** BGRA/BGRX input and output (libvmx's RGB↔YUV conversion), an
  AVX2 path, and the libvmx helpers `BGRXToUYVY` and `CalculatePSNR`.
- **Differences from libvmx:** At some sizes, libvmx's packed-to-planar
  conversion copies uninitialised memory into the padding rows or columns of
  its internal planes. This happens with UYVY/YUY2 rows that are not a
  multiple of 64 bytes when the frame also has padding (a height that isn't a
  multiple of 16, or a width that isn't a multiple of 16). That padding gets
  encoded, so libvmx's output is not deterministic at those sizes. This port
  fills the padding with fixed values, so its frames still decode everywhere,
  but they can't be compared with libvmx byte for byte. Code values beyond
  libvmx's 4096-entry length table (undefined behaviour in C) are encoded
  correctly here; no input I found reaches them.

See [`BENCH.md`](BENCH.md) for speed.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Copyright (c) 2026 Saul González
and Puny.bz Inc.

This crate is a port of libvmx, Copyright (c) 2025 Open Media Transport
Contributors, MIT license. The full notice is in [`NOTICE`](NOTICE).

"Open Media Transport" names the protocol this crate implements. This project
is not affiliated with or endorsed by the OMT project or vMix.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in this work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
