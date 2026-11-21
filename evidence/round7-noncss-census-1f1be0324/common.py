from pathlib import Path
import json,hashlib
p=Path(__file__).resolve().parent
cache=p.parents[1]
d=cache/'round6-noncss-measure'
old=d/'manifest-runs/census-8e7144b7e'
ref=cache/'round6-reference-158/census'
qa=cache/'round7-qa/combined-1f1be0324'
binary=qa/'pyro'
head='1f1be032436bfeee9f48cdb5acbc0f71d21d5af9'
digest='543a7b85e3ecd8b99400d8c0c6861ff4756621b106d86407ca9d9098ca9b0d4d'
def load(f):return json.loads(f.read_bytes())
def sha(f):
 with f.open('rb') as s:return hashlib.file_digest(s,'sha256').hexdigest()
def save(f,x):f.write_text(json.dumps(x,indent=1)+'\n')
