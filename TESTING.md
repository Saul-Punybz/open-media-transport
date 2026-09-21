# Testing `omt` with real OMT products

Thank you for helping. This project is a Rust implementation of
[Open Media Transport](https://www.openmediatransport.org/) (OMT). It is **early**:
so far it has only been tested against the reference implementation (libomtnet) on one
Mac. What we need most is to know whether it works with real products and across real
networks, including when it doesn't.

`omt` is a small command-line tool. It can **list** the OMT sources on your network,
**send** a test pattern that other OMT software can receive, and **receive** from any OMT
source, printing statistics and saving a snapshot.

## 1. Get `omt`

**Download** (when a release is published): pick the archive for your system from the
repository's *Releases* page, unpack it, and run `omt` from a terminal.

- macOS: the binary is not signed. If macOS refuses to open it, run
  `xattr -d com.apple.quarantine ./omt` once.
- Windows: `omt.exe` in PowerShell or Command Prompt.

**Or build it** (needs [Rust](https://rustup.rs)):

```sh
git clone https://github.com/Saul-Punybz/open-media-transport
cd open-media-transport
cargo build --release -p omt-cli
./target/release/omt version
```

## 2. Network and firewall

OMT needs, on every machine involved:

- **TCP 6400–6600** incoming, for the sender (the first free port is used);
- **UDP 5353** (mDNS / Bonjour), for discovery.

Both machines on the same subnet, no client isolation (guest Wi-Fi often blocks this).
The first time `omt send` runs, macOS or Windows may ask whether to allow incoming
connections: allow them.

## 3. Tests

Run what you can; any one of them helps. Keep the terminal output.

### A. See what is on the network

```sh
omt list --seconds 10
```

Expected: one line per OMT source, like `"MY-PC (Camera 1)"  my-pc.local:6400  [192.168.1.20]`.
Compare with what your OMT product shows.

### B. Receive from a real product

Start a source in your product (vMix output, OBS with the OMT plugin, SIENNA or vMix OMT
tools, a Raspberry Pi OMT encoder…). Then, using the exact name from `omt list`:

```sh
omt recv "MY-PC (Camera 1)" --seconds 30 --snapshot shot.bmp
```

Expected: `connected`, then one line per second with resolution, frame rate received,
bitrate and audio; `decode errors 0`; and `shot.bmp` showing the picture. Try
`--preview` too (1/8-size preview video). If discovery fails, use `host:port` instead of
the name.

### C. Send to a real product

```sh
omt send --name "Rust Test" --size 1920x1080 --fps 30
```

Then add the source in your product (for example `MY-MAC (Rust Test)`). Expected: colour
bars, a grey ramp, and a black box moving left to right that turns **white with a short
1 kHz beep once a second** (use it to judge audio/video sync). Put the source on
preview/program in your product: `omt send` prints the tally it receives. Try other
sizes and rates: `--fps 29.97`, `--fps 59.94`, `--size 3840x2160`.

### D. Two machines

Run `omt send` on one computer and `omt recv` (or your OMT product) on another.

## 4. What to report

Open an issue on the repository (or send it to whoever asked you to test) with:

```
Product and version:     e.g. vMix 28.0.0.42 / OBS 31 + OMT plugin 1.0.0.19 / SIENNA OMT Monitor
Your OS and version:     e.g. Windows 11 24H2 / macOS 15.5 / Ubuntu 24.04
omt version:             output of `omt version`
Test:                    A / B / C / D, and direction (omt → product or product → omt)
Network:                 same machine / wired LAN / Wi-Fi
Command:                 exactly what you ran
Result:                  worked / partly / failed
What you saw:            picture right? audio? tally? errors? CPU use?
Output:                  paste the terminal output
Snapshot:                attach shot.bmp if you made one
```

Failures are as useful as successes. If you can, a packet capture helps a lot:
`tshark -i <interface> -f "tcp portrange 6400-6600 or udp port 5353" -a duration:30 -w omt.pcapng`
(Wireshark's command-line tool), attached to the report.

## Known limitations

- Only tested against libomtnet 1.0.0.19 on macOS; Windows and Linux builds are checked by
  CI but have not been run against real products.
- `omt recv` saves snapshots of 8-bit video only; 10-bit (P216/PA16) streams are received
  but not saved.
- Redirects and the OMT discovery server are not supported yet.
- This project is not affiliated with or endorsed by the Open Media Transport project or vMix.
