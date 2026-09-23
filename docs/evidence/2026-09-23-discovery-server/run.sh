#!/bin/sh
# Usage: run.sh upstream|ours EVIDENCE_DIR SERVER_IP
# (a) upstream: libomtnet's OMTDiscoveryServer; (b) ours: `omt discovery-server`.
# Clients on both sides: our `omt` and the libomtnet harness, which reads
# DiscoveryServer from $OMT_STORAGE_PATH/settings.xml (OMTSettings.cs:35-37,62).
set -u
MODE=$1
EV=$2
IP=$3
OUT=/private/tmp/claude-501/-Users-saulgonzalez/0927cde0-dbb7-4bd6-81b0-220c942218fd/scratchpad/discovery-server-out
H=$OUT/harness/libomtnet-harness
OMT=$OUT/omt
URL=omt://$IP:6399
MACHINE=SAULS-MACBOOK-PRO.LOCAL
ST=$OUT/storage-$MODE
mkdir -p "$EV" "$ST"
printf '<Settings>\n  <DiscoveryServer>%s</DiscoveryServer>\n</Settings>\n' "$URL" > "$ST/settings.xml"
cp "$ST/settings.xml" "$EV/settings.xml"
step() { echo "== $(date +%H:%M:%S) $*" >> "$EV/steps.log"; }

start_server() {
  if [ "$MODE" = upstream ]; then
    "$OUT/upstream-server/OMTDiscoveryServer" >> "$EV/server.log" 2>&1 &
  else
    "$OMT" discovery-server >> "$EV/server.log" 2>&1 &
  fi
  SV=$!
}

tshark -q -i lo0 -f "tcp port 6399" -w "$EV/server-6399.pcapng" -a duration:60 >/dev/null 2>&1 &
T1=$!
tshark -q -i en0 -f "udp port 5353" -w "$EV/mdns-en0.pcapng" -a duration:60 >/dev/null 2>&1 &
T2=$!
sleep 3

step "server starts ($MODE) on $URL"
start_server
sleep 1

step "our sender registers with the server"
"$OMT" send --name discovery-server-ours --size 320x180 --discovery-server $URL --seconds 40 > "$EV/our-send.log" 2>&1 &
OS=$!
sleep 2

step "libomtnet list (connects after our sender registered)"
OMT_STORAGE_PATH=$ST "$H" list 3 > "$EV/libomtnet-list.log" 2> "$EV/libomtnet-list.err"
step "libomtnet recv by name"
OMT_STORAGE_PATH=$ST "$H" recv "$MACHINE (discovery-server-ours)" 3 > "$EV/libomtnet-recv.log" 2> "$EV/libomtnet-recv.err"

step "libomtnet sender registers with the server"
OMT_STORAGE_PATH=$ST "$H" send 'discovery-server-lib&<x>' 22 > "$EV/libomtnet-send.log" 2> "$EV/libomtnet-send.err" &
LS=$!
sleep 2
step "our list, server only"
"$OMT" list --discovery-server $URL --no-mdns --seconds 3 > "$EV/our-list.log" 2>&1
step "our recv by name, server only"
"$OMT" recv "$MACHINE (discovery-server-lib&<x>)" --discovery-server $URL --no-mdns --seconds 3 > "$EV/our-recv.log" 2>&1

step "server restart (both senders stay up)"
kill $SV; wait $SV
sleep 2
start_server
sleep 3
step "libomtnet list after restart"
OMT_STORAGE_PATH=$ST "$H" list 3 > "$EV/libomtnet-list-restart.log" 2> "$EV/libomtnet-list-restart.err"
step "our list after restart"
"$OMT" list --discovery-server $URL --no-mdns --seconds 2 > "$EV/our-list-restart.log" 2>&1

wait $LS
step "libomtnet sender gone; our list again"
"$OMT" list --discovery-server $URL --no-mdns --seconds 2 > "$EV/our-list-end.log" 2>&1
wait $OS
sleep 1
step "server stops"
kill $SV; wait $SV
wait $T1 $T2
step done
