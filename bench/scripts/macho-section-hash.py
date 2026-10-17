"""SHA-256 of code/constant sections of thin 64-bit Mach-O executables.

Usage: python3 macho-section-hash.py BINARY...  -> JSON {binary: {"SEG,sect": sha256}}

Whole-file hashes of two clean builds of the same source differ when built in
different target directories (paths end up in debug info), so they cannot show
which source a benchmark binary was built from. The four selected sections
(__TEXT,__text, __TEXT,__const, __TEXT,__cstring, __DATA_CONST,__const) were
reproducible across fresh builds of the same SHA in the recorded build
conditions (same toolchain, SDK, locked dependencies and release profile; see
bench/results/build-provenance-audit).

Under those same conditions, matching hashes for these selected sections are
consistent with a binary having been built from the compared source; they do
not cover other sections, linking or debug info, and other toolchains or
settings may not reproduce. Differing hashes, under the same conditions, show
the binary's code or constants differ from that build.
"""
import hashlib
import json
import struct
import sys

SECTIONS = {("__TEXT", "__text"), ("__TEXT", "__const"), ("__TEXT", "__cstring"), ("__DATA_CONST", "__const")}


def section_hashes(path):
    data = open(path, "rb").read()
    magic, _cputype, _subtype, _filetype, ncmds, _sizeofcmds, _flags, _reserved = struct.unpack_from("<IiiIIIII", data, 0)
    if magic != 0xFEEDFACF:
        raise SystemExit(f"{path}: not a thin 64-bit little-endian Mach-O")
    out, off = {}, 32
    for _ in range(ncmds):
        cmd, size = struct.unpack_from("<II", data, off)
        if cmd == 0x19:  # LC_SEGMENT_64
            nsects = struct.unpack_from("<I", data, off + 64)[0]
            s = off + 72
            for _ in range(nsects):
                sect = data[s : s + 16].rstrip(b"\0").decode()
                seg = data[s + 16 : s + 32].rstrip(b"\0").decode()
                _addr, sz, foff = struct.unpack_from("<QQI", data, s + 32)
                if (seg, sect) in SECTIONS and foff:
                    out[f"{seg},{sect}"] = hashlib.sha256(data[foff : foff + sz]).hexdigest()
                s += 80
        off += size
    return out


if __name__ == "__main__":
    if len(sys.argv) < 2 or sys.argv[1] in ("-h", "--help"):
        print(__doc__)
        raise SystemExit(0 if len(sys.argv) > 1 else 2)
    print(json.dumps({p: section_hashes(p) for p in sys.argv[1:]}, indent=2))
