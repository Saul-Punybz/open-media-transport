#!/usr/bin/env python3
"""Joins the two receivers' `pixels` lines on the per-frame metadata and their
audio hashes on the timestamp, and reports how many match."""
import glob, re, sys

def pixels(path):
    out = {}
    for line in open(path, errors="replace"):
        m = re.match(r"pixels (.*) fnv1a64=([0-9a-f]+) stride=(\d+)", line.strip())
        if m:
            out[m.group(1)] = (m.group(2), m.group(3))
    return out

def audio(path, ours):
    out = {}
    pat = r"audio ts=(-?\d+) .*fnv1a64=([0-9a-f]+)" if ours else r"recv audio ts=(-?\d+) .*fnv1a64=([0-9a-f]+)"
    for line in open(path, errors="replace"):
        m = re.match(pat, line.strip())
        if m:
            out[m.group(1)] = m.group(2)
    return out

def codecs(path, ours):
    pat = r"video ts=\S+ (\d+x\d+) codec=(\w+)" if ours else r"recv video ts=\S+ (\d+x\d+) codec=(\w+)"
    return sorted({m.group(1) + " " + m.group(2) for l in open(path, errors="replace") if (m := re.match(pat, l.strip()))})

total_bad = 0
for theirs_path in sorted(glob.glob("*-libomtnet.txt")):
    tag = theirs_path[: -len("-libomtnet.txt")]
    ours_path = tag + "-ours.txt"
    a, b = pixels(theirs_path), pixels(ours_path)
    common = sorted(set(a) & set(b))
    same = [k for k in common if a[k] == b[k]]
    bad = len(common) - len(same)
    total_bad += bad
    ta, tb = audio(theirs_path, False), audio(ours_path, True)
    acommon = set(ta) & set(tb)
    asame = sum(ta[k] == tb[k] for k in acommon)
    total_bad += len(acommon) - asame
    print(f"{tag:40} video {len(same)}/{len(common)} identical "
          f"(libomtnet {codecs(theirs_path, False)}, ours {codecs(ours_path, True)}); "
          f"audio {asame}/{len(acommon)} identical")
    for k in common:
        if a[k] != b[k]:
            print(f"    DIFFER {k}: libomtnet {a[k]} ours {b[k]}")
print("ALL IDENTICAL" if total_bad == 0 else f"{total_bad} DIFFERENCES")
sys.exit(1 if total_bad else 0)
