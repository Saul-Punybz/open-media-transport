# Our discovery against libomtnet

Date: 21 Sep 2026. Host: macOS 26.5.1. Tools outside Claude: `dns-sd`, `tshark` on `en0`.
Our code: `crates/open-media-transport/src/discovery.rs` (`mdns-sd` 0.21.4, `hostname` 0.4.2),
driven by the `omt-discover` and `omt-recv` examples. Their side: libomtnet 1.0.0.19
through `interop/libomtnet-harness`.

## A. We find a libomtnet source and receive from it by name

libomtnet announced `Disc A`. `omt-discover browse` resolved
`SAULS-MACBOOK-PRO.LOCAL (Disc A)`, host `Sauls-MacBook-Pro.local.`, port 6400. Then
`omt-recv "SAULS-MACBOOK-PRO.LOCAL (Disc A)"` looked the name up, connected and decoded
121 video and 120 audio frames (PSNR 58.4–58.8 dB, channel 0 RMS 0.1769, channel 1
silent). Files: `a-omt-discover-browse.txt`, `a-omt-recv-by-name.txt`.

macOS answers for its own host with loopback addresses too (`127.0.0.1`, `::1`,
`fe80::1`, see `a-omt-discover-browse.txt`). The receiver in that run picked
`127.0.0.1`, which only works on the same machine; addresses are now ordered
routable → link-local → loopback.

## B. libomtnet finds a source we announce

1. **First attempt: a conflict** (`b1-first-attempt-dnssd-zone.txt`). With the SRV
   target set to the OS host name, `Sauls-MacBook-Pro.local.`, `mdns-sd` probed that
   name, got answers from macOS's own responder with other addresses, and renamed its
   records: one interface showed SRV target `Sauls-MacBook-Pro-2.local.`. The system's
   name was not touched (`scutil --get LocalHostName` was `Sauls-MacBook-Pro` before
   and after).
2. **Fix: our own host name.** SRV target `Sauls-MacBook-Pro-omt.local.`
   (`b2-dnssd-zone.txt`): one SRV record, port 6599, TXT `""`, instance
   `SAULS-MACBOOK-PRO.LOCAL (Rust Source)` — the same form libomtnet uses (D2, D3, D5).
   libomtnet's discovery listed it: `b2-libomtnet-list.txt`. `b2-mdns-omt.pcapng` is
   the 25 mDNS packets of that run that mention `omt`.
3. **Loopback leak fixed.** Our host still published `fe80::1`, the link-local address
   of `lo0` (`b2-dnssd-host-before-lo0-fix.txt`); `mdns-sd`'s loopback filter matches
   only 127/8 and ::1. After excluding the `lo0`/`lo` interfaces by name, only `en0`'s
   addresses remain (`b3-dnssd-host-after-lo0-fix.txt`).

## Not shown

- libomtnet *connecting* to a source we announce: we have no sender yet; nothing
  listened on 6599.
- A second machine, Windows or Linux browsers, vMix or OBS.
- Two of our processes announcing at once on one machine (they would both publish
  `<host>-omt.local.` with the same addresses; expected to be fine, untested).
