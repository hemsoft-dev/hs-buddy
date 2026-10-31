import path from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { validateLaunch, type LaunchedRuntime } from './electron-memory-runtime'

const { access } = vi.hoisted(() => ({ access: vi.fn() }))
vi.mock('node:fs/promises', async importOriginal => ({
  ...(await importOriginal<typeof import('node:fs/promises')>()),
  access,
}))

const profile = path.resolve('isolated-memory-profile')
const partition = path.join(profile, 'Partitions', 'buddy')
const runtime = (electronVersion = '44.9.3') =>
  ({
    electronVersion,
    appVersion: 'test',
  }) as LaunchedRuntime

beforeEach(() => {
  vi.useFakeTimers()
  access.mockReset()
})
afterEach(() => {
  vi.useRealTimers()
})

describe('memory harness profile isolation', () => {
  it('rejects a preseeded config without an Electron-created session', async () => {
    access.mockImplementation(async filename => {
      if (filename !== path.join(profile, 'config.json')) throw new Error('ENOENT')
    })
    const result = expect(validateLaunch(runtime(), profile)).rejects.toThrow(
      'Electron did not initialize the isolated profile'
    )
    await vi.runAllTimersAsync()
    await result
    expect(access).toHaveBeenCalledWith(partition)
    expect(access).not.toHaveBeenCalledWith(path.join(profile, 'config.json'))
  })

  it('accepts the Electron-created persisted session independently of config seeding', async () => {
    access.mockImplementation(async filename => {
      if (filename !== partition) throw new Error('ENOENT')
    })
    const result = expect(validateLaunch(runtime(), profile)).resolves.toMatchObject({
      electronVersion: '44.9.3',
    })
    await vi.runAllTimersAsync()
    await result
  })

  it('still rejects an unsupported Electron version after verifying isolation', async () => {
    access.mockResolvedValue(undefined)
    await expect(validateLaunch(runtime('45.0.0'), profile)).rejects.toThrow('Expected Electron 44')
  })
})
