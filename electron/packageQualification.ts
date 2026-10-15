import { accessSync, constants } from 'node:fs'

const PACKAGE_NATIVE_MODULES = [
  { dependency: 'node-pty', binding: 'spawn' },
  { dependency: 'koffi', binding: 'load' },
  { dependency: '@napi-rs/keyring', binding: 'AsyncEntry' },
] as const

export interface PackageDependencyReport {
  nativeModules: string[]
  copilotPackage: string
  copilotBinary: string
}

export interface PackageSmokeResult extends Record<string, unknown> {
  ok: boolean
  platform: NodeJS.Platform
  arch: string
  error?: string
}

function assertExecutable(file: string): void {
  accessSync(file, constants.X_OK)
}

export function requireMountedRenderer(rendererLoaded: unknown): true {
  if (rendererLoaded !== true) throw new Error('Renderer did not mount into #root')
  return true
}

export async function requirePackageRenderer(
  renderer: Promise<true>,
  preloadReady: Promise<unknown>
): Promise<true> {
  try {
    return await renderer
  } catch (error: unknown) {
    if ((await preloadReady) !== true)
      throw new Error('Preload did not set __buddyPreloadReady', { cause: error })
    throw error
  }
}

export async function waitForMountedRenderer(
  check: () => Promise<unknown>,
  delay: () => Promise<void>,
  attempts = 120
): Promise<true> {
  for (let attempt = 0; attempt < attempts; attempt++) {
    if ((await check()) === true) return true
    await delay()
  }
  return requireMountedRenderer(false)
}

function assertPackagedResolution(
  dependency: string,
  resolvedPath: string,
  resourcesPath: string,
  platform: NodeJS.Platform
): void {
  const normalize = (value: string) => value.replaceAll('\\', '/').replace(/\/$/, '')
  const resolved = normalize(resolvedPath)
  const resources = normalize(resourcesPath)
  const [comparableResolved, comparableResources] =
    platform === 'win32' ? [resolved.toLowerCase(), resources.toLowerCase()] : [resolved, resources]
  if (!comparableResolved.startsWith(`${comparableResources}/`)) {
    throw new Error(`${dependency} resolved outside packaged resources: ${resolvedPath}`)
  }
}

function qualifyNativeModules(
  platform: NodeJS.Platform,
  resourcesPath: string,
  loadModule: (specifier: string) => unknown,
  resolveModule: (specifier: string) => string
): string[] {
  for (const { dependency, binding } of PACKAGE_NATIVE_MODULES) {
    const loaded = loadModule(dependency)
    if (
      (typeof loaded !== 'object' && typeof loaded !== 'function') ||
      loaded === null ||
      typeof (loaded as Record<string, unknown>)[binding] !== 'function'
    ) {
      throw new Error(`${dependency} loaded without its ${binding} binding`)
    }
    assertPackagedResolution(dependency, resolveModule(dependency), resourcesPath, platform)
    if (dependency === 'node-pty') {
      const command = platform === 'win32' ? (process.env.ComSpec ?? 'cmd.exe') : '/bin/sh'
      const args = platform === 'win32' ? ['/d', '/s', '/c', 'exit 0'] : ['-c', 'exit 0']
      const ptyModule = loaded as {
        spawn: (file: string, args: string[], options: { cols: number; rows: number }) => unknown
      }
      ptyModule.spawn(command, args, { cols: 80, rows: 24 })
    }
  }
  return PACKAGE_NATIVE_MODULES.map(module => module.dependency)
}

/** Load every native dependency from the packaged Electron runtime. */
export function qualifyPackageDependencies(
  platform: NodeJS.Platform,
  arch: string,
  resourcesPath: string,
  loadModule: (specifier: string) => unknown,
  resolveModule: (specifier: string) => string,
  verifyExecutable: (file: string) => void = assertExecutable
): PackageDependencyReport {
  const nativeModules = qualifyNativeModules(platform, resourcesPath, loadModule, resolveModule)
  const copilotPackage = `@github/copilot-${platform}-${arch}`
  const copilotBinary = resolveModule(copilotPackage)
  if (!copilotBinary.trim()) throw new Error(`${copilotPackage} resolved to an empty path`)
  assertPackagedResolution(copilotPackage, copilotBinary, resourcesPath, platform)
  verifyExecutable(copilotBinary)

  return {
    nativeModules,
    copilotPackage,
    copilotBinary,
  }
}

export async function persistPackageSmokeResult(
  getResult: () => Promise<PackageSmokeResult>,
  writeResult: (result: PackageSmokeResult) => Promise<void>,
  exit: (code: number) => void
): Promise<void> {
  let exitCode = 0
  let result: PackageSmokeResult
  try {
    result = await getResult()
  } catch (error: unknown) {
    exitCode = 1
    result = {
      ok: false,
      platform: process.platform,
      arch: process.arch,
      error: String(error),
    }
  }

  try {
    await writeResult(result)
  } finally {
    exit(exitCode)
  }
}
