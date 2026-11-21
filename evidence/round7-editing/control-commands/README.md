# Scripted text-control commands

For input and textarea controls, script-triggered insertText, delete, forwardDelete, cut, insertParagraph, insertLineBreak, undo and redo now use the control edit path without dispatching beforeinput. Keyboard edits keep their cancelable beforeinput event. Cut keeps its independently cancelable clipboard event. Input events and undo bookkeeping still run when the control changes. Rich-host history behavior is unchanged.

Independent QA verified 68 gains and zero losses across 27 testdriver pages. Pyro passes increased from 4,133 to 4,201 among 4,597 named assertions. Both builds completed all pages with harness status 0; 11 pages passed completely on each build. Every gained name passed in signed Chrome 158. Gains by command were 12 each for delete, forwardDelete, insertText, undo and redo, four for cut, and two each for textarea insertParagraph and insertLineBreak.

| WPT text-control page | Parent passes | Candidate passes | Chrome 158 passes |
|---|---:|---:|---:|
| Password | 616 | 636 | 632 |
| Text | 620 | 642 | 642 |
| Textarea | 620 | 646 | 648 |

Both exact Pyro builds also ran the previous 27-page Enter testdriver cohort and the 54-page static editing and DOM Range cohort. All 2,852 and 13,060 outcomes respectively held. These cohorts overlap in coverage and are reported separately.

## Direct behavior

Independent QA recorded 102 direct scenarios, including keyboard versus script, canceled beforeinput and cut, selection movement, actual value changes, and undo and redo. For input and textarea, final values and selection offsets matched signed Chrome in all 64 scenarios, and event sequences matched in 62. The 48 keyboard rows and all 32 rich-host rows retained their complete parent observations. Scripted undo in text controls now emits input without beforeinput; a canceled keyboard undo still leaves the edit in place. Both engines left the password and selection unchanged on attempted password cuts. Pyro dispatched a cut event there; Chrome did not. Single-line insertParagraph and insertLineBreak can also differ in return value despite identical content.

Four exact pyro-webapi integration filters each executed and passed one test: rich_paste_is_sanitized, shortcuts_copy_and_paste_with_clipboard_events, exec_command_formatting_and_structure, and undo_and_redo_restore_text_controls_and_hosts.

## Evidence and reproduction

* The privately built clean parent was `2cefb3273eb8e528ab550dfe89f994a2c9a38c2c`, tree `871ceaddea053d0c6a9653521c19f4bfb8dc68f4`, binary SHA-256 `48217aa33e7b40d9e8a442b14ffbaa7e145fe0bd05a5241990a8444610445f7b`.
* The privately built clean candidate was `bacb54f825cd2795346e9247890ec52116c11181`, tree `2c0feaea25ed09abf92436b5e7a2b1bd57ff14f1`, binary SHA-256 `c96794446f9b11c5673235847531785df9d9b6edea48739c994645b5e851c29d`. Both used the wptiter render profile and pinned JSC SHA-256 `3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029`.
* WPT was pinned to `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. The primary cohort ran on both exact builds and the archived signed Chrome 158.0.8127.37 launcher using wptserve, testdriver, and one worker. The guard cohorts reuse their original signed Chrome results in matching profiles.
* Named rows contain URL, subtest, occurrence, parent status, candidate status and Chrome status. The primary ledger contains 4,633 distinct rows including 36 names reported only by Chrome on input-events-exec-command.html. Both Pyro builds reported the same 4,597 names. Supplemental direct observations are in direct-controls.json.gz.

```sh
gzip -dc testdriver-named.jsonl.gz | jq -sc 'map(.[0]) | unique' > urls.json
bun bench/wpt/run.mjs --wpt "$WPT" --dirs editing,input-events,uievents/textInput \
  --server wptserve --testdriver --jobs 1 --tests urls.json --bin "$LABEL=$BIN" --out "$LABEL.json"
```
