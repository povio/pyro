import gzip
import json
import sys
from pathlib import Path


output, frozen, focus = map(Path, sys.argv[2:])
mode = sys.argv[1]
del sys.argv[1:]
sys.path.insert(0, str(frozen))
import verify


primary = verify.load(output / 'outcomes-main.json.gz')
previous = verify.load(output / 'previous/outcomes-main.json.gz')
delta = verify.load(output / 'all-main-vs-previous.json.gz')
selected = set(verify.load(focus / 'spm-urls.json'))
changed = {item[0] for metric in ('observed','passes','status','pageState') for direction in delta[metric].values() for item in direction}
urls = sorted(url for url in selected | changed if primary[url][0] == previous[url][0] == 'complete')
assert set(urls) <= set(verify.load(frozen / 'main.json')) and not set(urls) & set(verify.load(frozen / 'withheld.json'))
if mode == 'plan':
    (output / 'control-urls.json').write_text(json.dumps(urls, indent=1)+'\n')
    print('Safe candidate controls:', len(urls), 'Excluded non-completions:', sorted((selected|changed)-set(urls)))
else:
    assert mode == 'compare' and urls == verify.load(output / 'control-urls.json')
    repeats = [verify.collect([output / f'control-{index}.json'], set(urls), False) for index in (1,2)]
    changed_rows = {url:[primary[url], *(rows[url] for rows in repeats)] for url in urls if not all(rows[url] == primary[url] for rows in repeats)}
    (output / 'control-differences.json.gz').write_bytes(gzip.compress(json.dumps(changed_rows, separators=(',',':'), sort_keys=True).encode(), mtime=0))
    print('Compared candidate primary to two controls:',len(urls),'pages; divergent rows:',sorted(changed_rows))
