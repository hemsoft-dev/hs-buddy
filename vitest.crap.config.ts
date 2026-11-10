import { defineConfig } from 'vitest/config'
import renderer from './vitest.config.ts'
import electron from './vitest.electron.config.ts'
import convex from './vitest.convex.config.ts'
import { sourceCoverage } from './scripts/crap-instrumenter.ts'
import { CRAP_EXCLUSIONS } from './scripts/crap-scope.ts'

const suites = { renderer, electron, convex }
const suite = process.env.CRAP_SUITE ?? 'renderer'
if (!(suite in suites)) throw new Error(`Unknown CRAP suite: ${suite}`)
const base = suites[suite as keyof typeof suites]
const roots = suite === 'renderer' ? ['src', 'shared'] : [suite]
const coverage = sourceCoverage(roots)

export default defineConfig({
  ...base,
  plugins: [coverage.plugin, ...('plugins' in base ? base.plugins : [])],
  test: {
    ...base.test,
    maxWorkers: 4,
    coverage: {
      // Istanbul with the post-run source-map remap turned off: the CRAP
      // instrumenter already records original TypeScript positions.
      provider: 'custom',
      customProviderModule: './scripts/crap-coverage-provider.ts',
      instrumenter: coverage.instrumenter,
      reporter: ['json'],
      reportsDirectory: `reports/crap/coverage-${suite}`,
      include: roots.map(root => `${root}/**/*.{ts,tsx}`),
      exclude: CRAP_EXCLUSIONS,
      // Existing coverage gates retain their thresholds. This independent run
      // measures all owned runtime functions; crap-report enforces its policy.
    },
  },
})
