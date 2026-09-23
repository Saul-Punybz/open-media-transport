#!/bin/sh
# libomtnet's sender, watched at once by libomtnet's receiver and by ours
# (examples/omt-recv, built on open_media_transport::media), with the same
# OMTPreferredVideoFormat. Usage: run.sh BIN_DIR SOURCE "MODE:FORMAT ..."
#   BIN_DIR has harness/ (interop/libomtnet-harness + libvmx.dylib) and omt-recv.
#   SOURCE is what the harness sends: uyvy, uyva, p216 or pa16.
#   MODE is full or preview; FORMAT an OMTPreferredVideoFormat name.
set -u
BIN=$1; SRC=$2; CASES=$3
H=$BIN/harness/libomtnet-harness
n=$(echo $CASES | wc -w)
"$H" send "decode-$SRC" $((4 + 7 * n)) "$SRC" > "send-$SRC.txt" 2>&1 &
SP=$!
sleep 3
PORT=$(grep -o 'port=[0-9]*' "send-$SRC.txt" | head -1 | cut -d= -f2)
for c in $CASES; do
  mode=${c%%:*}; fmt=${c#*:}
  tag="$SRC-$mode-$fmt"
  "$H" recv "omt://127.0.0.1:$PORT" 5 "$mode" "$fmt" > "$tag-libomtnet.txt" 2>&1 &
  A=$!
  "$BIN/omt-recv" "127.0.0.1:$PORT" 5 "$mode" --format "$fmt" > "$tag-ours.txt" 2>&1 &
  B=$!
  wait $A $B
done
wait $SP
