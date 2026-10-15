# Pre-merge benchmark gate

CI calls `.github/workflows/benchmarks.yml` for pull requests, main pushes,
merge groups, scheduled runs, and manual dispatch. `ci-complete` requires that
call to succeed. The standalone benchmark workflow also supports manual dispatch;
main trend runs now come through CI, avoiding a duplicate workflow per push.

## Scope and advisory decisions

Every PR receives a benchmark job. `scripts/bench-policy.ts` classifies its
changes before dependency installation or measurements:

- Runtime changes under `src`, `electron`, `convex`, `shared`, or `perf`, lockfile
  changes, dependency changes, and CI wiring changes run a blocking comparison.
- Documentation and ordinary test-only changes succeed with an explicit skip
  reason. A package version bump does not change dependency or harness policy.
- Benchmark definitions, benchmark scripts/configuration, setup fixtures, and
  toolchain changes run an advisory comparison. The summary and `bench-policy.json`
  list the exact paths or package fields responsible. Changing an ordinary
  production dependency remains blocking.

An advisory comparison records measured regressions without failing the job.
An unavailable baseline dependency installation is also explicitly reported
for advisory changes. Other execution failures and incomplete measurement files
fail qualification, so missing evidence cannot silently pass.
Reported relative margins of error at or above 100% are unusable and fail
qualification. Advisory runs with no matching benchmark identities retain both
measured revisions and explicitly report that comparison is unavailable.
An initial revision without a parent records advisory trend measurements.

## Measurements and decision

Base and candidate run on the same GitHub-hosted Ubuntu runner, with Node 24.12.0
and Bun 1.4.2. Each revision installs its own frozen dependencies. Results cached
from another runner are never used as the gate baseline.

The job collects three interleaved pairs, in this order:

1. Base, candidate.
2. Candidate, base.
3. Base, candidate.

`bench-qualify.ts` requires three compatible, nonempty samples per revision and
builds per-benchmark medians using `bench-median.ts`. The established CI threshold
remains a throughput drop **greater than 20%**. Qualification additionally
requires nonoverlapping reported uncertainty bounds: duration-relative RME is
converted to reciprocal throughput bounds before comparison. Some benchmarks
settle at different speeds in different processes; for example,
`JSON.stringify (cache write) > 1000 entries` has measured about 540 or about
720 ops/s per process on identical code. A three-run median reports whichever
speed two runs hit. A drop that crosses the threshold is therefore treated as
that noise only when all three of these hold:

1. The baseline itself produced the candidate's median speed: the candidate
   median's upper bound reaches the slowest baseline sample's lower bound.
2. Every sample of both revisions stays within a **1.5x** spread. A wider spread
   indicates an outlier, so one extreme sample on either revision can neither
   fail the gate nor hide a slowdown.
3. The geometric means of the three samples differ by no more than the
   threshold. They average speed noise out, while every sample of a real
   slowdown shifts.

These bounds do not account for every source of runner variance; interleaving
and medians reduce order effects and single-sample noise. The local performance skill's five-run,
5% baseline checks remain a separate, stricter investigation procedure.

The job has a **35-minute timeout**, including setup, six benchmark invocations,
comparison, and upload. Benchmark CLI calls use `--run` to disable watch mode.
Timeouts fail the required gate. The Vitest 5 migration retains Tinybench 6's
full 1,000 ms measurement window. Three interleaved pairs require six separate
full benchmark invocations. Representative qualification run
[37280369901](https://github.com/hemsoft-dev/hs-buddy/actions/runs/37280369901)
completed the benchmark job in 29m09s, above the previously documented 25-minute
budget. The 35-minute budget covers this measured cost and setup, comparison,
and upload while preserving all three pairs, the measurement window, regression
threshold, and uncertainty qualification.

Benchmark qualification is separate from fast lint, typecheck, and review
feedback. Migration CI run
[34451422759](https://github.com/hemsoft-dev/hs-buddy/actions/runs/34451422759)
proved that the former 15-minute limit expired during the fifth sample.
Record the hosted job duration when changing
sample count or benchmark definitions; do not silently increase the budget.

## Evidence and reproduction

For completed comparison runs, the `bench-results` artifact retains policy, all
six samples, both median files, and `bench-summary.md` for 30 days. The same
comparison is printed in the job log and Actions step summary, including the
advisory reason when applicable. Skip-mode runs retain the policy file only;
advisory baseline-install failures retain policy and an unavailable summary.

- Run `bun run test -- scripts/bench-policy.test.ts scripts/bench-qualify.test.ts
scripts/benchmarks-workflow.test.ts scripts/ci-memory-workflow.test.ts` to check
  path classification, version-only skips, sample cardinality, thresholds,
  uncertainty handling, and aggregate failure handling.
- Run `bun scripts/bench-json.ts --directory <revision> --output <sample.json>`
  from the candidate checkout to collect either Vitest 4 or Vitest 5 output.
  Name samples `bench-baseline-run-1.json` through `-3.json` and
  `bench-results-run-1.json` through `-3.json` in the comparison directory.
- Save `bench-policy.json` with `mode` (`enforce` or `advisory`) and nonempty
  `reasons`, then run the absolute path to `scripts/bench-qualify.ts` with Bun
  from that directory. A measured regression in enforce mode exits 1.
- For a hosted failure proof, temporarily slow a benchmarked production
  function and verify the benchmark job and `ci-complete` fail. Remove the
  slowdown and require passing CI before merge. If the same PR changes the
  harness, explicitly record any temporary enforce-mode override used for this
  proof and remove it before final review.

## Initial hosted failure proof

[PR #669](https://github.com/hemsoft-dev/hs-buddy/pull/669), implementing
[issue #655](https://github.com/hemsoft-dev/hs-buddy/issues/655), measured the controlled
slowdown in [CI run 34000524452](https://github.com/hemsoft-dev/hs-buddy/actions/runs/34000524452).
Its benchmark job completed in 14m 4s, within the 15-minute budget. All four
projection benchmarks regressed by 99.9%; no other benchmark failed. The
199,119-byte artifact retained all six samples, policy, medians, and summary.
`ci-complete` failed with benchmarks as its only failed dependency. The temporary
slowdown and enforce override were removed after this proof. The pull request
records final no-regression CI evidence before merge.
