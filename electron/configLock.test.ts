import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  utimesSync,
  writeFileSync,
  symlinkSync,
} from 'node:fs'
import * as fs from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { STALE_AFTER_MS, acquireConfigLock, lockDirFor, withConfigLock } from './configLock'

/** One-shot interception points for the lock's file-system calls. */
const hooks = vi.hoisted(() => ({
  existsSync: null as ((path: string) => boolean) | null,
  renameSync: null as ((from: string, to: string) => void) | null,
  readdirSync: null as ((path: string) => void) | null,
}))

vi.mock('node:fs', async importOriginal => {
  const actual = await importOriginal<typeof import('node:fs')>()
  return {
    ...actual,
    existsSync: (path: Parameters<typeof actual.existsSync>[0]) =>
      hooks.existsSync?.(String(path)) ?? actual.existsSync(path),
    renameSync: (from: Parameters<typeof actual.renameSync>[0], to: string) => {
      hooks.renameSync?.(String(from), to)
      actual.renameSync(from, to)
    },
    readdirSync: (path: Parameters<typeof actual.readdirSync>[0]) => {
      hooks.readdirSync?.(String(path))
      return actual.readdirSync(path)
    },
  }
})

function ageDir(dir: string): void {
  const past = (Date.now() - STALE_AFTER_MS - 5_000) / 1000
  utimesSync(dir, past, past)
}

let dir = ''
let configPath = ''
let lockDir = ''

function useTempConfig(): void {
  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), 'buddy-config-lock-'))
    configPath = join(dir, 'config.json')
    lockDir = lockDirFor(configPath)
  })

  afterEach(() => {
    rmSync(dir, { recursive: true, force: true })
  })
}

/** Nest `levels` claim markers under the lock dir, as crashed claimers leave. */
function buildDeadChain(levels: number): void {
  let level = lockDir
  for (let depth = 0; depth < levels; depth += 1) {
    mkdirSync(level)
    level = join(level, 'claim')
  }
}

function ageDeadChain(levels: number): void {
  let level = lockDir
  for (let depth = 0; depth < levels; depth += 1) {
    ageDir(level)
    level = join(level, 'claim')
  }
}

describe('configLock', () => {
  useTempConfig()

  it('names the lock directory next to the config file', () => {
    expect(lockDirFor('/x/Buddy/config.json')).toBe('/x/Buddy/config.json.lock')
  })

  it('acquires and releases', () => {
    const release = acquireConfigLock(configPath)
    expect(release).not.toBeNull()
    expect(existsSync(lockDir)).toBe(true)
    release?.release()
    expect(existsSync(lockDir)).toBe(false)
  })

  it('waits for a held lock, then proceeds unlocked after the timeout', () => {
    const held = acquireConfigLock(configPath)
    expect(held).not.toBeNull()
    let clock = 1_000
    const sleep = vi.fn((ms: number) => {
      clock += ms
    })
    const warn = vi.fn()
    const second = acquireConfigLock(configPath, {
      timeoutMs: 100,
      monotonic: () => clock,
      sleep,
      warn,
    })
    expect(second).toBeNull()
    expect(sleep).toHaveBeenCalled()
    expect(warn).toHaveBeenCalledWith(expect.stringContaining('busy for 100ms'))
    // The holder's lock is untouched by the loser.
    expect(existsSync(lockDir)).toBe(true)
    held?.release()
  })

  it('waits in real time with the default sleep, then gives up quietly', () => {
    const held = acquireConfigLock(configPath)
    expect(held).not.toBeNull()
    const started = Date.now()
    // No injected clock, sleep or warn: exercises the real poll and the
    // default no-op warning.
    expect(acquireConfigLock(configPath, { timeoutMs: 40 })).toBeNull()
    expect(Date.now() - started).toBeGreaterThanOrEqual(10)
    held?.release()
  })
})

describe('configLock stale takeover', () => {
  useTempConfig()

  it('takes over a stale lock in place and releases it fully', () => {
    mkdirSync(lockDir)
    ageDir(lockDir)
    const sleep = vi.fn()
    const release = acquireConfigLock(configPath, { timeoutMs: 200, sleep })
    expect(release).not.toBeNull()
    expect(sleep).not.toHaveBeenCalled()
    // Taken over in place: the same directory, now carrying the marker.
    expect(existsSync(join(lockDir, 'claim'))).toBe(true)
    release?.release()
    expect(existsSync(lockDir)).toBe(false)
  })

  it('a taken-over lock looks fresh to the next waiter', () => {
    mkdirSync(lockDir)
    ageDir(lockDir)
    const owner = acquireConfigLock(configPath, { timeoutMs: 200, sleep: vi.fn() })
    expect(owner).not.toBeNull()
    const late = acquireConfigLock(configPath, { timeoutMs: 30, sleep: vi.fn() })
    expect(late).toBeNull()
    expect(existsSync(lockDir)).toBe(true)
    owner?.release()
  })

  it('withdraws a claim when the directory is not the stale instance it observed', () => {
    mkdirSync(lockDir)
    ageDir(lockDir)
    let reads = 0
    // On the second clock read (the identity re-check after claiming), the
    // stale directory is replaced by a fresh one, as if its holder released
    // it and another writer acquired anew.
    const now = () => {
      reads += 1
      if (reads === 2) {
        rmSync(lockDir, { recursive: true, force: true })
        mkdirSync(lockDir)
      }
      return Date.now()
    }
    let mono = 0
    const release = acquireConfigLock(configPath, {
      timeoutMs: 30,
      now,
      monotonic: () => mono,
      sleep: vi.fn((ms: number) => {
        mono += ms
      }),
    })
    // The fresh lock belongs to someone else: no takeover, timed out.
    expect(release).toBeNull()
    expect(existsSync(join(lockDir, 'claim'))).toBe(false)
    expect(existsSync(lockDir)).toBe(true)
  })

  it('only one of two claimers wins a stale lock, and the loser keeps polling', () => {
    mkdirSync(lockDir)
    ageDir(lockDir)
    mkdirSync(join(lockDir, 'claim')) // another waiter claimed it just now
    let clock = 1_000
    const sleep = vi.fn((ms: number) => {
      clock += ms
    })
    const release = acquireConfigLock(configPath, { timeoutMs: 50, monotonic: () => clock, sleep })
    expect(release).toBeNull()
    // No busy spin: the loser slept and respected the deadline.
    expect(sleep).toHaveBeenCalled()
    expect(existsSync(join(lockDir, 'claim'))).toBe(true)
  })
})

describe('configLock claim chains', () => {
  useTempConfig()

  it('claims through a marker left by a crashed claimer and releases the whole chain', () => {
    mkdirSync(lockDir)
    mkdirSync(join(lockDir, 'claim'))
    ageDir(join(lockDir, 'claim'))
    ageDir(lockDir)
    const release = acquireConfigLock(configPath, { timeoutMs: 200, sleep: vi.fn() })
    expect(release).not.toBeNull()
    // Nothing of the dead claimer was removed; the takeover nested below it.
    expect(existsSync(join(lockDir, 'claim', 'claim'))).toBe(true)
    release?.release()
    expect(existsSync(lockDir)).toBe(false)
  })

  it('does not disturb a live claimer that is already nested below a dead one', () => {
    mkdirSync(lockDir)
    mkdirSync(join(lockDir, 'claim'))
    ageDir(join(lockDir, 'claim'))
    ageDir(lockDir)
    mkdirSync(join(lockDir, 'claim', 'claim')) // a live claimer, fresh
    let clock = 1_000
    const sleep = vi.fn((ms: number) => {
      clock += ms
    })
    const release = acquireConfigLock(configPath, { timeoutMs: 50, monotonic: () => clock, sleep })
    expect(release).toBeNull()
    expect(sleep).toHaveBeenCalled()
    expect(existsSync(join(lockDir, 'claim', 'claim'))).toBe(true)
  })

  it('retries when a stale lock vanishes before it can be observed', () => {
    mkdirSync(lockDir)
    ageDir(lockDir)
    let reads = 0
    const now = () => {
      reads += 1
      if (reads === 2) rmSync(lockDir, { recursive: true, force: true })
      return Date.now()
    }
    const release = acquireConfigLock(configPath, { timeoutMs: 200, sleep: vi.fn(), now })
    expect(release).not.toBeNull()
    release?.release()
  })
})

describe('configLock release and fallback', () => {
  useTempConfig()

  it('reports whether the lock is still the instance it created', () => {
    const handle = acquireConfigLock(configPath)
    expect(handle?.held()).toBe(true)
    // Taken over while this process was suspended: released and re-created.
    rmSync(lockDir, { recursive: true, force: true })
    mkdirSync(lockDir)
    expect(handle?.held()).toBe(false)
    // Releasing must not strip the new holder's lock.
    handle?.release()
    expect(existsSync(lockDir)).toBe(true)
  })

  it('re-acquires before writing when the lock was lost meanwhile', () => {
    const warn = vi.fn()
    const claim = join(lockDir, 'claim')
    // Replace the lock during the ownership check that follows acquisition
    // (the first look for a nested claim), as a takeover during a long
    // suspension would.
    hooks.existsSync = path => {
      hooks.existsSync = null
      rmSync(lockDir, { recursive: true, force: true })
      mkdirSync(lockDir)
      ageDir(lockDir)
      return path === claim
    }
    const ran = withConfigLock(configPath, () => existsSync(claim), { warn, sleep: vi.fn() })
    expect(warn).toHaveBeenCalledWith(expect.stringContaining('re-acquiring'))
    // The replacement was stale, so the second acquisition claimed it.
    expect(ran).toBe(true)
    expect(existsSync(lockDir)).toBe(false)
  })

  it('measures the deadline on the monotonic clock, not wall time', () => {
    const held = acquireConfigLock(configPath)
    let mono = 0
    const sleep = vi.fn((ms: number) => {
      mono += ms
    })
    // Wall time runs backwards (within the stale window, so the lock stays
    // fresh); the wait must still end after 50 ms.
    let wall = Date.now()
    const release = acquireConfigLock(configPath, {
      timeoutMs: 50,
      now: () => (wall -= 100),
      monotonic: () => mono,
      sleep,
    })
    expect(release).toBeNull()
    expect(mono).toBeGreaterThanOrEqual(50)
    held?.release()
  })

  it('tolerates releasing twice', () => {
    const release = acquireConfigLock(configPath)
    release?.release()
    expect(() => release?.release()).not.toThrow()
    expect(existsSync(lockDir)).toBe(false)
  })

  it('runs the callback under the lock and releases even when it throws', () => {
    expect(() =>
      withConfigLock(configPath, () => {
        expect(existsSync(lockDir)).toBe(true)
        throw new Error('boom')
      })
    ).toThrow('boom')
    expect(existsSync(lockDir)).toBe(false)
  })
})

describe('configLock recovery', () => {
  useTempConfig()

  it('honours the deadline when the lock path cannot be observed', () => {
    // A dangling directory link: mkdir says it exists, stat cannot follow it.
    // A Windows junction exercises the same path without symlink privileges.
    symlinkSync(join(dir, 'gone'), lockDir, process.platform === 'win32' ? 'junction' : 'dir')
    let mono = 0
    const sleep = vi.fn((ms: number) => {
      mono += ms
    })
    const warn = vi.fn()
    const handle = acquireConfigLock(configPath, {
      timeoutMs: 40,
      monotonic: () => mono,
      sleep,
      warn,
    })
    expect(handle).toBeNull()
    expect(sleep).toHaveBeenCalled()
    expect(warn).toHaveBeenCalledWith(expect.stringContaining('busy for 40ms'))
    expect(existsSync(join(dir, 'gone'))).toBe(false)
  })

  it('repeats an idempotent write when the lock was lost during it', () => {
    const warn = vi.fn()
    let runs = 0
    const result = withConfigLock(
      configPath,
      () => {
        runs += 1
        if (runs === 1) {
          // Suspended inside the write past the stale window: another
          // writer took the lock over in place.
          ageDir(lockDir)
          mkdirSync(join(lockDir, 'claim'))
        }
        return runs
      },
      { warn, sleep: vi.fn(), timeoutMs: 50, onLostDuringWrite: 'repeat' }
    )
    expect(result).toBe(2)
    expect(warn).toHaveBeenCalledWith(expect.stringContaining('repeating it'))
  })

  it('only reports a lost lock for a transaction that cannot be replayed', () => {
    const warn = vi.fn()
    let runs = 0
    const result = withConfigLock(
      configPath,
      () => {
        runs += 1
        ageDir(lockDir)
        mkdirSync(join(lockDir, 'claim'))
        return runs
      },
      { warn, sleep: vi.fn(), timeoutMs: 50 }
    )
    expect(result).toBe(1)
    expect(warn).toHaveBeenCalledWith(expect.stringContaining('may have been overwritten'))
  })

  it('creates a missing config directory rather than giving up the lock', () => {
    const warn = vi.fn()
    const nested = join(dir, 'missing', 'deeper', 'config.json')
    const handle = acquireConfigLock(nested, { warn })
    expect(handle).not.toBeNull()
    expect(existsSync(lockDirFor(nested))).toBe(true)
    expect(warn).not.toHaveBeenCalled()
    handle?.release()
  })

  it('proceeds unlocked when the lock directory cannot be created', () => {
    const warn = vi.fn()
    // A file where the config directory should be: no lock can live there.
    writeFileSync(join(dir, 'notadir'), '')
    const release = acquireConfigLock(join(dir, 'notadir', 'config.json'), { warn })
    expect(release).toBeNull()
    expect(warn).toHaveBeenCalledWith(expect.stringContaining('proceeding unlocked'))
  })
})

describe('configLock lost ownership', () => {
  useTempConfig()

  it('invalidates the displaced owner before removing the claim that displaced it', () => {
    const first = acquireConfigLock(configPath)
    ageDir(lockDir)
    const second = acquireConfigLock(configPath, { timeoutMs: 50, sleep: vi.fn() })
    expect(second?.held()).toBe(true)
    // Something keeps the claim marker from being removed, so the release
    // stops there; the displaced owner's token must already be gone.
    writeFileSync(join(lockDir, 'claim', 'stray'), '')
    second?.release()
    expect(existsSync(join(lockDir, 'claim'))).toBe(true)
    expect(existsSync(join(lockDir, 'owner'))).toBe(false)
    expect(first?.held()).toBe(false)
    first?.release()
    expect(existsSync(join(lockDir, 'claim'))).toBe(true)
  })

  it('treats a nested claim as lost ownership', () => {
    const first = acquireConfigLock(configPath)
    expect(first?.held()).toBe(true)
    // This process was suspended past the stale window and another writer
    // took the lock over in place.
    ageDir(lockDir)
    const second = acquireConfigLock(configPath, { timeoutMs: 200, sleep: vi.fn() })
    expect(second).not.toBeNull()
    expect(first?.held()).toBe(false)
    expect(second?.held()).toBe(true)
    first?.release()
    expect(existsSync(join(lockDir, 'claim'))).toBe(true)
    second?.release()
    expect(existsSync(lockDir)).toBe(false)
  })
})

describe('configLock dead chains', () => {
  useTempConfig()

  it('recovers a chain that reached the depth limit', () => {
    // Eight successive claimers crashed, each owning a deeper stale marker.
    buildDeadChain(9)
    const parent = join(lockDir, ...Array<string>(7).fill('claim'))
    writeFileSync(join(parent, 'owner'), 'displaced')
    ageDeadChain(9)
    // The displaced parent owner is invalidated before its marker moves.
    hooks.renameSync = () => {
      hooks.renameSync = null
      expect(existsSync(join(parent, 'owner'))).toBe(false)
    }
    // Dropping the deepest marker refreshes its parent, so the next attempt
    // claims it once the stale window has passed again.
    let wall = Date.now()
    const sleep = vi.fn(() => {
      wall += STALE_AFTER_MS + 1_000
    })
    const handle = acquireConfigLock(configPath, { timeoutMs: 500, now: () => wall, sleep })
    expect(handle).not.toBeNull()
    expect(handle?.held()).toBe(true)
    handle?.release()
    expect(existsSync(lockDir)).toBe(false)
  })

  it('never deletes a marker recreated under the dead one it observed', () => {
    buildDeadChain(9)
    ageDeadChain(9)
    const deepest = join(lockDir, ...Array<string>(8).fill('claim'))
    // Between observing the dead marker and moving it aside, another
    // writer replaces it with its own fresh claim.
    hooks.renameSync = from => {
      hooks.renameSync = null
      if (from !== deepest) throw new Error(`unexpected rename of ${from}`)
      rmSync(deepest, { recursive: true, force: true })
      mkdirSync(deepest)
      writeFileSync(join(deepest, 'owner'), 'other')
    }
    expect(acquireConfigLock(configPath, { timeoutMs: 50, sleep: vi.fn() })).toBeNull()
    expect(fs.readFileSync(join(deepest, 'owner'), 'utf8')).toBe('other')
    expect(fs.readdirSync(join(lockDir, ...Array<string>(7).fill('claim')))).toEqual(['claim'])
  })

  it('sweeps stale leftovers of a recoverer that crashed mid-way', () => {
    buildDeadChain(9)
    const parent = join(lockDir, ...Array<string>(7).fill('claim'))
    mkdirSync(join(parent, 'dead-leftover'))
    ageDeadChain(9)
    ageDir(join(parent, 'dead-leftover'))
    let wall = Date.now()
    const sleep = vi.fn(() => {
      wall += STALE_AFTER_MS + 1_000
    })
    const handle = acquireConfigLock(configPath, { timeoutMs: 500, now: () => wall, sleep })
    expect(handle).not.toBeNull()
    handle?.release()
    expect(existsSync(lockDir)).toBe(false)
  })

  it('returns to acquisition when the chain vanishes during the leftover sweep', () => {
    buildDeadChain(9)
    ageDeadChain(9)
    // Another waiter recovered and released the whole chain meanwhile.
    hooks.readdirSync = () => {
      hooks.readdirSync = null
      rmSync(lockDir, { recursive: true, force: true })
    }
    const handle = acquireConfigLock(configPath, { timeoutMs: 50, sleep: vi.fn() })
    expect(handle?.held()).toBe(true)
    handle?.release()
  })
})

describe('configLock clock set-back', () => {
  useTempConfig()

  it('re-dates a lock from the future instead of stealing it', () => {
    mkdirSync(lockDir)
    // The holder wrote it before the clock was set back past the window.
    const ahead = (Date.now() + STALE_AFTER_MS + 60_000) / 1000
    utimesSync(lockDir, ahead, ahead)
    expect(acquireConfigLock(configPath, { timeoutMs: 50, sleep: vi.fn() })).toBeNull()
    expect(Math.abs(fs.statSync(lockDir).mtimeMs - Date.now())).toBeLessThan(5_000)
  })

  it('recovers an abandoned lock from the future once the window has passed', () => {
    mkdirSync(lockDir)
    let wall = Date.now()
    const ahead = (wall + STALE_AFTER_MS + 60_000) / 1000
    utimesSync(lockDir, ahead, ahead)
    const sleep = vi.fn(() => {
      wall += STALE_AFTER_MS + 1_000
    })
    const handle = acquireConfigLock(configPath, { timeoutMs: 500, now: () => wall, sleep })
    expect(handle?.held()).toBe(true)
    handle?.release()
    expect(existsSync(lockDir)).toBe(false)
  })
})
