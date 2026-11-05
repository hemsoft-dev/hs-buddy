# Native performance diagnostics

An optional trace for the hosted Windows run of
[the matching-fixture qualification](NATIVE-PERFORMANCE.md). It exists because
the Windows view-switch maximum exceeded its 500 ms budget and the retained
samples could not say where the time went. It records no cause by itself and it
is not a fix. The budget verdict of an instrumented run still governs that run.
A pass with the trace in place is an instrumented pass, never a product
correction.

## What it separates

In the same five-pair fixture, the trace splits one response sample into driver
file calls and waiting, the native request read and dispatch, navigation and
settings persistence, and the delay between two frame callbacks.

It records durations and counters only, under the existing request id. It does
not observe physical input or GPU presentation, and it says nothing about Mac
identity. Draw, present and frame delivery all fall inside the frame gap, and
this trace does not separate them. That would take a real profiler.

## Where it runs

Only a native run on Windows is traced. Native admits a trace when both hold:

- The existing `BUDDY_PERF_DIR` probe is active, and its canonical root is a
  `buddy-paired-perf-*` directory directly under the canonical temp directory.
- `BUDDY_CONFIG_PATH` is that root's own `config.json`.

A failed admission makes the trace unavailable and leaves the probe, its receipts
and its statuses working. Electron and other platforms keep their original
results, with no `responseTrace` and no `trace` in any receipt.

## Reading a row

`comparison.json` gains `runs[n].native.responseTrace`, one row per entry of
`responseSamples`, in the same order. No metric or budget reads it, and the
per-run log line leaves it out. A row holds `route`, `cycle`, `driver`, `native`
and `nativeUnavailable`.

`driver` comes from the Python clock:

| Field | Meaning |
| --- | --- |
| `writeMs`, `replaceMs` | Writing `request.tmp`, then renaming it to `request.json` |
| `waitMs` | From that rename until the matching receipt was parsed |
| `pollCount` | Receipt read attempts during the wait |
| `maxReadMs` | Longest single receipt read attempt |
| `maxPollGapMs` | Longest gap between read attempts, including the existing 5 ms sleep |

`native` comes from one `Instant` clock inside the app. Offsets count from the
moment the successful request read returned:

| Field | Meaning |
| --- | --- |
| `pollGapMs` | Previous poll's end to this read's start, including the existing 5 ms timer |
| `readMs` | The successful request read |
| `removeDoneMs` | Offset: the request file was removed |
| `updateEntryMs` | Offset: the window update began |
| `setSectionDoneMs` | Offset: navigation returned |
| `firstFrameMs`, `secondFrameMs` | Offsets: entry to the two existing frame callbacks |
| `firstToSecondFrameMs` | Their difference, one combined bucket |
| `settings.lockMs`, `loadMs`, `saveMs`, `installMs` | Time in each stage of the config update, inside `setSectionDoneMs` |
| `settings.lockCalls`, `loadCalls`, `saveCalls`, `installCalls`, `attempts` | How often each ran, bounded at 32 |
| `previousId`, `previousWriteMs` | The previous receipt's completed write, including its serialization |

`saveMs` holds `sync_all` and the rename as one bucket. Lock includes any wait
for the config lock. A failed save's `resync` is not timed: that request's status
is no longer `ok`, so the run stops anyway.

A receipt cannot report its own write, so the next receipt carries it. The last
receipt's write stays unobserved, and no request is added to obtain it. The first
measured row has request id 3 and previousId 2, because two unmeasured requests
precede the cycles.

The two clocks overlap and are never subtracted from each other. There is no
transport residual. Compare component durations within one side.

`native` is `null` when `nativeUnavailable` holds one of these fixed reasons:

| Reason | Meaning |
| --- | --- |
| `admission` | Native did not admit this root or config |
| `request-id` | The request id was not a positive JSON-safe integer |
| `invalidated` | A stage or scope re-entered, a count overflowed, or a stage unwound |
| `incomplete` | A span never ran, as for a refused route |
| `missing` | The receipt carried no trace |
| `malformed` | Bad JSON, a duplicate key, a nonfinite constant, or an unexpected field |
| `wrong-id` | The trace named a different request |
| `unhealthy` | A value is out of range, offsets run backward, stages exceed their span, or the previous write does not name the prior request |
| `assembly` | Building the row raised an error |

An unavailable trace cannot skip a sample, mask an original error or change a
verdict. Driver timing is null if its clock fails, becomes nonfinite or
decreases, or a duration exceeds the fixed bound.

## Reading a result

Compare hot samples with cool samples from the same run:

- Large settings stages support that bucket's contribution. Cool settings during
  hot samples weaken it.
- A large request or poll bucket with cool navigation and frames weakens settings
  as the dominant bucket.
- A large combined frame bucket narrows the time to render, present or delivery,
  and does not separate them.
- Large driver file, read or wait time with a cool native span implicates the
  driver or transport bucket, without claiming an OS cause.

Several buckets can contribute, and a dominant one does not falsify the others.
If no native sample exceeds 100 ms, the trial is inconclusive and the original
budget verdict stands. That result authorizes neither a repeat until green nor
another logging round.

## Overhead

The trace has a real cost, and part of it falls inside the measured span:

- About a dozen monotonic clock reads and one small allocation per request, plus
  two clock reads per native poll.
- About 390 more receipt bytes per request to serialize and write, about 450 in
  total. A hot disk can make this larger.
- Four more clock reads per driver request, two per receipt read attempt, and one
  retained receipt string.
- One canonicalization of three paths at launch, inside startup.

Row assembly runs after the sample's `ms` is taken. Earlier cool medians are
context only. They are neither a guard for this overhead nor a promise of
stability, since each run gets a different VM.
