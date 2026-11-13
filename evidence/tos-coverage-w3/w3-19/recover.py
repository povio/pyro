#!/usr/bin/env python3
"""Verify retained W3 selections/reports without rerunning TOS or copying secrets.
Usage: python3 recover.py /private/tmp /path/to/retained/manifest.json
Writes the summary to stdout. Input report hashes identify retained originals.
"""
import hashlib
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
MANIFEST_SHA = '542d479b709249d072e3b57c1324c894c4378fc5373892ae64c0687486f2ae19'
TOS = 'f9fde4aa0dce7d1d7a1ffebffe3306399973cd80'
RUNS = {
    'pyro-w3-l19': (0, 32),
    'pyro-w3-l19u': (29, 3),
    'pyro-w3-l19h': (30, 2),
    'pyro-w3-l19hr': (30, 2),
    'pyro-w3-l19m': (30, 2),
}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def key(case):
    return case['file'], tuple(case['titlePath'])


def specs(node):
    yield from node.get('specs', [])
    for child in node.get('suites', []):
        yield from specs(child)


def main():
    root, manifest_path = map(Path, sys.argv[1:])
    assert digest(manifest_path) == MANIFEST_SHA
    manifest = json.loads(manifest_path.read_text())
    common = {key(case) for case in manifest['common']}
    selected = []
    selection_hashes = {}
    for number in range(1, 20):
        path = HERE.parent / 'selections' / f'w3-{number:02}.selection.json'
        selection_hashes[path.name] = digest(path)
        selection = json.loads(path.read_text())
        assert selection['manifest_sha256'] == MANIFEST_SHA
        selected.extend(key(case) for case in selection['cases'])
    assert len(selected) == len(set(selected)) == 275
    assert set(selected) <= common
    files = {file for file, _ in selected}
    assert len(files) == 39
    selection = json.loads((HERE.parent / 'selections/w3-19.selection.json').read_text())
    expected = {case['titlePath'][-1]: case for case in selection['cases']}
    assert len(expected) == 32
    results = []
    for run, counts in RUNS.items():
        folder = root / run
        report_path = folder / 'lightpanda.json'
        meta_path = folder / 'metadata.json'
        cleanup_path = folder / 'db-post-cleanup.txt'
        report = json.loads(report_path.read_text())
        meta = json.loads(meta_path.read_text())
        assert meta['tos_commit'] == TOS and meta['expected_selected_cases'] == 32
        assert not meta.get('diagnostic'), 'Do not mix diagnostic proxy runs into coverage'
        observed = []
        failed = []
        for spec in specs(report):
            assert spec['title'] in expected
            assert len(spec['tests']) == 1
            test = spec['tests'][0]
            assert len(test['results']) == 1, 'Retries need separate treatment'
            assert test['status'] in ('expected', 'unexpected')
            observed.append(spec['title'])
            if test['status'] == 'unexpected':
                failed.append(expected[spec['title']])
        assert len(observed) == len(set(observed)) == 32
        stats = report['stats']
        assert (stats['expected'], stats['unexpected']) == counts
        assert stats['skipped'] == stats['flaky'] == 0
        assert len(failed) == counts[1]
        cleanup = cleanup_path.read_text().splitlines()
        assert cleanup in ([], ['fe_e2e_w3_f9fde4aa_lfix1_template|pyro_w3_e2e|f|t'])
        results.append({
            'artifact_directory': run,
            **{field: meta[field] for field in ('run_slug', 'browser_commit', 'browser_sha256')},
            'passed': counts[0], 'failed': counts[1], 'skipped': 0, 'flaky': 0,
            'failed_cases': failed,
            'run_databases_remaining': 0,
            'retained_template_present': bool(cleanup),
            'source_sha256': {p.name: digest(p) for p in (report_path, meta_path, cleanup_path)},
        })
    assert results[2]['failed_cases'] == results[3]['failed_cases'] == results[4]['failed_cases']
    print(json.dumps({
        'tos_commit': TOS, 'manifest_sha256': MANIFEST_SHA,
        'scope': 'Recovered historical results; not a current-main run or aggregate pass rate',
        'w3_selected_distinct_cases': 275, 'w3_selected_files': 39,
        'selection_sha256': selection_hashes, 'runs': results,
    }, indent=2))


if __name__ == '__main__':
    main()
