# Benchmark: vmx-codec vs libvmx (C++ SIMD)

Measured 23 Sep 2026 on an Apple M4 laptop (arm64, macOS), Rust 1.98.1,
`--release`, default target features. Other agents were using the machine
(load average about 2.2 to 2.5 when each run started; builds were held off
with a lock), so run-to-run noise is several percent. Run it yourself with:

```sh
cargo run --release -p vmx-codec --example bench_vs_libvmx -- 120
```

Setup: 1920x1080 UYVY 8-bit, 120 frames, **one thread on each side**,
quality fixed per frame. The test image is a synthetic picture made of
gradients, mild noise and hard edges. Before timing starts, the harness checks
that both encoders produce the same bytes.

- **vmx-codec**: this crate. On aarch64 the transforms use NEON through
  `std::arch`, on x86-64 SSE2 (see `src/simd.rs`); everything else is safe
  Rust.
- **libvmx**: upstream libvmx at `544bcfb`, 128-bit path (NEON via sse2neon
  on this ARM machine), `-O3`. Its AVX2 path only exists on x86-64 and was not
  measured. The C timings include a copy of the input and output frame (about
  4 MB each) made by the test wrapper, so C is slightly penalised.

| Profile / quality | Frame size | Rust encode | C encode | Rust decode | C decode |
|---|---|---|---|---|---|
| OMT HQ, q80 | 97 KB (47 Mbit/s @60) | **341 fps** | 327–339 fps | **1112–1131 fps** | 1252 fps |
| HQ, q98 | 1.2 MB (583 Mbit/s @60) | **137 fps** | 142 fps | **101 fps** | 135 fps |

Encoding is now on par with libvmx at the OMT default quality and within
about 4% at q98. Decoding is 1.13x (q80) to 1.35x (q98) slower; what is left
there is the AC symbol reader (the inverse transform is a small share).

### Before this work (same machine, same day, 60 frames, load about 2.4)

| Profile / quality | Rust encode | C encode | Rust decode | C decode |
|---|---|---|---|---|
| OMT HQ, q80 | 111 fps | 337 fps | 454 fps | 1216 fps |
| HQ, q98 | 45 fps | 142 fps | 63 fps | 135 fps |

### Where the speed came from

Measured one change at a time (OMT HQ q80 unless noted):

| Change | Effect |
|---|---|
| Transforms on NEON / SSE2 via a generic `Isa` trait (was lane emulation that LLVM compiled to scalar code) | encode 110 → 174 fps |
| 32-bit bit-writer flushes; AC coding walks only non-zero coefficients (mask + `trailing_zeros`) | encode 174 → 217 fps |
| Packed 4:2:2 split/merge in one pass with constant offsets | encode 217 → 286, decode 524 → 940 fps |
| Bit writes inlined, run + value code in one write | q98 encode 78 → 122 fps |
| AC decoding from one 64-bit peek per symbol, natural-order coefficients, row-wise DC fill | decode 937 → 1037 fps |
| SIMD non-zero mask (SSE2 movemask / NEON narrow + pairwise add) | encode 294 → 333 fps |
| Bit-writer state in registers, branchless flush | q98 encode 124 → 135 fps |
| Register-cached AC bit reader (and kept in registers) | q98 decode 77 → 101 fps |

Tried and dropped because they measured no better or worse: evaluating the
forward DCT row pass on the transposed block in plain Rust (slower, 110 → 82
fps, LLVM emitted scalar code), inlining the forward DCT into the block loop,
branch-free consumption of AC symbols, a 12-bit lookup table for short AC
codes (libvmx's approach), and `SQDMULH` for the even DCT constants.

Both implementations split frames into slices across threads
(`EncoderConfig::threads`, `Decoder::set_threads`) and produce the same output
at any thread count.

### Correctness of the SIMD paths

Every NEON / SSE2 kernel has a portable twin (`lanes::Scalar`), and unit tests
compare them on 100k random blocks each, biased towards saturation and
wrap-around values (transforms, both bit depths, arbitrary quantisation
matrices; the non-zero mask at every density and position). The bit writer
and the AC readers have lockstep tests against bit-by-bit reference
implementations, including truncated and corrupt streams. The conformance
tests against libvmx pass on aarch64 (NEON) and, run under Rosetta 2 with
libvmx built for x86-64, on x86-64 (SSE2).
