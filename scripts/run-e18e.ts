import { spawnSync } from 'node:child_process'
import { readFileSync, rmSync } from 'node:fs'
import { createRequire } from 'node:module'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createE18ePlaceholder } from './e18e-placeholder'
import { parseE18eReport, requireSuccessfulAnalyzer } from './e18e-report'

const root = fileURLToPath(new URL('../', import.meta.url))
const documentedDirectExceptions = new Set([
  '@opentelemetry/resources',
  '@opentelemetry/sdk-metrics',
  '@types/node',
  // Root and import-in-the-middle use 3.x; Vitest 5 still requires ^2.3.2.
  'es-module-lexer',
  'esbuild',
  'globals',
  'prettier',
  // Lighthouse retains 25.10.0; the direct Electron memory sampler uses 25.11.0.
  'puppeteer-core',
  'typescript',
  'vscode-jsonrpc',
])
const ansiPattern = new RegExp(`${String.fromCharCode(27)}\\[[0-9;]*m`, 'g')

function stripAnsi(value: string): string {
  return value.replace(ansiPattern, '')
}

function duplicatePackageName(message: string): string | undefined {
  return /^\[duplicate dependency\] (?<name>\S+) has /u.exec(message)?.groups?.name
}

function formatPackageList(packages: string[]): string {
  return packages.length > 0 ? packages.join(', ') : 'none'
}

function analyze() {
  const require = createRequire(import.meta.url)
  const analyzerRoot = resolve(dirname(require.resolve('@e18e/cli')), '..')
  const { version } = JSON.parse(readFileSync(resolve(analyzerRoot, 'package.json'), 'utf8')) as {
    version: string
  }
  const manifest = JSON.parse(readFileSync(resolve(root, 'package.json'), 'utf8')) as {
    name: string
    version: string
    devDependencies: Record<string, string>
  }
  if (manifest.devDependencies['@e18e/cli'] !== version) {
    throw new Error(`e18e requires the exact manifest pin; installed version is ${version}`)
  }
  console.log(`e18e analyzer version: ${version}`)
  const result = spawnSync(
    'node',
    [
      resolve(analyzerRoot, 'cli.js'),
      'analyze',
      '--log-level',
      'error',
      '--report-level',
      'warn',
      '--json',
    ],
    { cwd: root, encoding: 'utf8', timeout: 300_000, maxBuffer: 32 * 1024 * 1024 }
  )
  if (result.stderr) process.stderr.write(result.stderr)
  if (result.status !== 0 && result.stdout) process.stderr.write(result.stdout)
  requireSuccessfulAnalyzer(result)
  return parseE18eReport(result.stdout, manifest)
}

export function runE18e(): number {
  const electronMain = resolve(root, 'dist-electron/main.js')
  const createdPlaceholder = createE18ePlaceholder(electronMain)
  try {
    const report = analyze()
    const errors = report.messages.filter(message => message.severity === 'error')
    const warnings = report.messages.filter(message => message.severity === 'warning')
    const directDuplicates = warnings
      .map(message => stripAnsi(message.message))
      .filter(message => message.includes('root@'))
      .map(duplicatePackageName)
      .filter((name): name is string => Boolean(name))
    const documentedDirectDuplicates = directDuplicates.filter(name =>
      documentedDirectExceptions.has(name)
    )
    const undocumentedDirectDuplicates = directDuplicates.filter(
      name => !documentedDirectExceptions.has(name)
    )
    console.log(`e18e duplicate dependency count: ${report.duplicateCount}`)
    console.log(`e18e warnings: ${warnings.length}`)
    console.log(
      `documented direct dependency exceptions: ${formatPackageList(documentedDirectDuplicates)}`
    )
    console.log(
      `undocumented direct dependency findings: ${formatPackageList(undocumentedDirectDuplicates)}`
    )
    for (const error of errors) console.error(stripAnsi(error.message))
    return errors.length > 0 || undocumentedDirectDuplicates.length > 0 ? 1 : 0
  } finally {
    if (createdPlaceholder) rmSync(electronMain, { force: true })
  }
}

if (import.meta.main) {
  try {
    process.exitCode = runE18e()
  } catch (error: unknown) {
    console.error(error instanceof Error ? error.message : String(error))
    process.exitCode = 1
  }
}
