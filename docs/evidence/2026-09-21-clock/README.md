# Generated timestamps (`clock::Clock`) as seen by libomtnet

Date: 21 Sep 2026. `omt-send "Rust Clock" 9`, now stamping frames with one `Clock` for video
and one for audio (libomtnet's timestamp −1 mode, `docs/PROTOCOL.md` C2–C5), received by
`libomtnet-harness recv "SAULS-MACBOOK-PRO.LOCAL (Rust Clock)" 5` (libomtnet 1.0.0.19).

- Every video timestamp libomtnet received is a multiple of 333,333 ticks (30 fps), as with
  libomtnet's own clock (`../2026-09-21-libomtnet-loopback`), and audio frames carry the same
  timestamps as the video frames sent with them.
- 120 video and 119 audio frames in 5 s, i.e. the clock held 30 fps; 0 frames dropped.
- The decoded pixels of the 4 marked frames equal those of libomtnet's own sender for the same
  frame numbers (`../2026-09-21-our-receiver-vs-libomtnet/pixel-hashes.txt`).

The 29.97 fps interval (333,667 ticks, C5) is unit-tested only.
