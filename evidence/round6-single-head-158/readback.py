"""Join the previously published local non-CSS census and Studio CSS reference to frozen LP."""
from collections import Counter, defaultdict
from pathlib import Path
import gzip
import hashlib
import json

root = Path(__file__).resolve().parent
previous = root.parent / "round6-chrome158"
supp = root / "supplement"

def load(path):
    return json.loads(path.read_bytes())

def rows(path):
    with gzip.open(path, "rt") as source:
        for line in source:
            yield json.loads(line)

def emit(handle, row):
    handle.write(json.dumps(row, separators=(",", ":")) + "\n")

def full(page):
    return page is not None and page[2]

def compare(counter, lp, chrome):
    counter.update(paired=1, lpFull=int(full(lp)), chromeFull=int(full(chrome)),
                   lpOnlyFull=int(full(lp) and not full(chrome)),
                   chromeOnlyFull=int(full(chrome) and not full(lp)))

def bundle(name):
    return gzip.open(root / (name + ".jsonl.gz"), "wt", compresslevel=6)

wpt = "a421f2fea5eb7f91e0db399e44b0fb0779ee1603"
previous_stats = load(previous / "cbdeb-comparison.json")
assert previous_stats["stage"] == "final"
scope = defaultdict(Counter)
inventory = {r[0]:r for r in rows(previous / "cross-machine-inventory.jsonl.gz")}
local = {r[0]:r[1:] for r in rows(previous / "cbdeb-pages.jsonl.gz")}
css = {r[0]:r for r in rows(previous / "studio-css-pages.jsonl.gz")}
assert len(inventory) == 39989 and len(local) == 31450 and len(css) == 8162
lp_names = defaultdict(dict)
for url, canonical, occurrence, lp, chrome in rows(previous / "cbdeb-names.jsonl.gz"):
    if url in css and lp:
        status, original = lp
        assert (original,occurrence) not in lp_names[url]
        lp_names[url][original, occurrence] = status
selection = json.loads(gzip.decompress((supp / "selections.json.gz").read_bytes()))
done = {r["key"]:r for r in map(json.loads,(supp / "completed.jsonl").read_text().splitlines())}
holds = load(supp / "additional-holds.json")
partial = load(supp / "css-full-067.json.partial")
interrupted = set(selection["css-full-067"])
assert len(interrupted) == 95 and not partial["results"]
assert set(selection) == set(done) | {"css-full-067"}
added = {}
for key, receipt in done.items():
    raw = gzip.decompress((supp / (key + ".json.gz")).read_bytes())
    assert hashlib.sha256(raw).hexdigest() == receipt["receiptSHA256"]
    shard = json.loads(raw)
    assert shard["wptRevision"] == wpt and shard["jobs"] == 4
    assert not shard["restarts"] and not shard["watchdogKills"]
    assert set(selection[key]) == {r["url"] for r in shard["results"]}
    assert len(selection[key]) == len(shard["results"])
    for r in shard["results"]:
        url = r["url"]
        assert url not in local and url not in added and css[url][1:4] == [r["source"],r["blob"],r["environment"]]
        counts = Counter()
        for sub in r["subtests"]:
            name = sub["name"]
            counts[name] += 1
            lp_names[url][name, counts[name]] = sub["status"]
        passed = sum(v == "Pass" for v in lp_names[url].values())
        strict = r["status"] == "complete" and r["harnessStatus"] == "OK" and bool(counts) and passed == len(r["subtests"])
        added[url] = [[r["status"],r["harnessStatus"],strict],key]
studio_names = defaultdict(dict)
mac_names = defaultdict(dict)
for url, name, occurrence, studio, mac in rows(previous / "studio-css-names.jsonl.gz"):
    if studio is not None:
        studio_names[url][name,occurrence] = studio
    if mac is not None:
        mac_names[url][name,occurrence] = mac

with bundle("css-names") as names, bundle("inventory") as pages:
    for url,source,blob,env,category,label,mac,studio in inventory.values():
        lp = local[url][0] if url in local else added.get(url,[None])[0]
        shard = local[url][2] if url in local else added.get(url,[None,None])[1]
        reason = None
        if url in css:
            ref = studio["page"]
            if lp is None:
                reason = (["shard interrupted; individual execution unknown"] if url in interrupted
                          else holds.get(url) or [css[url][8]])
                assert all(reason)
            for name,i in sorted(lp_names[url].keys() | studio_names[url].keys() | mac_names[url].keys()):
                l = lp_names[url].get((name,i))
                c = studio_names[url].get((name,i))
                m = mac_names[url].get((name,i))
                emit(names,[url,name,i,l,c,m])
                if lp and ref:
                    counter = scope["CSS"]
                    counter.update(lpPass=int(l=="Pass"),chromePass=int(c=="Pass"),
                                   lpOnly=int(l=="Pass" and c!="Pass"),chromeOnly=int(c=="Pass" and l!="Pass"),
                                   both=int(l==c=="Pass"),missingLpName=int(c is not None and l is None),
                                   missingChromeName=int(l is not None and c is None))
        else:
            ref = mac["page"]
            if lp is None:
                reason = mac["reasons"]
                assert reason
        emit(pages,[url,source,blob,env,category,lp,shard,reason,mac,studio])
        if lp and ref:
            compare(scope[category],lp,ref)
for k,v in previous_stats["scopes"]["non-css"].items():
    if k in scope["non-CSS"]:
        assert scope["non-CSS"][k] == v
    scope["non-CSS"][k] = v
assert scope["CSS"]["paired"] == 1091+6998-95
assert sum(row["paired"] for row in scope.values()) == 38353
stats = dict(wpt=wpt,lp=previous_stats["head"],lpBinarySha256=previous_stats["binarySHA256"],
             chrome="158.0.8127.37",manifestVariants=len(inventory),supplementComplete=len(added),
             interrupted=len(interrupted),additionalHolds=len(holds),studioExcluded=39,
             matched=scope,combined=scope["non-CSS"]+scope["CSS"])
(root / "summary.json").write_text(json.dumps(stats,indent=2)+"\n")
