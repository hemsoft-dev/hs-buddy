import { createHash } from 'node:crypto'
import { posix, win32 } from 'node:path'
import { AsyncEntry } from '@napi-rs/keyring'
import { acquireConfigLock, type ConfigLockHandle, type LockOptions } from '../configLock'

const POLLEN_CREDENTIAL_SERVICE = 'hs-buddy-pollen-v1'

/** Must match buddy-core/pollen_credentials.rs, including ASCII-only case folding. */
export function credentialAccountForPath(configPath: string, platform = process.platform): string {
  let normalized = platform === 'win32' ? win32.resolve(configPath) : posix.resolve(configPath)
  if (platform === 'win32') {
    normalized = normalized.startsWith('\\\\?\\UNC\\')
      ? `//${normalized.slice(8)}`
      : normalized.replace(/^\\\\\?\\/, '')
    normalized = normalized.replace(/\\/g, '/').replace(/[A-Z]/g, letter => letter.toLowerCase())
  }
  return `pollen-${createHash('sha256').update(normalized, 'utf8').digest('hex')}`
}

type CredentialErrorCode = 'unavailable' | 'busy' | 'configuration' | 'conflict' | 'invalid'
const ERROR_MESSAGES: Record<CredentialErrorCode, string> = {
  unavailable: 'Secure storage is unavailable. Unlock your OS credential store and try again.',
  busy: 'Configuration is busy. Try again after the other Buddy instance finishes saving.',
  configuration:
    'The configuration could not be read or updated. Check the file and its permissions.',
  conflict:
    'The legacy key changed or differs from secure storage. Save your preferred key again to finish migration.',
  invalid: 'Enter an API key without control characters, up to 4096 bytes.',
}

export class PollenCredentialError extends Error {
  constructor(readonly code: CredentialErrorCode) {
    super(ERROR_MESSAGES[code])
    this.name = 'PollenCredentialError'
  }
}

export function pollenCredentialErrorMessage(error: unknown): string {
  return error instanceof PollenCredentialError
    ? ERROR_MESSAGES[error.code]
    : ERROR_MESSAGES.unavailable
}

export interface CredentialVault {
  read(): Promise<string | null>
  write(value: string): Promise<void>
  delete(): Promise<void>
}

export interface LegacyPollenConfig {
  read(): string
  /** Compare the current candidate before clearing; preserve neighboring config fields. */
  clear(expected: string): boolean
}

export function createPollenVault(
  configPath: string,
  platform = process.platform
): CredentialVault {
  const account = credentialAccountForPath(configPath, platform)
  // An explicit Windows target writes an empty secret in this binding's
  // constructor. Never use that constructor for Windows reads.
  const entry =
    platform === 'linux'
      ? AsyncEntry.withTarget('default', POLLEN_CREDENTIAL_SERVICE, account, {
          linux: { store: 'secret-service' },
        })
      : new AsyncEntry(POLLEN_CREDENTIAL_SERVICE, account)
  return {
    read: () => entry.getPassword(),
    write: value => entry.setPassword(value),
    delete: async () => {
      await entry.deleteCredential()
    },
  }
}

/** Internal main-process access only. Renderer callers receive configured status. */
export class PollenCredentials {
  private vault: CredentialVault | undefined

  constructor(
    private readonly configPath: string,
    private readonly legacy: LegacyPollenConfig,
    vault?: CredentialVault,
    private readonly lockOptions: LockOptions = {}
  ) {
    this.vault = vault
  }

  private entry(): CredentialVault {
    try {
      return (this.vault ??= createPollenVault(this.configPath))
    } catch (_: unknown) {
      throw new PollenCredentialError('unavailable')
    }
  }

  private async readVault(): Promise<string | null> {
    try {
      return await this.entry().read()
    } catch (_: unknown) {
      throw new PollenCredentialError('unavailable')
    }
  }

  private async writeVault(value: string): Promise<void> {
    try {
      await this.entry().write(value)
    } catch (_: unknown) {
      throw new PollenCredentialError('unavailable')
    }
    if ((await this.readVault()) !== value) throw new PollenCredentialError('unavailable')
  }

  private readLegacy(): string {
    try {
      const value = this.legacy.read()
      if (typeof value !== 'string') throw new Error('Invalid legacy value')
      return value
    } catch (_: unknown) {
      throw new PollenCredentialError('configuration')
    }
  }

  private clearLegacy(candidate: string, lock: ConfigLockHandle): void {
    if (!lock.held()) throw new PollenCredentialError('busy')
    try {
      if (!this.legacy.clear(candidate)) throw new PollenCredentialError('conflict')
    } catch (error: unknown) {
      if (error instanceof PollenCredentialError) throw error
      throw new PollenCredentialError('configuration')
    }
  }

  private async withLock<T>(operation: (lock: ConfigLockHandle) => Promise<T>): Promise<T> {
    let lock: ConfigLockHandle | null
    try {
      lock = acquireConfigLock(this.configPath, this.lockOptions)
    } catch (_: unknown) {
      throw new PollenCredentialError('busy')
    }
    if (!lock) throw new PollenCredentialError('busy')
    try {
      if (!lock.held()) throw new PollenCredentialError('busy')
      return await operation(lock)
    } finally {
      lock.release()
    }
  }

  load(): Promise<string | null> {
    return this.withLock(async lock => {
      const candidate = this.readLegacy()
      const stored = await this.readVault()
      if (stored !== null) {
        if (candidate && stored && candidate !== stored) throw new PollenCredentialError('conflict')
        this.clearLegacy(candidate, lock)
        return stored || null
      }
      if (!candidate) return null
      this.validate(candidate)
      if (!lock.held()) throw new PollenCredentialError('busy')
      await this.writeVault(candidate)
      try {
        this.clearLegacy(candidate, lock)
      } catch (error: unknown) {
        await this.rollbackMigration(candidate, lock)
        throw error
      }
      return candidate
    })
  }

  private async rollbackMigration(candidate: string, lock: ConfigLockHandle): Promise<void> {
    try {
      if (lock.held() && (await this.readVault()) === candidate) await this.entry().delete()
    } catch (_: unknown) {
      /* Keep recoverable legacy bytes if vault rollback is unavailable. */
    }
  }

  async save(value: string): Promise<void> {
    if (typeof value !== 'string') throw new PollenCredentialError('invalid')
    const key = value.trim()
    this.validate(key)
    await this.withLock(async lock => {
      const candidate = this.readLegacy()
      if (!lock.held()) throw new PollenCredentialError('busy')
      // An empty protected entry is a durable clear tombstone. A failed JSON
      // cleanup or stale old writer must not resurrect a cleared key.
      await this.writeVault(key)
      this.clearLegacy(candidate, lock)
    })
  }

  private validate(value: string): void {
    const hasControl = Array.from(value).some(character => {
      const code = character.codePointAt(0) ?? 0
      return code <= 31 || (code >= 127 && code <= 159)
    })
    if (Buffer.byteLength(value, 'utf8') > 4096 || hasControl) {
      throw new PollenCredentialError('invalid')
    }
  }
}
