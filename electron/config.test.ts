const userDataDir = await vi.hoisted(async () => {
  const { mkdtempSync } = await import('node:fs')
  const { tmpdir } = await import('node:os')
  const { join } = await import('node:path')
  return mkdtempSync(join(tmpdir(), 'buddy-config-test-'))
})

vi.mock('electron', () => ({
  // The config manager takes the shared lock on `<userData>/config.json`
  // while constructing the store; give it a real, writable directory.
  app: { getPath: vi.fn(() => userDataDir) },
  safeStorage: {
    isEncryptionAvailable: vi.fn(() => false),
    getSelectedStorageBackend: vi.fn(() => 'gnome_libsecret'),
    encryptString: vi.fn(() => Buffer.from('opaque-ciphertext')),
    decryptString: vi.fn(),
  },
}))
import { safeStorage } from 'electron'
const protectedPollen = vi.hoisted(() => ({
  load: vi.fn<() => Promise<string | null>>().mockResolvedValue(null),
  save: vi.fn<(value: string) => Promise<void>>().mockResolvedValue(undefined),
  legacy: null as null | { read(): string; clear(expected: string): boolean },
}))
vi.mock('./services/pollenCredentials', () => ({
  PollenCredentials: class {
    constructor(_path: string, legacy: { read(): string; clear(expected: string): boolean }) {
      protectedPollen.legacy = legacy
    }
    load = protectedPollen.load
    save = protectedPollen.save
  },
}))
const configFixture = vi.hoisted(() => ({ initial: {} as Record<string, unknown> }))
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

vi.mock('electron-store', async () => ({
  default: (await import('./__mocks__/configStore')).createConfigStoreMock(configFixture),
}))

vi.mock('../src/types/config', () => ({
  configSchema: {},
  defaultConfig: {
    ui: { theme: 'dark', accentColor: '#0078d4', zoomLevel: 1.0 },
    github: { accounts: [] },
    schedule: { forecastDays: 3 },
    finance: { watchlist: ['AAPL'] },
  },
}))

import { configManager, CONVEX_URL } from './config'

describe('config', () => {
  beforeEach(async () => {
    vi.clearAllMocks()
    vi.mocked(safeStorage.isEncryptionAvailable).mockReturnValue(false)
    await configManager.reset()
  })

  it('exports CONVEX_URL as a string', () => {
    expect(typeof CONVEX_URL).toBe('string')
    expect(CONVEX_URL.length).toBeGreaterThan(0)
  })

  it('routes weather coordinates to protected session storage without plaintext config writes', async () => {
    const location = { latitude: 12.3456, longitude: -65.4321, name: 'Private place' }
    configManager.setUiValue('weatherLocation', location)
    expect(configManager.getUiValue('weatherLocation')).toEqual(location)
    expect(JSON.stringify(configManager.getConfig())).not.toContain('12.3456')
    expect(JSON.stringify(configManager.getConfig())).not.toContain('Private place')
    await configManager.reset()
    expect(configManager.getUiValue('weatherLocation')).toBeNull()
  })

  it('writes only OS-encrypted bytes through the config store', () => {
    vi.mocked(safeStorage.isEncryptionAvailable).mockReturnValue(true)
    const location = { latitude: 12.3456, longitude: -65.4321, name: 'Private place' }
    configManager.setUiValue('weatherLocation', location)
    expect(configManager.getConfig().weatherLocationCiphertext).toBe(
      Buffer.from('opaque-ciphertext').toString('base64')
    )
    expect(JSON.stringify(configManager.getConfig())).not.toContain('Private place')
    expect(safeStorage.encryptString).toHaveBeenCalledWith(JSON.stringify(location))
  })

  it('removes the legacy plaintext field during construction before encryption is ready', async () => {
    const location = { latitude: 12.3456, longitude: -65.4321, name: 'Private place' }
    configFixture.initial = { ui: { weatherLocation: location }, github: { accounts: [] } }
    try {
      vi.resetModules()
      const { configManager: migrated } = await import('./config')
      expect(migrated.getConfig().ui.weatherLocation).toBeNull()
      expect(JSON.stringify(migrated.getConfig())).not.toContain('Private place')
      expect(migrated.getUiValue('weatherLocation')).toEqual(location)
      expect(safeStorage.encryptString).not.toHaveBeenCalled()
    } finally {
      configFixture.initial = {}
    }
  })

  it('persists the legacy location at startup without a Weather card read and restores it after restart', async () => {
    const location = { latitude: 12.3456, longitude: -65.4321, name: 'Private place' }
    configFixture.initial = {
      ui: { weatherLocation: location, dashboardCards: { weather: false } },
      github: { accounts: [] },
    }
    try {
      vi.resetModules()
      const { configManager: migrated } = await import('./config')
      expect(safeStorage.encryptString).not.toHaveBeenCalled()
      vi.mocked(safeStorage.isEncryptionAvailable).mockReturnValue(true)
      migrated.migrateWeatherLocation()
      const persisted = migrated.getConfig()
      expect(persisted.ui.weatherLocation).toBeNull()
      expect(persisted.weatherLocationCiphertext).toBe(
        Buffer.from('opaque-ciphertext').toString('base64')
      )
      expect(JSON.stringify(persisted)).not.toContain('Private place')
      configFixture.initial = structuredClone(persisted) as unknown as Record<string, unknown>
      vi.mocked(safeStorage.decryptString).mockReturnValue(JSON.stringify(location))
      vi.resetModules()
      const { configManager: restarted } = await import('./config')
      expect(restarted.getUiValue('weatherLocation')).toEqual(location)
    } finally {
      configFixture.initial = {}
    }
  })

  it('configManager is defined', () => {
    expect(configManager).toBeDefined()
  })

  it('configManager has expected methods', () => {
    expect(typeof configManager.getGitHubAccounts).toBe('function')
    expect(typeof configManager.addGitHubAccount).toBe('function')
    expect(typeof configManager.removeGitHubAccount).toBe('function')
    expect(typeof configManager.replaceGitHubAccounts).toBe('function')
    expect(typeof configManager.getScheduleForecastDays).toBe('function')
    expect(typeof configManager.setScheduleForecastDays).toBe('function')
  })

  it('getGitHubAccounts returns an array', () => {
    const accounts = configManager.getGitHubAccounts()
    expect(Array.isArray(accounts)).toBe(true)
  })

  describe('addGitHubAccount', () => {
    it('adds a new account', () => {
      configManager.addGitHubAccount({ username: 'user1', org: 'org1' })
      const accounts = configManager.getGitHubAccounts()
      expect(accounts).toEqual([{ username: 'user1', org: 'org1' }])
    })

    it('throws when adding duplicate account', () => {
      configManager.addGitHubAccount({ username: 'user1', org: 'org1' })
      expect(() => configManager.addGitHubAccount({ username: 'user1', org: 'org1' })).toThrow(
        'already exists'
      )
    })
  })

  describe('removeGitHubAccount', () => {
    it('removes an existing account', () => {
      configManager.addGitHubAccount({ username: 'user1', org: 'org1' })
      configManager.removeGitHubAccount('user1', 'org1')
      const accounts = configManager.getGitHubAccounts()
      expect(accounts).toEqual([])
    })

    it('clears the removed account provider override', () => {
      configManager.addGitHubAccount({ username: 'user1', org: 'org1' })
      configManager.setUsageProviderOverride('user1', 'org1', 'codex')

      configManager.removeGitHubAccount('user1', 'org1')

      expect(configManager.getUsageProviderOverrides()).toEqual({})
    })
  })

  describe('updateGitHubAccount', () => {
    it('updates an existing account', () => {
      configManager.addGitHubAccount({ username: 'user1', org: 'org1' })
      configManager.updateGitHubAccount('user1', 'org1', { username: 'user1-updated' })
      const accounts = configManager.getGitHubAccounts()
      expect(accounts[0].username).toBe('user1-updated')
    })

    it('throws when account not found', () => {
      expect(() => configManager.updateGitHubAccount('nouser', 'noorg', {})).toThrow('not found')
    })
  })

  describe('replaceGitHubAccounts', () => {
    it('replaces the durable fallback and removes orphaned overrides', () => {
      configManager.addGitHubAccount({ username: 'old', org: 'org' })
      configManager.setUsageProviderOverride('old', 'org', 'codex')

      configManager.replaceGitHubAccounts([
        { username: 'HemSoft', org: 'HemSoft', usageProvider: 'codex' },
      ])

      expect(configManager.getGitHubAccounts()).toEqual([
        { username: 'HemSoft', org: 'HemSoft', usageProvider: 'codex' },
      ])
      expect(configManager.getUsageProviderOverrides()).toEqual({})
    })

    it('rejects duplicate identities case-insensitively', () => {
      expect(() =>
        configManager.replaceGitHubAccounts([
          { username: 'HemSoft', org: 'HemSoft' },
          { username: 'hemsoft', org: 'hemsoft' },
        ])
      ).toThrow('Duplicate GitHub account')
    })
  })

  describe('usage provider default ownership', () => {
    it('seeds Codex for an unassigned HemSoft account', () => {
      configManager.addGitHubAccount({ username: 'HemSoft', org: 'HemSoft' })

      expect(configManager.getUsageProviderOverrides()).toEqual({ 'hemsoft/hemsoft': 'codex' })
      expect(configManager.getUsageProviderDefaultOverrides()).toEqual({
        'hemsoft/hemsoft': 'codex',
      })
    })

    it('seeds the HemSoft assignment case-insensitively after a connected snapshot', () => {
      configManager.replaceGitHubAccounts([{ username: 'hemsoft', org: 'HEMSOFT' }])

      expect(configManager.getUsageProviderOverrides()).toEqual({ 'hemsoft/hemsoft': 'codex' })
    })

    it('does not replace an explicit HemSoft provider', () => {
      configManager.addGitHubAccount({
        username: 'HemSoft',
        org: 'HemSoft',
        usageProvider: 'copilot',
      })

      expect(configManager.getUsageProviderOverrides()).toEqual({})
      expect(configManager.getUsageProviderDefaultOverrides()).toEqual({})
    })

    it('does not seed HemSoft when another account explicitly owns Codex', () => {
      configManager.replaceGitHubAccounts([
        { username: 'Other', org: 'HemSoft', usageProvider: 'codex' },
        { username: 'HemSoft', org: 'HemSoft' },
      ])

      expect(configManager.getUsageProviderOverrides()).toEqual({})
      expect(configManager.getUsageProviderDefaultOverrides()).toEqual({})
    })

    it('removes the HemSoft seed when another account explicitly owns Codex', () => {
      configManager.addGitHubAccount({ username: 'HemSoft', org: 'HemSoft' })

      configManager.replaceGitHubAccounts([
        { username: 'Other', org: 'HemSoft', usageProvider: 'codex' },
        { username: 'HemSoft', org: 'HemSoft' },
      ])

      expect(configManager.getUsageProviderOverrides()).toEqual({})
      expect(configManager.getUsageProviderDefaultOverrides()).toEqual({})
    })

    it('does not seed HemSoft when another local override owns Codex', () => {
      configManager.addGitHubAccount({ username: 'Other', org: 'HemSoft' })
      configManager.setUsageProviderOverride('Other', 'HemSoft', 'codex')

      configManager.addGitHubAccount({ username: 'HemSoft', org: 'HemSoft' })

      expect(configManager.getUsageProviderOverrides()).toEqual({ 'hemsoft/other': 'codex' })
      expect(configManager.getUsageProviderDefaultOverrides()).toEqual({})
    })

    it('lets a local override release a connected Codex assignment', () => {
      configManager.replaceGitHubAccounts([
        { username: 'Other', org: 'HemSoft', usageProvider: 'codex' },
      ])
      configManager.setUsageProviderOverride('Other', 'HemSoft', 'copilot')

      configManager.addGitHubAccount({ username: 'HemSoft', org: 'HemSoft' })

      expect(configManager.getUsageProviderOverrides()).toEqual({
        'hemsoft/other': 'copilot',
        'hemsoft/hemsoft': 'codex',
      })
    })

    it('removes a seeded default when a connected provider becomes explicit', () => {
      configManager.addGitHubAccount({ username: 'HemSoft', org: 'HemSoft' })

      configManager.replaceGitHubAccounts([
        { username: 'HemSoft', org: 'HemSoft', usageProvider: 'copilot' },
      ])

      expect(configManager.getUsageProviderOverrides()).toEqual({})
      expect(configManager.getUsageProviderDefaultOverrides()).toEqual({})
    })
  })

  describe('usage provider persistence and reconciliation', () => {
    it('keeps a user override when a connected provider disagrees', () => {
      configManager.addGitHubAccount({ username: 'HemSoft', org: 'HemSoft' })
      configManager.setUsageProviderOverride('HemSoft', 'HemSoft', 'codex')

      configManager.replaceGitHubAccounts([
        { username: 'HemSoft', org: 'HemSoft', usageProvider: 'copilot' },
      ])

      expect(configManager.getUsageProviderOverrides()).toEqual({ 'hemsoft/hemsoft': 'codex' })
      expect(configManager.getUsageProviderDefaultOverrides()).toEqual({})
    })

    it('keeps an explicit local Copilot selection for HemSoft', () => {
      configManager.addGitHubAccount({ username: 'HemSoft', org: 'HemSoft' })
      configManager.setUsageProviderOverride('HemSoft', 'HemSoft', 'copilot')

      expect(configManager.getUsageProviderOverrides()).toEqual({ 'hemsoft/hemsoft': 'copilot' })
    })

    it('matches configured accounts case-insensitively', () => {
      configManager.addGitHubAccount({ username: 'HemSoft', org: 'HemSoft' })

      expect(configManager.hasGitHubAccount('hemsoft', 'hemsoft')).toBe(true)
      expect(configManager.hasGitHubAccount('other', 'hemsoft')).toBe(false)
    })

    it('stores an account provider without credentials', () => {
      configManager.addGitHubAccount({ username: 'HemSoft', org: 'HemSoft' })
      configManager.setUsageProviderOverride('HemSoft', 'HemSoft', 'codex')

      expect(configManager.getUsageProviderOverrides()).toEqual({ 'hemsoft/hemsoft': 'codex' })
    })

    it('persists provider and provenance in one GitHub store write', () => {
      configManager.addGitHubAccount({ username: 'HemSoft', org: 'HemSoft' })
      const managerStore = (
        configManager as unknown as {
          store: { writes: string[] }
        }
      ).store
      managerStore.writes = []

      configManager.setUsageProviderOverride('HemSoft', 'HemSoft', 'copilot')

      expect(managerStore.writes).toEqual(['github'])
    })

    it('clears an account provider during Convex reconciliation', () => {
      configManager.setUsageProviderOverride('HemSoft', 'HemSoft', 'codex')
      configManager.setUsageProviderOverride('HemSoft', 'HemSoft', null)

      expect(configManager.getUsageProviderOverrides()).toEqual({})
    })

    it('prunes an override when an external config edit removes its account', () => {
      configManager.addGitHubAccount({ username: 'HemSoft', org: 'HemSoft' })
      configManager.setUsageProviderOverride('HemSoft', 'HemSoft', 'codex')
      const managerStore = (
        configManager as unknown as {
          store: {
            get: (key: string, defaultValue?: unknown) => unknown
            set: (key: string, value: unknown) => void
          }
        }
      ).store

      managerStore.set('github.accounts', [])

      expect(managerStore.get('github.usageProviderOverrides')).toEqual({})
    })
  })

  describe('UI values', () => {
    it('getUiValue returns default when not set', () => {
      const value = configManager.getUiValue('theme')
      expect(value).toBeDefined()
    })

    it('setUiValue and getUiValue roundtrip', () => {
      configManager.setUiValue('theme', 'light')
      expect(configManager.getUiValue('theme')).toBe('light')
    })
  })

  describe('Copilot PR Review Prompt Template', () => {
    it('gets and sets template', () => {
      configManager.setCopilotPRReviewPromptTemplate('my template')
      expect(configManager.getCopilotPRReviewPromptTemplate()).toBe('my template')
    })
  })

  describe('Schedule Forecast Days', () => {
    it('getScheduleForecastDays returns a number', () => {
      const days = configManager.getScheduleForecastDays()
      expect(typeof days).toBe('number')
    })

    it('setScheduleForecastDays clamps to min 1', () => {
      configManager.setScheduleForecastDays(0)
      expect(configManager.getScheduleForecastDays()).toBe(1)
    })

    it('setScheduleForecastDays clamps to max 30', () => {
      configManager.setScheduleForecastDays(100)
      expect(configManager.getScheduleForecastDays()).toBe(30)
    })
  })

  describe('Notification Settings', () => {
    it('gets and sets notification sound enabled', () => {
      configManager.setNotificationSoundEnabled(true)
      expect(configManager.getNotificationSoundEnabled()).toBe(true)
    })

    it('gets and sets notification sound path', () => {
      configManager.setNotificationSoundPath('/path/to/sound.mp3')
      expect(configManager.getNotificationSoundPath()).toBe('/path/to/sound.mp3')
    })
  })

  describe('Finance Watchlist', () => {
    it('gets default watchlist', () => {
      const watchlist = configManager.getFinanceWatchlist()
      expect(Array.isArray(watchlist)).toBe(true)
    })

    it('setFinanceWatchlist deduplicates and uppercases', () => {
      configManager.setFinanceWatchlist(['aapl', 'AAPL', 'goog'])
      expect(configManager.getFinanceWatchlist()).toEqual(['AAPL', 'GOOG'])
    })

    it('setFinanceWatchlist handles non-array gracefully', () => {
      // @ts-expect-error testing defensive behavior
      configManager.setFinanceWatchlist(null)
      expect(configManager.getFinanceWatchlist()).toEqual([])
    })

    it('setFinanceWatchlist filters empty strings', () => {
      configManager.setFinanceWatchlist(['AAPL', '', '  ', 'GOOG'])
      expect(configManager.getFinanceWatchlist()).toEqual(['AAPL', 'GOOG'])
    })
  })

  describe('Full config access', () => {
    it('getConfig returns an object', () => {
      const config = configManager.getConfig()
      expect(config).toBeDefined()
    })

    it('redacts a legacy key from full snapshots and the generic UI getter', async () => {
      configFixture.initial = { ui: { pollenApiKey: 'private-fixture-key', future: 7 } }
      try {
        vi.resetModules()
        const manager = (await import('./config')).configManager
        expect(manager.getUiValue('pollenApiKey')).toBe('')
        expect(manager.getConfig().ui.pollenApiKey).toBe('')
        expect(JSON.stringify(manager.getConfig())).not.toContain('private-fixture-key')
        expect((manager.getConfig().ui as unknown as { future: number }).future).toBe(7)
        expect(() => manager.setUiValue('pollenApiKey', 'new-fixture-key')).toThrow(
          'secure storage'
        )
      } finally {
        configFixture.initial = {}
      }
    })

    it('routes provider reads and saves through the protected module', async () => {
      protectedPollen.load.mockResolvedValueOnce('protected-fixture')
      expect(await configManager.getPollenApiKey()).toBe('protected-fixture')
      await configManager.setPollenApiKey('new-fixture-key')
      expect(protectedPollen.save).toHaveBeenLastCalledWith('new-fixture-key')
      expect(JSON.stringify(configManager.getConfig())).not.toContain('new-fixture-key')
    })

    it('clears only an acknowledged legacy candidate and preserves neighboring fields', async () => {
      configFixture.initial = { ui: { pollenApiKey: 'legacy-fixture', future: 7 } }
      try {
        vi.resetModules()
        const manager = (await import('./config')).configManager
        await manager.getPollenApiKey()
        const legacy = protectedPollen.legacy!
        expect(legacy.read()).toBe('legacy-fixture')
        expect(legacy.clear('stale-fixture')).toBe(false)
        expect(legacy.read()).toBe('legacy-fixture')
        expect(legacy.clear('legacy-fixture')).toBe(true)
        expect(legacy.read()).toBe('')
        expect(legacy.clear('')).toBe(true)
        expect((manager.getConfig().ui as unknown as { future: number }).future).toBe(7)
      } finally {
        configFixture.initial = {}
      }
    })

    it('does not erase settings when secure reset fails', async () => {
      configManager.addGitHubAccount({ username: 'user', org: 'org' })
      protectedPollen.save.mockRejectedValueOnce(new Error('vault unavailable'))
      await expect(configManager.reset()).rejects.toThrow('vault unavailable')
      expect(configManager.getGitHubAccounts()).toEqual([{ username: 'user', org: 'org' }])
    })
  })

  describe('migrateFromEnv', () => {
    afterEach(() => {
      vi.unstubAllEnvs()
    })

    it('skips migration when accounts already exist', () => {
      configManager.addGitHubAccount({ username: 'existing', org: 'org' })
      // Should not throw
      configManager.migrateFromEnv()
    })

    it('migrates from env when no accounts exist and env vars are set', () => {
      vi.stubEnv('VITE_GITHUB_USERNAME', 'envuser')
      vi.stubEnv('VITE_GITHUB_ORG', 'envorg')
      configManager.migrateFromEnv()
      const accounts = configManager.getGitHubAccounts()
      expect(accounts).toEqual([{ username: 'envuser', org: 'envorg' }])
    })

    it('keeps the account another instance migrated first, without a duplicate', () => {
      vi.stubEnv('VITE_GITHUB_USERNAME', 'envuser')
      vi.stubEnv('VITE_GITHUB_ORG', 'envorg')
      configManager.addGitHubAccount({ username: 'envuser', org: 'envorg' })
      expect(() => configManager.migrateFromEnv()).not.toThrow()
      expect(configManager.getGitHubAccounts()).toEqual([{ username: 'envuser', org: 'envorg' }])
    })

    it('handles missing env vars gracefully', () => {
      vi.stubEnv('VITE_GITHUB_USERNAME', '')
      vi.stubEnv('VITE_GITHUB_ORG', '')
      configManager.migrateFromEnv()
      const accounts = configManager.getGitHubAccounts()
      expect(accounts).toEqual([])
    })
  })

  describe('Utility methods', () => {
    it('getStorePath returns a string', () => {
      const storePath = configManager.getStorePath()
      expect(typeof storePath).toBe('string')
    })

    it('reset clears the store', async () => {
      configManager.addGitHubAccount({ username: 'user', org: 'org' })
      await configManager.reset()
      const accounts = configManager.getGitHubAccounts()
      expect(accounts).toEqual([])
    })
  })
})
