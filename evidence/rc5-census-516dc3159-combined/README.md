# Shipped RC.5 non-CSS testharness census

Both macOS arm64 machines ran the exact RC.5 release workflow executable at 516dc3159ff4602bd3a11e98607c262c0ffc65c4, SHA-256 9b2454c277a52452150d59b33c64c7d46f0206781c0068f5a6b0dd0631ffd87e. Both hosts reported macOS 27.2.1 build 26B131 and the same dynamically loaded system JavaScriptCore UUID C083204E-71DA-3BC5-AFF4-524B5EAFF741, bundle 22625.2.8.11.9. The measured workflow archive reports fork:false. The separately packaged npm darwin-arm64 browser uses a patched static engine. The Studio archive supplies a per-ordinal completion ledger, original run logs, traces, configurations and machine/executable provenance; separate per-shard launch start records are available only in the even archive. The even partition covered 15,072 URLs locally and the odd partition covered 15,107 URLs on Studio. The 320 disjoint shards account for all 30,179 eligible URLs at WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. The comparator was signed Chrome stable 158.0.8127.37; the historical LP comparator is 1f1be0324. Reference results retain their original execution provenance.

| Engine | Named passes / observations | Strict pages |
| --- | ---: | ---: |
| Shipped LP | 1,691,766 / 1,785,720 | 15,277 |
| Signed Chrome stable | 1,754,948 / 1,802,021 | 24,385 |
| Historical LP | 1,690,433 / 1,789,453 | 15,344 |

Confirmed LP-only passing occurrences total 12,623, and confirmed Chrome-only occurrences total 75,805. Per-attempt port proof is missing for 0 shards and affects 0 occurrence rows. The confirmed ledgers exclude these rows, which are retained with the source shard, original names and statuses in unresolved-mappings.jsonl.gz. Raw-identity asymmetries are reported separately in the complete comparison counters. The official inventory has 31,827 URLs, including 1,469 historical holds and 179 additional held variants. Coverage and per-environment dispositions, page errors, incomplete runs, missing named observations and native exits are preserved in comparison/. Raw-input commits retain individual receipt and launch provenance.

## Reproducing the join

Use the even machine archive at 9a39f6638cee1d721bb028607c9975f13ef8ebed, the 320-shard V2 selector at 850ef1221795720b477d5e481627b3cfe1a5fbe6, and the historical LP/signed Chrome evidence at 01f49f43da24a20ecb20d99c967fc5de54272120. The private Studio release rc5-studio-setup-v1 in xban1x/pyro-private-census-transfer-c3372ad21 supplies the raw archive, the separate role-port archive, their checksums and the transferred runner. Independent SHA-256 verification matched GitHub's asset digests for each payload and its checksum file.

- Odd raw archive rc5-studio-odd-516dc3159.tar.zst SHA-256 fa934498306f8c145f238fd909cb866539e99a619e8afb6858f55d40d42fcd13
- Odd role-port archive rc5-studio-odd-516dc3159-role-ports.tar.zst SHA-256 27d247eade8fcba7558a1bd792a7bc10416cd2731f0ec01cf40014495d482b82

Extract the raw tarball to STUDIO_RAW and the port pack to STUDIO_PORT_ARCHIVE. Set V2 to the frozen tasks.json/selector directory; PRIOR to the historic census root containing tasks.json, selections/ and evidence/; EVEN_MACHINE to the even archive's machine directory; LOSS_AUDIT to the original published losses.json; and RUNNERPACK to rc5-studio-runner.tar.gz. Use a fresh directory for each generated output.

    python3 port_proofs.py "$STUDIO_RAW" "$V2/tasks.json" verified-ports --manifest "$STUDIO_PORT_ARCHIVE/role-port-proof/manifest.json"
    python3 import_studio.py "$STUDIO_RAW" verified-ports "$V2" studio-machine "$RUNNERPACK"
    python3 exporter/reference_index.py "$PRIOR/evidence" reference.sqlite
    python3 exporter/aggregate.py --plans "$V2" --prior "$PRIOR" --reference-db reference.sqlite --loss-audit "$LOSS_AUDIT" --run "$EVEN_MACHINE" --run studio-machine --owner all --require-complete --cache shard-cache --out joined
    python3 directional.py joined rebuilt-asymmetric
    python3 mapping_audit.py joined rebuilt-asymmetric

Each named row retains the original browser-reported string and one-based occurrence identity. The canonicalizer handles only recognized encoded WPT-owned URL authority ports with observed server-role evidence. Held URLs and unobserved names are excluded from pass counts. The public wpt.fyi stable corpus and the npm patched-engine binary are separate comparisons.

## Remaining gap

Of 75,805 Chrome-only passing occurrences, LP reported a non-passing status for 58,002 and reported no matching name for 17,803. LP uniquely passed 12,623 occurrences, a net deficit of 63,182. Strict-page asymmetries were 623 LP-only and 9,731 Chrome-only. LP recorded 27,023 complete pages, 1,667 errors and 1,489 timeouts. Chrome recorded 28,941 complete, 346 errors and 892 timeouts. Both launch partitions recorded zero browser restarts or watchdog kills.

The largest per-directory net named deficits were WebNN (13,093), HTML (10,351) and WebGL (4,248). These are measured directory counts. The saved asymmetric ledgers retain every individual assertion for causal diagnosis. Of the earlier 292 tracked losses, 183 now passed, 87 failed, 18 names were missing and four URL variants remained held.

Independent readback verified every joined LP URL/name/occurrence/status from both raw archives and each frozen Chrome page/named cell. The check also reconstructed the four directional file sequences and verified the original 160 Studio server configurations and logs. The independent verifier and its result are included.
