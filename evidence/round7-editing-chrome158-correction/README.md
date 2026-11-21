# Signed Chrome 158 correction for local round-seven editing

Eleven historical comparison receipts invoked a local wrapper that started system Google Chrome stable **157.0.8103.80**. The requested reference is signed Google Chrome stable **158.0.8127.37**. The corrected run measured the 47 distinct URLs once on that signed build and pinned WPT revision `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`.

All 47 static pages had harness status 0 and the browser completed without a restart or watchdog termination. Chrome 158 passed **9,353 of 9,827 named occurrences and 37 full pages**. Every historical Chrome 157 static page result and named outcome matched its corrected Chrome 158 observation. The original LP gains, losses and Chrome comparison counts on these receipts therefore hold for the corrected reference as measured. The archived 157 receipts retain their original provenance.

| Profile | URLs | Reported names | Named passes | Strict pages |
| --- | ---: | ---: | ---: | ---: |
| Historical system Chrome 157, static, one job | 47 | 9,827 | 9,353 | 37 |
| Signed Chrome 158, static, one job | 47 | 9,827 | 9,353 | 37 |
| Previously published signed Chrome 158 census, wptserve, four jobs | 47 | 9,827 | 9,353 | 37 |

The two Chrome 158 profiles recorded six mutually exclusive passing occurrences apiece, in the whitespace deletion and forward-deletion pages. The structured census includes newline characters in the original assertion labels while the static report has a second occurrence of a literal single-line label. Both original sets and duplicate indices are preserved. The two profiles agree on which URLs fully pass. The signed static launcher uses the established round-six profile; its flags are recorded in signed-launch.json.

The static correction used the same round-seven testharness implementation as the LP runs, at `305840f45`, with a small no-replay patch for browser or transport loss. One worker visited the original URL selection on the static server with fresh page contexts. The patch and audited runner are included. A preliminary run using the older round-six guarded runner omitted per-page harness status in static mode. The final signed158-static receipt supplies both page status and harness status. All 47 URLs are ordinary frozen-eligible manifest entries, and no recorded native-termination or browser-hang URL entered the correction.

## Readback

Each named row has a receipt cohort, URL, original comparison name, one-based duplicate index, and five columns in the order shown in summary.json. Each browser observation includes status and original name. Each page row retains page status, harness status, strict pass, passing names and reported names for all five columns. Repeated URLs in different cohorts support checking the old comparison in its original context. Only 47 distinct URLs count toward the totals above. Comparisons.json includes both LP directions against both signed profiles.

The source receipt hashes are in receipt-mapping.json. The fresh raw result is included in compressed form with only its local executable path abbreviated. The SHA-256 of the original unmodified receipt, runner, archived and system executables, and launchers are in summary.json. Archive signature verification and the WPT manifest identity are recorded in reference-identity.json. The original archived Chrome 158 census publication is `ee24e07f6b15b399d6e8e2157c763809341b6e44`.

Run `shasum -a 256 -c SHA256SUMS` and `python3 verify.py .` from this directory. The acquisition used the following command from the detached frozen checkout with the included guarded runner beside bench/wpt/run.mjs.

```sh
WPT_CHROME_STATE_DIR="$CORRECTION/static-launch-state" bun bench/wpt/run-no-native-static.mjs \
  --kind testharness --wpt "$PINNED_WPT" --dirs editing,selection --jobs 1 \
  --tests "$CORRECTION/urls.json" --bin "signed158=$REFERENCE/census/launch-158" \
  --out "$CORRECTION/signed158-static.json"
```
