#!/usr/bin/env python3
"""Summarize raw/*.txt into SUMMARY.json (min/median/max per configuration and variant)."""
import json, pathlib, re, statistics, sys

d = pathlib.Path(sys.argv[1])
rx = re.compile(r"^end footprint \+([\d.]+) MiB  rss \+([\d.]+) MiB  cpu ([\d.]+) s  wall ([\d.]+) s", re.M)
hdr = re.compile(r"source_bytes=(\d+) utf16_units=(\d+)")
rows = {}
for f in sorted((d / "raw").glob("*.txt")):
    config, rnd, variant, _ = f.name.rsplit(".", 3)
    text = f.read_text()
    m, h = rx.search(text), hdr.search(text)
    assert m and h, f
    rec = rows.setdefault(config, {"source_bytes": int(h[1]), "utf16_units": int(h[2]), "variants": {}})
    v = rec["variants"].setdefault(variant, {"footprint_mib": [], "rss_mib": [], "cpu_s": [], "wall_s": []})
    for k, x in zip(("footprint_mib", "rss_mib", "cpu_s", "wall_s"), m.groups()):
        v[k].append(float(x))
out = {}
for config, rec in rows.items():
    out[config] = {**{k: rec[k] for k in ("source_bytes", "utf16_units")}, "variants": {}}
    for variant, v in rec["variants"].items():
        out[config]["variants"][variant] = {
            k: {"n": len(xs), "min": min(xs), "median": round(statistics.median(xs), 3), "max": max(xs)} for k, xs in v.items()
        }
json.dump(out, sys.stdout, indent=1)
print()
