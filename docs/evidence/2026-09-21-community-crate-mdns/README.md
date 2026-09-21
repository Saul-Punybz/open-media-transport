# What the community crate announces over mDNS

Date: 21 Sep 2026. Host: macOS 26.5.1, Apple Silicon. Tool: Apple's `/usr/bin/dns-sd`.

Subject: [MikanseiLaboratory/openmediatransport-rs](https://github.com/MikanseiLaboratory/openmediatransport-rs)
at `7711da4`, debug build, running its own example:

```
./target/debug/examples/send_colorbar --width 320 --height 180 --fps 5 --no-animate
```

While it ran (a few seconds each time, then killed):

| File | Command | Shows |
|---|---|---|
| `dnssd-browse.txt` | `dns-sd -B _omt._tcp local` | instance name `LOCALHOST (Colorbars)` |
| `dnssd-lookup.txt` | `dns-sd -L "LOCALHOST (Colorbars)" _omt._tcp local` | SRV target `LOCALHOST.local.:6400` |
| `dnssd-resolve.txt` | `dns-sd -G v4v6 LOCALHOST.local` | `LOCALHOST.local` answered with this machine's LAN, link-local **and loopback** addresses |

The machine's real name is `Sauls-MacBook-Pro.local` (`hostname`, `scutil --get LocalHostName`).
Neither `HOSTNAME` nor `COMPUTERNAME` is in the process environment on macOS, and the crate
builds its machine name from those variables, falling back to `localhost`
(`src/discovery/address.rs:279-284` in that crate).

libomtnet in the same situation would announce `SAULS-MACBOOK-PRO.LOCAL (Colorbars)`:
it uses `gethostname()` upper-cased (`reference/libomtnet/src/mac/MacPlatform.cs:51-73`).
That comparison is from reading libomtnet, **not** from running it.

What this does and does not show:
- **Shows**: on macOS the crate's senders all use the machine name `LOCALHOST`, and it
  publishes `LOCALHOST.local` with loopback addresses among the answers.
- **Implies, not tested**: two Macs running it would both claim `LOCALHOST.local`
  (an mDNS name conflict), and two sources with the same name would collide as
  instances. A remote receiver may be handed `127.0.0.1` or `::1` for this host.
- **Does not show** anything about its framing, handshake or video.
