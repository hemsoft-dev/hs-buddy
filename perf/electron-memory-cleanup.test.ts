import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { spawn } from 'node:child_process'
import { describe, expect, it, vi } from 'vitest'
import { removeOwnedProfile, withProfileCleanup } from './electron-memory-cleanup'

describe('owned memory-profile cleanup', () => {
  it('allows a transient Windows directory lock to release without rerunning the measurement', async () => {
    const busy = Object.assign(new Error('owned profile busy'), { code: 'EBUSY' })
    const remove = vi.fn().mockRejectedValueOnce(busy).mockResolvedValue(undefined)
    await expect(removeOwnedProfile('owned-fixture', remove)).resolves.toBeUndefined()
    expect(remove).toHaveBeenCalledTimes(2)
  })

  it('fails after a bounded persistent lock and does not hide other filesystem errors', async () => {
    const busy = Object.assign(new Error('owned profile busy'), { code: 'EBUSY' })
    const locked = vi.fn().mockRejectedValue(busy)
    await expect(removeOwnedProfile('owned-fixture', locked)).rejects.toBe(busy)
    expect(locked).toHaveBeenCalledTimes(4)
    const denied = Object.assign(new Error('not transient'), { code: 'EACCES' })
    const remove = vi.fn().mockRejectedValue(denied)
    await expect(removeOwnedProfile('owned-fixture', remove)).rejects.toBe(denied)
    expect(remove).toHaveBeenCalledTimes(1)
  })
})

describe('memory-profile primary failures', () => {
  it('preserves an earlier measurement failure when required shutdown also fails', async () => {
    const primary = new Error('measurement failed')
    const secondary = new Error('shutdown failed')
    const close = vi.fn().mockRejectedValue(secondary)
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    try {
      await expect(
        withProfileCleanup(
          'owned-fixture',
          async () => {
            throw primary
          },
          close
        )
      ).rejects.toBe(primary)
      expect(close).toHaveBeenCalledTimes(1)
      expect(warn).toHaveBeenCalledWith(
        'Memory-profile cleanup also failed. The original measurement error is retained.'
      )
    } finally {
      warn.mockRestore()
    }
  })

  it('keeps cleanup failure fatal after a successful measurement', async () => {
    const failure = new Error('shutdown failed')
    await expect(
      withProfileCleanup(
        'owned-fixture',
        async () => 17,
        async () => {
          throw failure
        }
      )
    ).rejects.toBe(failure)
  })
})

describe('actual owned profile locks', () => {
  it.skipIf(process.platform !== 'win32')(
    'removes an actual owned profile after its child releases the Windows cwd lock',
    async () => {
      const profile = await mkdtemp(path.join(tmpdir(), 'buddy-memory-owned-test-'))
      expect(path.dirname(profile)).toBe(path.resolve(tmpdir()))
      expect(path.basename(profile)).toMatch(/^buddy-memory-owned-test-/)
      const child = spawn(
        process.execPath,
        ['-e', 'process.stdout.write("ready");setTimeout(()=>{},1000)'],
        {
          cwd: profile,
          windowsHide: true,
          stdio: ['ignore', 'pipe', 'pipe'],
        }
      )
      const closed = new Promise<void>((resolve, reject) => {
        child.once('error', reject)
        child.once('close', () => resolve())
      })
      try {
        await new Promise<void>((resolve, reject) => {
          child.stdout.once('data', () => resolve())
          child.once('error', reject)
        })
        await expect(rm(profile, { recursive: true, force: true })).rejects.toMatchObject({
          code: 'EBUSY',
        })
        const measurement = vi.fn().mockResolvedValue(17)
        await expect(withProfileCleanup(profile, measurement, async () => {})).resolves.toBe(17)
        expect(measurement).toHaveBeenCalledTimes(1)
      } finally {
        await closed
        await rm(profile, { recursive: true, force: true })
      }
    }
  )
})
