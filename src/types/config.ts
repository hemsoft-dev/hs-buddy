import type { Schema } from 'electron-store'
import { DEFAULT_RECENTLY_MERGED_DAYS } from '../constants'

/**
 * GitHub account configuration
 * Uses GitHub CLI (gh) authentication - no tokens stored!
 */
export interface GitHubAccount {
  username: string
  org: string
  repoRoot?: string
  usageProvider?: UsageProvider
}

export type UsageProvider = 'copilot' | 'codex'

/** Rectangle describing display bounds or work area */
interface DisplayRect {
  x: number
  y: number
  width: number
  height: number
}

/**
 * Application configuration stored in electron-store
 * Authentication uses GitHub CLI (gh auth). Remembered location is OS-encrypted.
 */
export interface AppConfig {
  weatherLocationCiphertext?: string // OS-encrypted value, never plaintext coordinates
  github: {
    accounts: GitHubAccount[]
    usageProviderOverrides?: Partial<Record<string, UsageProvider>>
    usageProviderDefaultOverrides?: Partial<Record<string, UsageProvider>>
  }
  ui: {
    theme: 'dark' | 'light'
    accentColor: string // Hex color (e.g., '#0e639c')
    fontColor: string // Font/text color hex (e.g., '#cccccc')
    bgPrimary: string // Primary background hex color
    bgSecondary: string // Secondary background hex color
    statusBarBg: string // Status bar background hex color
    statusBarFg: string // Status bar foreground/text hex color
    fontFamily: string
    monoFontFamily: string
    zoomLevel: number
    sidebarWidth: number
    paneSizes: number[]
    displayId: number // Electron display ID for multi-monitor tracking
    displayBounds: DisplayRect // Saved display bounds for robust restore
    displayWorkArea: DisplayRect // Saved display work area for robust restore
    showBookmarkedOnly: boolean // Filter org repos to bookmarked only
    assistantOpen: boolean // Whether the Copilot Assistant pane is open
    terminalOpen: boolean // Whether the bottom terminal panel is open
    terminalPanelHeight: number // Preferred height of the terminal panel in pixels
    favoriteUsers: string[] // Favorite user keys ('org/login') that sort to the top
    dashboardCards: Record<string, boolean> // Dashboard card visibility (cardId → visible)
    weatherLocation: { latitude: number; longitude: number; name: string } | null // Saved weather city
    pollenApiKey: string // Legacy Google Pollen key, migrated to OS storage. Public snapshots return ''.
    enterpriseSlug: string // GitHub Enterprise slug for enterprise billing APIs
  }
  pr: {
    refreshInterval: number // minutes
    autoRefresh: boolean
    recentlyMergedDays: number // days to look back for recently merged PRs
  }
  copilot: {
    ghAccount: string // GitHub CLI account username for Copilot SDK (empty = active account)
    model: string // LLM model to use (e.g., 'claude-sonnet-4.5', 'gpt-4o')
    premiumModel: string // Model for high-stakes tasks (e.g., address PR comments)
    prReviewPromptTemplate: string // Default prompt template for PR review (supports {{prUrl}})
  }
  automation: {
    scheduleForecastDays: number // Number of days to show in schedule forecast (1-30)
  }
  notifications: {
    playSoundOnReviewComplete: boolean // Play an audio file when a Copilot PR review finishes
    reviewCompleteSoundPath: string // Absolute path to a local audio file (empty = no sound)
  }
  finance: {
    watchlist: string[] // Ticker symbols tracked on the Finance dashboard card
  }
}

/**
 * JSON Schema for configuration validation
 */
export const configSchema: Schema<AppConfig> = {
  weatherLocationCiphertext: { type: 'string', default: '' },
  github: {
    type: 'object',
    properties: {
      accounts: {
        type: 'array',
        items: {
          type: 'object',
          properties: {
            username: { type: 'string' },
            org: { type: 'string' },
            repoRoot: { type: 'string' },
            usageProvider: { type: 'string', enum: ['copilot', 'codex'] },
          },
          required: ['username', 'org'],
        },
        default: [],
      },
      usageProviderOverrides: {
        type: 'object',
        additionalProperties: { type: 'string', enum: ['copilot', 'codex'] },
        default: {},
      },
      usageProviderDefaultOverrides: {
        type: 'object',
        additionalProperties: { type: 'string', enum: ['copilot', 'codex'] },
        default: {},
      },
    },
    required: ['accounts'],
  },
  ui: {
    type: 'object',
    properties: {
      theme: {
        type: 'string',
        enum: ['dark', 'light'],
        default: 'dark',
      },
      accentColor: {
        type: 'string',
        default: '#0e639c',
      },
      fontColor: {
        type: 'string',
        default: '#cccccc',
      },
      bgPrimary: {
        type: 'string',
        default: '#1e1e1e',
      },
      bgSecondary: {
        type: 'string',
        default: '#252526',
      },
      statusBarBg: {
        type: 'string',
        default: '#181818',
      },
      statusBarFg: {
        type: 'string',
        default: '#9d9d9d',
      },
      fontFamily: {
        type: 'string',
        default: 'Inter',
      },
      monoFontFamily: {
        type: 'string',
        default: 'Cascadia Code',
      },
      zoomLevel: {
        type: 'number',
        minimum: 80,
        maximum: 150,
        default: 100,
      },
      sidebarWidth: {
        type: 'number',
        minimum: 200,
        maximum: 500,
        default: 300,
      },
      paneSizes: {
        type: 'array',
        items: { type: 'number' },
        default: [300, 900],
      },
      displayId: {
        type: 'number',
        default: 0,
      },
      displayBounds: {
        type: 'object',
        properties: {
          x: { type: 'number', default: 0 },
          y: { type: 'number', default: 0 },
          width: { type: 'number', default: 0 },
          height: { type: 'number', default: 0 },
        },
        default: { x: 0, y: 0, width: 0, height: 0 },
      },
      displayWorkArea: {
        type: 'object',
        properties: {
          x: { type: 'number', default: 0 },
          y: { type: 'number', default: 0 },
          width: { type: 'number', default: 0 },
          height: { type: 'number', default: 0 },
        },
        default: { x: 0, y: 0, width: 0, height: 0 },
      },
      showBookmarkedOnly: {
        type: 'boolean',
        default: false,
      },
      assistantOpen: {
        type: 'boolean',
        default: false,
      },
      terminalOpen: {
        type: 'boolean',
        default: false,
      },
      terminalPanelHeight: {
        type: 'number',
        minimum: 100,
        maximum: 1200,
        default: 300,
      },
      favoriteUsers: {
        type: 'array',
        items: { type: 'string' },
        default: [],
      },
      dashboardCards: {
        type: 'object',
        default: {},
      },
      weatherLocation: {
        anyOf: [
          {
            type: 'object',
            properties: {
              latitude: { type: 'number' },
              longitude: { type: 'number' },
              name: { type: 'string' },
            },
            required: ['latitude', 'longitude', 'name'],
          },
          { type: 'null' },
        ],
        default: null,
      },
      pollenApiKey: {
        type: 'string',
        default: '',
      },
      enterpriseSlug: {
        type: 'string',
        default: '',
      },
    },
    required: [
      'theme',
      'accentColor',
      'fontColor',
      'bgPrimary',
      'bgSecondary',
      'statusBarBg',
      'statusBarFg',
      'fontFamily',
      'monoFontFamily',
      'zoomLevel',
      'sidebarWidth',
      'paneSizes',
      'displayId',
      'showBookmarkedOnly',
      'assistantOpen',
      'terminalOpen',
      'terminalPanelHeight',
      'favoriteUsers',
      'dashboardCards',
      'weatherLocation',
      'pollenApiKey',
      'enterpriseSlug',
    ],
  },
  pr: {
    type: 'object',
    properties: {
      refreshInterval: {
        type: 'number',
        minimum: 1,
        maximum: 60,
        default: 15,
      },
      autoRefresh: {
        type: 'boolean',
        default: false,
      },
      recentlyMergedDays: {
        type: 'number',
        minimum: 1,
        maximum: 365,
        default: 7,
      },
    },
    required: ['refreshInterval', 'autoRefresh', 'recentlyMergedDays'],
  },
  copilot: {
    type: 'object',
    properties: {
      ghAccount: {
        type: 'string',
        default: '',
      },
      model: {
        type: 'string',
        default: 'claude-sonnet-4.5',
      },
      premiumModel: {
        type: 'string',
        default: 'claude-opus-4.6',
      },
      prReviewPromptTemplate: {
        type: 'string',
        default: '',
      },
    },
    required: ['ghAccount', 'model', 'premiumModel', 'prReviewPromptTemplate'],
  },
  automation: {
    type: 'object',
    properties: {
      scheduleForecastDays: {
        type: 'number',
        minimum: 1,
        maximum: 30,
        default: 3,
      },
    },
    required: ['scheduleForecastDays'],
  },
  notifications: {
    type: 'object',
    properties: {
      playSoundOnReviewComplete: {
        type: 'boolean',
        default: false,
      },
      reviewCompleteSoundPath: {
        type: 'string',
        default: '',
      },
    },
    required: ['playSoundOnReviewComplete', 'reviewCompleteSoundPath'],
  },
  finance: {
    type: 'object',
    properties: {
      watchlist: {
        type: 'array',
        items: { type: 'string' },
        default: ['^GSPC', '^IXIC', '^DJI', 'BTC-USD'],
      },
    },
    required: ['watchlist'],
  },
}

/**
 * Default configuration values
 */
export const defaultConfig: AppConfig = {
  github: {
    accounts: [],
    usageProviderOverrides: {},
    usageProviderDefaultOverrides: {},
  },
  ui: {
    theme: 'dark',
    accentColor: '#0e639c',
    fontColor: '#cccccc',
    bgPrimary: '#1e1e1e',
    bgSecondary: '#252526',
    statusBarBg: '#181818',
    statusBarFg: '#9d9d9d',
    fontFamily: 'Inter',
    monoFontFamily: 'Cascadia Code',
    zoomLevel: 100,
    sidebarWidth: 300,
    paneSizes: [300, 900],
    displayId: 0,
    displayBounds: { x: 0, y: 0, width: 0, height: 0 },
    displayWorkArea: { x: 0, y: 0, width: 0, height: 0 },
    showBookmarkedOnly: false,
    assistantOpen: false,
    terminalOpen: false,
    terminalPanelHeight: 300,
    favoriteUsers: [],
    dashboardCards: {},
    weatherLocation: null,
    pollenApiKey: '',
    enterpriseSlug: '',
  },
  pr: {
    refreshInterval: 15,
    autoRefresh: true,
    recentlyMergedDays: DEFAULT_RECENTLY_MERGED_DAYS,
  },
  copilot: {
    ghAccount: '', // Empty = use currently-active gh CLI account
    model: 'claude-sonnet-4.5',
    premiumModel: 'claude-opus-4.6',
    prReviewPromptTemplate: '',
  },
  automation: {
    scheduleForecastDays: 3,
  },
  notifications: {
    playSoundOnReviewComplete: false,
    reviewCompleteSoundPath: '',
  },
  finance: {
    watchlist: ['^GSPC', '^IXIC', '^DJI', 'BTC-USD'],
  },
}
