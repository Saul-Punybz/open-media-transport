#!/usr/bin/env bash
# Runs `omt` against the upstream libomtnet harness in both directions on one
# machine, over loopback. Used by .github/workflows/interop.yml.
#
#   interop.sh OMT HARNESS_DLL LOGDIR
#
# 1. libomtnet sends, omt receives   (by omt://127.0.0.1:PORT)
# 2. omt sends, libomtnet receives   (by omt://127.0.0.1:PORT)
# 3. and 4. the same by full source name, found through our discovery server
#    (`omt discovery-server`) instead of mDNS. libomtnet is pointed at it
#    with its settings.xml (OMTSettings.cs:41, OMTDiscovery.cs:56).
# 5. and 6. by full source name over mDNS. Informational only: multicast on
#    CI runners is not something to gate on.
#
# Fails unless frames arrive in cases 1-4.
set -euo pipefail
omt=$1 harness=$2 logs=$3
mkdir -p "$logs"
tag=${RUNNER_OS:-local}-$$
fail=0
pids=()
cleanup() { for p in "${pids[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT

# Waits up to 30 s for a line matching $2 in file $1 and prints it.
wait_for() {
  for _ in $(seq 300); do
    if line=$(grep -m1 -E "$2" "$1" 2>/dev/null); then echo "$line"; return 0; fi
    sleep 0.1
  done
  echo "timed out waiting for /$2/ in $1:" >&2; cat "$1" >&2; return 1
}

check() { # name, command...
  local name=$1; shift
  if "$@"; then echo "PASS $name"; echo "| $name | pass |" >> "$logs/summary.md"
  else echo "FAIL $name"; echo "| $name | **FAIL** |" >> "$logs/summary.md"; fail=1; fi
}

# As check, but a failure is recorded without failing the run.
inform() { # name, command...
  local name=$1; shift
  if "$@"; then echo "PASS $name"; echo "| $name | pass |" >> "$logs/summary.md"
  else echo "FAIL (informational) $name"; echo "| $name | fail (informational) |" >> "$logs/summary.md"; fi
}

# omt recv printed video and audio with sound and never failed to decode.
omt_received() {
  local log=$1
  grep -qE "\| [1-9][0-9.]* fps received" "$log" &&
    grep -qE "audio 48000 Hz 2 ch, [1-9][0-9]*/" "$log" &&
    ! grep -qE "decode errors [1-9]" "$log" &&
    grep -q "decode errors 0" "$log"
}

# The harness receiver counted video and audio frames from our sender.
harness_received() {
  local log=$1
  grep -qE "^recv video .* codec=UYVY" "$log" &&
    grep -qE "^recv done video=[1-9][0-9]* audio=[1-9][0-9]* info=omt/open-media-transport/" "$log"
}

printf '| case | result |\n|---|---|\n' > "$logs/summary.md"

echo "== 1. libomtnet send -> omt recv (URL)"
dotnet "$harness" send "ci-harness-$tag" 20 > "$logs/1-harness-send.log" 2>&1 & pids+=($!)
port=$(wait_for "$logs/1-harness-send.log" '^send address=' | sed -E 's/.* port=([0-9]+).*/\1/')
"$omt" recv "omt://127.0.0.1:$port" --seconds 8 > "$logs/1-omt-recv.log" 2>&1 || true
cat "$logs/1-omt-recv.log"
check "libomtnet send -> omt recv (omt://127.0.0.1:$port)" omt_received "$logs/1-omt-recv.log"
wait "${pids[-1]}" || true; cat "$logs/1-harness-send.log" | grep -v "^log:" || true

echo "== 2. omt send -> libomtnet recv (URL)"
"$omt" send --name "ci-omt-$tag" --size 640x360 --seconds 20 > "$logs/2-omt-send.log" 2>&1 & pids+=($!)
port=$(wait_for "$logs/2-omt-send.log" '^sending ' | sed -E 's/.* on port ([0-9]+).*/\1/')
dotnet "$harness" recv "omt://127.0.0.1:$port" 8 > "$logs/2-harness-recv.log" 2>&1 || true
grep -v "^log:" "$logs/2-harness-recv.log" || true
check "omt send -> libomtnet recv (omt://127.0.0.1:$port)" harness_received "$logs/2-harness-recv.log"
wait "${pids[-1]}" || true; cat "$logs/2-omt-send.log"

echo "== 3./4. by name through omt discovery-server"
"$omt" discovery-server --port 6399 --seconds 60 > "$logs/ds.log" 2>&1 & pids+=($!)
sleep 1
# libomtnet reads DiscoveryServer from settings.xml in OMT_STORAGE_PATH on
# Linux (LinuxPlatform.cs:67-72) and in %ProgramData%\OMT on Windows
# (Win32Platform.cs:55-58).
if [ "${RUNNER_OS:-}" = Windows ]; then store=/c/ProgramData/OMT; else store="$logs/omt-storage"; export OMT_STORAGE_PATH="$store"; fi
mkdir -p "$store"
printf '<Settings><DiscoveryServer>omt://127.0.0.1:6399</DiscoveryServer></Settings>\n' > "$store/settings.xml"

dotnet "$harness" send "ci-harness-ds-$tag" 20 > "$logs/3-harness-send.log" 2>&1 & pids+=($!)
name=$(wait_for "$logs/3-harness-send.log" '^send address=' | sed -E 's/^send address=(.*) url=.*/\1/')
echo "harness source: $name"
"$omt" recv "$name" --discovery-server omt://127.0.0.1:6399 --no-mdns --seconds 8 > "$logs/3-omt-recv.log" 2>&1 || true
cat "$logs/3-omt-recv.log"
check "libomtnet send -> omt recv (\"$name\" via discovery server)" omt_received "$logs/3-omt-recv.log"

"$omt" send --name "ci-omt-ds-$tag" --size 640x360 --seconds 20 --discovery-server omt://127.0.0.1:6399 > "$logs/4-omt-send.log" 2>&1 & pids+=($!)
name=$(wait_for "$logs/4-omt-send.log" '^sending ' | sed -E 's/^sending "(.*)" on port.*/\1/')
echo "omt source: $name"
sleep 2
dotnet "$harness" recv "$name" 8 > "$logs/4-harness-recv.log" 2>&1 || true
grep -v "^log:" "$logs/4-harness-recv.log" || true
check "omt send -> libomtnet recv (\"$name\" via discovery server)" harness_received "$logs/4-harness-recv.log"
rm -f "$store/settings.xml"
cat "$logs/ds.log"

echo "== 5./6. by name over mDNS (informational)"
dotnet "$harness" send "ci-harness-mdns-$tag" 25 > "$logs/5-harness-send.log" 2>&1 & pids+=($!)
name=$(wait_for "$logs/5-harness-send.log" '^send address=' | sed -E 's/^send address=(.*) url=.*/\1/')
echo "harness source: $name"
"$omt" list --seconds 5 > "$logs/5-omt-list.log" 2>&1 || true
cat "$logs/5-omt-list.log"
"$omt" recv "$name" --seconds 8 > "$logs/5-omt-recv.log" 2>&1 || true
cat "$logs/5-omt-recv.log"
inform "libomtnet send -> omt recv (\"$name\" via mDNS)" omt_received "$logs/5-omt-recv.log"

"$omt" send --name "ci-omt-mdns-$tag" --size 640x360 --seconds 25 > "$logs/6-omt-send.log" 2>&1 & pids+=($!)
name=$(wait_for "$logs/6-omt-send.log" '^sending ' | sed -E 's/^sending "(.*)" on port.*/\1/')
echo "omt source: $name"
sleep 2
dotnet "$harness" list 5 > "$logs/6-harness-list.log" 2>&1 || true
grep -v "^log:" "$logs/6-harness-list.log" || true
dotnet "$harness" recv "$name" 8 > "$logs/6-harness-recv.log" 2>&1 || true
grep -v "^log:" "$logs/6-harness-recv.log" || true
inform "omt send -> libomtnet recv (\"$name\" via mDNS)" harness_received "$logs/6-harness-recv.log"

cat "$logs/summary.md"
exit $fail
