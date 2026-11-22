# Local even-shard pass-set supplement

The named files are exact directional differences between cumulative LP c3372ad21 and signed Chrome stable 158.0.8127.37 on the 15,072 URLs measured locally. A row is [URL, canonical name, one-based occurrence, current LP, Chrome, previous LP]. Each observed cell preserves [status, original reported name]. Null denotes a missing name.

The strict page files compare cumulative LP against prior LP 1f1be0324. Every record has the three engines' page outcome, harness status, strict flag, passing and observed name counts, page error, harness message and each nonpassing original subtest with its diagnostic. Each engine's source receipt key and input SHA-256 are included. Names that passed previously but are now unobserved, and newly observable passing names, are separately listed with canonical name and occurrence. The nonpassing diagnostics retain their unmodified source names; repeated original names are disambiguated by occurrence. A strict page completes with an OK harness and a nonempty set of all passing names.

Reproduction uses only existing receipts.

    python3 extract.py --matched "$W/final-local" --current-raw "$W/raw" \
      --previous-raw "$PRIOR/raw" --chrome-raw "$SIGNED/raw" --out "$W/local-supplement"

The script verifies the matched input digests and the original raw receipt digest for every changed page. The frozen local denominators exclude the Studio assignment, 179 additional preflight omissions and 1,469 prior holds.
