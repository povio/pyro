import gzip
import json
import subprocess
from pathlib import Path


def archived(ref, path):
    data = subprocess.check_output(['git','show',ref+':docs/validation/'+path])
    return json.loads(gzip.decompress(data) if path.endswith('.gz') else data)


def names(rows):
    return {(url,name):status for url,(_,_,subs) in rows.items() for name,status in subs}


root = Path(__file__).resolve().parent
prefix = 'studio-css-round7/'
focused = archived('9d54460d','round7-css-transition-reset/directions.json.gz')['batch14']['names']
assert len(focused['gain']) == 5 and not focused['loss']
restored = [row[:2] for row in archived('849ff0a3',prefix+'batch15/status-changes-main.json')['assertions'] if row[2:] == ['Pass','Fail']]
assert len(restored) == 4 and all('calc-size-interpolation-expansion' in url for url,_ in restored)
def flex_names(label):
    rows = archived('cb87ba37',f'round7-flex-width/{label}.json.gz')['results']
    return {(row['url'],sub['name']) for row in rows for sub in row['subtests'] if sub['status']=='Pass'}

widths = flex_names('candidate') - flex_names('base')
assert len(widths) == 24 and not flex_names('base') - flex_names('candidate')
assert widths == flex_names('candidate-repeat') - flex_names('base-repeat')
original = archived('0aba0c48',prefix+'batch16/outcomes-main.json.gz')
repeat_urls = set(archived('0aba0c48',prefix+'batch16/matched-repeats/urls.json'))
assert len(repeat_urls) == 9
for batch in (17,18):
    rows = json.loads(gzip.decompress((root/f'batch{batch}/outcomes-main.json.gz').read_bytes()))
    with gzip.open(root/'batch14/outcomes-main.json.gz') as file:
        base = json.load(file)
    with gzip.open(root/'frozen/outcomes-chrome-main.json.gz') as file:
        chrome = names(json.load(file))
    actual, before = names(rows), names(base)
    for group,expected in [(restored,'Pass'),(focused['gain'],'Fail')]:
        assert all(before[url,name] == expected and actual[url,name] == chrome[url,name] == 'Pass' for url,name in group)
    assert all(rows[url] == base[url] and original[url][:2] != rows[url][:2] for url in repeat_urls)
    assert {url for url in rows if original[url][:2] != rows[url][:2]} == repeat_urls
    losses = [pair for pair in before if before[pair]=='Pass' and actual.get(pair)!='Pass']
    assert not losses
    if batch == 18:
        with gzip.open(root/'batch17/outcomes-main.json.gz') as file:
            previous = names(json.load(file))
        assert previous.keys() == actual.keys() and {pair for pair in actual if actual[pair]!=previous[pair]} == widths
        assert all(previous[pair]=='Fail' and actual[pair]==chrome[pair]=='Pass' for pair in widths)
    mapped = dict(restoredCalcSize=restored,focusedTransitionGains=focused['gain'],originalBatch16Statuses={url:original[url][:2] for url in sorted(repeat_urls)},flexWidthGainsVs17=sorted(widths) if batch==18 else [])
    (root/f'focused-map-{batch}.json').write_text(json.dumps(mapped,indent=2)+'\n')
