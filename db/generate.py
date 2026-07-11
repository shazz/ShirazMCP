#!/usr/bin/env python3
"""Generate the Shazam-for-SNDH tune-fingerprint database from the SNDH archive.

Two-tier fingerprint per tune:
  - full-image SHA-1  -> exact fast-path match (98.7% unique on the archive)
  - content-defined chunk hashes (CDC) -> fallback that survives player-version
    mismatch: the music-data chunks match even when the surrounding replay code
    differs (proven on STE/STFM/SID variants of the same tune).

Outputs (loaded by the ShirazMCP `identify_tune` runtime):
  db/sndh_tune_db.json      full-hash -> {composer, title, file}
  db/sndh_chunk_index.jsonl [chunk_hash_hex, [tune_full_hashes]]  (rare chunks only)

Usage:  python3 db/generate.py  [archive_dir]
The archive is NOT committed (large, third-party). Fetch it from
https://sndh.atari.org into refs/sndh_lf/ (or pass a path). See db/README.md.
"""
from __future__ import annotations
import gzip, hashlib, json, os, sys
from collections import Counter, defaultdict

HERE = os.path.dirname(os.path.abspath(__file__))
ARCHIVE = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "..", "refs", "sndh_lf")

GEAR = [int.from_bytes(hashlib.md5(bytes([i])).digest()[:4], "big") for i in range(256)]
MASK32, AVG_MASK, MIN_CHUNK, MAX_CHUNK = 0xFFFFFFFF, 0x1FF, 128, 2048
RARE_MAX = 8  # a chunk is discriminative if it appears in <= this many tunes


def cdc_chunks(data: bytes):
    n, start = len(data), 0
    while start < n:
        h, i, limit, cut = 0, start, min(start + MAX_CHUNK, n), min(start + MAX_CHUNK, n)
        while i < limit:
            h = ((h << 1) + GEAR[data[i]]) & MASK32
            i += 1
            if i - start >= MIN_CHUNK and (h & AVG_MASK) == 0:
                cut = i
                break
        yield data[start:cut]
        start = cut


def chunk_set(data: bytes) -> set:
    return {hashlib.blake2b(c, digest_size=8).hexdigest() for c in cdc_chunks(data)}


def data_start(buf: bytes) -> int:
    hdns = buf.find(b"HDNS", 16)
    ds = (hdns + 4) if hdns != -1 else 16
    return ds + (ds & 1)


def meta_of(buf: bytes) -> dict:
    m = {}
    for tag, key in ((b"TITL", "title"), (b"COMM", "composer")):
        p = buf.find(tag, 12, 512)
        if p != -1:
            e = buf.find(b"\0", p + 4)
            if e != -1:
                m[key] = buf[p + 4:e].decode("latin-1", "replace").strip()
    return m


def main() -> None:
    files = sorted(os.path.join(r, n) for r, _, ns in os.walk(ARCHIVE)
                   for n in ns if n.lower().endswith(".sndh"))
    if not files:
        sys.exit(f"no .sndh files under {ARCHIVE} — see db/README.md")
    tunes, full2id, inv, freq = [], {}, defaultdict(list), Counter()
    for p in files:
        buf = open(p, "rb").read()
        if len(buf) < 20 or buf[:2] != b"\x60\x00":
            continue
        full = hashlib.sha1(buf).hexdigest()
        rel = os.path.relpath(p, ARCHIVE)
        tid = full2id.get(full)
        if tid is None:                       # integer id = index into `tunes`
            tid = len(tunes)
            full2id[full] = tid
            tunes.append({"full": full, **meta_of(buf), "file": rel})
        for c in chunk_set(buf[data_start(buf):]):
            inv[c].append(tid)
            freq[c] += 1

    # tunes[]: id -> {full, composer, title, file}; chunks: [chunk_hex, [ids]]
    json.dump(tunes, open(os.path.join(HERE, "sndh_tunes.json"), "w"))
    with gzip.open(os.path.join(HERE, "sndh_chunks.jsonl.gz"), "wt") as f:
        for c, ids in inv.items():
            if freq[c] <= RARE_MAX:
                f.write(json.dumps([c, sorted(set(ids))], separators=(",", ":")) + "\n")
    print(f"{len(files)} files -> {len(tunes)} unique tunes, "
          f"{sum(1 for v in freq.values() if v <= RARE_MAX)} discriminative chunks")


if __name__ == "__main__":
    main()
