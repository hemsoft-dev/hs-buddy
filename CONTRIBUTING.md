# Contributing to Buddy

Thanks for your interest in contributing! This guide covers the setup, conventions, and expectations for PRs.

## Prerequisites

- [Bun](https://bun.sh/) (package manager & script runner)
- [Node.js](https://nodejs.org/) 22.12+ workspace-wide, required by the ESM Electron downloader
- [.NET Aspire CLI](https://aspire.dev/get-started/install-cli/) (AppHost orchestration)
- [Convex CLI](https://docs.convex.dev/getting-started) (`npm i -g convex`)
- Bash for the release-workflow tests: on Windows install [Git for Windows](https://gitforwindows.org/)
  with Git Bash; on Linux/macOS make Bash available on `PATH`. The tests verify
  shell quoting and temporary-file operations, then automatically use a compatible
  Git Bash installation if the Windows WSL launcher comes first on `PATH`.
  Missing or incompatible shells fail with an installation hint instead of skipping
  release behavior checks. No manual `PATH` reordering is required.

The Windows renderer unit suite uses at most four workers to avoid starving native
Git/Bash subprocesses on shared development machines. Other platforms keep the
Vitest default. All tests, isolation, deadlines, and coverage floors remain active.

## Getting Started

```bash
# Clone and install
git clone https://github.com/hemsoft-dev/hs-buddy.git
cd hs-buddy
bun run setup

# Start Convex dev server (separate terminal)
bun run convex:dev

# Start Electron dev mode
bun run dev
```

`bun run setup` installs the frozen root dependencies, installs the isolated
`aspire-apphost/` dependencies, and regenerates its ignored `.aspire/modules/`
SDK. The AppHost package is separate from the application dependency graph, and
warm `bun run typecheck` calls only perform a fast bootstrap preflight instead
of rerunning Aspire restore.

### Environment Variables

Create a `.env.local` file in the project root with:

```env
VITE_CONVEX_URL=<your-convex-deployment-url>
```

## Development Scripts

| Command                          | Purpose                                                  |
| -------------------------------- | -------------------------------------------------------- |
| `bun run setup`                  | Install root dependencies and restore the Aspire AppHost |
| `bun run dev`                    | Start Electron in dev mode                               |
| `bun run test`                   | Run all unit tests                                       |
| `bun run test:watch`             | Run tests in watch mode                                  |
| `bun run test:coverage`          | Run renderer coverage (99/99/100/100 minimum)            |
| `bun run test:mutation`          | Run the blocking Stryker mutation-quality gate           |
| `bun run test:electron:coverage` | Run Electron coverage (97/89/97/98 minimum)              |
| `bun run test:convex:coverage`   | Run Convex coverage (90/90/90/90 minimum)                |
| `bun run test:e2e`               | Run Playwright E2E tests                                 |
| `bun run lint`                   | ESLint (zero warnings allowed)                           |
| `bun run lint:md`                | Markdownlint across tracked Markdown sources             |
| `bun run typecheck`              | TypeScript across all tsconfigs                          |
| `bun run knip`                   | Dead code & unused dependency detection                  |
| `bun run format:check`           | Prettier format verification                             |

## PR Conventions

### Required Merge Checks

The active [default-branch ruleset](https://github.com/hemsoft-dev/hs-buddy/rules/15947577)
requires these GitHub Actions checks before a pull request can merge into `main`:

| Check context | Workflow            | Purpose                                                                         |
| ------------- | ------------------- | ------------------------------------------------------------------------------- |
| `ci-complete` | `CI`                | Aggregates lint, type, unit, Electron, IPC, Convex, E2E, memory, and build jobs |
| `npm audit`   | `Security Scanning` | Rejects high-severity dependency vulnerabilities                                |

Both checks must come from the GitHub Actions app (`integration_id: 15368`). The
ruleset also requires every change to the existing `main` branch to arrive
through a pull request. Its pull-request rule requires all review conversations
to be resolved and uses zero required approving reviews. Zero is intentional:
GitHub does not count an approval from the pull-request author, and this
repository currently uses the same `HemSoft` identity for authorship and merge
authorization.

A human authorizes a merge by explicitly selecting the exact pull request after
checking the required statuses and reviewer feedback, then using GitHub's merge
control. Automation acting for a human may merge only after that human names
the exact pull request in the automation's control channel. An issue label,
automated comment, workflow result, or general instruction does not authorize a
merge.

The ruleset has no bypass actors, does not exempt branch creation, does not
require a pull request branch to be updated with the latest `main` commit, and
allows the repository's configured merge, squash, and rebase methods. There is
no standing emergency bypass. If an urgent repair cannot use a pull request, a
repository administrator must record the reason in an issue or incident,
temporarily change the ruleset, make only the required repair, then immediately
restore and verify the ruleset. Any permanent bypass or policy change must be
documented here through a pull request.

The same ruleset requires CodeQL results for GitHub Actions and
JavaScript/TypeScript. A missing or running analysis blocks the update, as does a high or critical
security alert introduced by the proposed change. The CodeQL configuration,
scope, alert ownership, dismissal rules, and verification commands are in
[CodeQL scanning](docs/CODEQL.md).

Repository administrators can inspect the enforced policy with:

```bash
gh api repos/hemsoft-dev/hs-buddy/rulesets/15947577 \
  --jq '{
    enforcement,
    conditions,
    bypass_actors,
    current_user_can_bypass,
    rules: [.rules[] | select(
      .type == "required_status_checks" or
      .type == "pull_request" or
      .type == "code_scanning"
    )]
  }'
```

### PR feedback cycle

Use `ci-feedback` during draft review and `ci-complete` for final merge
qualification. See [CI feedback and memory qualification](docs/CI-FEEDBACK.md)
for draft deferral, selective execution, parallel samples, and scheduled coverage.

### Dependency vulnerability response

Dependabot alerts and security-update pull requests are enabled. `HemSoft` owns
alert triage, and every high or critical alert must become assigned, tracked
work within the response times in
[Dependabot security response](docs/DEPENDABOT-SECURITY.md). The policy also
documents the required remediation checks and a safe tabletop verification.

### Commit Messages

We use [Conventional Commits](https://www.conventionalcommits.org/) enforced by commitlint:

```text
type(scope): subject
```

**Types**: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`

- Subject must be lowercase (no start-case, PascalCase, or UPPER_CASE)
- Header max length: 100 characters

### Branch Naming

Use descriptive branch names prefixed with the type:

```text
feat/terminal-folder-view
fix/coderabbit-edit-detection
docs/contributing-guide
```

### Pre-commit Hooks

Husky runs automatically on commit:

1. **lint-staged** — Prettier + ESLint on staged files
2. **Full renderer test suite with coverage** — must maintain the configured ratchet
3. **Typecheck** — all four tsconfig projects

> **Tip**: Use `--no-verify` only if you've already validated locally.

Commits do not change the package version or `CHANGELOG.md`, so parallel pull
requests do not conflict on them.

### Releases

To publish a release, run `bun run bump` in a pull request. It raises the patch
version in `package.json`. When that pull request merges and CI passes on
`main`, the release workflow tags the commit and publishes notes generated from
the pull request titles merged since the previous release. Pull request titles
are the release notes, so write them as Conventional Commits that describe the
user-visible change.

## Testing Expectations

Coverage is enforced independently for each maintained suite, in statements/branches/functions/lines order:

- **Renderer** (`bun run test:coverage`): 99% / 99% / 100% / 100%
- **Electron** (`bun run test:electron:coverage`): 97% / 89% / 97% / 98%
- **Convex** (`bun run test:convex:coverage`): 90% / 90% / 90% / 90%

Renderer thresholds ratchet toward 100% when measured coverage permits. Use
`/* v8 ignore start */` / `/* v8 ignore next */` only for genuinely untestable
paths (IPC bridges, audio playback, thin API wrappers).

- New features must include tests — no exceptions
- Test files live alongside source: `Component.test.tsx` or in a dedicated test file

### Test Stack

- **Vitest** with happy-dom environment
- **React Testing Library** for component tests
- **Playwright** for E2E tests

### Mutation testing

Run `bun run test:mutation` before changing the mutation scope or threshold.
See [Mutation testing](docs/MUTATION-TESTING.md) for the maintained baseline,
exclusion reasons, report locations, runtime budget, and threshold-update
rules.

## Code Quality

- ESLint enforces `no-explicit-any` as error
- Catch clauses must be typed: `catch (_: unknown)`
- Max cyclomatic complexity: 10 for async arrow functions
- Knip must report zero findings (no dead exports or unused deps)
- This is a frameless Electron app — all menus live in `TitleBar.tsx`, not Electron's native menu

## Architecture Notes

- **Renderer**: React 19 + TypeScript + Vite
- **Main process**: Electron with IPC handlers in `electron/`
- **Backend**: Convex serverless (schemas in `convex/`)
- **GitHub API**: Domain modules under `src/api/github/`
- **Review providers**: Extensible pattern in `src/reviewProviders/`

## Getting Help

Open an issue or reach out to `@relias-engineering/developer-experience`.
