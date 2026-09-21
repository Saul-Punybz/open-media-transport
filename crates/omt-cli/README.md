# omt

Command-line tool for [Open Media Transport](https://www.openmediatransport.org/) (OMT),
built on the [`open-media-transport`](../open-media-transport) crate.

```text
omt list [--seconds N]                                   show sources on the network
omt send [--name NAME] [--size WxH] [--fps F] [--seconds N]   send a test pattern
omt recv SOURCE [--seconds N] [--snapshot FILE.bmp] [--preview]   receive and report
omt version
```

`send` produces 75% colour bars, a grey ramp and a moving box that flashes white with a
1 kHz beep once a second (for checking A/V sync). `recv` connects to a source by the name
`omt list` shows (or `host:port`), prints resolution, frame rate, bitrate and audio every
second, and can save the last frame as a BMP.

See [`TESTING.md`](../../TESTING.md) for how to test it against OMT products.

Licensed under either of Apache License 2.0 or MIT, at your option.
