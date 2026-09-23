# omt

Command-line tool for [Open Media Transport](https://www.openmediatransport.org/) (OMT),
built on the [`open-media-transport`](../open-media-transport) crate.

```text
omt list [--seconds N]                                   show sources on the network
omt send [--name NAME] [--size WxH] [--fps F] [--seconds N] [--10bit]   send a test pattern
omt recv SOURCE [--seconds N] [--snapshot FILE.bmp|FILE.png] [--preview]   receive and report
omt discovery-server [--port N] [--seconds N]           run a discovery server
omt version
```

`send` produces 75% colour bars, a grey ramp and a moving box that flashes white with a
1 kHz beep once a second (for checking A/V sync). `recv` connects to a source by the name
`omt list` shows (or `host:port`), prints resolution, frame rate, bitrate and audio every
second, and can save the last frame: `.png` keeps a 10-bit source at 16 bits per sample (and
keeps alpha), `.bmp` is 8-bit RGB. `send --10bit` sends a 10-bit (P216) source whose grey
ramp covers every 10-bit level, to test that path end to end.

On networks without multicast, run `omt discovery-server` on one machine (port 6399 by
default) and pass `--discovery-server omt://HOST[:PORT]` to `list`, `send` and `recv`.
`send` then registers with the server instead of announcing over mDNS; `list` and `recv`
ask the server as well as mDNS (only the server with `--no-mdns`). libomtnet applications
use the same server when `settings.xml` sets `DiscoveryServer`.

See [`TESTING.md`](../../TESTING.md) for how to test it against OMT products.

Licensed under either of Apache License 2.0 or MIT, at your option.
