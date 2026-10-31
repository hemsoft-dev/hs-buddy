import { spawnSync } from 'node:child_process'
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'

const hook = readFileSync(resolve(process.cwd(), '.husky/pre-commit'), 'utf8')
const temporaryDirectories: string[] = []
const gitLocalEnvironmentVariables = spawnSync('git', ['rev-parse', '--local-env-vars'], {
  encoding: 'utf8',
  timeout: 10_000,
}).stdout.split(/\r?\n/)

function isolatedEnvironment(overrides: NodeJS.ProcessEnv = {}): NodeJS.ProcessEnv {
  const environment = { ...process.env, ...overrides }
  for (const variable of gitLocalEnvironmentVariables) delete environment[variable]
  return environment
}

function shellPath(path: string): string {
  const normalized = path.replaceAll('\\', '/')
  return /^[A-Za-z]:\//.test(normalized)
    ? `/${normalized[0].toLowerCase()}${normalized.slice(2)}`
    : normalized
}

function run(root: string, command: string, args: string[] = []) {
  const result = spawnSync(command, args, {
    cwd: root,
    encoding: 'utf8',
    timeout: 10_000,
    env: isolatedEnvironment(),
  })
  if (result.error) throw result.error
  return result
}

function git(root: string, ...args: string[]): string {
  const result = run(root, 'git', args)
  if (result.status !== 0) throw new Error(result.stderr)
  return result.stdout
}

function executable(path: string, contents: string): void {
  writeFileSync(path, contents)
  chmodSync(path, 0o755)
}

function createFixture() {
  const root = mkdtempSync(join(tmpdir(), 'hs-buddy-pre-commit-'))
  temporaryDirectories.push(root)
  git(root, 'init', '--quiet')
  git(root, 'config', 'user.name', 'Hook Test')
  git(root, 'config', 'user.email', 'hook-test@example.com')

  const hooksDirectory = join(root, '.git', 'hooks')
  const fakeBin = join(root, 'fake-bin')
  mkdirSync(join(root, '.husky'))
  mkdirSync(fakeBin)
  writeFileSync(join(root, '.husky', 'pre-commit'), hook)
  writeFileSync(join(hooksDirectory, 'pre-commit-markdown.ps1'), '')
  writeFileSync(join(root, 'src.ts'), 'const value=1\n')
  writeFileSync(join(root, 'package.json'), '{}\n')
  writeFileSync(join(root, 'vitest.config.ts'), 'export default {}\n')
  git(root, 'add', '.')

  executable(
    join(fakeBin, 'bun'),
    `#!/usr/bin/env sh
printf 'bun %s\\n' "$*" >> "$HOOK_TEST_LOG"
if [ "$*" = "$HOOK_FAIL_COMMAND" ]; then
  # Simulate a crash mid-write: mutate the file before reporting failure so
  # rollback assertions can tell a real restore from a no-op.
  printf '{\\n  "version": "0.0.2-crashed"\\n}\\n' > "$HOOK_PACKAGE_JSON"
  exit 17
fi
`
  )
  executable(
    join(fakeBin, 'npx'),
    `#!/usr/bin/env sh
printf 'npx %s\\n' "$*" >> "$HOOK_TEST_LOG"
printf 'const value = 1\\n' > "$HOOK_FORMAT_FILE"
printf '{\\n  "version": "0.0.1"\\n}\\n' > "$HOOK_PACKAGE_JSON"
git add "$HOOK_FORMAT_FILE" "$HOOK_PACKAGE_JSON"
`
  )
  executable(
    join(fakeBin, 'pwsh'),
    `#!/usr/bin/env sh
printf 'pwsh %s\\n' "$*" >> "$HOOK_TEST_LOG"
`
  )

  return {
    root,
    fakeBin,
    log: join(root, 'hook.log'),
    source: join(root, 'src.ts'),
    packageJson: join(root, 'package.json'),
  }
}

function runHook(fixture: ReturnType<typeof createFixture>, failCommand = '') {
  const result = spawnSync(
    'sh',
    ['-c', 'PATH="$HOOK_TEST_BIN:$PATH"; export PATH; sh .husky/pre-commit'],
    {
      cwd: fixture.root,
      encoding: 'utf8',
      timeout: 10_000,
      env: isolatedEnvironment({
        HOOK_FAIL_COMMAND: failCommand,
        HOOK_FORMAT_FILE: shellPath(fixture.source),
        HOOK_PACKAGE_JSON: shellPath(fixture.packageJson),
        HOOK_TEST_BIN: shellPath(fixture.fakeBin),
        HOOK_TEST_LOG: shellPath(fixture.log),
      }),
    }
  )
  if (result.error) throw result.error
  return result
}

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) {
    rmSync(directory, { recursive: true, force: true })
  }
})

describe('pre-commit hook ordering', () => {
  it('leaves the worktree and index unchanged when downstream validation fails', () => {
    const fixture = createFixture()
    const worktreeBefore = readFileSync(fixture.source, 'utf8')
    const indexBefore = git(fixture.root, 'diff', '--cached', '--binary')

    const result = runHook(fixture, 'run test:coverage')

    expect(result.status).toBe(17)
    expect(readFileSync(fixture.log, 'utf8')).not.toContain('npx lint-staged')
    expect(readFileSync(fixture.source, 'utf8')).toBe(worktreeBefore)
    expect(git(fixture.root, 'diff', '--cached', '--binary')).toBe(indexBefore)
  }, 35_000)

  it('formats and stages files only after every validation passes', () => {
    const fixture = createFixture()

    const result = runHook(fixture)

    expect(result.status, result.stderr).toBe(0)
    expect(readFileSync(fixture.log, 'utf8').split(/\r?\n/)).toEqual([
      expect.stringContaining('pwsh '),
      'bun run test:coverage',
      'bun run typecheck',
      'npx lint-staged',
      'bun run coverage:ratchet',
      'bun scripts/bump-revision.ts',
      '',
    ])
    expect(readFileSync(fixture.source, 'utf8')).toBe('const value = 1\n')
    expect(git(fixture.root, 'show', ':src.ts')).toBe('const value = 1\n')
  }, 35_000)

  it('restores package.json when the revision bump fails after formatting', () => {
    const fixture = createFixture()

    const result = runHook(fixture, 'scripts/bump-revision.ts')

    expect(result.status).toBe(17)
    expect(result.stderr).toContain('package.json restored')

    // Formatting (a completed earlier phase-2 step) stays applied and staged;
    // the documented recovery is `git restore --staged --worktree .`.
    expect(readFileSync(fixture.source, 'utf8')).toBe('const value = 1\n')
    expect(git(fixture.root, 'show', ':src.ts')).toBe('const value = 1\n')

    // The fake bun half-wrote a bumped version before crashing; the restore
    // must roll that mutation back to the pre-bump (formatted) content in
    // both the worktree and the index.
    const formattedPackageJson = '{\n  "version": "0.0.1"\n}\n'
    expect(readFileSync(fixture.packageJson, 'utf8')).toBe(formattedPackageJson)
    expect(git(fixture.root, 'show', ':package.json')).toBe(formattedPackageJson)
  }, 35_000)
})
