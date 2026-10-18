"""Regenerate numeric summaries only; RESULT.md remains hand-maintained."""
from pathlib import Path
from collections import Counter
import json
import statistics

out = Path(__file__).parent
result = {"scans": [], "profiles": []}
for label in ("main", "candidate"):
    for order in ("initial", "reverse"):
        row = json.loads((out / f"{label}-scan-{order}.json").read_text())
        for size in (294, 600):
            samples = [r["elapsedMs"] for r in row["samples"] if r["size"] == size and not r["warmup"]]
            result["scans"].append({"label": label, "order": order, "links": size, "samples": samples,
                                    "medianMs": statistics.median(samples)})
    rows = json.loads((out / f"profile-{label}" / "observations.json").read_text())
    for process in rows:
        for nav in process["navigations"]:
            events = nav["network"]
            cpu = lambda sample: (sample["userNs"] + sample["systemNs"]) / 1e6
            cached = {event.get("ordinal") for event in events if event["kind"] == "cache" or
                      event["kind"] == "response" and event.get("fromDiskCache")}
            cached.discard(None)
            assets = [e["hostMs"] for e in events if e.get("type") in {"Script", "Stylesheet", "Image", "Font", "Media"}]
            result["profiles"].append({
                "label": label, "navigationOrdinal": nav["navigationOrdinal"],
                "usableMs": nav["usableHostMs"] - nav["startHostMs"],
                "dclMs": nav["dclHostMs"] - nav["startHostMs"],
                "loadMs": nav["loadHostMs"] - nav["startHostMs"],
                "postUsabilityHorizonMs": nav["observationEndHostMs"] - nav["usableHostMs"],
                "lastObservedAssetMs": max(assets) - nav["startHostMs"] if assets else None,
                "browserCpuToUsableMs": cpu(nav["usable"]) - cpu(nav["before"]),
                "browserCpuToHorizonMs": cpu(nav["end"]) - cpu(nav["before"]),
                "rssUsableMiB": nav["usable"]["rssKiB"] / 1024,
                "requestTypes": dict(Counter(e.get("type", "Other") for e in events if e["kind"] == "request")),
                "cacheHitRequests": len(cached),
                "cdpDecodedDataBytes": sum(e.get("dataLength", 0) for e in events if e["kind"] == "data"),
                "terminalEncodedBodyBytes": sum(e.get("encodedDataLength", 0) for e in events if e["kind"] == "finished"),
                "failedRequests": sum(e["kind"] == "failed" for e in events),
            })
(out / "summary.json").write_text(json.dumps(result, indent=2) + "\n")
print("Wrote summary.json; narrative qualifications preserved.")
