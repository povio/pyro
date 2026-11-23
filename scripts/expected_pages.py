"""Final expected-pages export for the round 7 batch-1000 build (documentation only; nothing is built).

Usage: python3 scripts/expected_pages.py <freeze-sha>
Writes expected-pages.jsonl (one line per census page credited to an implemented root)
and expected-roots.jsonl (one line per implemented root, incl. owner-reported unlisted counts).
Page selection matches scripts/estimate.py: census 5892e136 strict pages (Chrome strict pass,
Pyro not), page_list when a row has one, else the row's SEL selector.
Confidence per page: a row whose owner figure is below its listed window pages has its
surplus pages marked tentative ('.tentative' URLs first, then the rest in list order);
a page firm in any row is firm. Worker/sharedworker/serviceworker variants are flagged and
not counted in the window totals."""
import gzip, json, os, re, sys, collections

FREEZE = sys.argv[1] if len(sys.argv) > 1 else "?"
Q = os.path.expanduser("~/Library/Caches/lightningpanda/round7-queue")
ns = {}
src = open(os.path.join(Q, "scripts/estimate.py")).read().rsplit("main(sys.argv[1],sys.argv[2])", 1)[0]
exec(compile(src, "estimate.py", "exec"), ns)
SEL, B = ns["SEL"], ns["B"]
strict = ns["load"]()
q01 = [u for _, u, _ in json.load(open(os.path.join(Q, "q01-named-access-pages.json"))) if "command-and-commandfor" not in u]

W = lambda x: bool(re.search(r"\.(worker|sharedworker|serviceworker)(-module)?\.html", x))
MB = ("html", "fetch", "IndexedDB", "encoding"); SD = ("css", "referrer-policy", "content-security-policy")
IMPL = ("frozen", "implemented-unbuilt", "landed")
mach = lambda u: "MacBook" if u.split("/")[0] in MB else "Studio" if u.split("/")[0] in SD else "other"
shas = lambda s: sorted(set(re.findall(r"\b[0-9a-f]{9}\b|\b[0-9a-f]{40}\b", str(s or ""))))

L = [json.loads(l) for l in open("ledger.jsonl")]
pages = {}   # url -> record
roots = []
for r in L:
    if r["state"] not in IMPL:
        continue
    i = r["id"]
    pl = r.get("page_list")
    css_listed = [u.lstrip("/") for u in (pl or []) if u.lstrip("/").startswith("css/")]
    if isinstance(pl, list):
        pg = [u for u in pl if u in strict]
    elif i == "Q01":
        pg = [u for u in q01 if u in strict]
    elif SEL.get(i):
        pg = sorted(u for u in strict if any(re.search(p, u) for p in SEL[i]))
    else:
        pg = None
    owner = r.get("est_pages_owner") if isinstance(r.get("est_pages_owner"), int) else None
    firm, tent = [], []
    if pg is not None:
        win = [u for u in pg if not W(u)]
        nvar = len(pg) - len(win)
        cut = 0
        if owner is not None and owner < len(pg):
            cut = max(0, (len(pg) - owner) - nvar)
        tp = r.get("tentative_pages")  # owner-named tentative pages go first (some rows store a count instead)
        named = set(tp) if isinstance(tp, list) else set()
        order = sorted(win, key=lambda u: (u not in named, ".tentative" not in u, win.index(u)))
        tent = order[:cut]
        firm = [u for u in win if u not in tent]
        for u in pg:
            p = pages.setdefault(u, {"url": u, "dir": u.split("/")[0], "machine": mach(u),
                                     "variant": "worker" if W(u) else "window",
                                     "roots": [], "commits": [],
                                     "confidence": "worker-variant (not counted)" if W(u) else "tentative"})
            p["roots"].append(i)
            for c in shas(r.get("commit")):
                if c not in p["commits"]:
                    p["commits"].append(c)
            if u in firm:
                if r.get("confidence_override") == "tentative-counted":  # coordinator ruled: counted, but not firm
                    if p["confidence"] != "firm":
                        p["confidence"] = "tentative-counted"
                else:
                    p["confidence"] = "firm"
    for u in css_listed:  # Studio CSS pages are outside census 5892e136; listed as owner-reported
        p = pages.setdefault(u, {"url": u, "dir": "css", "machine": "Studio", "variant": "window",
                                 "roots": [], "commits": [], "confidence": "owner-reported (css, not in census)"})
        p["roots"].append(i)
        for c in shas(r.get("commit")):
            if c not in p["commits"]:
                p["commits"].append(c)
    unlisted = owner if (pg is None and owner is not None and not i.startswith(("SC-", "S0"))) else 0
    studio_receipt = None
    if pg is None and not css_listed:
        rc = r.get("receipts") or {}
        sp = rc.get("strict_page_gains_potential") or rc.get("pages")
        try:
            studio_receipt = int(str(sp).split()[0]) if sp is not None else None
        except Exception:
            studio_receipt = None
    roots.append({"root": i, "state": r["state"], "machine": r.get("machine"), "owner": r.get("claimed_by"),
                  "commits": shas(r.get("commit")), "commit_field": r.get("commit"),
                  "listed_pages": len(pg) if pg is not None else None,
                  "window_firm": len(firm), "window_tentative": len(tent),
                  "worker_variants": (len(pg) - len(firm) - len(tent)) if pg is not None else 0,
                  "owner_figure": owner, "owner_reported_unlisted": unlisted or None,
                  "studio_receipt_pages": studio_receipt, "css_listed": len(css_listed) or None,
                  "title": r.get("title")})

# Pages a root targets that Pyro already passes in baseline c3372ad21: listed, 0 credit.
_strict_rc5 = set()
for l in gzip.open(B + "comparison/pages.jsonl.gz", "rt"):
    x = json.loads(l)
    if x[2][2] and not x[1][2]:
        _strict_rc5.add(x[0])
_already = ns["already_passing"]() & _strict_rc5
for r in L:
    if r["state"] not in IMPL:
        continue
    pl = r.get("page_list")
    cand = [u for u in pl if u in _already] if isinstance(pl, list) else \
           [u for u in _already if SEL.get(r["id"]) and any(re.search(p, u) for p in SEL[r["id"]])]
    for u in cand:
        p = pages.setdefault(u, {"url": u, "dir": u.split("/")[0], "machine": mach(u),
                                 "variant": "worker" if W(u) else "window",
                                 "roots": [], "commits": [], "confidence": "alreadyPassing (c3372ad21, 0 credit)"})
        if r["id"] not in p["roots"]:
            p["roots"].append(r["id"])
        for c in shas(r.get("commit")):
            if c not in p["commits"]:
                p["commits"].append(c)

with open("expected-pages.jsonl", "w") as f:
    for u in sorted(pages):
        f.write(json.dumps({**pages[u], "freeze": FREEZE}, ensure_ascii=False) + "\n")
with open("expected-roots.jsonl", "w") as f:
    for x in roots:
        f.write(json.dumps({**x, "freeze": FREEZE}, ensure_ascii=False) + "\n")

S = collections.Counter()
for p in pages.values():
    if p["variant"] == "window" and p["dir"] in MB:
        S[(p["dir"], p["confidence"])] += 1
unl = sum(x["owner_reported_unlisted"] or 0 for x in roots)
mb_firm = sum(v for (d, c), v in S.items() if c == "firm")
mb_tent = sum(v for (d, c), v in S.items() if c == "tentative")
print(json.dumps({"freeze": FREEZE, "pages": len(pages), "roots": len(roots),
                  "macbook_window_firm": mb_firm, "macbook_window_tentative": mb_tent,
                  "macbook_owner_reported_unlisted": unl,
                  "macbook_window_tentative_counted_by_ruling": sum(v for (d, c), v in S.items() if c == "tentative-counted"),
                  "macbook_counted_estimate": mb_firm + unl + sum(v for (d, c), v in S.items() if c == "tentative-counted"),
                  "by_dir": {f"{d}/{c}": v for (d, c), v in sorted(S.items())},
                  "macbook_worker_variants": sum(1 for p in pages.values() if p["variant"] == "worker" and p["dir"] in MB)},
                 indent=1))
