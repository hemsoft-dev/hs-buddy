import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs'
import { spawn, spawnSync, type ChildProcess } from 'node:child_process'
import { join, resolve } from 'node:path'

interface TargetRuntime {
  platform: string
  arch: string
}

interface SmokeResult {
  ok?: boolean
  platform?: string
  arch?: string
  rendererLoaded?: boolean
  nativeModules?: string[]
  copilotPackage?: string
  copilotBinary?: string
  error?: string
}

const supportedPlatforms = new Set(['win32', 'linux', 'darwin'])
const supportedArchitectures = new Set(['x64', 'arm64'])
const releaseRoot = resolve('release')
const smokeOutput = join(releaseRoot, 'package-smoke-result.json')
const smokeUserData = join(releaseRoot, 'package-smoke-user-data')
const logPath = join(releaseRoot, 'package-smoke.log')
const output: string[] = []
let packagedFiles: string[] = []

mkdirSync(releaseRoot, { recursive: true })
rmSync(smokeOutput, { force: true })
rmSync(smokeUserData, { force: true, recursive: true })

function expectedRuntime(): TargetRuntime {
  const [platform, arch] = process.argv.slice(2)
  if (!platform || !supportedPlatforms.has(platform)) {
    throw new Error('Usage: bun scripts/package-smoke.ts <win32|linux|darwin> <x64|arm64>')
  }
  if (!arch || !supportedArchitectures.has(arch)) {
    throw new Error('Expected architecture must be x64 or arm64')
  }
  return { platform, arch }
}

function listFiles(directory: string): string[] {
  if (!existsSync(directory)) return []

  const files: string[] = []
  for (const entry of readdirSync(directory)) {
    const entryPath = join(directory, entry)
    if (statSync(entryPath).isDirectory()) files.push(...listFiles(entryPath))
    else files.push(entryPath)
  }
  return files
}

function findExecutable(target: TargetRuntime): string {
  const executable = process.env.BUDDY_PACKAGE_EXECUTABLE
  if (!executable || !existsSync(executable)) {
    throw new Error(
      `No installed or mounted Buddy executable found for ${target.platform}-${target.arch}`
    )
  }
  return executable
}

function launchDetails(target: TargetRuntime, executable: string): [string, string[]] {
  const appArguments = [`--user-data-dir=${smokeUserData}`]
  if (target.platform === 'linux') {
    return ['xvfb-run', ['--auto-servernum', executable, ...appArguments]]
  }
  return [executable, appArguments]
}

function terminateProcessTree(child: Pick<ChildProcess, 'pid' | 'kill'>, platform: string): void {
  if (!child.pid) return
  if (platform === 'win32') {
    spawnSync('taskkill', ['/pid', String(child.pid), '/t', '/f'], { stdio: 'ignore' })
    return
  }
  try {
    process.kill(-child.pid, 'SIGKILL')
  } catch (_: unknown) {
    child.kill('SIGKILL')
  }
}

async function waitForExit(
  launchCommand: string,
  launchArguments: string[],
  platform: string
): Promise<{ code: number | null; signal: NodeJS.Signals | null }> {
  const child = spawn(launchCommand, launchArguments, {
    env: {
      ...process.env,
      BUDDY_PACKAGE_SMOKE_FILE: smokeOutput,
      ELECTRON_DISABLE_SECURITY_WARNINGS: 'true',
    },
    detached: platform !== 'win32',
    stdio: ['ignore', 'pipe', 'pipe'],
  })

  child.stdout.on('data', chunk => output.push(`[stdout] ${String(chunk).trimEnd()}`))
  child.stderr.on('data', chunk => output.push(`[stderr] ${String(chunk).trimEnd()}`))

  return new Promise((resolveExit, reject) => {
    let exitResult: { code: number | null; signal: NodeJS.Signals | null } | undefined
    let closeGrace: ReturnType<typeof setTimeout> | undefined
    let timeoutError: Error | undefined
    const timeout = setTimeout(() => {
      timeoutError = new Error(
        'Packaged app did not complete its startup smoke test within 60 seconds'
      )
      terminateProcessTree(child, platform)
      closeGrace = setTimeout(() => reject(timeoutError), 5_000)
    }, 60_000)

    child.once('error', error => {
      clearTimeout(timeout)
      clearTimeout(closeGrace)
      reject(error)
    })
    child.once('exit', (code, signal) => {
      clearTimeout(timeout)
      const result = { code, signal }
      exitResult = result
      if (timeoutError) return
      closeGrace = setTimeout(() => resolveExit(result), 5_000)
    })
    child.once('close', (code, signal) => {
      clearTimeout(timeout)
      clearTimeout(closeGrace)
      if (timeoutError) {
        reject(timeoutError)
        return
      }
      resolveExit(exitResult ?? { code, signal })
    })
  })
}

function readSmokeResult(): SmokeResult {
  if (!existsSync(smokeOutput))
    throw new Error('Packaged app exited without writing a smoke result')
  return JSON.parse(readFileSync(smokeOutput, 'utf8')) as SmokeResult
}

function validateRuntimeIdentity(result: SmokeResult, target: TargetRuntime): void {
  if (result.platform !== target.platform || result.arch !== target.arch) {
    throw new Error(
      `Runtime was ${String(result.platform)}-${String(result.arch)}, expected ${target.platform}-${target.arch}`
    )
  }
  if (!result.rendererLoaded) throw new Error('Packaged renderer did not load')
}

function validateDependencies(result: SmokeResult, target: TargetRuntime): void {
  for (const dependency of ['node-pty', 'koffi', '@napi-rs/keyring']) {
    if (!result.nativeModules?.includes(dependency)) {
      throw new Error(`Packaged native module did not load: ${dependency}`)
    }
  }
  if (!result.copilotPackage?.includes(`copilot-${target.platform}-${target.arch}`)) {
    throw new Error('Packaged Copilot package does not match the runtime platform and architecture')
  }
  if (!result.copilotBinary) throw new Error('Packaged Copilot binary was not verified')
}

function validateResult(result: SmokeResult, exitCode: number | null, target: TargetRuntime): void {
  if (exitCode !== 0 || !result.ok) {
    throw new Error(result.error ?? `Packaged app exited with code ${String(exitCode)}`)
  }
  validateRuntimeIdentity(result, target)
  validateDependencies(result, target)
}

function appendFailure(error: unknown): void {
  const packageListing = packagedFiles.slice(0, 200)
  output.push(
    `FAILURE: ${String(error)}`,
    `Package contents (${packagedFiles.length} files; first ${packageListing.length}):`,
    ...packageListing
  )
}

async function run(): Promise<void> {
  try {
    const target = expectedRuntime()
    packagedFiles = listFiles(releaseRoot)
    const executable = findExecutable(target)
    const [launchCommand, launchArguments] = launchDetails(target, executable)
    output.push(
      `Qualifying ${target.platform}-${target.arch}`,
      `Executable: ${executable}`,
      `Command: ${launchCommand} ${launchArguments.join(' ')}`
    )

    const exitResult = await waitForExit(launchCommand, launchArguments, target.platform)
    output.push(`Exit: code=${String(exitResult.code)} signal=${String(exitResult.signal)}`)
    const result = readSmokeResult()
    output.push(`Result: ${JSON.stringify(result)}`)
    validateResult(result, exitResult.code, target)
    output.push('Package startup qualification passed.')
  } catch (error: unknown) {
    appendFailure(error)
    throw error
  } finally {
    writeFileSync(logPath, `${output.join('\n')}\n`)
    console.log(output.join('\n'))
  }
}

await run()
