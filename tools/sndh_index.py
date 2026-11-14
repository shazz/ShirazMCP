#!/usr/bin/env python3
"""Searchable index of a local SNDH archive: find a tune by NAME (title/composer/file).

    python3 tools/sndh_index.py --build                 # scan refs/sndh_lf once
    python3 tools/sndh_index.py bmx                     # search title/composer/path
    python3 tools/sndh_index.py "leaving terramis"      # fuzzy: finds "Leavin Teramis"
    python3 tools/sndh_index.py --archive ~/sndh --index ~/sndh.json --build

Why: `find -iname` searches FILENAMES, which is how a tune gets reported missing when the file
is `Leavin_Teramis.sndh` but you searched for "Terramis". This indexes the header TAGS (TITL, COMM,
YEAR, FLAG, ##) so a title search finds it whatever the file is called. It is the opposite lookup
of db/generate.py (bytes -> name, used by identify_music); this one is name -> file.

FLAG: a/b/c/d = the MFP timer the replay uses; e = STE DMA sound (an emulator without DMA-PCM
plays it silent). Never trust a flag: play the tune. A subtune count > 1 means one file holds
several songs (a player picks one by number, from 1).

The archive is not bundled (third-party): download it from https://sndh.atari.org.
Dependencies: Python 3.8+ standard library only.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from difflib import SequenceMatcher
from pathlib import Path

# Anchored to the repo root, not the cwd: agents run with their own working directory.
REPO = Path(__file__).resolve().parent.parent  # tools/ -> repo
ARCHIVE = REPO / "refs/sndh_lf"   # refs/ is gitignored
INDEX = REPO / "refs/sndh_index.json"
HEAD = 512  # tags live in the header; no need to read whole files


def tag_str(buf: bytes, tag: bytes) -> str:
    """A NUL-terminated string tag (TITL/COMM/YEAR/FLAG)."""
    p = buf.find(tag, 12, HEAD)
    if p == -1:
        return ""
    e = buf.find(b"\0", p + len(tag))
    if e == -1:
        return ""
    return buf[p + len(tag):e].decode("latin-1", "replace").strip()


def subtune_count(buf: bytes) -> int:
    """The `##NN` tag. Absent means a single song."""
    m = re.search(rb"##(\d\d)", buf[:HEAD])
    return int(m.group(1)) if m else 1


def build(archive: Path, index: Path) -> int:
    files = sorted(archive.rglob("*.sndh"))
    if not files:
        sys.exit(f"no .sndh under {archive} - is the archive present?")
    out = []
    for p in files:
        buf = p.open("rb").read(HEAD)
        if b"SNDH" not in buf[:64]:
            continue  # packed or not an SNDH image; skip rather than guess
        out.append({
            "title": tag_str(buf, b"TITL"),
            "composer": tag_str(buf, b"COMM"),
            "year": tag_str(buf, b"YEAR"),
            "flag": tag_str(buf, b"FLAG"),
            "subtunes": subtune_count(buf),
            "file": str(p.relative_to(archive)),
            "bytes": p.stat().st_size,
        })
    index.parent.mkdir(parents=True, exist_ok=True)
    index.write_text(json.dumps(out, indent=0))
    silent = sum(1 for t in out if "e" in t["flag"])
    multi = sum(1 for t in out if t["subtunes"] > 1)
    print(f"{len(files)} files -> {len(out)} tunes indexed in {index}")
    print(f"  {silent} are FLAG ~e (STE DMA sound: verify by playing)")
    print(f"  {multi} carry more than one subtune")
    return 0


def score(t: dict, q: str) -> float:
    """Best similarity across title, composer and filename.

    Substring hits win outright; otherwise fall back to fuzzy ratio so a
    misremembered spelling ("Terramis" vs "Teramis") still surfaces.
    """
    best = 0.0
    for field in (t["title"], t["composer"], t["file"]):
        f = field.lower()
        if q in f:
            return 1.0 + len(q) / max(len(f), 1)  # prefer the tightest match
        best = max(best, SequenceMatcher(None, q, f).ratio())
    return best


def search(query: str, limit: int, index: Path) -> int:
    if not index.exists():
        sys.exit(f"no {index} - run: python3 {sys.argv[0]} --build")
    tunes = json.loads(index.read_text())
    q = query.lower()
    ranked = sorted(tunes, key=lambda t: score(t, q), reverse=True)[:limit]
    if not ranked or score(ranked[0], q) < 0.45:
        print(f"nothing close to {query!r}. Try a distinctive word, or a stem "
              f"(the archive's spelling often differs from the title you were given).")
        return 1

    print(f"{'TITLE':<34} {'COMPOSER':<20} {'FLAG':<6} {'SUB':>3}  FILE")
    for t in ranked:
        if score(t, q) < 0.45:
            break
        warn = "  <-- STE DMA (~e): verify it plays" if "e" in t["flag"] else ""
        print(f"{t['title'][:33]:<34} {t['composer'][:19]:<20} "
              f"{t['flag']:<6} {t['subtunes']:>3}  {t['file']}{warn}")
    print("\nVerify: boot the host program, then identify_music, or play the file in an SNDH player.")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description="Find a tune in the local SNDH archive by name.")
    ap.add_argument("query", nargs="*", help="title, composer or filename fragment")
    ap.add_argument("--build", action="store_true", help="(re)build the index")
    ap.add_argument("-n", type=int, default=10, help="max results (default 10)")
    ap.add_argument("--archive", type=Path, default=ARCHIVE, help="SNDH archive directory")
    ap.add_argument("--index", type=Path, default=INDEX, help="index file to write/read")
    a = ap.parse_args()
    if a.build:
        return build(a.archive.expanduser(), a.index.expanduser())
    if not a.query:
        ap.error("give a search term, or --build")
    return search(" ".join(a.query), a.n, a.index.expanduser())


if __name__ == "__main__":
    sys.exit(main())
