import { dialog, ipcMain, shell, type IpcMainInvokeEvent } from 'electron'
import type { AppConfig, GitHubAccount } from '../../src/types/config'
import {
  getNotificationSoundMimeType,
  isSupportedNotificationSoundPath,
  MAX_NOTIFICATION_SOUND_BYTES,
} from '../../src/utils/notificationSound'
import { getErrorMessage } from '../../src/utils/errorUtils'
import { assertValidGitHubAccountSlug } from '../../src/utils/githubAuthUtils'
import { CONFIG_UI_KEYS, IPC_INVOKE } from '../../src/ipc/contracts'
import { configManager } from '../config'
import { readFileSnapshot } from '../services/fileSnapshots'
import { pollenCredentialErrorMessage } from '../services/pollenCredentials'

type UiConfigKey = keyof AppConfig['ui']
type ConfigUiChannelKey = (typeof CONFIG_UI_KEYS)[number]

function parseOptionalRepoRoot(value: unknown): string | undefined {
  if (value === undefined) return undefined
  if (typeof value !== 'string') throw new Error('Repository root must be text')
  return value || undefined
}

function parseOptionalUsageProvider(value: unknown): GitHubAccount['usageProvider'] {
  if (value === undefined) return undefined
  if (value !== 'copilot' && value !== 'codex') {
    throw new Error('Usage provider must be Copilot or Codex')
  }
  return value
}

function parseGitHubAccountSnapshot(candidate: unknown): GitHubAccount {
  if (!candidate || typeof candidate !== 'object') {
    throw new Error('GitHub account must be an object')
  }
  const { username, org, repoRoot, usageProvider } = candidate as Record<string, unknown>
  if (typeof username !== 'string' || typeof org !== 'string') {
    throw new Error('Account identity must be text')
  }
  assertValidGitHubAccountSlug(username)
  assertValidGitHubAccountSlug(org)
  const parsedRepoRoot = parseOptionalRepoRoot(repoRoot)
  const parsedUsageProvider = parseOptionalUsageProvider(usageProvider)
  return {
    username,
    org,
    ...(parsedRepoRoot ? { repoRoot: parsedRepoRoot } : {}),
    ...(parsedUsageProvider ? { usageProvider: parsedUsageProvider } : {}),
  }
}

// Maps each contract channel key to its AppConfig['ui'] property name.
// Record<ConfigUiChannelKey, …> ensures every key from CONFIG_UI_KEYS has an
// entry — adding/removing a key in contracts.ts will produce a compile error here.
const CHANNEL_TO_CONFIG_KEY: Record<ConfigUiChannelKey, UiConfigKey> = {
  theme: 'theme',
  'accent-color': 'accentColor',
  'bg-primary': 'bgPrimary',
  'bg-secondary': 'bgSecondary',
  'statusbar-bg': 'statusBarBg',
  'statusbar-fg': 'statusBarFg',
  'font-color': 'fontColor',
  'font-family': 'fontFamily',
  'mono-font-family': 'monoFontFamily',
  'zoom-level': 'zoomLevel',
  'sidebar-width': 'sidebarWidth',
  'pane-sizes': 'paneSizes',
  'show-bookmarked-only': 'showBookmarkedOnly',
  'favorite-users': 'favoriteUsers',
  'dashboard-cards': 'dashboardCards',
  'weather-location': 'weatherLocation',
  'pollen-api-key': 'pollenApiKey',
  'enterprise-slug': 'enterpriseSlug',
}

// Derived from CONFIG_UI_KEYS — guaranteed to stay in sync with contracts.ts
const UI_VALUE_CHANNELS: Array<readonly [ConfigUiChannelKey, UiConfigKey]> = CONFIG_UI_KEYS.map(
  key => [key, CHANNEL_TO_CONFIG_KEY[key]] as const
)

function registerUiValueHandler<K extends UiConfigKey>(channel: string, key: K): void {
  ipcMain.handle(`config:get-${channel}`, () => {
    return configManager.getUiValue(key)
  })

  ipcMain.handle(
    `config:set-${channel}`,
    (_event: IpcMainInvokeEvent, value: AppConfig['ui'][K]) => {
      if (key === 'enterpriseSlug') {
        if (typeof value !== 'string') {
          return { success: false, error: 'Enterprise slug must be a string' }
        }

        const enterpriseSlug = value.trim()
        try {
          if (enterpriseSlug) assertValidGitHubAccountSlug(enterpriseSlug)
        } catch (error: unknown) {
          return { success: false, error: getErrorMessage(error) }
        }

        configManager.setUiValue('enterpriseSlug', enterpriseSlug)
        return { success: true }
      }

      configManager.setUiValue(key, value)
      return { success: true }
    }
  )
}

async function readNotificationSoundAsBase64(
  soundPath: string
): Promise<{ base64: string; mimeType: string } | null> {
  try {
    const { data: buffer } = await readFileSnapshot(soundPath, MAX_NOTIFICATION_SOUND_BYTES)
    return {
      base64: buffer.toString('base64'),
      mimeType: getNotificationSoundMimeType(soundPath),
    }
  } catch (_: unknown) {
    return null
  }
}

function registerTerminalAndPanelHandlers(): void {
  ipcMain.handle(IPC_INVOKE.CONFIG_GET_ASSISTANT_OPEN, () => {
    return configManager.getUiValue('assistantOpen')
  })

  ipcMain.handle(
    IPC_INVOKE.CONFIG_SET_ASSISTANT_OPEN,
    (_event: IpcMainInvokeEvent, value: boolean) => {
      if (typeof value !== 'boolean') return { success: false }
      configManager.setUiValue('assistantOpen', value)
      return { success: true }
    }
  )

  ipcMain.handle(IPC_INVOKE.CONFIG_GET_TERMINAL_OPEN, () => {
    return configManager.getUiValue('terminalOpen')
  })

  ipcMain.handle(
    IPC_INVOKE.CONFIG_SET_TERMINAL_OPEN,
    (_event: IpcMainInvokeEvent, value: boolean) => {
      if (typeof value !== 'boolean') return { success: false }
      configManager.setUiValue('terminalOpen', value)
      return { success: true }
    }
  )

  ipcMain.handle(IPC_INVOKE.CONFIG_GET_TERMINAL_PANEL_HEIGHT, () => {
    return configManager.getUiValue('terminalPanelHeight')
  })

  ipcMain.handle(
    IPC_INVOKE.CONFIG_SET_TERMINAL_PANEL_HEIGHT,
    (_event: IpcMainInvokeEvent, value: number) => {
      if (!Number.isFinite(value) || value < 100 || value > 1200) return { success: false }
      configManager.setUiValue('terminalPanelHeight', value)
      return { success: true }
    }
  )

  ipcMain.handle(IPC_INVOKE.CONFIG_GET_SCHEDULE_FORECAST_DAYS, () => {
    return configManager.getScheduleForecastDays()
  })

  ipcMain.handle(
    IPC_INVOKE.CONFIG_SET_SCHEDULE_FORECAST_DAYS,
    (_event: IpcMainInvokeEvent, days: number) => {
      configManager.setScheduleForecastDays(days)
      return { success: true }
    }
  )

  ipcMain.handle(IPC_INVOKE.CONFIG_GET_COPILOT_PR_REVIEW_PROMPT_TEMPLATE, () => {
    return configManager.getCopilotPRReviewPromptTemplate()
  })

  ipcMain.handle(
    IPC_INVOKE.CONFIG_SET_COPILOT_PR_REVIEW_PROMPT_TEMPLATE,
    (_event: IpcMainInvokeEvent, template: string) => {
      configManager.setCopilotPRReviewPromptTemplate(template)
      return { success: true }
    }
  )
}

function registerNotificationHandlers(): void {
  let lastPickedNotificationSoundPath: string | null = null

  ipcMain.handle(IPC_INVOKE.CONFIG_GET_NOTIFICATION_SOUND_ENABLED, () => {
    return configManager.getNotificationSoundEnabled()
  })

  ipcMain.handle(
    IPC_INVOKE.CONFIG_SET_NOTIFICATION_SOUND_ENABLED,
    (_event: IpcMainInvokeEvent, enabled: boolean) => {
      if (typeof enabled !== 'boolean') return { success: false }
      configManager.setNotificationSoundEnabled(enabled)
      return { success: true }
    }
  )

  ipcMain.handle(IPC_INVOKE.CONFIG_GET_NOTIFICATION_SOUND_PATH, () => {
    return configManager.getNotificationSoundPath()
  })

  ipcMain.handle(
    IPC_INVOKE.CONFIG_SET_NOTIFICATION_SOUND_PATH,
    (_event: IpcMainInvokeEvent, filePath: string) => {
      if (typeof filePath !== 'string') return { success: false }

      const normalizedFilePath = filePath.trim()
      if (!normalizedFilePath) {
        lastPickedNotificationSoundPath = null
        configManager.setNotificationSoundPath('')
        return { success: true }
      }

      if (!isSupportedNotificationSoundPath(normalizedFilePath)) {
        return { success: false }
      }

      if (lastPickedNotificationSoundPath !== normalizedFilePath) {
        return { success: false }
      }

      configManager.setNotificationSoundPath(normalizedFilePath)
      lastPickedNotificationSoundPath = null
      return { success: true }
    }
  )

  ipcMain.handle(IPC_INVOKE.CONFIG_PICK_AUDIO_FILE, async () => {
    const result = await dialog.showOpenDialog({
      title: 'Select notification sound',
      filters: [{ name: 'Audio Files', extensions: ['mp3', 'wav', 'ogg', 'flac', 'aac', 'm4a'] }],
      properties: ['openFile'],
    })
    if (result.canceled || result.filePaths.length === 0) {
      lastPickedNotificationSoundPath = null
      return { success: false, canceled: true }
    }

    const [filePath] = result.filePaths
    if (!isSupportedNotificationSoundPath(filePath)) {
      lastPickedNotificationSoundPath = null
      return { success: false }
    }

    lastPickedNotificationSoundPath = filePath
    return { success: true, filePath }
  })

  ipcMain.handle(IPC_INVOKE.CONFIG_PLAY_NOTIFICATION_SOUND, async () => {
    const soundPath = configManager.getNotificationSoundPath()
    if (!isSupportedNotificationSoundPath(soundPath)) return null
    return readNotificationSoundAsBase64(soundPath)
  })
}

function registerMiscConfigHandlers(): void {
  ipcMain.handle(IPC_INVOKE.CONFIG_GET_FINANCE_WATCHLIST, () => {
    return configManager.getFinanceWatchlist()
  })

  ipcMain.handle(
    IPC_INVOKE.CONFIG_SET_FINANCE_WATCHLIST,
    (_event: IpcMainInvokeEvent, symbols: unknown) => {
      if (!Array.isArray(symbols) || !symbols.every(s => typeof s === 'string')) {
        return { success: false }
      }
      configManager.setFinanceWatchlist(symbols)
      return { success: true }
    }
  )

  ipcMain.handle(
    IPC_INVOKE.CONFIG_SYNC_GITHUB_ACCOUNTS,
    (_event: IpcMainInvokeEvent, value: unknown) => {
      if (!Array.isArray(value)) {
        return { success: false, error: 'GitHub accounts must be an array' }
      }

      try {
        const accounts = value.map(parseGitHubAccountSnapshot)
        configManager.replaceGitHubAccounts(accounts)
        return {
          success: true,
          usageProviderOverrides: configManager.getUsageProviderOverrides(),
          usageProviderDefaultOverrides: configManager.getUsageProviderDefaultOverrides(),
        }
      } catch (error: unknown) {
        return { success: false, error: getErrorMessage(error) }
      }
    }
  )

  ipcMain.handle(
    IPC_INVOKE.CONFIG_SET_USAGE_PROVIDER_OVERRIDE,
    (_event: IpcMainInvokeEvent, username: unknown, org: unknown, provider: unknown) => {
      if (typeof username !== 'string' || typeof org !== 'string') {
        return { success: false, error: 'Account identity must be text' }
      }
      if (provider !== null && provider !== 'copilot' && provider !== 'codex') {
        return { success: false, error: 'Usage provider must be Copilot or Codex' }
      }

      try {
        assertValidGitHubAccountSlug(username)
        assertValidGitHubAccountSlug(org)
        if (provider !== null && !configManager.hasGitHubAccount(username, org)) {
          return { success: false, error: 'Account is not configured locally' }
        }
        configManager.setUsageProviderOverride(username, org, provider)
        return { success: true }
      } catch (error: unknown) {
        return { success: false, error: getErrorMessage(error) }
      }
    }
  )

  ipcMain.handle(IPC_INVOKE.CONFIG_GET_CONFIG, () => {
    return configManager.getConfig()
  })

  ipcMain.handle(IPC_INVOKE.CONFIG_GET_STORE_PATH, () => {
    return configManager.getStorePath()
  })

  ipcMain.handle(IPC_INVOKE.CONFIG_OPEN_IN_EDITOR, () => {
    shell.openPath(configManager.getStorePath())
    return { success: true }
  })

  ipcMain.handle(IPC_INVOKE.CONFIG_RESET, async () => {
    try {
      await configManager.reset()
      return { success: true }
    } catch (_: unknown) {
      return {
        success: false,
        error:
          'Could not reset settings. Unlock secure storage, check file permissions, then retry.',
      }
    }
  })
}

export function registerConfigHandlers(): void {
  for (const [channel, key] of UI_VALUE_CHANNELS) {
    if (key !== 'pollenApiKey') registerUiValueHandler(channel, key)
  }
  ipcMain.handle(IPC_INVOKE.CONFIG_GET_POLLEN_API_KEY, async () => {
    try {
      return { configured: Boolean(await configManager.getPollenApiKey()) }
    } catch (error: unknown) {
      return { configured: false, error: pollenCredentialErrorMessage(error) }
    }
  })
  ipcMain.handle(IPC_INVOKE.CONFIG_SET_POLLEN_API_KEY, async (_event, value: unknown) => {
    if (typeof value !== 'string') return { success: false, error: 'API key must be text.' }
    try {
      await configManager.setPollenApiKey(value)
      return { success: true }
    } catch (error: unknown) {
      return { success: false, error: pollenCredentialErrorMessage(error) }
    }
  })
  registerTerminalAndPanelHandlers()
  registerNotificationHandlers()
  registerMiscConfigHandlers()
}
