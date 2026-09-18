# Benchmark: vmx-codec (Rust scalar) vs libvmx (C++ SIMD)

Measured 18 Sep 2026 on an Apple M4 laptop (arm64, macOS). Other builds were
running at the same time (load average about 5), so treat these as rough
figures, not a lab result. Run it yourself with:

```sh
cargo run --release -p vmx-codec --example bench_vs_libvmx -- 60
```

Setup: 1920x1080 UYVY 8-bit, 60 frames, **one thread on each side**,
quality fixed per frame. The test image is a synthetic picture made of
gradients, mild noise and hard edges. Before timing starts, the harness checks
that both encoders produce the same bytes.

- **Rust scalar**: `vmx-codec` 0.1.0, portable safe Rust with no SIMD
  intrinsics, `--release`, default target features.
- **libvmx SIMD**: upstream libvmx at `544bcfb`, 128-bit path (NEON via
  sse2neon on this ARM machine), `-O3`. libvmx has no scalar C path, so there
  is no "C scalar" figure. Its AVX2 path only exists on x86-64 and was not
  measured here. The C timings include a copy of the input and output frame
  (about 4 MB each) made by the test wrapper, so C is slightly penalised.

| Profile / quality | Frame size | Rust encode | C encode | Rust decode | C decode |
|---|---|---|---|---|---|
| OMT HQ, q80 | 97 KB (47 Mbit/s @60) | **110 fps** | 318 fps | **458 fps** | 1171 fps |
| HQ, q98 | 1.2 MB (583 Mbit/s @60) | **44 fps** | 144 fps | **64 fps** | 137 fps |

libvmx's hand-written SIMD is about **2.2x to 3.2x** faster than this scalar
port. Even so, one core of the Rust version already encodes 1080p60 at the
OMT default quality, and decodes it several times faster than real time.
Both implementations split frames into slices across threads
(`EncoderConfig::threads`, `Decoder::set_threads`) and produce the same output
at any thread count. SIMD kernels, using `std::arch` behind a safe wrapper or
portable SIMD, are the next step for closing the gap.
