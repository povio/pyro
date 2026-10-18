#!/usr/bin/env python3
"""Aggregate retained JSC text summaries; argument is the evidence directory.

No inferred milliseconds: sampling intervals are nominal. Top-function tables
are truncated by JSC, so counts there are published lower bounds, not totals.
"""
import collections
import hashlib
import json
from pathlib import Path
import re
import sys


def aggregate(root):
    verified = 0
    for line in (root / "SHA256SUMS").read_text().splitlines():
        if not line.strip():
            continue
        expected, name = line.split(None, 1)
        path = root / name.strip()
        if root.resolve() not in path.resolve().parents:
            raise ValueError("checksum path escapes evidence directory")
        if hashlib.sha256(path.read_bytes()).hexdigest() != expected:
            raise ValueError(f"checksum mismatch: {name}")
        verified += 1
    result = {"verified_files": verified, "lanes": {}}
    for lane in ("B2", "C2"):
        files = sorted((root / f"b7-{lane}" / "profile").glob("JSCSamp*.txt"))
        if not files:
            raise ValueError(f"missing profiles: {lane}")
        tiers = collections.Counter()
        functions = collections.Counter()
        records = []
        for path in files:
            text = path.read_text()
            totals = set(map(int, re.findall(r"Total samples: (\d+)", text)))
            if len(totals) != 1 or next(iter(totals)) == 0:
                raise ValueError(f"empty or inconsistent profile: {path.name}")
            total = next(iter(totals))
            counts = dict((m[1].strip(), int(m[2])) for m in re.finditer(
                r"^([A-Za-z/+ ]+):\s+(\d+)\s+\(", text, re.M))
            # Builtins are counted again alongside the execution tier by JSC.
            if sum(v for k, v in counts.items() if k != "js builtin") != total:
                raise ValueError(f"exclusive tier sum differs: {path.name}")
            tiers.update(counts)
            top = text.split("Top functions", 1)[1].split("Sampling rate", 1)[0]
            for match in re.finditer(r"^\s*(\d+)\s+'(.*)'", top, re.M):
                # Preserve hash/source ID: native names alone are ambiguous.
                functions[match[2]] += int(match[1])
            records.append({"file": path.name, "samples": total, "tiers": counts})
        total = sum(r["samples"] for r in records)
        result["lanes"][lane] = {
            "samples": total, "profiles": records,
            "tier_counts": dict(tiers),
            "tier_percent_of_samples": {k: round(v * 100 / total, 2) for k, v in tiers.items()},
            "published_top_function_counts": dict(functions.most_common()),
        }
    return result


if __name__ == "__main__":
    print(json.dumps(aggregate(Path(sys.argv[1])), indent=2))
