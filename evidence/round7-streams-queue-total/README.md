# Streams queue total after dequeue

Base eb81d184a, source 5ea0e3298, signed census 1549075e9, WPT checkout a421f2fea5eb7f91e0db399e44b0fb0779ee1603.

The census records the same positive rounding residue failure on readable and writable default streams in Window, dedicated worker, shared worker and service worker. Larger byte-stream clusters involve separate queue, BYOB and tee algorithms. Prior byte queue change 07a58b2de operates on queued bytes. This change implements the negative-only clamp from DequeueValue on both default queue paths.

Run `node evidence/round7-streams-queue-total/check.cjs /path/to/wpt` from the repository root. The driver evaluates the production Streams factory with WeakMap slots and runs both upstream WPT files in Node. Baseline and candidate JSONL record the eight named results, six passes increasing to eight. The positive residue gives desiredSize −2.2044604925031312e−17 after the third dequeue. Browser-global runs and independent QA are coordinated separately.
