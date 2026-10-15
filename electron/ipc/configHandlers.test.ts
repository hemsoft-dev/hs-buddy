import { describe, it, expect, vi, beforeEach } from 'vitest'

vi.mock('electron', () => ({
  dialog: {
    showOpenDialog: vi.fn().mockResolvedValue({ canceled: true, filePaths: [] }),
  },
  ipcMain: {
    handle: vi.fn(),
  },
  shell: {
    openPath: vi.fn(),
  },
}))

vi.mock('../services/fileSnapshots', () => ({
  readFileSnapshot: vi.fn(),
}))

vi.mock('../config', () => ({
  configManager: {
    getUiValue: vi.fn(),
    setUiValue: vi.fn(),
    getPollenApiKey: vi.fn().mockResolvedValue(null),
    setPollenApiKey: vi.fn().mockResolvedValue(undefined),
    getScheduleForecastDays: vi.fn(() => 3),
    setScheduleForecastDays: vi.fn(),
    getCopilotPRReviewPromptTemplate: vi.fn(() => ''),
    setCopilotPRReviewPromptTemplate: vi.fn(),
    getNotificationSoundEnabled: vi.fn(() => false),
    setNotificationSoundEnabled: vi.fn(),
    getNotificationSoundPath: vi.fn(() => ''),
    setNotificationSoundPath: vi.fn(),
    getFinanceWatchlist: vi.fn(() => ['AAPL']),
    setFinanceWatchlist: vi.fn(),
    getConfig: vi.fn(() => ({})),
    getStorePath: vi.fn(() => '/mock/config.json'),
    reset: vi.fn(),
  },
}))

vi.mock('../../src/utils/notificationSound', () => ({
  isSupportedNotificationSoundPath: vi.fn((p: string) => p.endsWith('.mp3') || p.endsWith('.wav')),
  getNotificationSoundMimeType: vi.fn(() => 'audio/mpeg'),
  MAX_NOTIFICATION_SOUND_BYTES: 10_000_000,
}))

vi.mock('../../src/ipc/contracts', async importOriginal => {
  const actual = await importOriginal<typeof import('../../src/ipc/contracts')>()
  return {
    ...actual,
    CONFIG_UI_KEYS: ['theme', 'accent-color', 'zoom-level', 'enterprise-slug'] as const,
  }
})

import { ipcMain, dialog, shell } from 'electron'
import { registerConfigHandlers } from './configHandlers'
import { configManager } from '../config'
import { readFileSnapshot } from '../services/fileSnapshots'

const mockConfigManager = vi.mocked(configManager)

describe('configHandlers', () => {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  let handlers: Map<string, (...args: any[]) => any>

  beforeEach(() => {
    vi.clearAllMocks()
    handlers = new Map()
    vi.mocked(ipcMain.handle).mockImplementation((channel, handler) => {
      handlers.set(channel, handler)
    })
    registerConfigHandlers()
  })

  it('registers UI value channels from CONFIG_UI_KEYS', () => {
    expect(handlers.has('config:get-theme')).toBe(true)
    expect(handlers.has('config:set-theme')).toBe(true)
    expect(handlers.has('config:get-accent-color')).toBe(true)
    expect(handlers.has('config:set-accent-color')).toBe(true)
    expect(handlers.has('config:get-zoom-level')).toBe(true)
    expect(handlers.has('config:set-zoom-level')).toBe(true)
  })

  it('registers assistant-open handlers', () => {
    expect(handlers.has('config:get-assistant-open')).toBe(true)
    expect(handlers.has('config:set-assistant-open')).toBe(true)
  })

  it('registers terminal handlers', () => {
    expect(handlers.has('config:get-terminal-open')).toBe(true)
    expect(handlers.has('config:set-terminal-open')).toBe(true)
    expect(handlers.has('config:get-terminal-panel-height')).toBe(true)
    expect(handlers.has('config:set-terminal-panel-height')).toBe(true)
  })

  it('registers notification handlers', () => {
    expect(handlers.has('config:get-notification-sound-enabled')).toBe(true)
    expect(handlers.has('config:set-notification-sound-enabled')).toBe(true)
    expect(handlers.has('config:get-notification-sound-path')).toBe(true)
    expect(handlers.has('config:set-notification-sound-path')).toBe(true)
    expect(handlers.has('config:pick-audio-file')).toBe(true)
    expect(handlers.has('config:play-notification-sound')).toBe(true)
  })

  it('registers full config handlers', () => {
    expect(handlers.has('config:get-config')).toBe(true)
    expect(handlers.has('config:get-store-path')).toBe(true)
    expect(handlers.has('config:open-in-editor')).toBe(true)
    expect(handlers.has('config:reset')).toBe(true)
  })

  describe('config:get-assistant-open', () => {
    it('returns the assistantOpen value', () => {
      mockConfigManager.getUiValue.mockReturnValue(true)
      const result = handlers.get('config:get-assistant-open')!()
      expect(mockConfigManager.getUiValue).toHaveBeenCalledWith('assistantOpen')
      expect(result).toBe(true)
    })
  })

  describe('config:set-assistant-open', () => {
    it('sets the value and returns success', () => {
      const result = handlers.get('config:set-assistant-open')!({}, true)
      expect(mockConfigManager.setUiValue).toHaveBeenCalledWith('assistantOpen', true)
      expect(result).toEqual({ success: true })
    })

    it('rejects non-boolean values', () => {
      const result = handlers.get('config:set-assistant-open')!({}, 'not-boolean')
      expect(result).toEqual({ success: false })
      expect(mockConfigManager.setUiValue).not.toHaveBeenCalled()
    })
  })

  describe('config:set-terminal-panel-height', () => {
    it('accepts valid height', () => {
      const result = handlers.get('config:set-terminal-panel-height')!({}, 300)
      expect(result).toEqual({ success: true })
      expect(mockConfigManager.setUiValue).toHaveBeenCalledWith('terminalPanelHeight', 300)
    })

    it('rejects height below minimum', () => {
      const result = handlers.get('config:set-terminal-panel-height')!({}, 50)
      expect(result).toEqual({ success: false })
    })

    it('rejects height above maximum', () => {
      const result = handlers.get('config:set-terminal-panel-height')!({}, 1500)
      expect(result).toEqual({ success: false })
    })

    it('rejects NaN', () => {
      const result = handlers.get('config:set-terminal-panel-height')!({}, NaN)
      expect(result).toEqual({ success: false })
    })
  })

  describe('config:set-notification-sound-enabled', () => {
    it('sets enabled to true', () => {
      const result = handlers.get('config:set-notification-sound-enabled')!({}, true)
      expect(result).toEqual({ success: true })
      expect(mockConfigManager.setNotificationSoundEnabled).toHaveBeenCalledWith(true)
    })

    it('rejects non-boolean', () => {
      const result = handlers.get('config:set-notification-sound-enabled')!({}, 'yes')
      expect(result).toEqual({ success: false })
    })
  })

  describe('config:set-notification-sound-path', () => {
    it('clears path when empty string provided', () => {
      const result = handlers.get('config:set-notification-sound-path')!({}, '  ')
      expect(result).toEqual({ success: true })
      expect(mockConfigManager.setNotificationSoundPath).toHaveBeenCalledWith('')
    })

    it('rejects unsupported file extension', () => {
      const result = handlers.get('config:set-notification-sound-path')!({}, '/some/file.exe')
      expect(result).toEqual({ success: false })
    })

    it('rejects non-string value', () => {
      const result = handlers.get('config:set-notification-sound-path')!({}, 123)
      expect(result).toEqual({ success: false })
    })
  })

  describe('config:pick-audio-file', () => {
    it('returns canceled when dialog is dismissed', async () => {
      vi.mocked(dialog.showOpenDialog).mockResolvedValue({
        canceled: true,
        filePaths: [],
      })
      const result = await handlers.get('config:pick-audio-file')!()
      expect(result).toEqual({ success: false, canceled: true })
    })

    it('returns success with filePath for valid audio file', async () => {
      vi.mocked(dialog.showOpenDialog).mockResolvedValue({
        canceled: false,
        filePaths: ['/path/to/sound.mp3'],
      })
      const result = await handlers.get('config:pick-audio-file')!()
      expect(result).toEqual({ success: true, filePath: '/path/to/sound.mp3' })
    })
  })

  describe('config:set-finance-watchlist', () => {
    it('sets valid watchlist', () => {
      const result = handlers.get('config:set-finance-watchlist')!({}, ['AAPL', 'GOOG'])
      expect(result).toEqual({ success: true })
      expect(mockConfigManager.setFinanceWatchlist).toHaveBeenCalledWith(['AAPL', 'GOOG'])
    })

    it('rejects non-array', () => {
      const result = handlers.get('config:set-finance-watchlist')!({}, 'AAPL')
      expect(result).toEqual({ success: false })
    })

    it('rejects array with non-string elements', () => {
      const result = handlers.get('config:set-finance-watchlist')!({}, [1, 2, 3])
      expect(result).toEqual({ success: false })
    })
  })

  describe('config:get-config', () => {
    it('returns the full config', () => {
      const mockConfig = { ui: { theme: 'dark' } }
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      mockConfigManager.getConfig.mockReturnValue(mockConfig as any)
      const result = handlers.get('config:get-config')!()
      expect(result).toEqual(mockConfig)
    })
  })

  describe('config:reset', () => {
    it('calls configManager.reset and returns success', async () => {
      const result = await handlers.get('config:reset')!()
      expect(mockConfigManager.reset).toHaveBeenCalled()
      expect(result).toEqual({ success: true })
    })
  })

  describe('secure pollen credentials', () => {
    it('returns configured status without exposing the saved key', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValueOnce('private-fixture-key')
      expect(await handlers.get('config:get-pollen-api-key')!()).toEqual({ configured: true })
      expect(mockConfigManager.getUiValue).not.toHaveBeenCalledWith('pollenApiKey')
    })

    it('returns unconfigured status for an empty vault', async () => {
      mockConfigManager.getPollenApiKey.mockResolvedValueOnce(null)
      expect(await handlers.get('config:get-pollen-api-key')!()).toEqual({ configured: false })
    })

    it('hides raw vault failures on both getter and setter', async () => {
      mockConfigManager.getPollenApiKey.mockRejectedValueOnce(new Error('private-fixture-key'))
      const status = await handlers.get('config:get-pollen-api-key')!()
      expect(status.configured).toBe(false)
      expect(status.error).toContain('Unlock your OS credential store')
      expect(status.error).not.toContain('private-fixture-key')
      mockConfigManager.setPollenApiKey.mockRejectedValueOnce(new Error('private-fixture-key'))
      const result = await handlers.get('config:set-pollen-api-key')!({}, 'private-fixture-key')
      expect(result.success).toBe(false)
      expect(result.error).not.toContain('private-fixture-key')
    })

    it('uses the secure setter for save and clear, and rejects non-text input', async () => {
      const set = handlers.get('config:set-pollen-api-key')!
      expect(await set({}, 'fixture-key')).toEqual({ success: true })
      expect(mockConfigManager.setPollenApiKey).toHaveBeenLastCalledWith('fixture-key')
      expect(await set({}, '')).toEqual({ success: true })
      expect(mockConfigManager.setPollenApiKey).toHaveBeenLastCalledWith('')
      expect(await set({}, 42)).toEqual({ success: false, error: 'API key must be text.' })
      expect(mockConfigManager.setPollenApiKey).toHaveBeenCalledTimes(2)
      expect(mockConfigManager.setUiValue).not.toHaveBeenCalled()
    })
  })

  describe('config:open-in-editor', () => {
    it('opens the store path and returns success', () => {
      mockConfigManager.getStorePath.mockReturnValue('/mock/config.json')
      const result = handlers.get('config:open-in-editor')!()
      expect(shell.openPath).toHaveBeenCalledWith('/mock/config.json')
      expect(result).toEqual({ success: true })
    })
  })

  describe('config:get-schedule-forecast-days', () => {
    it('returns forecast days', () => {
      const result = handlers.get('config:get-schedule-forecast-days')!()
      expect(mockConfigManager.getScheduleForecastDays).toHaveBeenCalled()
      expect(result).toBe(3)
    })
  })

  describe('config:set-schedule-forecast-days', () => {
    it('sets the forecast days and returns success', () => {
      const result = handlers.get('config:set-schedule-forecast-days')!({}, 7)
      expect(mockConfigManager.setScheduleForecastDays).toHaveBeenCalledWith(7)
      expect(result).toEqual({ success: true })
    })
  })

  describe('config:get-copilot-pr-review-prompt-template', () => {
    it('returns the template', () => {
      const result = handlers.get('config:get-copilot-pr-review-prompt-template')!()
      expect(mockConfigManager.getCopilotPRReviewPromptTemplate).toHaveBeenCalled()
      expect(result).toBe('')
    })
  })

  describe('config:set-copilot-pr-review-prompt-template', () => {
    it('sets the template and returns success', () => {
      const result = handlers.get('config:set-copilot-pr-review-prompt-template')!(
        {},
        'new template'
      )
      expect(mockConfigManager.setCopilotPRReviewPromptTemplate).toHaveBeenCalledWith(
        'new template'
      )
      expect(result).toEqual({ success: true })
    })
  })

  describe('config:get-notification-sound-enabled', () => {
    it('returns the enabled state', () => {
      const result = handlers.get('config:get-notification-sound-enabled')!()
      expect(mockConfigManager.getNotificationSoundEnabled).toHaveBeenCalled()
      expect(result).toBe(false)
    })
  })

  describe('config:get-notification-sound-path', () => {
    it('returns the sound path', () => {
      const result = handlers.get('config:get-notification-sound-path')!()
      expect(mockConfigManager.getNotificationSoundPath).toHaveBeenCalled()
      expect(result).toBe('')
    })
  })

  describe('config:set-notification-sound-path (pick flow)', () => {
    it('accepts path that was previously picked', async () => {
      // Pick a file first
      vi.mocked(dialog.showOpenDialog).mockResolvedValue({
        canceled: false,
        filePaths: ['/path/to/sound.mp3'],
      })
      await handlers.get('config:pick-audio-file')!()

      // Now set the path (should match lastPickedNotificationSoundPath)
      const result = handlers.get('config:set-notification-sound-path')!({}, '/path/to/sound.mp3')
      expect(result).toEqual({ success: true })
      expect(mockConfigManager.setNotificationSoundPath).toHaveBeenCalledWith('/path/to/sound.mp3')
    })

    it('rejects path that was not previously picked', () => {
      const result = handlers.get('config:set-notification-sound-path')!({}, '/unpicked/sound.mp3')
      expect(result).toEqual({ success: false })
    })
  })

  describe('config:pick-audio-file (unsupported)', () => {
    it('rejects unsupported audio file extension', async () => {
      vi.mocked(dialog.showOpenDialog).mockResolvedValue({
        canceled: false,
        filePaths: ['/path/to/video.avi'],
      })
      const result = await handlers.get('config:pick-audio-file')!()
      expect(result).toEqual({ success: false })
    })
  })

  describe('config:play-notification-sound', () => {
    it('returns null when no sound path configured', async () => {
      mockConfigManager.getNotificationSoundPath.mockReturnValue('')
      const result = await handlers.get('config:play-notification-sound')!()
      expect(result).toBeNull()
    })

    it('returns base64 audio data when valid sound file exists', async () => {
      mockConfigManager.getNotificationSoundPath.mockReturnValue('/sounds/alert.mp3')
      vi.mocked(readFileSnapshot).mockResolvedValueOnce({
        data: Buffer.from('fake-audio-data'),
        stats: { size: 1024 } as never,
      })

      const result = await handlers.get('config:play-notification-sound')!()
      expect(result).toEqual({
        base64: Buffer.from('fake-audio-data').toString('base64'),
        mimeType: 'audio/mpeg',
      })
    })

    it('returns null when file is too large', async () => {
      mockConfigManager.getNotificationSoundPath.mockReturnValue('/sounds/alert.mp3')
      vi.mocked(readFileSnapshot).mockRejectedValueOnce(new Error('File exceeds the read limit'))

      const result = await handlers.get('config:play-notification-sound')!()
      expect(result).toBeNull()
    })

    it('returns null when the file cannot be opened', async () => {
      mockConfigManager.getNotificationSoundPath.mockReturnValue('/sounds/missing.mp3')
      vi.mocked(readFileSnapshot).mockRejectedValueOnce(new Error('ENOENT'))

      const result = await handlers.get('config:play-notification-sound')!()
      expect(result).toBeNull()
    })

    it('returns null when path is not a regular file', async () => {
      mockConfigManager.getNotificationSoundPath.mockReturnValue('/sounds/dir.mp3')
      vi.mocked(readFileSnapshot).mockRejectedValueOnce(new Error('Not a regular file'))

      const result = await handlers.get('config:play-notification-sound')!()
      expect(result).toBeNull()
    })
  })

  describe('config:get-store-path', () => {
    it('returns the store path', () => {
      const result = handlers.get('config:get-store-path')!()
      expect(result).toBe('/mock/config.json')
    })
  })

  describe('config:get-finance-watchlist', () => {
    it('returns the finance watchlist', () => {
      const result = handlers.get('config:get-finance-watchlist')!()
      expect(mockConfigManager.getFinanceWatchlist).toHaveBeenCalled()
      expect(result).toEqual(['AAPL'])
    })
  })

  describe('UI value handlers', () => {
    it('config:get-theme returns theme value', () => {
      mockConfigManager.getUiValue.mockReturnValue('dark')
      const result = handlers.get('config:get-theme')!()
      expect(result).toBe('dark')
    })

    it('config:set-theme sets theme value', () => {
      const result = handlers.get('config:set-theme')!({}, 'light')
      expect(mockConfigManager.setUiValue).toHaveBeenCalledWith('theme', 'light')
      expect(result).toEqual({ success: true })
    })

    it('config:get-zoom-level returns zoom level', () => {
      mockConfigManager.getUiValue.mockReturnValue(1.2)
      const result = handlers.get('config:get-zoom-level')!()
      expect(result).toBe(1.2)
    })

    it('config:set-zoom-level sets zoom level', () => {
      const result = handlers.get('config:set-zoom-level')!({}, 1.5)
      expect(mockConfigManager.setUiValue).toHaveBeenCalledWith('zoomLevel', 1.5)
      expect(result).toEqual({ success: true })
    })

    it('config:get-enterprise-slug returns enterprise slug', () => {
      mockConfigManager.getUiValue.mockReturnValue('example-enterprise')
      const result = handlers.get('config:get-enterprise-slug')!()
      expect(mockConfigManager.getUiValue).toHaveBeenCalledWith('enterpriseSlug')
      expect(result).toBe('example-enterprise')
    })

    it('config:set-enterprise-slug validates, trims, and sets the enterprise slug', () => {
      const result = handlers.get('config:set-enterprise-slug')!({}, ' example-enterprise ')
      expect(mockConfigManager.setUiValue).toHaveBeenCalledWith(
        'enterpriseSlug',
        'example-enterprise'
      )
      expect(result).toEqual({ success: true })
    })

    it('config:set-enterprise-slug rejects invalid slugs', () => {
      const result = handlers.get('config:set-enterprise-slug')!({}, 'bad;slug')
      expect(result).toEqual({
        success: false,
        error: "Invalid GitHub account slug: 'bad;slug'",
      })
      expect(mockConfigManager.setUiValue).not.toHaveBeenCalled()
    })

    it('config:set-enterprise-slug rejects non-string values', () => {
      const result = handlers.get('config:set-enterprise-slug')!({}, 42)
      expect(result).toEqual({ success: false, error: 'Enterprise slug must be a string' })
      expect(mockConfigManager.setUiValue).not.toHaveBeenCalled()
    })
  })

  describe('config:get-terminal-open', () => {
    it('returns the terminal open state', () => {
      mockConfigManager.getUiValue.mockReturnValue(false)
      const result = handlers.get('config:get-terminal-open')!()
      expect(mockConfigManager.getUiValue).toHaveBeenCalledWith('terminalOpen')
      expect(result).toBe(false)
    })
  })

  describe('config:set-terminal-open', () => {
    it('rejects non-boolean', () => {
      const result = handlers.get('config:set-terminal-open')!({}, 'yes')
      expect(result).toEqual({ success: false })
    })

    it('sets valid boolean value', () => {
      const result = handlers.get('config:set-terminal-open')!({}, true)
      expect(result).toEqual({ success: true })
      expect(mockConfigManager.setUiValue).toHaveBeenCalledWith('terminalOpen', true)
    })
  })

  describe('config:get-terminal-panel-height', () => {
    it('returns the terminal panel height', () => {
      mockConfigManager.getUiValue.mockReturnValue(300)
      const result = handlers.get('config:get-terminal-panel-height')!()
      expect(mockConfigManager.getUiValue).toHaveBeenCalledWith('terminalPanelHeight')
      expect(result).toBe(300)
    })
  })

  describe('config:play-notification-sound', () => {
    it('returns null when sound path is not supported', async () => {
      mockConfigManager.getNotificationSoundPath.mockReturnValue('/path/to/sound.ogg')
      const handler = handlers.get('config:play-notification-sound')!
      const result = await handler({})
      expect(result).toBeNull()
    })

    it('returns base64 audio data for valid sound file', async () => {
      mockConfigManager.getNotificationSoundPath.mockReturnValue('/path/to/sound.mp3')
      vi.mocked(readFileSnapshot).mockResolvedValue({
        data: Buffer.from('audio-data'),
        stats: { size: 1024 } as import('node:fs').Stats,
      })

      const handler = handlers.get('config:play-notification-sound')!
      const result = await handler({})

      expect(result).toEqual({
        base64: Buffer.from('audio-data').toString('base64'),
        mimeType: 'audio/mpeg',
      })
      expect(readFileSnapshot).toHaveBeenCalledWith('/path/to/sound.mp3', 10_000_000)
    })

    it('returns null when file is too large', async () => {
      mockConfigManager.getNotificationSoundPath.mockReturnValue('/path/to/sound.mp3')
      vi.mocked(readFileSnapshot).mockRejectedValue(new Error('File exceeds the read limit'))

      const handler = handlers.get('config:play-notification-sound')!
      const result = await handler({})
      expect(result).toBeNull()
    })

    it('returns null when the opened-file read throws', async () => {
      mockConfigManager.getNotificationSoundPath.mockReturnValue('/path/to/sound.mp3')
      vi.mocked(readFileSnapshot).mockRejectedValue(new Error('ENOENT'))

      const handler = handlers.get('config:play-notification-sound')!
      const result = await handler({})
      expect(result).toBeNull()
    })

    it('returns null when path is a directory', async () => {
      mockConfigManager.getNotificationSoundPath.mockReturnValue('/path/to/sound.mp3')
      vi.mocked(readFileSnapshot).mockRejectedValue(new Error('Not a regular file'))

      const handler = handlers.get('config:play-notification-sound')!
      const result = await handler({})
      expect(result).toBeNull()
    })
  })
})
