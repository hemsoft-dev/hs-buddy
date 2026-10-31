import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

const packageJson = JSON.parse(readFileSync('package.json', 'utf8')) as {
  scripts: Record<string, string>
}
// Three CLI launches share the test budget; each child also has a real deadline.
const LINT_PROCESS_TIMEOUT_MS = 10_000

const markdownConfig = readFileSync('.markdownlint-cli2.jsonc', 'utf8')
const workflow = readFileSync('.github/workflows/ci.yml', 'utf8').replaceAll('\r\n', '\n')

function jobBlock(name: string): string {
  const block = workflow.split(`  ${name}:`)[1]?.split(/\n {2}[\w-]+:/, 1)[0]
  if (!block) throw new Error(`Missing workflow job ${name}`)
  return block
}

describe('Markdown CI contract', () => {
  it('runs the repository Markdown command in the existing lint job', () => {
    expect(packageJson.scripts['lint:md']).toBe(
      'markdownlint --dot --config .markdownlint-cli2.jsonc --configPointer /config "**/*.md"'
    )
    expect(jobBlock('lint')).toContain('- name: Lint Markdown\n        run: bun run lint:md')
  })

  it('keeps workflow Markdown covered by the repository-wide glob', () => {
    expect(packageJson.scripts['lint:md']).toContain('**/*.md')
    expect(packageJson.scripts['lint:md']).toContain('--dot')
    expect(markdownConfig).not.toContain('.github/workflows/**')
    const ignores = readFileSync('.markdownlintignore', 'utf8')
    expect(ignores).not.toContain('.github/workflows/**')
    expect(ignores.trim().split(/\r?\n/)).toEqual([
      '**/node_modules/**',
      'dist/**',
      'dist-electron/**',
      '.github/agents/**',
      'test-results/**',
      'release/**',
      '.github/aw/logs/**',
    ])
  })

  it(
    'rejects malformed hidden workflows, ignores excluded files, and accepts repaired Markdown',
    () => {
      const directory = mkdtempSync(join(tmpdir(), 'buddy-markdown-'))
      try {
        copyFileSync('.markdownlintignore', join(directory, '.markdownlintignore'))
        mkdirSync(join(directory, '.github', 'workflows'), { recursive: true })
        mkdirSync(join(directory, '.github', 'agents'), { recursive: true })
        const fixture = join(directory, '.github', 'workflows', 'fixture.md')
        const broken = '# Heading\n\n### Skipped level\n'
        writeFileSync(fixture, broken)
        writeFileSync(join(directory, '.github', 'agents', 'ignored.md'), broken)
        const lint = () =>
          spawnSync(
            process.execPath,
            [
              fileURLToPath(import.meta.resolve('markdownlint-cli')),
              '--dot',
              '--config',
              resolve('.markdownlint-cli2.jsonc'),
              '--configPointer',
              '/config',
              '**/*.md',
            ],
            { cwd: directory, encoding: 'utf8', timeout: LINT_PROCESS_TIMEOUT_MS }
          )
        const failure = lint()
        expect(failure.error).toBeUndefined()
        expect(failure.status).toBe(1)
        expect(failure.stderr).toContain('MD001')
        expect(failure.stderr).toContain('fixture.md')
        expect(failure.stderr).not.toContain('ignored.md')
        writeFileSync(fixture, '# Heading\n\n## Next level\n')
        const success = lint()
        expect(success.status, success.stderr).toBe(0)
        // Published main already disables MD041 in .markdownlint.jsonc.
        // The replacement must preserve that effective policy, not add an H1 requirement.
        writeFileSync(fixture, '## Legacy heading\n\n### Next level\n')
        const legacy = lint()
        expect(legacy.status, legacy.stderr).toBe(0)
      } finally {
        rmSync(directory, { recursive: true, force: true })
      }
    },
    3 * LINT_PROCESS_TIMEOUT_MS + 5_000
  )

  it('propagates a Markdown failure through both aggregate gates', () => {
    expect(jobBlock('ci-feedback')).toMatch(/needs:[\s\S]*\blint\b/)
    expect(jobBlock('ci-complete')).toContain('ci-feedback')
  })
})
