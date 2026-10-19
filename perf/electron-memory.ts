import { spawnSync } from 'node:child_process'
import { randomUUID } from 'node:crypto'
import { mkdtemp, writeFile } from 'node:fs/promises'
import { withProfileCleanup } from './electron-memory-cleanup'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  evaluateRuns,
  formatBytes,
  PACKAGED_MEMORY_BUDGETS,
  type BudgetFailure,
  type CleanupMetric,
  type MedianScenarioMetrics,
  type ProcessKind,
  type ScenarioMetrics,
} from './electron-memory-model'
import {
  closeRuntime,
  collectScenario,
  launchRuntime,
  readPackageVersion,
  validateLaunch,
  type HarnessMode,
  type LaunchedRuntime,
} from './electron-memory-runtime'
import {
  navigate,
  runBrowserLifecycle,
  runNavigationScenario,
  runTerminalLifecycle,
  waitWithProgress,
} from './electron-memory-scenarios'

const require = createRequire(import.meta.url)
const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const DEFAULT_WARMUP_MS = 5 * 60 * 1_000
const DEFAULT_SETTLE_MS = 5_000
const DEFAULT_NAVIGATION_CYCLES = 3
const DEFAULT_CONVEX_URL = 'https://placeholder-memory-harness.convex.cloud'

interface HarnessOptions {
  mode: HarnessMode
  runs: number
  warmupMs: number
  settleMs: number
  navigationCycles: number
  skipBuild: boolean
  collectOnly: boolean
  budgetScale: number
  outputPath: string | null
  recordBaselinePath: string | null
}

interface HarnessRun {
  run: number
  mode: HarnessMode
  electronVersion: string
  appVersion: string
  profilePath: string
  scenarios: Record<string, ScenarioMetrics>
  observedProcessKinds: ProcessKind[]
}

export interface HarnessResult {
  schema: 1
  capturedAt: string
  platform: string
  architecture: string
  sourceRevision: string | null
  sampleId: string | null
  runnerImage: string | null
  runnerImageVersion: string | null
  options: HarnessOptions
  budgets: {
    absoluteScenario: 'dashboard-warm'
    totalWorkingSetBytes: number
    rendererWorkingSetBytes: number
    cleanupRatio: number
    cleanupMetrics: readonly CleanupMetric[]
  }
  runs: HarnessRun[]
  medians: Record<string, MedianScenarioMetrics>
  failures: BudgetFailure[]
  status: 'pass' | 'fail' | 'informational'
}

const HELP = [
  'Usage: bun perf/electron-memory.ts [options]',
  '',
  'Options:',
  '  --mode <packaged|development>  Runtime to measure (default: packaged)',
  '  --runs <n>                    Fresh isolated profiles to run (default: 1)',
  '  --warmup-ms <n>               Dashboard idle warmup (default: 300000)',
  '  --settle-ms <n>               Cleanup settling delay (default: 5000)',
  '  --navigation-cycles <n>       Representative route/lifecycle cycles (default: 3)',
  '  --skip-build                  Reuse the existing Vite/package output',
  '  --collect-only                Collect a CI sample; aggregate before enforcing budgets',
  '  --budget-scale <n>            Scale absolute caps for harness self-test',
  '  --output <path>               Write the full JSON result',
  '  --record-baseline <path>      Write median baseline JSON',
  '  --help                        Show this help',
].join('\n')

function parsePositiveNumber(raw: string | undefined, flag: string, integer = false): number {
  const value = Number(raw)
  if (!Number.isFinite(value) || value <= 0 || (integer && !Number.isInteger(value))) {
    throw new Error(`${flag} requires a positive ${integer ? 'integer' : 'number'}`)
  }
  return value
}

function parseNonNegativeNumber(raw: string | undefined, flag: string): number {
  const value = Number(raw)
  if (!Number.isFinite(value) || value < 0)
    throw new Error(`${flag} requires a non-negative number`)
  return value
}

function readValue(args: string[], index: number, flag: string): string {
  const value = args[index + 1]
  if (!value || value.startsWith('--')) throw new Error(`${flag} requires a value`)
  return value
}

function parseMode(value: string): HarnessMode {
  if (value === 'packaged' || value === 'development') return value
  throw new Error('--mode must be packaged or development')
}

export function parseArgs(args: string[]): HarnessOptions | null {
  const options: HarnessOptions = {
    mode: 'packaged',
    runs: 1,
    warmupMs: DEFAULT_WARMUP_MS,
    settleMs: DEFAULT_SETTLE_MS,
    navigationCycles: DEFAULT_NAVIGATION_CYCLES,
    skipBuild: false,
    collectOnly: false,
    budgetScale: 1,
    outputPath: null,
    recordBaselinePath: null,
  }

  for (let index = 0; index < args.length; index += 1) {
    const flag = args[index]
    if (flag === '--help') return null
    if (flag === '--skip-build') {
      options.skipBuild = true
      continue
    }
    if (flag === '--collect-only') {
      options.collectOnly = true
      continue
    }
    const value = readValue(args, index, flag)
    index += 1
    applyOption(options, flag, value)
  }
  return options
}

function applyOption(options: HarnessOptions, flag: string, value: string): void {
  const handlers: Record<string, () => void> = {
    '--mode': () => {
      options.mode = parseMode(value)
    },
    '--runs': () => {
      options.runs = parsePositiveNumber(value, flag, true)
    },
    '--warmup-ms': () => {
      options.warmupMs = parseNonNegativeNumber(value, flag)
    },
    '--settle-ms': () => {
      options.settleMs = parseNonNegativeNumber(value, flag)
    },
    '--navigation-cycles': () => {
      options.navigationCycles = parsePositiveNumber(value, flag, true)
    },
    '--budget-scale': () => {
      options.budgetScale = parsePositiveNumber(value, flag)
    },
    '--output': () => {
      options.outputPath = path.resolve(value)
    },
    '--record-baseline': () => {
      options.recordBaselinePath = path.resolve(value)
    },
  }
  const handler = handlers[flag]
  if (!handler) throw new Error(`Unknown option: ${flag}`)
  handler()
}

function runTool(args: string[]): void {
  const result = spawnSync(process.execPath, args, {
    cwd: ROOT,
    env: {
      ...cleanEnvironment(),
      VITE_CONVEX_URL: process.env.VITE_CONVEX_URL ?? DEFAULT_CONVEX_URL,
    },
    stdio: 'inherit',
  })
  if (result.status !== 0) throw new Error(`Command failed: bun ${args.join(' ')}`)
}

async function prepareRuntime(options: HarnessOptions): Promise<string> {
  if (options.mode === 'packaged' && process.platform !== 'win32') {
    throw new Error('The committed absolute packaged budget currently targets Windows x64')
  }
  if (!options.skipBuild) {
    runTool(['x', 'vite', 'build'])
    if (options.mode === 'packaged') {
      runTool(['x', 'electron-builder', '--dir', '--win', '--x64', '-c.npmRebuild=false'])
    }
  }
  if (options.mode === 'development') return require('electron') as string
  const version = await readPackageVersion()
  const executablePath = path.join(ROOT, 'release', version, 'win-unpacked', 'Buddy.exe')
  return executablePath
}

function cleanEnvironment(): Record<string, string> {
  return Object.fromEntries(
    Object.entries(process.env).flatMap(([key, value]) =>
      value === undefined ? [] : [[key, value]]
    )
  )
}

function scaledBudgets(scale: number) {
  return {
    absoluteScenario: PACKAGED_MEMORY_BUDGETS.absoluteScenario,
    totalWorkingSetBytes: PACKAGED_MEMORY_BUDGETS.totalWorkingSetBytes * scale,
    rendererWorkingSetBytes: PACKAGED_MEMORY_BUDGETS.rendererWorkingSetBytes * scale,
    cleanupRatio: PACKAGED_MEMORY_BUDGETS.cleanupRatio,
    cleanupMetrics: PACKAGED_MEMORY_BUDGETS.cleanupMetrics,
  }
}

async function runFreshProfile(
  run: number,
  options: HarnessOptions,
  executablePath: string | null
): Promise<HarnessRun> {
  const profilePath = await mkdtemp(path.join(tmpdir(), `buddy-memory-${randomUUID()}-`))
  let runtime: LaunchedRuntime | null = null
  return withProfileCleanup(
    profilePath,
    async () => {
      if (!executablePath) throw new Error('No Electron executable was prepared')
      runtime = await launchRuntime(options.mode, executablePath, profilePath)
      const page = runtime.page
      await page.waitForSelector('.activity-bar', { visible: true, timeout: 60_000 })
      const state = await validateLaunch(runtime, profilePath)
      console.log(`Run ${run}: Electron ${state.electronVersion}, profile ${profilePath}`)

      const scenarios: Record<string, ScenarioMetrics> = {}
      scenarios['dashboard-cold'] = await collectScenario('dashboard-cold', runtime)
      await waitWithProgress(options.warmupMs, 'dashboard idle warmup')
      scenarios['dashboard-warm'] = await collectScenario('dashboard-warm', runtime)

      await navigate(page, 'settings-accounts')
      await new Promise(resolve => setTimeout(resolve, options.settleMs))
      scenarios['settings-idle'] = await collectScenario('settings-idle', runtime)

      await runNavigationScenario(page, 1)
      await waitWithProgress(options.settleMs, 'navigation post-warmup baseline')
      scenarios['navigation-baseline'] = await collectScenario('navigation-baseline', runtime)

      await runNavigationScenario(page, options.navigationCycles)
      await waitWithProgress(options.settleMs, 'navigation cleanup')
      scenarios['navigation-cleanup'] = await collectScenario('navigation-cleanup', runtime)

      await runTerminalLifecycle(page, runtime, options, scenarios)
      await runBrowserLifecycle(page, runtime, options, scenarios)

      const observedProcessKinds = [
        ...new Set(
          Object.values(scenarios).flatMap(scenario => scenario.processes.map(p => p.kind))
        ),
      ].sort()
      return {
        run,
        mode: options.mode,
        electronVersion: state.electronVersion,
        appVersion: state.appVersion,
        profilePath,
        scenarios,
        observedProcessKinds,
      }
    },
    async () => {
      if (runtime) await closeRuntime(runtime)
    }
  )
}

async function writeJson(filePath: string, value: unknown): Promise<void> {
  await writeFile(filePath, `${JSON.stringify(value, null, 2)}\n`, 'utf8')
  console.log(`Wrote ${filePath}`)
}

async function recordBaseline(
  filePath: string,
  result: HarnessResult,
  firstRun: HarnessRun
): Promise<void> {
  await writeJson(filePath, {
    schema: 1,
    capturedAt: result.capturedAt,
    mode: firstRun.mode,
    platform: result.platform,
    architecture: result.architecture,
    electronVersion: firstRun.electronVersion,
    appVersion: firstRun.appVersion,
    runs: result.options.runs,
    warmupMs: result.options.warmupMs,
    budgets: result.budgets,
    medians: result.medians,
  })
}

function printFailures(failures: BudgetFailure[]): void {
  for (const failure of failures) {
    const isRatio = failure.scenario.endsWith('-cleanup') || failure.scenario.endsWith('-growth')
    const isBytes = !isRatio && failure.metric.toLowerCase().includes('bytes')
    const actual = isRatio
      ? `${(failure.actual * 100).toFixed(1)}% of baseline`
      : isBytes
        ? formatBytes(failure.actual)
        : failure.actual.toFixed(1)
    const limit = isRatio
      ? `${(failure.limit * 100).toFixed(1)}%`
      : isBytes
        ? formatBytes(failure.limit)
        : failure.limit.toFixed(1)
    console.error(`${failure.scenario}: ${failure.metric} ${actual} exceeds ${limit}`)
  }
}

function ciSampleIdentity() {
  return {
    sourceRevision: process.env.GITHUB_SHA ?? null,
    sampleId: process.env.MEMORY_SAMPLE_ID ?? null,
    runnerImage: process.env.ImageOS ?? null,
    runnerImageVersion: process.env.ImageVersion ?? null,
  }
}

async function main(): Promise<void> {
  const options = parseArgs(process.argv.slice(2))
  if (!options) {
    console.log(HELP)
    return
  }
  const executablePath = await prepareRuntime(options)
  const runs: HarnessRun[] = []
  for (let run = 1; run <= options.runs; run += 1) {
    runs.push(await runFreshProfile(run, options, executablePath))
  }
  const evaluation = evaluateRuns(
    options.mode,
    runs.map(run => run.scenarios),
    options.budgetScale
  )
  const failures = evaluation.failures
  const result: HarnessResult = {
    schema: 1,
    capturedAt: new Date().toISOString(),
    platform: process.platform,
    architecture: process.arch,
    ...ciSampleIdentity(),
    options,
    budgets: scaledBudgets(options.budgetScale),
    runs,
    medians: evaluation.medians,
    failures,
    status:
      options.mode === 'development' ? 'informational' : failures.length === 0 ? 'pass' : 'fail',
  }
  if (options.outputPath) await writeJson(options.outputPath, result)
  if (options.recordBaselinePath) await recordBaseline(options.recordBaselinePath, result, runs[0])
  console.log(`Electron memory result: ${result.status}`)
  if (failures.length > 0) {
    printFailures(failures)
    if (!options.collectOnly) process.exitCode = 1
  }
}

if (import.meta.main) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? error.stack : String(error))
    process.exitCode = 1
  })
}
