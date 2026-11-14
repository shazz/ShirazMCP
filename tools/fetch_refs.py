#!/usr/bin/env python3
"""Check (and restore) the third-party reference documents the skills read.

The documents are listed in .claude/skills/references.json. They are not in the repo (not MIT);
each user keeps a local copy under refs/ (gitignored). A copy counts only if its sha256 matches.

usage:
  python3 tools/fetch_refs.py --check [--skill NAME]   # report; exit 1 if any is missing or wrong
  python3 tools/fetch_refs.py --fetch [--skill NAME]   # restore missing ones, then report

--fetch tries, per document: this repo's git history (the commit in references.json, needs a clone
with history), then each listed source URL (only a body whose sha256 matches is kept). Anything it
cannot restore is printed with its title and sources so the user can get it by hand.
Standard library only.
"""
import argparse
import hashlib
import json
import subprocess
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / ".claude" / "skills" / "references.json"


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def state(doc: dict) -> str:
    p = ROOT / doc["path"]
    if not p.is_file():
        return "missing"
    return "ok" if sha256(p.read_bytes()) == doc["sha256"] else "mismatch"


def from_history(commit: str, doc: dict) -> bytes | None:
    try:
        out = subprocess.run(["git", "-C", str(ROOT), "show", f"{commit}:{doc['history_path']}"],
                             capture_output=True, check=True, timeout=30).stdout
    except (subprocess.SubprocessError, OSError):
        return None
    return out if sha256(out) == doc["sha256"] else None


def from_url(url: str, want: str) -> bytes | None:
    req = urllib.request.Request(url, headers={"User-Agent": "ShirazMCP-fetch-refs"})
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            body = r.read()
    except (OSError, ValueError):
        return None
    return body if sha256(body) == want else None


def restore(commit: str, doc: dict) -> str:
    data = from_history(commit, doc)
    how = "git history"
    if data is None:
        for url in doc.get("sources", []):
            data = from_url(url, doc["sha256"])
            if data is not None:
                how = url
                break
    if data is None:
        return "not restored"
    p = ROOT / doc["path"]
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_bytes(data)
    return f"restored from {how}"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    mode = ap.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check", action="store_true")
    mode.add_argument("--fetch", action="store_true")
    ap.add_argument("--skill", help="only the documents this skill uses")
    a = ap.parse_args()
    man = json.loads(MANIFEST.read_text())
    docs = [d for d in man["documents"] if not a.skill or a.skill in d["skills"]]
    if not docs:
        print(f"no reference documents for skill {a.skill!r}")
        return 0
    bad = 0
    for d in docs:
        s = state(d)
        note = ""
        if s != "ok" and a.fetch:
            note = " -> " + restore(man["history_commit"], d)
            s = state(d)
        print(f"{s:8} {d['path']}  ({d['title']}){note}")
        if s != "ok":
            bad += 1
            srcs = ", ".join(d.get("sources", [])) or "no public URL known"
            print(f"         get it by hand: {srcs}; licence: {d['licence']}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
