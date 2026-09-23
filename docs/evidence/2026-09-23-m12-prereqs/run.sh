#!/bin/sh
# Our sender forwards pre-encoded VMX1 frames (examples/omt-send-encoded);
# libomtnet's receiver (interop/libomtnet-harness) watches in full, preview
# and compressed-only mode. Usage: run.sh BIN_DIR
#   BIN_DIR has harness/ (libomtnet-harness + libvmx.dylib) and omt-send-encoded.
set -u
BIN=$1
H=$BIN/harness/libomtnet-harness
"$BIN/omt-send-encoded" "m12-prereqs-encoded" 20 > send-encoded.txt 2>&1 &
SP=$!
sleep 2
PORT=$(grep -o 'port=[0-9]*' send-encoded.txt | head -1 | cut -d= -f2)
"$H" recv "omt://127.0.0.1:$PORT" 5 - UYVY > recv-full-UYVY.txt 2>&1
"$H" recv "omt://127.0.0.1:$PORT" 5 preview UYVY > recv-preview-UYVY.txt 2>&1
"$H" recv "omt://127.0.0.1:$PORT" 5 compressed > recv-compressed.txt 2>&1
wait $SP
