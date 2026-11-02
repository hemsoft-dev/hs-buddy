# Resolve the yanked executor audit failure

2026-11-02 14:49 EST

The [Ubuntu job](https://github.com/hemsoft-dev/hs-buddy/actions/runs/41507094951/job/125490060056)
failed cargo-deny before package installation and lifecycle qualification because
async-executor 1.14.0 was yanked. The live crates.io registry lists 1.14.1 as
not yanked. Its [upstream changelog](https://github.com/smol-rs/async-executor/blob/master/CHANGELOG.md)
reports a fix for a use-after-free regression during Executor destruction.

Cargo generated the minimal lock update to 1.14.1: only that package's version
and checksum change; all other package records and its dependency edges remain
identical. No dependency requirement, audit policy, ignore, source code, wait
budget or test assertion changed.

The real local cargo-deny 0.20.2 command, `cargo deny --locked check`, fails on
the exact previous lock with the same yanked error and passes advisories, bans,
licenses and sources on the updated lock. Native compilation and actual
installed/Mac diagnostics require fresh hosted execution. This update is not
claimed to repair the unproven Mac input or PTY failures. Electron remains
primary and [issue #823](https://github.com/hemsoft-dev/hs-buddy/issues/823) stays open with its migration limits.
