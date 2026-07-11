#!/usr/bin/env python3
"""Shazam-for-SNDH: boot a demo, find its embedded SNDH music in live RAM, and
identify the tune two ways — by the SNDH header tags, and by fingerprint-matching
against the db/ tune database.

    python3 examples/identify_tune.py /path/to/harddrive

Requires the fingerprint DB in ../../db/ (see db/README.md) plus a built binary,
an X display, and SHIRAZMCP_TOS. This is the runnable prototype of GitHub #12.
"""
import gzip
import hashlib
import json
import os
import sys
from collections import Counter

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from mcp_client import McpClient  # noqa: E402
from boot_screenshot import find_binary  # noqa: E402

DB = os.path.join(os.path.dirname(__file__), "..", "..", "db")
GEAR = [int.from_bytes(hashlib.md5(bytes([i])).digest()[:4], "big") for i in range(256)]
MASK32, AVG_MASK, MIN_CHUNK, MAX_CHUNK = 0xFFFFFFFF, 0x1FF, 128, 2048


def cdc(data: bytes):
    n, s = len(data), 0
    while s < n:
        h, i, lim, cut = 0, s, min(s + MAX_CHUNK, n), min(s + MAX_CHUNK, n)
        while i < lim:
            h = ((h << 1) + GEAR[data[i]]) & MASK32
            i += 1
            if i - s >= MIN_CHUNK and (h & AVG_MASK) == 0:
                cut = i
                break
        yield data[s:cut]
        s = cut


def chunk_set(data: bytes) -> set:
    return {hashlib.blake2b(c, digest_size=8).hexdigest() for c in cdc(data)}


def sndh_tags(buf: bytes) -> dict:
    m = {}
    for tag, key in ((b"TITL", "title"), (b"COMM", "composer"), (b"YEAR", "year")):
        p = buf.find(tag, 0, 600)
        if p != -1:
            e = buf.find(b"\0", p + 4)
            if e != -1:
                m[key] = buf[p + 4:e].decode("latin-1", "replace").strip()
    return m


def read_range(c, base, total, step=1024) -> bytes:
    # read_memory returns at most 1024 bytes/call — step must stay <= that or
    # reads come back sparse (gappy), which wrecks the fingerprint.
    out = bytearray()
    while len(out) < total:
        r = c.call("read_memory", {"addr": f"${base + len(out):X}", "count": min(step, total - len(out))})
        out += bytes.fromhex(r["bytes_hex"])
    return bytes(out[:total])


def match_region(region: bytes, tunes, index):
    hdns = region.find(b"HDNS")
    ds = (hdns + 4) if hdns != -1 else 0
    votes = Counter()
    cs = chunk_set(region[ds + (ds & 1):])
    for ch in cs:
        for tid in index.get(ch, []):
            votes[tid] += 1
    disc = sum(1 for ch in cs if ch in index)
    return votes, disc


def main() -> None:
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    tunes = json.load(open(os.path.join(DB, "sndh_tunes.json")))
    index = {}
    for line in gzip.open(os.path.join(DB, "sndh_chunks.jsonl.gz"), "rt"):
        ch, ids = json.loads(line)
        index[ch] = ids

    env = dict(os.environ)
    env.setdefault("DISPLAY", ":0")
    c = McpClient(find_binary(), env=env)
    try:
        c.initialize()
        c.call("start", {"harddrive": sys.argv[1], "machine": "ste", "memsize_mb": 1}, timeout=30)
        c.call("run_frames", {"frames": 500}, timeout=40)

        hits = c.call("search_memory", {"pattern_ascii": "SNDH", "limit": 16})["hits"]
        bases = []
        for h in hits:
            a = int(h["addr"].replace("$", ""), 16)
            if bytes.fromhex(c.call("read_memory", {"addr": f"${a-12:X}", "count": 2})["bytes_hex"]) == b"\x60\x00":
                bases.append(a - 12)          # bra.w header — real SNDH image
        bases.sort()
        if not bases:
            print("no embedded SNDH image found"); return
        print(f"found {len(bases)} embedded SNDH tune(s) at {[hex(b) for b in bases]}\n")

        # identify each tune, bounded by the next SNDH so tunes don't blend
        for k, base in enumerate(bases):
            end = bases[k + 1] if k + 1 < len(bases) else base + 8192
            region = read_range(c, base, end - base)
            tags = sndh_tags(region)
            votes, disc = match_region(region, tunes, index)
            print(f"@ {hex(base)}  tags={tags}")
            if votes:
                tid, v = votes.most_common(1)[0]
                t = tunes[tid]
                print(f"  fingerprint -> {t.get('composer','?')} / {t.get('title','?')}  "
                      f"({v}/{disc} chunks)  [{t['file']}]\n")
            else:
                print(f"  fingerprint -> no DB match ({disc} discriminative chunks)\n")
    finally:
        c.call("stop")
        c.close()


if __name__ == "__main__":
    main()
