"""Checks libomtnet's view of frames our sender forwarded pre-encoded."""
import re
sent = {n: (int(l), h) for n, l, h in re.findall(
    r'sent N=(\d+) vmx_len=(\d+) decoded_uyvy_fnv1a64=(\w+)', open('send-encoded.txt').read())}
full = re.findall(r'pixels <HarnessFrame N=\\x22(\d+)\\x22 />\S* fnv1a64=(\w+)',
                  open('recv-full-UYVY.txt').read())
print(f"full UYVY: {len(full)} tagged frames, "
      f"{sum(sent.get(n, (0, ''))[1] == h for n, h in full)} with pixels identical to vmx_codec's decode")
comp = re.findall(r'compressed=(\d+) meta="<HarnessFrame N=\\x22(\d+)',
                  open('recv-compressed.txt').read())
print(f"compressed-only: {len(comp)} tagged frames, "
      f"{sum(n in sent and int(l) == sent[n][0] for l, n in comp)} with the length we sent")
prev = re.findall(r'recv video \S+ (\d+x\d+) codec=\w+ flags=(\d+) .*compressed=(\d+) meta="<HarnessFrame N=\\x22(\d+)',
                  open('recv-preview-UYVY.txt').read())
print(f"preview: {len(prev)} tagged frames, sizes {sorted(set(p[0] for p in prev))}, "
      f"flags {sorted(set(p[1] for p in prev))}, "
      f"{sum(n in sent and int(l) == sent[n][0] for _, _, l, n in prev)} carrying the full bitstream (P4)")
