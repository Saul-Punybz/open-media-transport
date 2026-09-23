# M12 prerequisites: pre-encoded send, shared discovery, VMX decoder fuzzing

23 Sep 2026, macOS (Apple silicon), branch `feat/m12-prereqs`.

## 1. Pre-encoded VMX1 frames, received by libomtnet

`run.sh BIN_DIR` starts `examples/omt-send-encoded` (640x360 UYVY at 30 fps,
compressed beforehand with `vmx_codec` at `OMT_LQ`, then passed to
`Sender::send_encoded_video`) and points `interop/libomtnet-harness recv` at it
three times, 5 s each: full frames as UYVY, preview, and compressed-only.
Every 30th frame carries `<HarnessFrame N="n" />\0`; for those the sender prints
the bitstream length and the FNV-1a 64 hash of the bitstream decoded to UYVY by
`vmx_codec`, and the harness prints the hash of libomtnet's decoded pixels.

`compare.py` → `comparison.txt`:

```
full UYVY: 5 tagged frames, 5 with pixels identical to vmx_codec's decode
compressed-only: 5 tagged frames, 5 with the length we sent
preview: 5 tagged frames, sizes ['80x45'], flags ['8'], 5 carrying the full bitstream (P4)
```

So libomtnet's receiver decodes frames we forward untouched to the same pixels
`vmx_codec` does; in compressed-only mode it gets exactly the bytes we were given;
and a preview connection gets the whole frame with flag 8 and decodes an 80x45
preview from it. This shows libomtnet's *receiver* accepting our forwarded frames.
It does not show libomtnet's *sender* forwarding VMX1 (V2, P4 are still from the
source, `OMTSend.cs:766-786,772`), so their Live column is left empty.

Files: `send-encoded.txt`, `recv-full-UYVY.txt`, `recv-preview-UYVY.txt`,
`recv-compressed.txt`.

## 2. One mDNS responder for several senders; interface selection

- `dns-sd-shared.txt`: `dns-sd -B _omt._tcp` while the ignored test
  `sender::tests::senders_share_one_mdns_responder` ran: two senders announced
  through one `Arc<Discovery>` both appear, and each is removed when dropped.
- `dns-sd-interfaces.txt`: during `discovery::tests::interface_selection_limits_announcements`,
  a source announced by a `Discovery` limited to a nonexistent interface
  (`m12-prereqs-if-hidden`) never appears; one on an unrestricted `Discovery`
  (`m12-prereqs-if-shown`) does.

## 3. Fuzzing `vmx_codec::Decoder`

`cargo +nightly fuzz run --target aarch64-apple-darwin vmx_decode -- -max_total_time=300`,
seeded with 90 frames encoded by `vmx_codec` (5 sizes x UYVY/UYVA/P216 x 3
profiles x progressive/interlaced; corpus not committed). The vmx-codec sources
were those of main at 2593990.

- Pass 1 stopped after 31,400 runs on an assertion in the target, not a crash in
  the decoder: `fuzz-vmx-decode-pass1-crash.txt`, minimized to
  `preview_len_extended_dcshift0.bin` (14 bytes: width 32, height 16, sel 5,
  bitstream `03 00 00 00 01 00 00 00 00`).
  A bitstream whose first byte is the extended format (3) with a DC shift of 0
  has a 5-byte header, but `Decoder::preview_len` counts 3 because it keys on
  `dc_shift > 0`. The "prefix" it returns is then 2 bytes short and
  `decode_preview` of that prefix fails while `decode_preview` of the whole
  frame succeeds. libvmx has the same rule (`VMX_GetEncodedPreviewLength`,
  `vmxcodec.cpp:1174-1192`, keyed on `DCShift > 0`, while `VMX_LoadFrom`
  accepts the extended form with any shift, `vmxcodec.cpp:517-523`), and
  libvmx never writes the extended form with shift 0, so only a hand-made or
  hostile stream triggers it. Reported, not fixed (vmx-codec is outside this branch).
- Pass 2, with that one input shape skipped, ran 6,066,878 inputs in 301 s
  (20,155/s, peak RSS 563 MB) with no panic, crash, or failed assertion:
  `fuzz-vmx-decode-pass2-stats.txt`.
