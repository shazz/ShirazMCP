---
name: atari-st-protections
description: Atari ST software protection / encryption systems (Rob Northen Copylock, Sly Packer, Anti-bitos, Toxic Packer, Little Protection, Cooper, Lock-o-matic, CID Encrypter, etc.) — what they do, how to recognize a protected/encrypted executable, and how to strip them. Distinct from compression packers. Use when a program is encrypted/anti-debugged rather than merely packed, or when building a deprotection plugin.
---

# Atari ST protections / encryption systems

A **protection** encrypts and/or anti-debugs an executable to resist cracking — as
opposed to a **packer**, which only compresses (see `atari-st-packers`). The ripper
must recognize a protected program (it decrypts itself at runtime, uses illegal
opcodes / trace-vector traps / stack tricks) and, to analyze it statically, strip
the protection first.

## Known systems (from `dec0de`)

| Protection | Author / notes |
|-----------|----------------|
| **Rob Northen Copylock** series 1 (1988) & 2 (1989) | The famous one — disk-based + code encryption, trace-vector traps |
| Toxic Packer v1.0 | NTM / Cameo |
| Little Protection v01, Megaprot v0.02 | R.AL |
| Sly Packer v2.0 | Orion |
| Cooper v0.5 & v0.6 | Cameo |
| Anti-bitos v1.0 / v1.4 / v1.6 / v1.61 | Illegal |
| Little Protection v2.05 & v2.06 | Zippy |
| Lock-o-matic v1.3 | Yoda |
| CID Encrypter v1.0bp | CID |

## Recognition tells

- Self-decrypting entry: the first instructions XOR/ADD-loop over the following code
  before jumping into it.
- **Trace/illegal-vector abuse:** installs handlers on the trace exception (`$24`),
  illegal-instruction (`$10`), privilege-violation (`$20`), or line-A/line-F
  (`$28`/`$2C`) vectors to detect debuggers and to decrypt on the fly.
- SR/stack manipulation to set the trace bit; timing checks; `move sr` privilege games.
- Copylock: reads fuzzy/long-track disk data as a key; the classic `$12345678`-style
  serial hidden in the decrypt loop.

## Stripping — reference tool `dec0de`

**`refs/orionfuzion/dec0de/`** is a complete, cross-platform deprotector covering all
the systems above:

- `src/dec0de.c` (~7200 lines, single file, well-documented) — the algorithms.
- `prebuilt/linux/dec0de` — a **runnable Linux binary** we can shell out to for
  ground-truth deprotection while developing/validating a native port.
- `samples/` — protected program samples per system (test corpus).

Design options for the MCP:
1. **Wrap** `dec0de` (shell out to the prebuilt binary) for on-disk `.PRG` files — fast
   path, zero reimplementation.
2. **Port** specific unprotectors into `Depacker`-style plugins (a `Deprotect` kind)
   for in-memory / integrated use.
3. In a **running** Hatari session, a protected program has usually already
   self-decrypted in RAM — so scanning live memory often sidesteps the protection
   entirely (analyze the decrypted image, like with packers).

## Related

- `atari-st-packers` — compression (ICE/Atomik) vs protection (here).
- `atari-st-bootsector` — disk-based protections often live in the bootsector.
- `r1pp3r` (see bootsector skill) — sandbox-executes protected boot-cracktros to rip them.
