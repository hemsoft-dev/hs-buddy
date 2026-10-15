import * as electron from 'electron'
import Store from 'electron-store'
import { join } from 'node:path'

import { withConfigLock, type LockOptions } from './configLock'
import { ProtectedWeatherLocation } from './services/protectedWeatherLocation'
import { PollenCredentials } from './services/pollenCredentials'
import {
  configSchema,
  defaultConfig,
  type AppConfig,
  type GitHubAccount,
  type UsageProvider,
} from '../src/types/config'
import {
  getUsageProviderOverrideKey,
  type UsageProviderOverrides,
} from '../src/utils/usageProviderOverrides'

/** Shared Convex URL — single source of truth for the main process. */
export const CONVEX_URL =
  import.meta.env.VITE_CONVEX_URL || 'https://balanced-trout-451.convex.cloud'

/**
 * Product-owned account defaults. These seed the durable provider override only
 * while an account has no explicit local or Convex provider selection.
 */
const INITIAL_USAGE_PROVIDER_OVERRIDES: UsageProviderOverrides = {
  'hemsoft/hemsoft': 'codex',
}

type UsageProviderAccountMap = Map<string, GitHubAccount>

function retainConfiguredOverrides(
  overrides: UsageProviderOverrides,
  accountByKey: UsageProviderAccountMap
): UsageProviderOverrides {
  return Object.fromEntries(Object.entries(overrides).filter(([key]) => accountByKey.has(key)))
}

function removeSupersededDefaults(
  accountByKey: UsageProviderAccountMap,
  overrides: UsageProviderOverrides,
  defaultOverrides: UsageProviderOverrides
): void {
  for (const [key, provider] of Object.entries(defaultOverrides)) {
    const account = accountByKey.get(key)
    if (overrides[key] !== provider) {
      delete defaultOverrides[key]
    } else if (account?.usageProvider !== undefined) {
      delete overrides[key]
      delete defaultOverrides[key]
    }
  }
}

function findExplicitProviderOwner(
  provider: UsageProvider,
  accountByKey: UsageProviderAccountMap,
  overrides: UsageProviderOverrides,
  defaultOverrides: UsageProviderOverrides
): string | undefined {
  return [...accountByKey].find(([key, account]) => {
    const override = overrides[key]
    const explicitProvider =
      override !== undefined && defaultOverrides[key] !== override
        ? override
        : account.usageProvider
    return explicitProvider === provider
  })?.[0]
}

function removeConflictingDefaults(
  accountByKey: UsageProviderAccountMap,
  overrides: UsageProviderOverrides,
  defaultOverrides: UsageProviderOverrides
): void {
  const codexOwnerKey = findExplicitProviderOwner(
    'codex',
    accountByKey,
    overrides,
    defaultOverrides
  )
  if (!codexOwnerKey) return
  for (const [key, provider] of Object.entries(defaultOverrides)) {
    if (provider !== 'codex' || key === codexOwnerKey) continue
    delete overrides[key]
    delete defaultOverrides[key]
  }
}

function seedInitialOverrides(
  accountByKey: UsageProviderAccountMap,
  overrides: UsageProviderOverrides,
  defaultOverrides: UsageProviderOverrides
): void {
  for (const [key, provider] of Object.entries(INITIAL_USAGE_PROVIDER_OVERRIDES)) {
    if (!provider) continue
    const account = accountByKey.get(key)
    const hasExplicitOwner = findExplicitProviderOwner(
      provider,
      accountByKey,
      overrides,
      defaultOverrides
    )
    if (
      account &&
      !hasExplicitOwner &&
      account.usageProvider === undefined &&
      overrides[key] === undefined
    ) {
      overrides[key] = provider
      defaultOverrides[key] = provider
    }
  }
}

/**
 * Configuration manager using electron-store for persistent storage
 * Stores in userData/config.json (OS-specific location)
 *
 * SECURITY NOTE: Uses GitHub CLI (gh) for authentication.
 * No tokens are stored in config or environment variables!
 * Authentication is handled securely by GitHub CLI in system keychain.
 * Remembered weather coordinates are stored only as OS-encrypted ciphertext.
 */
/** Where electron-store will put `config.json`, or null outside Electron (tests). */
function expectedConfigPath(): string | null {
  // Some unit tests mock `electron` without `app`; reading a missing export
  // from such a mock throws, so the lookup is guarded as a whole.
  try {
    const electronApp = (electron as { app?: typeof electron.app }).app
    return electronApp ? join(electronApp.getPath('userData'), 'config.json') : null
  } catch (_: unknown) {
    return null
  }
}

class ConfigManager {
  private store: Store<AppConfig>
  private weatherLocation: ProtectedWeatherLocation
  private pollenCredentials: PollenCredentials | undefined

  constructor() {
    // `conf` writes merged defaults during construction when the on-disk
    // object differs, so even initialization is a write that must not race
    // the native app: take the lock on the path the store will use.
    const expectedPath = expectedConfigPath()
    const createStore = (): Store<AppConfig> =>
      new Store<AppConfig>({
        schema: configSchema,
        defaults: defaultConfig,
        name: 'config', // Creates config.json in userData
        clearInvalidConfig: false, // Preserve config even if validation fails
        watch: true, // Watch for external changes
      })
    this.store = expectedPath
      ? withConfigLock(expectedPath, createStore, {
          warn: message => {
            console.warn(message)
          },
        })
      : createStore()
    if (expectedPath && this.store.path !== expectedPath) {
      console.warn(
        `[ConfigManager] store path ${this.store.path} differs from the locked ${expectedPath}`
      )
    }

    const legacyLocation = this.store.get('ui', defaultConfig.ui).weatherLocation ?? null
    // Remove plaintext before any further config writes, including before app ready.
    if (legacyLocation !== null) this.write('ui.weatherLocation', null)
    this.weatherLocation = new ProtectedWeatherLocation(
      {
        read: () => this.store.get('weatherLocationCiphertext', ''),
        write: ciphertext => {
          this.write('weatherLocationCiphertext', ciphertext)
        },
      },
      legacyLocation
    )

    this.store.onDidChange('github.accounts', () => {
      this.reconcileUsageProviderOverrides()
    })
    this.reconcileUsageProviderOverrides()

    console.log('[ConfigManager] Store location:', this.store.path)
  }

  /**
   * Every write goes through the shared `config.json.lock` so the native app
   * (which holds the same lock across its read-modify-write) and this
   * process cannot overwrite each other's changes. `conf` re-reads the file
   * inside `set`, so the lock covers the whole read-modify-write.
   */
  private write<K extends string>(key: K, value: unknown): void {
    // A single `set` is idempotent, so it may be repeated if the lock turns
    // out to have been lost during the write; transactions in `locked`
    // are not replayed (see withConfigLock).
    this.locked(
      () => {
        this.store.set(key, value as never)
      },
      { onLostDuringWrite: 'repeat' }
    )
  }

  private lockDepth = 0

  /**
   * Run a read-modify-write under the shared lock. Nested calls (a mutator
   * calling `write`) reuse the lock already held instead of waiting on it.
   */
  private locked<T>(fn: () => T, options: Pick<LockOptions, 'onLostDuringWrite'> = {}): T {
    if (this.lockDepth > 0) return fn()
    this.lockDepth += 1
    try {
      return withConfigLock(this.store.path, fn, {
        ...options,
        warn: message => {
          console.warn(message)
        },
      })
    } finally {
      this.lockDepth -= 1
    }
  }

  private reconcileUsageProviderOverrides(): UsageProviderOverrides {
    // Read, normalise and persist under one lock so another process's
    // provider selection cannot land between the read and the write.
    return this.locked(() => this.reconcileUsageProviderOverridesLocked())
  }

  private reconcileUsageProviderOverridesLocked(): UsageProviderOverrides {
    const accounts = this.getGitHubAccounts()
    const accountByKey = new Map(
      accounts.map(account => [getUsageProviderOverrideKey(account), account])
    )
    const storedOverrides = this.store.get('github.usageProviderOverrides', {})
    const overrides = retainConfiguredOverrides(storedOverrides, accountByKey)
    const storedDefaultOverrides = this.store.get('github.usageProviderDefaultOverrides', {})
    const defaultOverrides = retainConfiguredOverrides(storedDefaultOverrides, accountByKey)
    removeSupersededDefaults(accountByKey, overrides, defaultOverrides)
    removeConflictingDefaults(accountByKey, overrides, defaultOverrides)
    seedInitialOverrides(accountByKey, overrides, defaultOverrides)

    if (
      JSON.stringify(overrides) !== JSON.stringify(storedOverrides) ||
      JSON.stringify(defaultOverrides) !== JSON.stringify(storedDefaultOverrides)
    ) {
      this.persistUsageProviderState(overrides, defaultOverrides)
    }
    return overrides
  }

  private persistUsageProviderState(
    overrides: UsageProviderOverrides,
    defaultOverrides: UsageProviderOverrides
  ): void {
    this.locked(() => {
      const github = this.store.get('github', defaultConfig.github)
      this.write('github', {
        ...github,
        usageProviderOverrides: overrides,
        usageProviderDefaultOverrides: defaultOverrides,
      })
    })
  }

  // GitHub Account Management
  getGitHubAccounts(): GitHubAccount[] {
    return this.store.get('github.accounts', [])
  }

  hasGitHubAccount(username: string, org: string): boolean {
    const key = getUsageProviderOverrideKey({ username, org })
    return this.getGitHubAccounts().some(account => getUsageProviderOverrideKey(account) === key)
  }

  addGitHubAccount(account: GitHubAccount): void {
    // Read and write under one lock so two processes cannot both read the
    // same list and then serialise two stale replacements.
    this.locked(() => {
      const accounts = this.getGitHubAccounts()
      // Check for duplicates
      const exists = accounts.some(a => a.username === account.username && a.org === account.org)
      if (exists) {
        throw new Error(`GitHub account ${account.username}@${account.org} already exists`)
      }
      accounts.push(account)
      this.write('github.accounts', accounts)
    })
  }

  removeGitHubAccount(username: string, org: string): void {
    this.locked(() => {
      const accounts = this.getGitHubAccounts()
      const filtered = accounts.filter(a => !(a.username === username && a.org === org))
      this.write('github.accounts', filtered)
      this.setUsageProviderOverride(username, org, null)
    })
  }

  updateGitHubAccount(username: string, org: string, updates: Partial<GitHubAccount>): void {
    this.locked(() => {
      const accounts = this.getGitHubAccounts()
      const index = accounts.findIndex(a => a.username === username && a.org === org)
      if (index === -1) {
        throw new Error(`GitHub account ${username}@${org} not found`)
      }
      const previousKey = getUsageProviderOverrideKey(accounts[index])
      accounts[index] = { ...accounts[index], ...updates }
      this.write('github.accounts', accounts)
      if (getUsageProviderOverrideKey(accounts[index]) !== previousKey) {
        this.setUsageProviderOverride(username, org, null)
      }
    })
  }

  replaceGitHubAccounts(accounts: GitHubAccount[]): void {
    const accountKeys = new Set<string>()
    for (const account of accounts) {
      const key = getUsageProviderOverrideKey(account)
      if (accountKeys.has(key)) {
        throw new Error(`Duplicate GitHub account ${account.username}@${account.org}`)
      }
      accountKeys.add(key)
    }

    this.locked(() => {
      this.write('github.accounts', accounts)
      const overrides = Object.fromEntries(
        Object.entries(this.getUsageProviderOverrides()).filter(([key]) => accountKeys.has(key))
      )
      const defaultOverrides = Object.fromEntries(
        Object.entries(this.getUsageProviderDefaultOverrides()).filter(([key]) =>
          accountKeys.has(key)
        )
      )
      this.persistUsageProviderState(overrides, defaultOverrides)
    })
  }

  getUsageProviderOverrides(): UsageProviderOverrides {
    return this.reconcileUsageProviderOverrides()
  }

  getUsageProviderDefaultOverrides(): UsageProviderOverrides {
    this.reconcileUsageProviderOverrides()
    return this.store.get('github.usageProviderDefaultOverrides', {})
  }

  setUsageProviderOverride(username: string, org: string, provider: UsageProvider | null): void {
    this.locked(() => {
      const overrides = { ...this.getUsageProviderOverrides() }
      const key = getUsageProviderOverrideKey({ username, org })
      const defaultOverrides = {
        ...this.store.get('github.usageProviderDefaultOverrides', {}),
      }
      delete defaultOverrides[key]
      if (provider === null) {
        delete overrides[key]
      } else {
        overrides[key] = provider
      }
      this.persistUsageProviderState(overrides, defaultOverrides)
    })
  }

  /** Called after Electron is ready, even when the Weather card is hidden. */
  migrateWeatherLocation(): void {
    this.weatherLocation.persistPending()
  }

  getUiValue<K extends keyof AppConfig['ui']>(key: K): AppConfig['ui'][K] {
    if (key === 'pollenApiKey') return '' as AppConfig['ui'][K]
    if (key === 'weatherLocation') return this.weatherLocation.get() as AppConfig['ui'][K]
    return this.store.get(
      `ui.${key}` as keyof AppConfig,
      defaultConfig.ui[key] as unknown as NonNullable<AppConfig[keyof AppConfig]>
    ) as unknown as AppConfig['ui'][K]
  }

  setUiValue<K extends keyof AppConfig['ui']>(key: K, value: AppConfig['ui'][K]): void {
    if (key === 'pollenApiKey') throw new Error('Pollen keys must use secure storage')
    if (key === 'weatherLocation') {
      this.weatherLocation.set(value as AppConfig['ui']['weatherLocation'])
      return
    }
    this.write(`ui.${key}`, value)
  }

  // Copilot Settings (PR Review Prompt Template — still used via IPC)
  getCopilotPRReviewPromptTemplate(): string {
    return this.store.get('copilot.prReviewPromptTemplate', '')
  }

  setCopilotPRReviewPromptTemplate(template: string): void {
    this.write('copilot.prReviewPromptTemplate', template)
  }

  // Automation Settings
  getScheduleForecastDays(): number {
    return this.store.get('automation.scheduleForecastDays', 3)
  }

  setScheduleForecastDays(days: number): void {
    this.write('automation.scheduleForecastDays', Math.max(1, Math.min(30, days)))
  }

  // Notification Settings
  getNotificationSoundEnabled(): boolean {
    return this.store.get('notifications.playSoundOnReviewComplete', false)
  }

  setNotificationSoundEnabled(enabled: boolean): void {
    this.write('notifications.playSoundOnReviewComplete', enabled)
  }

  getNotificationSoundPath(): string {
    return this.store.get('notifications.reviewCompleteSoundPath', '')
  }

  setNotificationSoundPath(filePath: string): void {
    this.write('notifications.reviewCompleteSoundPath', filePath)
  }

  // Finance Settings
  getFinanceWatchlist(): string[] {
    return this.store.get('finance.watchlist', defaultConfig.finance.watchlist)
  }

  setFinanceWatchlist(symbols: string[]): void {
    // Defensive: ensure array of strings, dedupe, uppercase
    const cleaned = Array.from(
      new Set(
        (Array.isArray(symbols) ? symbols : []).flatMap(s => {
          if (typeof s !== 'string') return []
          const symbol = s.toUpperCase().trim()
          return symbol.length > 0 ? [symbol] : []
        })
      )
    )
    this.write('finance.watchlist', cleaned)
  }

  // Full config access
  getConfig(): AppConfig {
    const config = this.store.store
    return { ...config, ui: { ...config.ui, pollenApiKey: '' } }
  }

  private pollenStore(): PollenCredentials {
    return (this.pollenCredentials ??= new PollenCredentials(this.store.path, {
      read: () => this.store.get('ui.pollenApiKey', ''),
      clear: expected => {
        if (this.store.get('ui.pollenApiKey', '') !== expected) return false
        // PollenCredentials owns the strict shared lock. Re-entering write()
        // here would wait on our own lock. conf.set rereads neighboring fields.
        if (expected) this.store.set('ui.pollenApiKey', '')
        return true
      },
    }))
  }

  /** Main-process provider access only; renderer handlers expose status. */
  getPollenApiKey(): Promise<string | null> {
    return this.pollenStore().load()
  }

  setPollenApiKey(value: string): Promise<void> {
    return this.pollenStore().save(value)
  }

  // Migration helper from environment variables
  migrateFromEnv(): void {
    // Try to read the .env file pattern (legacy support)
    const username = process.env.VITE_GITHUB_USERNAME
    const org = process.env.VITE_GITHUB_ORG

    // Decide and write under one lock: two instances starting together must
    // not both see an empty list and then race to add the same account.
    const migrated = this.locked(() => {
      // Check if we already have accounts - don't overwrite
      if (this.getGitHubAccounts().length > 0) {
        console.log('[ConfigManager] GitHub accounts already configured, skipping migration')
        return false
      }
      if (!username || !org) {
        console.log('[ConfigManager] No environment variables found for migration')
        console.log('[ConfigManager] Add accounts manually through Settings or edit config.json')
        return false
      }
      console.log('[ConfigManager] Migrating from environment variables...')
      this.write('github.accounts', [{ username, org }])
      return true
    })
    if (migrated) {
      console.log('[ConfigManager] Migration complete - now using GitHub CLI authentication')
      console.log(
        '[ConfigManager] You can remove VITE_GITHUB_USERNAME and VITE_GITHUB_ORG from .env (no longer needed)'
      )
    }
  }

  // Utility methods
  getStorePath(): string {
    return this.store.path
  }

  async reset(): Promise<void> {
    await this.setPollenApiKey('')
    this.locked(() => {
      this.weatherLocation.set(null)
      this.store.clear()
    })
    console.log('[ConfigManager] Configuration reset to defaults')
  }
}

// Singleton instance
export const configManager = new ConfigManager()
