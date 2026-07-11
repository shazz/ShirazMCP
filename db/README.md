# SNDH tune-fingerprint database (Shazam-for-SNDH)

Fingerprint database that lets ShirazMCP identify the **specific tune** playing in
a running demo by matching the music data in RAM against the [SNDH archive](https://sndh.atari.org)
(GitHub issue #12). This is tune identification — naming the *song* — distinct from
player recognition (naming the *engine*, issue #3).

## Files

| File | What |
|---|---|
| `generate.py` | builds the DB from a local SNDH archive |
| `sndh_tunes.json` | array of tunes; index = tune id → `{full, composer, title, file}` |
| `sndh_chunks.jsonl.gz` | gzipped inverted index: `[chunk_hash, [tune_ids]]`, discriminative chunks only |

## Fingerprint scheme (two tiers)

- **Full-image SHA-1** (`full` in `sndh_tunes.json`) — exact fast path. 98.7% of the
  5,897-tune archive gets a unique full hash. Matches when a demo embeds the `.sndh`
  verbatim (common — SNDH is position-independent, so it appears byte-for-byte in RAM).
- **Content-defined chunk hashes** (`sndh_chunks.jsonl.gz`) — fallback. Boundaries are
  set by a gear rolling hash (not fixed offsets), so the music-data chunks match even
  when the surrounding replay code differs. This catches **same-tune / different-player**
  cases the full hash misses — validated on real STE/STFM/SID variants (e.g. Mad Max's
  "Wings of Death STE" ↔ "STFM" at 98% chunk overlap). Match = rank tunes by shared
  discriminative chunks (Jaccard-style overlap).

## Regenerating

The SNDH archive itself is **not committed** (large, third-party) — fetch it from
https://sndh.atari.org into `refs/sndh_lf/` (a `<composer>/<tune>.sndh` tree), then:

```sh
python3 db/generate.py                 # reads refs/sndh_lf/ by default
python3 db/generate.py /path/to/sndh   # or an explicit archive path
```

Corpus stats: 5,897 files → 5,835 unique tunes, ~172k discriminative chunks
(89% of chunks are unique to a single tune).
