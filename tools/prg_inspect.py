#!/usr/bin/env python3
"""First look at an Atari ST file: GEMDOS header, packer/protection/SNDH signatures, boot sector.

    python3 tools/prg_inspect.py FOO.PRG            # header + signature sweep + SNDH carve info
    python3 tools/prg_inspect.py DISK.ST            # boot sector: $1234 checksum, BPB
    python3 tools/prg_inspect.py FOO.PRG --carve-sndh out.sndh   # write the SNDH it finds

Do this BEFORE booting an emulator: a packed file shows nothing to grep, and every offset you
compute later is relative to the TEXT segment, so the header must be right first.

Signatures are hints, not proof (a string can sit in data). Packer/protection detail:
see the atari-st-packers and atari-st-protections skills.
Dependencies: Python 3.8+ standard library only.
"""
import argparse
import struct
import sys

SIGS = {
    b"ICE!": "Pack-Ice", b"Ice!": "Pack-Ice (old)", b"ATOM": "Atomik (ATOM..)",
    b"ATM5": "Atomik 3.x", b"ATM3": "Atomik 3.x", b"AUTOMATION": "Automation",
    b"TPWM": "TurboPacker", b"LSD!": "LSD packer", b"RNC\x01": "Rob Northen RNC method 1", b"RNC\x02": "Rob Northen RNC method 2",
    b"UPX!": "UPX", b"Pack-Ice": "Pack-Ice string",
    b"SNDH": "SNDH music", b"HDNS": "SNDH end marker", b"Copylock": "Copylock string",
}


def parse_prg(d):
    if len(d) < 28 or struct.unpack(">H", d[:2])[0] != 0x601A:
        return None
    _, text, data, bss, sym, _res, flags, absflag = struct.unpack(">HIIIIIIH", d[:28])
    return {"text": text, "data": data, "bss": bss, "sym": sym, "flags": flags, "absflag": absflag}


def report_prg(d, h):
    t, da = h["text"], h["data"]
    print(f"GEMDOS PRG  TEXT={t} DATA={da} BSS={h['bss']} SYMTAB={h['sym']} "
          f"flags=${h['flags']:08x} absflag={h['absflag']}")
    end = 28 + t + da + h["sym"]
    print(f"  relocation table at file offset {end} ({max(len(d) - end, 0)} bytes)"
          f"{'  <- none (absolute) ' if h['absflag'] else ''}")
    if 28 + t + da + h["sym"] > len(d):
        print("  WARNING: header sizes exceed the file - truncated, packed or not a plain PRG")
    entropy_hint(d[28:28 + t])


def entropy_hint(buf):
    if not buf:
        return
    distinct = len(set(buf[:4096]))
    zero = buf[:4096].count(0) / min(len(buf), 4096)
    verdict = "looks packed/encrypted (high entropy)" if distinct > 240 and zero < 0.05 else "looks like plain code/data"
    print(f"  first 4 KB: {distinct}/256 distinct byte values, {zero:.0%} zeros -> {verdict}")


def sweep(d):
    print("Signature sweep (file offset; TEXT-relative = offset - 28 for a PRG):")
    found = False
    for sig, name in SIGS.items():
        i, n = d.find(sig), d.count(sig)
        if i >= 0:
            found = True
            print(f"  {name:28s} at 0x{i:06x} (x{n})")
    if not found:
        print("  nothing found - packed with an unknown scheme, encrypted, or genuinely clean")


def carve_sndh(d, out):
    tag = d.find(b"SNDH")
    if tag < 12:
        print("No SNDH tag.")
        return
    start = tag - 12
    end = d.find(b"HDNS", tag)
    head = d[start:start + 12]
    ok = head[0:2] == b"\x60\x00" or head[0] == 0x60
    print(f"SNDH tag at 0x{tag:x}, file would start at 0x{start:x} (entry stubs "
          f"{'look like bra' if ok else 'DO NOT look like bra - check'})")
    tags = {}
    for k in (b"TITL", b"COMM", b"RIPP", b"CONV", b"YEAR", b"FLAG"):
        p = d.find(k, tag, tag + 600)
        if p >= 0:
            e = d.find(b"\0", p)
            tags[k.decode()] = d[p + 4:e].decode("latin-1", "replace")
    print("  tags:", tags or "none")
    if end < 0:
        print("  no HDNS terminator: length unknown - bisect by playing candidate sizes")
        end = min(len(d), start + 0x2000)
    else:
        end += 4
    if out:
        open(out, "wb").write(d[start:end])
        print(f"  wrote {end - start} bytes to {out}")


def boot_sector(d):
    s = d[:512]
    if len(s) < 512:
        return
    total = sum(struct.unpack(">256H", s)) & 0xFFFF
    print(f"Boot sector: word sum ${total:04x} -> {'EXECUTABLE ($1234)' if total == 0x1234 else 'not executable'}; "
          f"first bytes {s[:4].hex()}")
    w = lambda o: struct.unpack("<H", s[o:o + 2])[0]  # BPB fields are little-endian
    bps, spc, res, nfats, ndir, nsec, media, spf, spt, sides = (
        w(11), s[13], w(14), s[16], w(17), w(19), s[21], w(22), w(24), w(26))
    print(f"  BPB (little-endian): {bps} B/sector, {spc} sec/cluster, {res} reserved, {nfats} FATs, "
          f"{ndir} dir entries, {nsec} sectors, media ${media:02x}, {spf} sec/FAT, {spt} sec/track, {sides} sides")
    if total == 0x1234:
        print("  Executable boot sector: disassemble from offset $1E (or $0), check for virus tells "
              "(writes to $426/$42A/$472/$476, copies itself).")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("file")
    ap.add_argument("--carve-sndh", metavar="OUT", help="write the embedded SNDH to OUT")
    a = ap.parse_args()
    d = open(a.file, "rb").read()
    print(f"{a.file}: {len(d)} bytes")
    h = parse_prg(d)
    if h:
        report_prg(d, h)
    elif a.file.lower().endswith((".st", ".img")) or len(d) in (368640, 409600, 737280, 819200, 1474560):
        boot_sector(d)
    else:
        print("Not a GEMDOS PRG ($601A) and not a recognisable raw disk image "
              "(.MSA/.DIM/.STX need conversion first).")
    sweep(d)
    if b"SNDH" in d:
        carve_sndh(d, a.carve_sndh)
    return 0


if __name__ == "__main__":
    sys.exit(main())
