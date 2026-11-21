import collections, gzip, json
from pathlib import Path
here = Path(__file__).resolve().parent
selection = json.loads((here/'selection.json').read_bytes())
def load(path):
    doc=json.loads(gzip.decompress(path.read_bytes()))
    rows={r['url']:r for r in doc['results']}
    assert sorted(rows)==selection and len(rows)==len(doc['results'])
    return rows
cohorts=[load(here/'lp-base.json.gz'),load(here/'lp-candidate.json.gz'),load(here/'chrome158.json.gz')]
totals=[collections.Counter() for _ in cohorts]
changed=[]; failures=[]
for url in selection:
    cols=[]
    for counts,rows in zip(totals,cohorts):
        row=rows[url]; occ=collections.Counter(); names={}
        for t in row['subtests']:
            name=t['name'];occ[name]+=1;names[name,occ[name]]=t['status'];counts[t['status']]+=1
        counts['fullPages']+=int(row['status']=='complete' and row['harnessStatus']=='OK' and bool(names) and all(v=='Pass' for v in names.values()))
        counts['page_'+row['status']]+=1
        cols.append(names)
    for name,occ in sorted(set().union(*cols)):
        statuses=[c.get((name,occ)) for c in cols]
        if statuses[0]!=statuses[1]:changed.append([url,name,occ,*statuses])
        if statuses[1]!='Pass':failures.append([url,name,occ,*statuses])
gains=sum(c[4]=='Pass' and c[3]!='Pass' for c in changed)
losses=sum(c[3]=='Pass' and c[4]!='Pass' for c in changed)
result=dict(base=totals[0],head=totals[1],chrome=totals[2],gains=gains,losses=losses,failures=failures)
assert result == json.loads((here/"summary.json").read_text())
assert all(c[3] != "Pass" and c[4] == c[5] == "Pass" for c in changed)
print(json.dumps(result,indent=2))
