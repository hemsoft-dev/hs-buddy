import { describe, expect, it, vi } from 'vitest'
import {
  persistPackageSmokeResult,
  qualifyPackageDependencies,
  requireMountedRenderer,
  requirePackageRenderer,
  waitForMountedRenderer,
  type PackageSmokeResult,
} from './packageQualification'

function nativeModule(specifier: string): object {
  if (specifier === '@napi-rs/keyring') return { AsyncEntry: vi.fn() }
  return specifier === 'node-pty' ? { spawn: vi.fn(() => ({ kill: vi.fn() })) } : { load: vi.fn() }
}

describe('packaged dependency qualification', () => {
  it('loads native bindings and verifies the matching Copilot executable', () => {
    const loadModule = vi.fn(nativeModule)
    const resolveModule = vi.fn(() => '/package/copilot')
    const verifyExecutable = vi.fn()

    expect(
      qualifyPackageDependencies(
        'darwin',
        'arm64',
        '/package',
        loadModule,
        resolveModule,
        verifyExecutable
      )
    ).toEqual({
      nativeModules: ['node-pty', 'koffi', '@napi-rs/keyring'],
      copilotPackage: '@github/copilot-darwin-arm64',
      copilotBinary: expect.stringMatching(/[\\/]package[\\/]copilot$/),
    })
    expect(loadModule.mock.calls).toEqual([['node-pty'], ['koffi'], ['@napi-rs/keyring']])
    expect(
      (loadModule.mock.results[0].value as { spawn: ReturnType<typeof vi.fn> }).spawn
    ).toHaveBeenCalledOnce()
    expect(resolveModule).toHaveBeenCalledWith('@github/copilot-darwin-arm64')
    expect(verifyExecutable).toHaveBeenCalledWith(
      expect.stringMatching(/[\\/]package[\\/]copilot$/)
    )
  })

  it('fails a controlled missing-native-module fixture', () => {
    const loadModule = (specifier: string) => {
      if (specifier === 'node-pty') throw new Error('fixture: node-pty is missing')
      return nativeModule(specifier)
    }

    expect(() =>
      qualifyPackageDependencies(
        'linux',
        'x64',
        '/package',
        loadModule,
        vi.fn(() => '/package/native.node')
      )
    ).toThrow('fixture: node-pty is missing')
  })

  it.each([
    ['node-pty', 'not a module', 'spawn'],
    ['node-pty', null, 'spawn'],
    ['node-pty', {}, 'spawn'],
    ['koffi', {}, 'load'],
    ['@napi-rs/keyring', {}, 'AsyncEntry'],
  ])('rejects %s without its native binding', (missingDependency, invalidModule, binding) => {
    const loadModule = (specifier: string) =>
      specifier === missingDependency ? invalidModule : nativeModule(specifier)

    expect(() =>
      qualifyPackageDependencies(
        'linux',
        'x64',
        '/package',
        loadModule,
        vi.fn(() => '/package/native.node')
      )
    ).toThrow(`${missingDependency} loaded without its ${binding} binding`)
  })

  it('rejects dependency resolution outside packaged resources', () => {
    expect(() =>
      qualifyPackageDependencies(
        'linux',
        'x64',
        '/package/resources',
        nativeModule,
        () => '/checkout/node_modules/node-pty/index.js',
        vi.fn()
      )
    ).toThrow('node-pty resolved outside packaged resources')
  })
})

describe('packaged renderer qualification', () => {
  it('distinguishes preload and renderer failures', async () => {
    const rendererError = new Error('Renderer did not mount into #root')
    await expect(
      requirePackageRenderer(Promise.resolve(true), Promise.resolve(true))
    ).resolves.toBe(true)
    await expect(
      requirePackageRenderer(Promise.reject(rendererError), Promise.resolve(false))
    ).rejects.toThrow('Preload did not set __buddyPreloadReady')
    await expect(
      requirePackageRenderer(Promise.reject(rendererError), Promise.resolve(true))
    ).rejects.toBe(rendererError)
  })

  it('requires the renderer to report a mounted root', () => {
    expect(requireMountedRenderer(true)).toBe(true)
    expect(() => requireMountedRenderer(false)).toThrow('Renderer did not mount into #root')
  })

  it('waits for an asynchronous renderer mount', async () => {
    const check = vi.fn().mockResolvedValueOnce(false).mockResolvedValueOnce(true)
    const delay = vi.fn(async () => {})

    await expect(waitForMountedRenderer(check, delay, 2)).resolves.toBe(true)
    expect(check).toHaveBeenCalledTimes(2)
    expect(delay).toHaveBeenCalledTimes(1)
  })

  it('rejects a renderer that never mounts', async () => {
    await expect(
      waitForMountedRenderer(
        async () => false,
        async () => {},
        2
      )
    ).rejects.toThrow('Renderer did not mount into #root')
  })
})

describe('packaged Copilot qualification', () => {
  it('resolves the Windows executable and exercises the fallback shell', () => {
    const comSpec = process.env.ComSpec
    const pty = { spawn: vi.fn() }
    delete process.env.ComSpec
    try {
      expect(
        qualifyPackageDependencies(
          'win32',
          'x64',
          'C:\\package',
          specifier => (specifier === 'node-pty' ? pty : nativeModule(specifier)),
          () => 'C:\\package\\copilot.exe',
          vi.fn()
        ).copilotBinary
      ).toBe('C:\\package\\copilot.exe')
      expect(pty.spawn).toHaveBeenCalledWith('cmd.exe', ['/d', '/s', '/c', 'exit 0'], {
        cols: 80,
        rows: 24,
      })
    } finally {
      if (comSpec !== undefined) process.env.ComSpec = comSpec
    }
  })

  it('exercises node-pty with the configured Windows command shell', () => {
    vi.stubEnv('ComSpec', 'C:\\custom\\cmd.exe')
    const pty = { spawn: vi.fn() }
    try {
      qualifyPackageDependencies(
        'win32',
        'x64',
        'C:\\package',
        specifier => (specifier === 'node-pty' ? pty : nativeModule(specifier)),
        () => 'C:\\package\\native.node',
        vi.fn()
      )
      expect(pty.spawn).toHaveBeenCalledWith('C:\\custom\\cmd.exe', ['/d', '/s', '/c', 'exit 0'], {
        cols: 80,
        rows: 24,
      })
    } finally {
      vi.unstubAllEnvs()
    }
  })

  it('rejects an empty Copilot resolution and a missing binary', () => {
    expect(() =>
      qualifyPackageDependencies(
        'win32',
        'x64',
        'C:\\package',
        nativeModule,
        specifier => (specifier.startsWith('@github/') ? '' : 'C:\\package\\native.node'),
        vi.fn()
      )
    ).toThrow('@github/copilot-win32-x64 resolved to an empty path')

    const missingBinary = () => {
      throw new Error('fixture: Copilot binary is missing')
    }
    expect(() =>
      qualifyPackageDependencies(
        'win32',
        'x64',
        'C:\\package',
        nativeModule,
        () => 'C:\\package\\copilot.exe',
        missingBinary
      )
    ).toThrow('fixture: Copilot binary is missing')
  })
})

describe('package smoke result persistence', () => {
  const successfulResult: PackageSmokeResult = {
    ok: true,
    platform: 'linux',
    arch: 'x64',
  }

  it('writes a successful result and exits cleanly', async () => {
    const writeResult = vi.fn(async () => {})
    const exit = vi.fn()

    await persistPackageSmokeResult(async () => successfulResult, writeResult, exit)

    expect(writeResult).toHaveBeenCalledWith(successfulResult)
    expect(exit).toHaveBeenCalledWith(0)
  })

  it('records qualification failures before exiting unsuccessfully', async () => {
    const writeResult = vi.fn(async () => {})
    const exit = vi.fn()

    await persistPackageSmokeResult(
      async () => {
        throw new Error('fixture: qualification failed')
      },
      writeResult,
      exit
    )

    expect(writeResult).toHaveBeenCalledWith(
      expect.objectContaining({ ok: false, error: 'Error: fixture: qualification failed' })
    )
    expect(exit).toHaveBeenCalledWith(1)
  })

  it('still exits when persisting the result fails', async () => {
    const exit = vi.fn()

    await expect(
      persistPackageSmokeResult(
        async () => successfulResult,
        async () => {
          throw new Error('fixture: write failed')
        },
        exit
      )
    ).rejects.toThrow('fixture: write failed')
    expect(exit).toHaveBeenCalledWith(0)
  })
})
