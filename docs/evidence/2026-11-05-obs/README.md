# OBS Studio with the OMT plugin, against `omt`, one Mac

5 Nov 2026. First run against a shipping third-party OMT product. Both ends on one Mac:

| | |
|---|---|
| Our side | `omt` built from `main` at `ec11ec2` (`cargo build --release -p omt-cli`), sha256 `bdbfabf6…` |
| Their side | OBS Studio 33.0.1 (Homebrew cask), `omtplugin` 1.0.0.19 for macOS (official release, Developer ID StudioCoast Pty Ltd, notarized, sha256 `90b4284d…`) |
| Machine | macOS 26.5.1 arm64, Wi-Fi, one host (LAN address or `127.0.0.1`) |

The plugin embeds libomtnet, so this is not a new protocol implementation. It is the same
library as the earlier rows of `docs/INTEROP.md`, inside the real product and its real code paths.
No traffic left the machine: this says nothing about a network between OBS and `omt`.

OBS was driven through its local control port and the Tools menu. `OMT In` is an OMT Source; the
OMT Output was turned on in Tools > OMT Output Settings.

## Results

| # | Test | Result | Files |
|---|---|---|---|
| 1 | `omt send` 1920x1080 30 fps; OBS OMT Source by URL (`omt://127.0.0.1:6400`) | **works**: colour bars correct in OBS, sender shows 2 connections, 1 video receiver, 0 dropped | `1-obs-source.png` |
| 2 | Discovery both ways | **works** after the failure noted below: OBS's source list showed `MAC.LAN (OBSTest)`, `omt list` showed `MAC.LAN (OBS Output)` | |
| 3 | OBS OMT Source by name, moving picture | **works**: two grabs 1.5 s apart differ, the moving box is in a new place | `3-obs-byname-1.png`, `3-obs-byname-2.png`, `3-omt-send.txt` |
| 4 | `omt check` and `omt recv` of OBS Output by name, carrying test 3's picture (`omt` → OBS → `omt`) | **works**: 1280x720, 30.0 fps, 0 decode errors, `OK`, audio present, bars correct | `3-check-obs-output.txt`, `3-recv-obs-output.txt`, `3-recv-from-obs.png` |
| 5 | `omt send --10bit` 1920x1080 30 fps into OBS | **works**, connected and picture correct. **Not shown:** that OBS kept 10 bits. The plugin opened the receiver as `UYVYorBGRA` and OBS's canvas was 8-bit | `4-omt-send-10bit.txt`, `4-obs-10bit.png` |
| 6 | Sender restarted (now 1080p60), OBS left running | **works**: OBS reconnected by name by itself, sender shows 0 dropped. **Not shown:** that OBS displayed 60 fps; its canvas ran at 30 | `5-omt-send-60.txt`, `5-obs-after-reconnect.png` |
| 7 | OBS Output off for ~15 s, then on, while `omt recv` ran | **works**: receiver reported the reset, 0 fps during the gap, reconnected and returned to 30 fps by itself | `6-recv-across-restart.txt` |

0 exceptions in the plugin's log. `obs-plugin-omt.log` is that log, with discovery addresses removed.

## Open points

- **Discovery was empty for the first minutes.** Two `omt list` runs and one `dns-sd -B _omt._tcp`
  found nothing with our sender running; a direct connection worked. A few minutes later it worked
  in every run, including one after OBS had quit. The cause was not found and it did not come back.
- During some of test 4's runs OBS's default Mic/Aux input was live, so "audio present" there
  does not separate it from our beep. The microphone was muted before tests 5 to 7.
- Tally, HDR, frame-accurate pixel comparison, frame loss and latency were not measured.
