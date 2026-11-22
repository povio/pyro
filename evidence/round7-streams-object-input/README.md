# ReadableStream object inputs

Signed comparator 1549075e9 records two Chrome-pass/Pyro-fail names across Window and three worker globals. The eight original ledger rows are in signed-selection.jsonl. The source baseline is eb81d184a and candidate is f195aee95. Both runs used unchanged WPT checkout a421f2fea5eb7f91e0db399e44b0fb0779ee1603.

The constructor's optional Web IDL object argument rejects null before converting the strategy. ReadableStream.from's async_sequence conversion requires an object before looking up either iterator method. This rejects primitive strings while permitting iterable objects.

Source-level runs use the production factory and slot setup from the queue lane with upstream testharness.js in its shell environment. From passed 49/50 then 50/50. General passed 37/38 then 38/38. Every baseline pass was preserved. For either revision, run the candidate's check.cjs with Node from the source root, passing the WPT root and either streams/readable-streams/from.any.js or streams/readable-streams/general.any.js. Independent browser runs are separate.
