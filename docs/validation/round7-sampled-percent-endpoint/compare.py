import gzip
import json
import subprocess
from collections import Counter
from pathlib import Path


def git_bytes(ref, path):
    return subprocess.check_output(['git', 'show', f'{ref}:{path}'])


def rows(pages):
    result = {}
    for url, subtests in pages:
        counts = Counter()
        for name, status in subtests:
            counts[name] += 1
            result[url, name, counts[name]] = status
    return result


def parsed(run):
    return rows((page['url'], [(row['name'], row['status']) for row in page['subtests']]) for page in run['results'])


directory = Path(__file__).resolve().parent
candidate = json.loads(gzip.decompress((directory / 'candidate.json.gz').read_bytes()))
after = parsed(candidate)
assert len(after) == 3303 and len(candidate['results']) == 13
assert all(page['status'] == 'complete' and page['harnessStatus'] in (0, 'OK') for page in candidate['results'])
reference = json.loads(gzip.decompress(git_bytes('3fcadfa64', 'docs/validation/studio-css-e4ca26/outcomes-stable.json.gz')))
signed = rows((url, reference[url][2]) for url in candidate['plan'])
assert signed.keys() == after.keys()
report = {'statuses': dict(Counter(after.values())), 'signedStatuses': dict(Counter(f'{status}/{signed[key]}' for key, status in after.items())), 'arms': {}}
for arm, commit, folder, filename in [
    ('original', '93a1fca2e', 'sampled', 'baseline'), ('45cbd3f26', '93a1fca2e', 'sampled', 'candidate'),
    ('0b6f8dde5', 'd8922f397', 'intrinsic-height', 'candidate'), ('f2b0c8d9a', '3c95db732', 'fit-height', 'candidate'),
    ('ef0b513e6', 'c78bec5cb', 'stretch', 'candidate'), ('33a4562e6', 'c699335eb', 'intrinsic-width', 'candidate'),
    ('67b5a2237', 'bbf1c89dc', 'fixed-endpoint', 'candidate'), ('c2b2b954f', '2e72d7f9a', 'keyword-endpoint', 'candidate'),
    ('3fcadfa64', 'e42626664', 'sampled-discrete-keyword', 'candidate'),
    ('c236cca7e', '48b704a99', 'discrete-calc-endpoint', 'candidate'),
]:
    prefix = 'round7-' if arm in ('3fcadfa64', 'c236cca7e') else 'round7-calc-size-'
    run = json.loads(gzip.decompress(git_bytes(commit, f'docs/validation/{prefix}{folder}/{filename}.json.gz')))
    before = parsed(run)
    assert before.keys() == after.keys() and set(run['plan']) == set(candidate['plan'])
    changes = [dict(url=key[0], name=key[1], occurrence=key[2], before=before[key], after=after[key], signed=signed[key]) for key in sorted(after) if before[key] != after[key]]
    report['arms'][arm] = dict(gains=sum(row['after'] == 'Pass' for row in changes), losses=sum(row['before'] == 'Pass' for row in changes), changes=changes)
print(json.dumps(report, indent=1))
