# Windows 11 on a low-end x86 PC: release binary, tests, send/recv, codec speed

25 Sep 2026. Intel NUC7i3BNH:
- CPU: i3-7100U, 2 cores / 4 threads, 2.4 GHz, AVX2
- 8 GB RAM, HDD
- Windows 11 Home 10.0.26200, fresh install, Wi-Fi

Built from `main` at `48c3984`:
- rustc 1.98.1 `x86_64-pc-windows-gnu`, not MSVC (the Visual Studio Build Tools install failed on this machine)
- WinLibs MinGW 16.2 provides `dlltool` and `g++`

The first real run of this project on Windows hardware (CI aside), and the first native x86_64 run of the codec.
No OMT product and no second machine were involved.

## 1. v0.1.0 release binary does not start on a clean Windows

`omt-v0.1.0-x86_64-pc-windows-msvc.zip`: its SHA-256 matches the release notes (`82fc343c…`).

`omt.exe version` prints nothing and exits with `0xC0000135` (STATUS_DLL_NOT_FOUND); PowerShell shows exit code 53.
The binary imports `VCRUNTIME140.dll`, which a clean Windows does not include.
`feat/release-ci` (`+crt-static`, plus a CI check for vcruntime140) is the fix.
Until a new release is out, testers need the VC++ Redistributable.

## 2. Tests

`cargo test -p vmx-codec -p open-media-transport` (`cargo-test-output.txt`):
- 8 suites: 111 passed, 0 failed, 2 ignored
- `reference/libvmx` was not cloned yet, so the conformance tests skipped themselves

With `reference/libvmx` at `544bcfb`, `cargo test --release -p vmx-codec --test conformance`:
- 9 passed
- vmx-codec output is byte-identical to libvmx on native x86_64 (previously shown only under Rosetta)

## 3. send → recv on the same machine

`send-recv-output.txt`, `send-recv-perf.csv`.

`omt list` found the sender:

```
"DESKTOP-QRLFQ6I (Rust Test)"   DESKTOP-QRLFQ6I-omt.local:6400  [172.16.80.58]
```

So the `<host>-omt.local.` SRV target resolved on Windows, though only by our own receiver.
`omt recv` by name connected both channels and exchanged OMTInfo and tally.
The snapshot is correct (`snapshot-1080p30-960x540.png`, scaled down).

Sender CPU is a share of all 4 logical CPUs, so 25% means one thread saturated.
Every run had 0 decode errors and `dropped=0`.

| Case | fps received | Mbit/s | sender CPU |
|---|---|---|---|
| 1080p29.97 | 29.8–30.3 | 12.3 | 15% |
| 1080p30 `--10bit` | 28.9–30.2 | 12.3 | 19% |
| 1080p59.94 | 48.6–53.5 | 21 | 24% |
| 2160p30 | 9–13.6 | 12.6 | 25% |

With the default single encoder thread, 1080p59.94 and 2160p30 fall behind with no sign of it:
- `dropped` stays 0
- audio is sent in the same loop, so it falls behind too: at 2160p30, about 13 audio frames a second arrive instead of 30

STATUS.md's "one core still does 1080p60" holds on the M4, not on this CPU.

The first 1080p59.94 run failed with `not found in 5 s`.
The Wi-Fi address had just changed (172.16.80.58 → 172.20.10.5), and the rerun passed.
A 4-minute monitor (`discovery-monitor.txt`: `omt list` and `omt recv` by name every ~8 s against one sender) passed all 29 cycles.
The address did not change during it, so behaviour across an address change is still untested.

## 4. Codec speed: `bench_vs_libvmx`, 120 frames, 1920x1080 UYVY, 1 thread

`libvmx-ref/build.rs` compiles libvmx with `-mavx2 -msse4.2` on x86_64, so this is Rust SSE2 against C with AVX2.

| Profile | Rust encode | C encode | Rust decode | C decode |
|---|---|---|---|---|
| OMT HQ q80 (97 KB/frame) | 75.8 fps | 120.7 fps (1.59x) | 171.4 fps | 215.7 fps (1.26x) |
| HQ q98 (1.2 MB/frame) | 30.6 fps | 52.9 fps (1.73x) | 27.2 fps | 53.5 fps (1.97x) |

On the M4, encode is at parity (BENCH.md); on x86 there is still a clear gap, and AVX2 kernels would be the place to close it.

## 5. CLI: unknown options were ignored

`omt send --help` started sending the default pattern and never stopped.
A typo such as `--fsp 60` sent at 30 fps without a word.
`opt()` only looks options up, so anything else was ignored.

## Fixes on `fix/cli-args-threads-pacing`

`fix-verification-output.txt` has the output.

**Options:**
- unknown options and missing values are errors
- `--help`, `-h` and `omt help` print the usage

**`omt send --threads N` sets `SenderConfig::encoder_threads`:**

| Case | Threads | fps sent |
|---|---|---|
| 1080p59.94 | 1 | 53.7–56.6, with a warning |
| 1080p59.94 | 2 | full rate, no warning |
| 2160p30 | 1 | 14.5 |
| 2160p30 | 4 | 21.5 |

This CPU cannot reach 2160p30 even with 4 threads.

**Warning:** `omt send` warns when it falls short of the requested rate.
It suggests more threads only while there are cores left:

```
warning: sending 14.5 fps of the requested 30.00; video and audio are falling behind real time (try --threads 2 or a smaller --size)
```

## Not tested

- vMix, OBS, a second machine
- libomtnet on Windows: its Windows discovery path is the top risk in STATUS.md
  - .NET 10.0.401 is installed here, and `reference/libomtnet` is cloned at `029ef4e`
  - building and running it was left for the maintainer to approve
