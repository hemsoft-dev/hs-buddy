import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { describe, expect, it } from 'vitest'

interface StrykerConfig {
  coverageAnalysis: string
  ignoreStatic: boolean
  thresholds: { break: number }
}

const runnerSource = resolve('node_modules/@stryker-mutator/vitest-runner/dist/src')

// Since 10.1 the runner joins nested test names the way Vitest 5 matches them,
// which the repository used to patch in (#685). These tests keep that true.
describe('Stryker Vitest 5 compatibility', () => {
  it('selects the Vitest 5 test-name separator for the installed Vitest', () => {
    const runner = readFileSync(resolve(runnerSource, 'vitest-test-runner.js'), 'utf8')
    expect(runner).toContain("semver.gte(vitestWrapper.version, '5.0.0-0')")
    const vitest = JSON.parse(readFileSync('node_modules/vitest/package.json', 'utf8')) as {
      version: string
    }
    expect(Number(vitest.version.split('.')[0])).toBeGreaterThanOrEqual(5)
  })

  it('joins nested test names with the Vitest 5 separator', async () => {
    const helperUrl = pathToFileURL(resolve(runnerSource, 'test-helpers.js')).href
    const { collectTestName, VITEST_MODERN_TEST_NAME_SEPARATOR } = (await import(helperUrl)) as {
      collectTestName: (test: unknown, separator: string) => string
      VITEST_MODERN_TEST_NAME_SEPARATOR: string
    }

    const test = {
      name: 'kills the mutant',
      suite: { name: 'inner suite', suite: { name: 'outer suite' } },
    }

    expect(VITEST_MODERN_TEST_NAME_SEPARATOR).toBe(' > ')
    expect(collectTestName(test, VITEST_MODERN_TEST_NAME_SEPARATOR)).toBe(
      'outer suite > inner suite > kills the mutant'
    )
  })

  it('keeps per-test analysis and the existing break threshold enabled', () => {
    const config = JSON.parse(readFileSync('stryker.config.json', 'utf8')) as StrykerConfig

    expect(config.coverageAnalysis).toBe('perTest')
    expect(config.ignoreStatic).toBe(true)
    expect(config.thresholds.break).toBe(96.5)
  })
})
