#!/bin/bash
# usage: run1.sh OUTDIR FIXTURE BINARY SHA LABEL SLUG   (selection.json present) ; runs, cleans, summarises
set -uo pipefail
O=$1 FX=$2 B=$3 SHA=$4 LABEL=$5 SLUG=$6
[ "$(git -C $FX rev-parse HEAD)" = f9fde4aa0dce7d1d7a1ffebffe3306399973cd80 ] || { echo "FIXTURE HEAD"; exit 1; }
git -C $FX status --porcelain > $O/tos-pre-status.txt
[ "$(cat $O/tos-pre-status.txt)" = " M apps/pdfgen/src/data/common/common.models.ts" ] || [ ! -s $O/tos-pre-status.txt ] || { echo "FIXTURE DIRTY:"; cat $O/tos-pre-status.txt; exit 1; }
df -k /private/tmp | awk 'NR==2{ if ($4<2411724) {print "LOW DISK"; exit 1} }' || exit 1
# External diagnostic run.py generated and inspected separately; original fixture unchanged.
r=$(python3 $O/run.py | tail -1)
TPL=fe_e2e_w2tpl_$(git -C $FX rev-parse HEAD | cut -c1-8)_$(basename $FX | sed 's/^lp-tos-f9f-//' | tr -cd 'a-z0-9' | cut -c1-16)_template
miss=$(sed 's/\x1b\[[0-9;]*m//g' $O/coverage.log | grep -Eo 'template database cache miss \([0-9a-f]+\)' | grep -Eo '[0-9a-f]{12}' | head -1)
owner=$(PGPASSWORD="$(cat ~/lp-bench/pg_lp_bench.pass)" /opt/homebrew/bin/psql -h 127.0.0.1 -U lp_bench -d postgres -XAtc "select pg_get_userbyid(datdba) from pg_database where datname='$TPL'")
if [ -n "$miss" ] && [ "$owner" = lp_bench ] && grep -q "E2E_TEMPLATE_DATABASE='$TPL'" $O/run.py; then python3 /private/tmp/lp-w2cov/ledger.py register $TPL $FX $(git -C $FX rev-parse HEAD) $miss $SLUG > $O/ledger.txt; fi
python3 $O/cleanup.py > $O/cleanup.out 2>&1; cl=$?
LEDGER_SQL=$(python3 /private/tmp/lp-w2cov/ledger.py names | sed "s/.*/'&'/" | paste -sd, -)
left=$(PGPASSWORD="$(cat ~/lp-bench/pg_lp_bench.pass)" /opt/homebrew/bin/psql -h 127.0.0.1 -U lp_bench -d postgres -XAtc "select count(*) from pg_database where pg_get_userbyid(datdba)='lp_bench' and datname not in ($LEDGER_SQL)")
tpl=$(grep -Eo 'reusing template database|template database cache miss' $O/coverage.log | head -1)
echo "$(basename $O) $r cleanup=$cl run-DBs-left=$left template=[$tpl, retained] | $(grep -Eo '^ *[0-9]+ (passed|failed|flaky|skipped|did not run)' $O/coverage.log | tr -s ' ' | tr '\n' ' ') expected=$(python3 -c "import json;print(len(json.load(open('$O/selection.json'))['cases']))") $(grep -Eo 'Running [0-9]+ tests?' $O/coverage.log | head -1) secrets=$(cat $O/coverage.log $O/lightpanda.json 2>/dev/null | grep -c 'eyJ\|Bearer ')"
grep -E '^\s+✘|^\s+[0-9]+\) ' $O/coverage.log | grep -v '✓' | head -20 || true
