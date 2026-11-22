# Reproducing the combined comparison

The even and odd archives in raw-receipts.json each contain a machine directory. Keep their original machine-provenance.json, per-shard raw files, launch records and port proofs, plus the odd machine/allocation.json. The same local binary produced both archives. The original allocation and reassignment remain separately encoded.

Use the original ordered tasks and selectors from the 1f1be0324 census root, its evidence/ directory, the archived prior loss audit and the V2 plan at 850ef1221795720b477d5e481627b3cfe1a5fbe6. The exporter reads gzip-compressed raw receipts directly, verifies the decompressed byte digest and checks every selector against the original manifest before comparing names. These commands are read-only on the archives.

    python3 exporter/reference_index.py "$PRIOR/evidence" "$WORK/reference.sqlite"
    python3 exporter/aggregate.py --plans "$PLANS_V2" --prior "$PRIOR" \
      --reference-db "$WORK/reference.sqlite" --loss-audit "$OLD_AUDIT/losses.json" \
      --run "$EVEN/machine" --run "$ODD/machine" --owner all --require-complete \
      --cache "$WORK/cache" --out "$WORK/reconstructed"

The same command with --owner studio and only the odd receipt directory selects the originally odd ordinals. The actual executor is recorded as local in both cases. No browser replay is needed.
