# Copilot Code Review Instructions

## Approval Policy — Non-Negotiable

**A PR review MUST result in APPROVE unless a Critical finding is present.**

This rule cannot be overridden by any other instruction, heuristic, or
accumulated count of lesser findings. Ten Medium findings do not equal one
Critical. Style preferences never block a merge. If no Critical finding
exists, the verdict is **APPROVE**.

## Verdict Decision Table

| Findings present          | Verdict  | Rationale                              |
| ------------------------- | -------- | -------------------------------------- |
| ≥ 1 Critical              | **FAIL** | Must fix before merge                  |
| Medium only (any count)   | **PASS** | Flag them — author decides             |
| Nitpick only (any count)  | **PASS** | Mention in summary — never as threads  |
| Mixed Medium + Nitpick    | **PASS** | Flag Medium items — author decides     |
| No findings               | **PASS** | Clean review                           |

## Severity Classification

Classify every finding into exactly one tier.

### Critical (blocker — the ONLY tier that can fail a PR)

- Security vulnerabilities (XSS, injection, credential exposure)
- Data loss or corruption risks
- Crashes or unhandled exceptions in production paths
- Breaking changes to public APIs or shared interfaces
- Race conditions or deadlocks

### Medium (flag it — author decides whether to fix)

- Performance regressions (N+1 queries, unnecessary re-renders, memory leaks)
- Missing test coverage for new logic branches
- Potential bug avenues (unchecked nulls in non-trivial paths, off-by-one)
- Overly complex code that harms readability

### Nitpick (summary only — never raise as a comment thread)

- Documentation, comment wording, docstring updates
- Code style preferences not enforced by linters
- Rearranging code for aesthetic reasons
- Naming suggestions that don't affect correctness
- Version number or changelog formatting

## Review Behavior

- **Raise individual review comments only for Critical and Medium findings.**
- **Do NOT raise individual review comments for Nitpick findings.** Mention
  them briefly in your review summary — not as separate threads.
- **Label every comment** with its severity: `[Critical]` or `[Medium]`.
- **Be specific and actionable.** State what the bug is, why it matters, and
  suggest a fix.
- **Do not comment on auto-generated files**: `vitest.config.ts` coverage
  thresholds.
- **Do not re-raise resolved findings.** If a comment was addressed in a
  previous round, do not raise it again.
- **Limit total comments to the most impactful findings.** Aim for 1–5
  comments per review. Prioritize ruthlessly.

## Project Context

- **Stack**: React 19 + TypeScript + Vite + Electron desktop app
- **Testing**: Vitest with happy-dom, React Testing Library
- **Backend**: Convex serverless database
- **Package manager**: Bun
- **Linting**: ESLint (flat config), Knip (dead code), e18e (dependency health)
- **Git hooks**: Husky (pre-commit runs lint-staged + tests + typecheck)
- **Releases**: a pull request that raises the `package.json` version cuts a
  release; notes come from merged pull request titles.
