"""SHA-256 of code/constant sections of thin 64-bit Mach-O executables.

Usage: python3 macho-section-hash.py BINARY...  -> JSON {binary: {"SEG,sect": sha256}}

Whole-file hashes of two clean builds of the same source differ when built in
different target directories (paths end up in debug info), so they cannot show
which source a benchmark binary was built from. The machine-code and constant
sections are reproducible across fresh builds of the same SHA on the same
toolchain (verified in bench/results/build-provenance-audit), so equal section
hashes identify the source a binary was built from; unequal hashes show it
was not built from that source.
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
