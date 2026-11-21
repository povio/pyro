import importlib.util
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('batch1_compare', ROOT / 'batch1/compare.py')
baseline = importlib.util.module_from_spec(spec)
spec.loader.exec_module(baseline)


def compare(raw_dir, batch, cohort):
    target = ROOT / batch
    baseline.compare(raw_dir, target, cohort)
    outcomes = baseline.read(target / f'outcomes-{cohort}.json.gz')
    candidate = baseline.sets(outcomes)
    summary_path = target / f'summary-{cohort}.json'
    summary = baseline.read(summary_path)
    for label in ['batch1'] + (['batch3'] if batch == 'batch4' else []):
        other = baseline.read(ROOT / label / f'outcomes-{cohort}.json.gz')
        assert other.keys() == outcomes.keys()
        reference = baseline.sets(other)
        directions = {}
        for key in candidate.keys() | reference.keys():
            left, right = candidate.get(key, set()), reference.get(key, set())
            directions[key] = {'candidateOnly': sorted(left - right), 'referenceOnly': sorted(right - left)}
        baseline.save(target / f'difference-{cohort}-vs-{label}.json.gz', directions)
        summary['vs-' + label] = {key: {side: len(items) for side, items in value.items()} for key, value in directions.items()}
    baseline.save(summary_path, summary)


if __name__ == '__main__':
    raw, batch, cohort = sys.argv[1:]
    compare(Path(raw), batch, cohort)
