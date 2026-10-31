import {
  execFile as execFileCallback,
  spawn,
  type ChildProcessWithoutNullStreams,
} from 'node:child_process'
import { access, readFile } from 'node:fs/promises'
import { createServer } from 'node:net'
import path from 'node:path'
import { promisify } from 'node:util'
import { fileURLToPath } from 'node:url'
import { connect, type Browser, type Page } from 'puppeteer-core'
import {
  aggregateProcessMemory,
  formatBytes,
  identifyNewRendererProcessesAsWebviews,
  type ProcessKind,
  type ProcessMemorySample,
  type RendererMemorySample,
  type ScenarioMetrics,
} from './electron-memory-model'

const execFile = promisify(execFileCallback)
const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')

export type HarnessMode = 'packaged' | 'development'

export interface LaunchedRuntime {
  electronVersion: string
  appVersion: string
  browser: Browser
  child: ChildProcessWithoutNullStreams
  page: Page
  output: string[]
}

interface OsProcessMetric {
  pid: number
  parentPid: number | null
  name: string | null
  workingSetBytes: number
  privateBytes: number | null
  commandLine: string | null
}

function cleanEnvironment(): Record<string, string> {
  return Object.fromEntries(
    Object.entries(process.env).flatMap(([key, value]) =>
      value === undefined ? [] : [[key, value]]
    )
  )
}

export async function readPackageVersion(): Promise<string> {
  const packageJson = JSON.parse(await readFile(path.join(ROOT, 'package.json'), 'utf8')) as {
    version: string
  }
  return packageJson.version
}

async function reservePort(): Promise<number> {
  const server = createServer()
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject)
    server.listen(0, '127.0.0.1', () => resolve())
  })
  const address = server.address()
  await new Promise<void>((resolve, reject) =>
    server.close(error => (error ? reject(error) : resolve()))
  )
  if (!address || typeof address === 'string') throw new Error('Could not reserve a CDP port')
  return address.port
}

function retainOutput(output: string[], chunk: Buffer): void {
  output.push(...chunk.toString('utf8').split(/\r?\n/).filter(Boolean))
  if (output.length > 200) output.splice(0, output.length - 200)
}

async function waitForCdp(
  endpoint: string,
  child: ChildProcessWithoutNullStreams,
  output: string[]
): Promise<{ userAgent: string; webSocketUrl: string }> {
  const deadline = Date.now() + 60_000
  while (Date.now() < deadline) {
    if (child.exitCode !== null) {
      throw new Error(
        `Electron exited before CDP was ready (${child.exitCode})\n${output.join('\n')}`
      )
    }
    try {
      const response = await fetch(`${endpoint}/json/version`)
      if (response.ok) {
        const version = (await response.json()) as {
          'User-Agent'?: string
          webSocketDebuggerUrl?: string
        }
        if (version.webSocketDebuggerUrl) {
          return {
            userAgent: version['User-Agent'] ?? '',
            webSocketUrl: version.webSocketDebuggerUrl,
          }
        }
      }
    } catch (_: unknown) {
      // The endpoint is not listening yet.
    }
    await new Promise(resolve => setTimeout(resolve, 250))
  }
  throw new Error(`Timed out waiting for ${endpoint}\n${output.join('\n')}`)
}

async function waitForRendererTarget(endpoint: string): Promise<void> {
  const deadline = Date.now() + 60_000
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`${endpoint}/json/list`)
      if (response.ok) {
        const targets = (await response.json()) as Array<{ type?: string; url?: string }>
        if (targets.some(target => target.type === 'page' && Boolean(target.url))) {
          await new Promise(resolve => setTimeout(resolve, 5_000))
          return
        }
      }
    } catch (_: unknown) {
      // The renderer target is not published yet.
    }
    await new Promise(resolve => setTimeout(resolve, 100))
  }
  throw new Error(`Timed out waiting for Electron renderer target at ${endpoint}`)
}

async function firstPage(browser: Browser): Promise<Page> {
  const deadline = Date.now() + 60_000
  while (Date.now() < deadline) {
    const pages = await browser.pages()
    if (pages[0]) return pages[0]
    await new Promise(resolve => setTimeout(resolve, 100))
  }
  throw new Error('Electron exposed no renderer page')
}

export async function launchRuntime(
  mode: HarnessMode,
  executablePath: string,
  profilePath: string
): Promise<LaunchedRuntime> {
  const port = await reservePort()
  const profileArg = `--user-data-dir=${profilePath}`
  const args = mode === 'development' ? [ROOT, profileArg] : [profileArg]
  const output: string[] = []
  const child = spawn(executablePath, args, {
    cwd: ROOT,
    env: {
      ...cleanEnvironment(),
      BUDDY_DEBUG_PORT: String(port),
      BUDDY_MEMORY_HARNESS: '1',
      ELECTRON_DISABLE_SECURITY_WARNINGS: 'true',
    },
    stdio: ['pipe', 'pipe', 'pipe'],
    windowsHide: true,
  })
  child.stdout.on('data', chunk => retainOutput(output, chunk as Buffer))
  child.stderr.on('data', chunk => retainOutput(output, chunk as Buffer))
  const endpoint = `http://127.0.0.1:${port}`
  console.log(`Launching Electron pid=${child.pid} CDP=${endpoint}`)
  let browser: Browser | null = null
  try {
    const cdp = await waitForCdp(endpoint, child, output)
    console.log('Electron CDP browser endpoint is ready')
    await waitForRendererTarget(endpoint)
    console.log('Electron renderer target is ready')
    browser = await connect({ browserWSEndpoint: cdp.webSocketUrl })
    console.log('Connected Puppeteer to Electron CDP')
    const page = await firstPage(browser)
    console.log('Connected to Electron renderer page')
    const electronVersion = /Electron\/([^\s]+)/.exec(cdp.userAgent)?.[1] ?? 'unknown'
    return {
      browser,
      child,
      page,
      output,
      electronVersion,
      appVersion: await readPackageVersion(),
    }
  } catch (error: unknown) {
    browser?.disconnect()
    await terminateProcessTree(child)
    throw error
  }
}

function parseWindowsProcess(value: Record<string, unknown>): OsProcessMetric {
  const numberValue = (key: string): number => Number(value[key] ?? 0)
  return {
    pid: numberValue('ProcessId'),
    parentPid: numberValue('ParentProcessId') || null,
    name: typeof value.Name === 'string' ? value.Name : null,
    workingSetBytes: numberValue('WorkingSetSize'),
    privateBytes: numberValue('PrivatePageCount') || null,
    commandLine: typeof value.CommandLine === 'string' ? value.CommandLine : null,
  }
}

async function getWindowsProcessTree(rootPid: number): Promise<OsProcessMetric[]> {
  const script = [
    `$rootPid = ${rootPid}`,
    '$all = @(Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,Name,WorkingSetSize,PrivatePageCount,CommandLine)',
    '$ids = [System.Collections.Generic.HashSet[uint32]]::new()',
    '[void]$ids.Add([uint32]$rootPid)',
    'do {',
    '  $added = $false',
    '  foreach ($process in $all) {',
    '    if ($ids.Contains([uint32]$process.ParentProcessId) -and $ids.Add([uint32]$process.ProcessId)) { $added = $true }',
    '  }',
    '} while ($added)',
    '$all | Where-Object { $ids.Contains([uint32]$_.ProcessId) } | ConvertTo-Json -Compress',
  ].join('; ')
  const shell = process.env.SystemRoot
    ? path.join(process.env.SystemRoot, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe')
    : 'powershell.exe'
  const { stdout } = await execFile(
    shell,
    ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', script],
    {
      maxBuffer: 10 * 1024 * 1024,
    }
  )
  const parsed = JSON.parse(stdout || '[]') as Record<string, unknown> | Record<string, unknown>[]
  return (Array.isArray(parsed) ? parsed : [parsed]).map(parseWindowsProcess)
}

async function getOsProcessTree(rootPid: number): Promise<OsProcessMetric[]> {
  if (process.platform === 'win32') return getWindowsProcessTree(rootPid)
  return []
}

function readProcessType(commandLine: string): string | null {
  return /(?:^|\s)--type=([^\s"]+)/.exec(commandLine)?.[1] ?? null
}

function classifyProcess(metric: OsProcessMetric, rootPid: number): ProcessKind {
  if (metric.pid === rootPid) return 'main'
  const commandLine = metric.commandLine ?? ''
  const type = readProcessType(commandLine)
  if (type === 'renderer') {
    return /--webview-tag|--guest-instance-id/.test(commandLine) ? 'webview' : 'renderer'
  }
  if (type === 'gpu-process') return 'gpu'
  if (type === 'utility') return 'utility'
  return type ? 'other' : 'spawned-child'
}

function buildProcessSamples(
  rootPid: number,
  osProcesses: OsProcessMetric[]
): ProcessMemorySample[] {
  return osProcesses
    .map(process => ({
      pid: process.pid,
      parentPid: process.parentPid,
      kind: classifyProcess(process, rootPid),
      electronType: readProcessType(process.commandLine ?? ''),
      name: process.name,
      serviceName: null,
      workingSetBytes: process.workingSetBytes,
      privateBytes: process.privateBytes,
      source: 'os' as const,
    }))
    .sort((left, right) => left.pid - right.pid)
}

function metricValue(metrics: Array<{ name: string; value: number }>, name: string): number {
  return metrics.find(metric => metric.name === name)?.value ?? 0
}

async function collectRendererMetrics(page: Page): Promise<RendererMemorySample> {
  const session = await page.createCDPSession()
  try {
    await session.send('Performance.enable')
    await session.send('HeapProfiler.collectGarbage')
    const [performance, dom, heap] = await Promise.all([
      session.send('Performance.getMetrics') as Promise<{
        metrics: Array<{ name: string; value: number }>
      }>,
      session.send('Memory.getDOMCounters') as Promise<{
        documents: number
        nodes: number
        jsEventListeners: number
      }>,
      session.send('Runtime.getHeapUsage') as Promise<{
        usedSize: number
        embedderHeapUsedSize?: number
      }>,
    ])
    return {
      v8UsedHeapBytes: metricValue(performance.metrics, 'JSHeapUsedSize') || heap.usedSize,
      embedderHeapBytes:
        metricValue(performance.metrics, 'EmbedderHeapUsedSize') || heap.embedderHeapUsedSize || 0,
      domNodes: dom.nodes || metricValue(performance.metrics, 'Nodes'),
      documents: dom.documents || metricValue(performance.metrics, 'Documents'),
      eventListeners: dom.jsEventListeners || metricValue(performance.metrics, 'JSEventListeners'),
    }
  } finally {
    await session.detach()
  }
}

export async function collectScenario(
  name: string,
  runtime: LaunchedRuntime,
  rendererBaseline?: ScenarioMetrics
): Promise<ScenarioMetrics> {
  const renderer = await collectRendererMetrics(runtime.page)
  const sampledProcesses = buildProcessSamples(
    runtime.child.pid!,
    await getOsProcessTree(runtime.child.pid!)
  )
  const processes = rendererBaseline
    ? identifyNewRendererProcessesAsWebviews(rendererBaseline.processes, sampledProcesses)
    : sampledProcesses
  const scenario = {
    ...aggregateProcessMemory(processes),
    ...renderer,
    processes,
    capturedAt: new Date().toISOString(),
  }
  printScenario(name, scenario)
  return scenario
}

function printScenario(name: string, scenario: ScenarioMetrics): void {
  console.log(
    `${name}: total ${formatBytes(scenario.totalWorkingSetBytes)}, renderer ${formatBytes(scenario.rendererWorkingSetBytes)}, V8 ${formatBytes(scenario.v8UsedHeapBytes)}, embedder ${formatBytes(scenario.embedderHeapBytes)}, DOM ${scenario.domNodes} nodes/${scenario.documents} documents/${scenario.eventListeners} listeners`
  )
  for (const process of scenario.processes) {
    console.log(
      `  ${process.kind.padEnd(13)} pid=${String(process.pid).padEnd(7)} working=${formatBytes(process.workingSetBytes).padStart(10)} private=${process.privateBytes === null ? 'n/a' : formatBytes(process.privateBytes)} ${process.name ?? process.serviceName ?? ''}`
    )
  }
}

export async function validateLaunch(
  runtime: LaunchedRuntime,
  profilePath: string
): Promise<Pick<LaunchedRuntime, 'electronVersion' | 'appVersion'>> {
  // Fixture config is preseeded. Require the main window's Electron-created
  // persistent session directory to prove it initialized this isolated profile.
  const partitionPath = path.join(profilePath, 'Partitions', 'buddy')
  const deadline = Date.now() + 10_000
  while (Date.now() < deadline) {
    try {
      await access(partitionPath)
      break
    } catch (_: unknown) {
      await new Promise(resolve => setTimeout(resolve, 100))
    }
  }
  try {
    await access(partitionPath)
  } catch (error: unknown) {
    throw new Error(`Electron did not initialize the isolated profile at ${profilePath}`, {
      cause: error,
    })
  }
  if (!runtime.electronVersion.startsWith('44.')) {
    throw new Error(`Expected Electron 44, found ${runtime.electronVersion}`)
  }
  return runtime
}

async function waitForExit(
  child: ChildProcessWithoutNullStreams,
  timeoutMs: number
): Promise<boolean> {
  if (child.exitCode !== null) return true
  return Promise.race([
    new Promise<boolean>(resolve => child.once('exit', () => resolve(true))),
    new Promise<boolean>(resolve => setTimeout(() => resolve(false), timeoutMs)),
  ])
}

async function terminateProcessTree(child: ChildProcessWithoutNullStreams): Promise<void> {
  if (child.exitCode !== null) return
  if (process.platform === 'win32' && child.pid) {
    const taskkill = process.env.SystemRoot
      ? path.join(process.env.SystemRoot, 'System32', 'taskkill.exe')
      : 'taskkill.exe'
    await execFile(taskkill, ['/PID', String(child.pid), '/T', '/F']).catch(() => {})
  } else {
    child.kill()
  }
  await waitForExit(child, 5_000)
}

export async function closeRuntime(runtime: LaunchedRuntime): Promise<void> {
  await runtime.page
    .evaluate(() =>
      document.querySelector<HTMLButtonElement>('.window-control-button.close-button')?.click()
    )
    .catch(() => {})
  runtime.browser.disconnect()
  if (!(await waitForExit(runtime.child, 10_000))) {
    await terminateProcessTree(runtime.child)
  }
}
