import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { acquireConfigLock } from '../configLock'
import {
  createPollenVault,
  credentialAccountForPath,
  PollenCredentials,
  PollenCredentialError,
  pollenCredentialErrorMessage,
  type CredentialVault,
} from './pollenCredentials'

const native = vi.hoisted(() => ({
  construct: vi.fn(),
  target: vi.fn(),
  read: vi.fn(),
  write: vi.fn(),
  remove: vi.fn(),
}))
vi.mock('@napi-rs/keyring', () => ({
  AsyncEntry: class {
    constructor(...args: unknown[]) {
      native.construct(...args)
    }
    static withTarget(...args: unknown[]) {
      native.target(...args)
      return {
        getPassword: native.read,
        setPassword: native.write,
        deleteCredential: native.remove,
      }
    }
    getPassword = native.read
    setPassword = native.write
    deleteCredential = native.remove
  },
}))

const roots: string[] = []
beforeEach(() => {
  vi.resetAllMocks()
})
afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true })
})

function fixture(key = '') {
  const root = mkdtempSync(join(tmpdir(), 'buddy-pollen-fixture-'))
  roots.push(root)
  const path = join(root, 'config.json')
  writeFileSync(path, JSON.stringify({ ui: { pollenApiKey: key, future: 4 }, future: 5 }))
  const read = () =>
    JSON.parse(readFileSync(path, 'utf8')) as {
      ui: { pollenApiKey: string; future: number }
      future: number
    }
  const legacy = {
    read: vi.fn(() => read().ui.pollenApiKey),
    clear: vi.fn((expected: string) => {
      const current = read()
      if (current.ui.pollenApiKey !== expected) return false
      if (expected) {
        current.ui.pollenApiKey = ''
        writeFileSync(path, JSON.stringify(current))
      }
      return true
    }),
  }
  return { path, legacy, read }
}

function vault(initial: string | null = null) {
  let value = initial
  return {
    read: vi.fn(async () => value),
    write: vi.fn(async (next: string) => {
      value = next
    }),
    delete: vi.fn(async () => {
      value = null
    }),
  } satisfies CredentialVault
}

it('migrates only after vault read-back and preserves unrelated fields', async () => {
  const file = fixture('legacy-fixture')
  const store = vault()
  const credentials = new PollenCredentials(file.path, file.legacy, store)
  expect(await credentials.load()).toBe('legacy-fixture')
  expect(await store.read()).toBe('legacy-fixture')
  expect(file.read()).toEqual({ ui: { pollenApiKey: '', future: 4 }, future: 5 })
  expect(readFileSync(file.path, 'utf8')).not.toContain('legacy-fixture')
})

it('reads an existing secure value and reports a missing value without writes', async () => {
  const file = fixture()
  const store = vault('secure-fixture')
  expect(await new PollenCredentials(file.path, file.legacy, store).load()).toBe('secure-fixture')
  const empty = vault()
  expect(await new PollenCredentials(file.path, file.legacy, empty).load()).toBeNull()
  expect(empty.write).not.toHaveBeenCalled()
})

it('keeps recoverable JSON and hides raw vault errors', async () => {
  const file = fixture('legacy-fixture')
  const before = readFileSync(file.path, 'utf8')
  const store = vault()
  store.read.mockRejectedValue(new Error('raw private-fixture-value'))
  await expect(new PollenCredentials(file.path, file.legacy, store).load()).rejects.toEqual(
    new PollenCredentialError('unavailable')
  )
  store.write.mockRejectedValue(new Error('raw submitted-fixture-value'))
  await expect(
    new PollenCredentials(file.path, file.legacy, store).save('submitted-fixture-value')
  ).rejects.toEqual(new PollenCredentialError('unavailable'))
  expect(readFileSync(file.path, 'utf8')).toBe(before)
})

it('saves a new key only in the vault and trims pasted whitespace', async () => {
  const file = fixture('legacy-fixture')
  const store = vault()
  await new PollenCredentials(file.path, file.legacy, store).save('  submitted-fixture  ')
  expect(await store.read()).toBe('submitted-fixture')
  expect(file.read().ui.pollenApiKey).toBe('')
  expect(readFileSync(file.path, 'utf8')).not.toContain('submitted-fixture')
})

it('keeps a clear tombstone so a stale old writer cannot restore the key', async () => {
  const file = fixture('legacy-fixture')
  const store = vault()
  const credentials = new PollenCredentials(file.path, file.legacy, store)
  await credentials.save('')
  writeFileSync(file.path, JSON.stringify({ ui: { pollenApiKey: 'stale-fixture' } }))
  expect(await credentials.load()).toBeNull()
  expect(await store.read()).toBe('')
  expect(file.read().ui.pollenApiKey).toBe('')
})

it('does not discard an ambiguous legacy key or overwrite an existing secure key', async () => {
  const file = fixture('legacy-fixture')
  const store = vault('other-fixture')
  await expect(new PollenCredentials(file.path, file.legacy, store).load()).rejects.toEqual(
    new PollenCredentialError('conflict')
  )
  expect(file.read().ui.pollenApiKey).toBe('legacy-fixture')
  expect(store.write).not.toHaveBeenCalled()
})

it('preserves a concurrent legacy edit and rolls back only its own migration', async () => {
  const file = fixture('old-fixture')
  const store = vault()
  const originalWrite = store.write.getMockImplementation()!
  store.write.mockImplementation(async key => {
    await originalWrite(key)
    writeFileSync(file.path, JSON.stringify({ ui: { pollenApiKey: 'newer-fixture' }, future: 9 }))
  })
  await expect(new PollenCredentials(file.path, file.legacy, store).load()).rejects.toEqual(
    new PollenCredentialError('conflict')
  )
  expect(file.read().ui.pollenApiKey).toBe('newer-fixture')
  expect(await store.read()).toBeNull()
})

it('does not roll back a different vault value written during a conflict', async () => {
  const file = fixture('legacy-fixture')
  const store = vault()
  file.legacy.clear.mockImplementation(() => {
    store.read.mockResolvedValue('newer-secure-fixture')
    return false
  })
  await expect(new PollenCredentials(file.path, file.legacy, store).load()).rejects.toEqual(
    new PollenCredentialError('conflict')
  )
  expect(store.delete).not.toHaveBeenCalled()
})

it('preserves legacy data if rollback becomes unavailable', async () => {
  const file = fixture('legacy-fixture')
  const store = vault()
  file.legacy.clear.mockImplementation(() => {
    store.read.mockRejectedValue(new Error('private fixture error'))
    return false
  })
  await expect(new PollenCredentials(file.path, file.legacy, store).load()).rejects.toEqual(
    new PollenCredentialError('conflict')
  )
  expect(file.read().ui.pollenApiKey).toBe('legacy-fixture')
})

it('does not clear legacy JSON when read-back fails to confirm persistence', async () => {
  const file = fixture('legacy-fixture')
  const store = vault()
  store.write.mockImplementation(async () => {})
  await expect(new PollenCredentials(file.path, file.legacy, store).load()).rejects.toEqual(
    new PollenCredentialError('unavailable')
  )
  expect(file.read().ui.pollenApiKey).toBe('legacy-fixture')
})

it('maps malformed config and config-write failures to safe errors', async () => {
  const file = fixture('legacy-fixture')
  file.legacy.read.mockImplementationOnce(() => {
    throw new Error('private parse text')
  })
  const credentials = new PollenCredentials(file.path, file.legacy, vault())
  await expect(credentials.load()).rejects.toEqual(new PollenCredentialError('configuration'))
  file.legacy.read.mockReturnValueOnce(42 as never)
  await expect(credentials.load()).rejects.toEqual(new PollenCredentialError('configuration'))
  file.legacy.clear.mockImplementation(() => {
    throw new Error('private write text')
  })
  await expect(credentials.save('new-fixture')).rejects.toEqual(
    new PollenCredentialError('configuration')
  )
  expect(file.read().ui.pollenApiKey).toBe('legacy-fixture')
})

it('rejects invalid input before touching the vault or acquiring a lock', async () => {
  const file = fixture()
  const store = vault()
  const credentials = new PollenCredentials(file.path, file.legacy, store)
  for (const value of ['a\nb', 'a\u0085b', 'x'.repeat(4097), 'é'.repeat(2049), 42 as never]) {
    await expect(credentials.save(value)).rejects.toEqual(new PollenCredentialError('invalid'))
  }
  expect(store.write).not.toHaveBeenCalled()
  expect(existsSync(`${file.path}.lock`)).toBe(false)
})

it('fails closed on a busy lock and on ownership lost during a vault write', async () => {
  const file = fixture('legacy-fixture')
  const held = acquireConfigLock(file.path)
  expect(held).not.toBeNull()
  try {
    await expect(
      new PollenCredentials(file.path, file.legacy, vault(), { timeoutMs: 0 }).load()
    ).rejects.toEqual(new PollenCredentialError('busy'))
  } finally {
    held?.release()
  }
  const store = vault()
  const originalWrite = store.write.getMockImplementation()!
  store.write.mockImplementation(async key => {
    await originalWrite(key)
    mkdirSync(join(`${file.path}.lock`, 'claim'))
  })
  await expect(
    new PollenCredentials(file.path, file.legacy, store).save('new-fixture')
  ).rejects.toEqual(new PollenCredentialError('busy'))
  expect(file.read().ui.pollenApiKey).toBe('legacy-fixture')
})

it('pins Linux to persistent Secret Service and never passes a Windows explicit target', async () => {
  const file = fixture()
  native.read.mockResolvedValue('native-fixture')
  native.write.mockResolvedValue(undefined)
  native.remove.mockResolvedValue(true)
  const windows = createPollenVault(file.path, 'win32')
  expect(native.target).not.toHaveBeenCalled()
  expect(native.construct).toHaveBeenCalledWith(
    'hs-buddy-pollen-v1',
    credentialAccountForPath(file.path, 'win32')
  )
  expect(await windows.read()).toBe('native-fixture')
  await windows.write('next-fixture')
  await windows.delete()
  const linux = createPollenVault(file.path, 'linux')
  expect(native.target).toHaveBeenCalledWith(
    'default',
    'hs-buddy-pollen-v1',
    credentialAccountForPath(file.path, 'linux'),
    { linux: { store: 'secret-service' } }
  )
  expect(await linux.read()).toBe('native-fixture')
})

it('constructs the native adapter lazily and redacts constructor failures', async () => {
  const file = fixture()
  const credentials = new PollenCredentials(file.path, file.legacy)
  expect(native.construct).not.toHaveBeenCalled()
  native.construct.mockImplementation(() => {
    throw new Error('private constructor text')
  })
  native.target.mockImplementation(() => {
    throw new Error('private constructor text')
  })
  await expect(credentials.load()).rejects.toEqual(new PollenCredentialError('unavailable'))
})

it('separates profiles and matches Windows path case and extended-prefix variants', () => {
  const plain = credentialAccountForPath('C:\\Users\\Test\\Buddy\\config.json', 'win32')
  expect(plain).toBe(credentialAccountForPath('c:/users/test/buddy/config.json', 'win32'))
  expect(plain).toBe(
    credentialAccountForPath('\\\\?\\C:\\Users\\Test\\Buddy\\config.json', 'win32')
  )
  expect(credentialAccountForPath('/profiles/one/config.json', 'linux')).not.toBe(
    credentialAccountForPath('/profiles/two/config.json', 'linux')
  )
  expect(credentialAccountForPath('/Profiles/config.json', 'linux')).not.toBe(
    credentialAccountForPath('/profiles/config.json', 'linux')
  )
})

it('maps only known credential codes to public error text', () => {
  const known = new PollenCredentialError('conflict')
  known.message = 'private-fixture-key'
  expect(pollenCredentialErrorMessage(known)).toContain('Save your preferred key again')
  expect(pollenCredentialErrorMessage(known)).not.toContain('private-fixture-key')
  expect(pollenCredentialErrorMessage(new Error('private-fixture-key'))).toContain(
    'Unlock your OS credential store'
  )
})
