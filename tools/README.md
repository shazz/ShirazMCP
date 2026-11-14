# tools/ - helper scripts for reverse-engineering with ShirazMCP

Standalone, dependency-free (Python 3.8+ standard library). They work on files on disk; the MCP
server works on the running emulator. Use these first (cheap, no emulator), then ShirazMCP.

| Tool | What | Typical use |
|---|---|---|
| `prg_inspect.py FILE` | GEMDOS header (TEXT/DATA/BSS/symbols), packer/protection/SNDH signature sweep, entropy hint; for raw `.ST` images the boot-sector `$1234` checksum and BPB; `--carve-sndh OUT` extracts an embedded SNDH | Triage before anything else: is it packed? where is the music? |
| `sndh_index.py` | Fuzzy name -> file search over a local SNDH archive by header tags (title, composer, flag, subtunes) | Turn "the tune that plays in screen X" into a file once `identify_music` gave you a name |

`sndh_index.py` needs the SNDH archive, which is third-party and not shipped: download it from
<https://sndh.atari.org> into `refs/sndh_lf/` (gitignored) or pass `--archive`/`--index`. The
reverse lookup (bytes in RAM -> tune) is `db/generate.py` + the `identify_music` tool.

See also `python-client/` (drive the server from a script) and the skills in `.claude/skills/`.

## fetch_refs.py

The skills' full third-party references (BLiTTER manual, Hollis register list, timing tables) are
not in the repo (their licences are not MIT). `.claude/skills/references.json` lists them with a
sha256; your copies live in `refs/` (gitignored).

    python3 tools/fetch_refs.py --check            # all; exit 1 if any is missing or altered
    python3 tools/fetch_refs.py --fetch            # restore from git history or a listed source
    python3 tools/fetch_refs.py --check --skill atari-st-blitter

Each skill that uses one runs `--check` first (its "Step 0") and says so when a document is
missing, instead of filling in values from memory.
