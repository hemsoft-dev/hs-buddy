/**
 * E2E Test Fixtures
 *
 * Provides a custom Playwright test instance with Electron API mocks injected
 * via addInitScript. This allows the app to boot in a standalone browser
 * without requiring the Electron preload script.
 */
import { chromium, test as base, expect } from '@playwright/test'
import { installGitHubNetwork } from './github-network'

/**
 * Extend Playwright's base test with automatic IPC mocking.
 * All E2E specs should import { test, expect } from this file.
 */
export const test = base.extend<{
  convexQueries: Record<string, unknown>
  githubAccounts: Array<{ username: string; org: string }>
}>({
  convexQueries: [{}, { option: true }],
  githubAccounts: [[{ username: 'test-user', org: 'test-org' }], { option: true }],
  context: async ({ context, baseURL }, use, testInfo) => {
    if (testInfo.project.name === 'electron-cdp') {
      // eslint-disable-next-line react-hooks/rules-of-hooks
      await use(context)
      return
    }
    const unexpected = await installGitHubNetwork(context, baseURL!)
    // eslint-disable-next-line react-hooks/rules-of-hooks
    await use(context)
    // Stop pages while context interception is still active, then let Playwright close it.
    await Promise.all(context.pages().map(page => page.close()))
    expect(unexpected, 'Unhandled external requests').toEqual([])
  },
  page: async ({ page, convexQueries, githubAccounts }, use, testInfo) => {
    // Only inject mocks for browser-e2e project.
    // The electron-cdp project connects to a real Electron app with preload APIs.
    if (testInfo.project.name === 'electron-cdp') {
      const electronBrowser = await chromium.connectOverCDP('http://127.0.0.1:9222')
      try {
        const appPage = electronBrowser
          .contexts()
          .flatMap(context => context.pages())
          .find(candidate => {
            const rawUrl = candidate.url()
            if (!URL.canParse(rawUrl)) return false
            const url = new URL(rawUrl)
            return (
              url.protocol === 'file:' ||
              ((url.protocol === 'http:' || url.protocol === 'https:') &&
                (url.hostname === 'localhost' || url.hostname === '127.0.0.1'))
            )
          })
        if (!appPage) throw new Error('Electron development or packaged app page was unavailable')
        // eslint-disable-next-line react-hooks/rules-of-hooks
        await use(appPage)
      } finally {
        await electronBrowser.close()
      }
      return
    }

    await page.addInitScript(queries => {
      window.__buddyE2EQueries = queries
    }, convexQueries)

    // Inject Electron API mocks BEFORE any page scripts execute.
    // This mimics what the preload script provides in the real Electron environment.
    await page.addInitScript(accounts => {
      // Track IPC listeners for event simulation
      const ipcListeners: Record<string, Array<(...args: unknown[]) => void>> = {}

      // Mock window.ipcRenderer (returns itself for chaining, matching the real preload bridge)
      const ipcRendererMock = {
        on(channel: string, listener: (...args: unknown[]) => void) {
          if (!ipcListeners[channel]) ipcListeners[channel] = []
          ipcListeners[channel].push(listener)
          return ipcRendererMock
        },
        off(channel: string, listener: (...args: unknown[]) => void) {
          const arr = ipcListeners[channel]
          if (arr) {
            const idx = arr.indexOf(listener)
            if (idx >= 0) arr.splice(idx, 1)
          }
        },
        send(_channel: string, ..._args: unknown[]) {
          // No-op for sends in test mode
        },
        // eslint-disable-next-line complexity
        invoke(channel: string, ..._args: unknown[]): Promise<unknown> {
          // Return sensible defaults for known IPC channels
          switch (channel) {
            case 'config:get-config':
              return Promise.resolve({
                github: {
                  accounts,
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
                },
                pr: {
                  refreshInterval: 15,
                  autoRefresh: true,
                  recentlyMergedDays: 7,
                },
                copilot: {
                  ghAccount: '',
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
              })
            case 'config:get-pane-sizes':
              return Promise.resolve([250, 600, 300])
            case 'config:get-assistant-open':
              return Promise.resolve(false)
            case 'config:get-notification-sound-enabled':
              return Promise.resolve(false)
            case 'config:get-notification-sound-path':
              return Promise.resolve('')
            case 'cache:initialize':
              return Promise.resolve({
                entries: {},
                stats: { entryCount: 0, totalBytes: 0 },
                removedKeys: [],
              })
            case 'cache:read':
              return Promise.resolve(null)
            case 'cache:touch':
              return Promise.resolve({ success: true })
            case 'cache:stats':
              return Promise.resolve({ entryCount: 0, totalBytes: 0 })
            case 'cache:write':
            case 'cache:delete':
            case 'cache:clear':
              return Promise.resolve({ success: true })
            case 'github:get-cli-token':
              return Promise.resolve('mock-token-for-e2e')
            case 'github:get-active-account':
              return Promise.resolve('test-user')
            case 'github:switch-account':
              return Promise.resolve({ success: true })
            case 'system:get-fonts':
              return Promise.resolve(['Arial', 'Helvetica', 'monospace'])
            case 'config:get-store-path':
              return Promise.resolve('/tmp/e2e-store.json')
            case 'config:open-in-editor':
            case 'config:reset':
              return Promise.resolve({ success: true })
            case 'terminal:create':
              return Promise.resolve({ id: 'mock-terminal-1' })
            case 'terminal:list':
              return Promise.resolve([])
            case 'terminal:resize':
            case 'terminal:write':
            case 'terminal:close':
              return Promise.resolve({ success: true })
            default:
              // For any config:set-* or config:get-* not explicitly handled
              if (channel.startsWith('config:set-')) return Promise.resolve({ success: true })
              if (channel.startsWith('config:get-')) return Promise.resolve(null)
              return Promise.resolve({ success: true })
          }
        },
      }
      ;(window as unknown as Record<string, unknown>).ipcRenderer = ipcRendererMock

      // Mock window.shell
      ;(window as unknown as Record<string, unknown>).shell = {
        openExternal: (_url: string) => Promise.resolve(),
        openInAppBrowser: (_url: string, _title?: string) => Promise.resolve(),
        fetchPageTitle: (_url: string) => Promise.resolve('Mock Page Title'),
      }

      // Mock window.github
      ;(window as unknown as Record<string, unknown>).github = {
        getCliToken: (_username?: string) => Promise.resolve('mock-token-for-e2e'),
        getActiveAccount: () => Promise.resolve({ username: 'test-user', org: 'test-org' }),
        switchAccount: (_username: string) => Promise.resolve({ success: true }),
      }

      // Mock window.terminal (matches preload bridge: spawn, attach, write, resize, kill, resolveRepoPath)
      ;(window as unknown as Record<string, unknown>).terminal = {
        spawn: () => Promise.resolve({ success: false, error: 'E2E mode: terminal not available' }),
        attach: () => Promise.resolve({ success: false }),
        write: () => {}, // send-based, no return value
        resize: () => {}, // send-based, no return value
        kill: () => Promise.resolve({ success: false }),
        resolveRepoPath: () => Promise.resolve({ success: false }),
      }

      // Mock window.ralph (used by Ralph loops/dashboard)
      ;(window as unknown as Record<string, unknown>).ralph = {
        launch: () => Promise.resolve({ runId: 'mock-run-1', status: 'running' }),
        stop: () => Promise.resolve({ success: true }),
        list: () => Promise.resolve([]),
        getStatus: () => Promise.resolve(null),
        getConfig: () => Promise.resolve({}),
        getScriptsPath: () => Promise.resolve('/mock/scripts'),
        listTemplates: () => Promise.resolve([]),
        selectDirectory: () => Promise.resolve(null),
        onStatusChange: () => {},
        offStatusChange: () => {},
      }
    }, githubAccounts)

    // eslint-disable-next-line react-hooks/rules-of-hooks
    await use(page)
  },
})

export { expect }

/**
 * Wait for the app shell to be ready (activity bar visible).
 * Use this at the start of tests that need the full app to be loaded.
 */
export async function waitForAppReady(page: import('@playwright/test').Page) {
  await page.goto('/')
  // Wait for the activity bar to render — indicates the app shell is ready
  await page.locator('.activity-bar').waitFor({ state: 'visible', timeout: 30_000 })
}
