# Copilot Code Review Instructions

## Severity Classification

Classify every finding into exactly one tier. Only raise review comments for Critical and Medium findings.

### Critical (blocker — must fix before merge)

- Security vulnerabilities (XSS, injection, credential exposure)
- Data loss or corruption risks
- Crashes, unhandled exceptions in production paths
- Breaking changes to public APIs or shared interfaces
- Race conditions or deadlocks

### Medium (should fix — PR author decides)

- Performance regressions (N+1 queries, unnecessary re-renders, memory leaks)
- Test failures or missing test coverage for new logic branches
- Potential bug avenues (unchecked nulls in non-trivial paths, off-by-one errors)
- Overly complex code that harms readability (cyclomatic complexity, deep nesting)

### Nitpick (suggestion only — never block)

- Documentation improvements, comment wording, docstring updates
- Code style preferences not enforced by linters
- Rearranging code for aesthetic reasons
- Naming suggestions that don't affect correctness
- Version number or changelog formatting

## Review Behavior

- **Raise individual review comments only for Critical and Medium findings.**
- **Do NOT raise individual review comments for Nitpick findings.** If you have nitpicks, mention them briefly in your review summary — not as separate threads.
- **Label every comment** with its severity: `[Critical]`, `[Medium]`, or `[Nitpick]`.
- **Be specific and actionable.** State what the bug is, why it matters, and suggest a fix.
- **Do not comment on files you didn't review** (e.g., auto-generated coverage config).
- **Do not re-raise resolved findings.** If a comment was addressed in a previous round, do not raise it again — even if the implementation differs from your suggestion.
- **Limit total comments to the most impactful findings.** Aim for 1–5 comments per review, not 10+. Prioritize ruthlessly.

## Project Context

- **Stack**: React 19 + TypeScript + Vite + Electron desktop app
- **Testing**: Vitest with happy-dom, React Testing Library
- **Backend**: Convex serverless database
- **Package manager**: Bun
- **Linting**: ESLint (flat config), Knip (dead code), e18e (dependency health)
- **Git hooks**: Husky (pre-commit runs lint-staged + tests + typecheck)
- **Auto-generated files** (ignore these in review): `vitest.config.ts` coverage thresholds
- **Releases**: a pull request that raises the `package.json` version cuts a release; notes come from merged pull request titles, and `CHANGELOG.md` is frozen at 0.1.1698
