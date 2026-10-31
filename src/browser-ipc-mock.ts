/**
 * Browser-safe IPC mock for non-Electron contexts (Lighthouse CI, browser testing).
 *
 * When the app loads outside Electron (e.g., `npx vite --mode e2e`), the preload
 * bridge that defines `window.ipcRenderer`, `window.shell`, etc. isn't available.
 * This module provides minimal mock implementations so the app can render its full
 * UI shell instead of crashing or staying stuck on the loading screen.
 *
 * In Electron, `window.ipcRenderer` is already defined by `preload.ts` — the guard
 * at the top of `installBrowserIpcMock()` makes this a no-op.
 */

/* eslint-disable @typescript-eslint/no-explicit-any */

type IpcListener = (...args: any[]) => void
type IpcListenerMap = Record<string, IpcListener[]>

const invokeDefaults: Record<string, unknown> = {
  'config:get-config': {
    github: { accounts: [] },
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
    pr: { refreshInterval: 15, autoRefresh: true, recentlyMergedDays: 7 },
    copilot: {
      ghAccount: '',
      model: 'claude-sonnet-4.5',
      premiumModel: 'claude-opus-4.6',
      prReviewPromptTemplate: '',
    },
    automation: { scheduleForecastDays: 3 },
    notifications: { playSoundOnReviewComplete: false, reviewCompleteSoundPath: '' },
    finance: { watchlist: [] },
  },
  'config:get-pane-sizes': [250, 600, 300],
  'config:get-assistant-open': false,
  'config:get-notification-sound-enabled': false,
  'config:get-notification-sound-path': '',
  'cache:initialize': { entries: {}, stats: { entryCount: 0, totalBytes: 0 }, removedKeys: [] },
  'cache:read': null,
  'cache:touch': { success: true },
  'cache:stats': { entryCount: 0, totalBytes: 0 },
  'cache:write': { success: true },
  'cache:delete': { success: true },
  'cache:clear': { success: true },
  'github:get-cli-token': '',
  'github:get-active-account': null,
  'system:get-fonts': [],
}

function resolveByPrefix(channel: string): unknown {
  if (channel.startsWith('config:set-')) return { success: true }
  if (channel.startsWith('config:get-')) return null
  return null
}

function createIpcRendererMock() {
  const listeners: IpcListenerMap = {}

  const mock = {
    on(channel: string, listener: IpcListener) {
      if (!listeners[channel]) listeners[channel] = []
      listeners[channel].push(listener)
      return () => {
        mock.off(channel, listener)
      }
    },
    off(channel: string, listener: IpcListener) {
      const arr = listeners[channel]
      if (arr) {
        const idx = arr.indexOf(listener)
        if (idx >= 0) arr.splice(idx, 1)
      }
    },
    send(_channel: string, ..._args: unknown[]) {},
    invoke(channel: string, ..._args: unknown[]): Promise<unknown> {
      return Promise.resolve(invokeDefaults[channel] ?? resolveByPrefix(channel))
    },
  }

  return mock
}

export function installBrowserIpcMock(): void {
  if (window.ipcRenderer) return
  console.warn('[Browser Mock] Electron preload unavailable — installing IPC mocks')
  const w = window as unknown as Record<string, unknown>
  w.ipcRenderer = createIpcRendererMock()
  w.shell = {
    openExternal: () => Promise.resolve({ success: false, error: 'Browser mock' }),
    openInAppBrowser: () => Promise.resolve({ success: false, error: 'Browser mock' }),
    fetchPageTitle: () => Promise.resolve({ success: false }),
  }
  w.github = {
    getCliToken: () => Promise.resolve(''),
    getActiveAccount: () => Promise.resolve(null),
    switchAccount: () => Promise.resolve({ success: false }),
    getCopilotUsage: () => Promise.resolve({ success: false }),
    getCopilotQuota: () => Promise.resolve({ success: false }),
    getCopilotBudget: () => Promise.resolve({ success: false }),
    getCopilotMemberUsage: () => Promise.resolve({ success: false }),
    getUserPremiumRequests: () => Promise.resolve({ success: false }),
    getCopilotEnterpriseUsers: () =>
      Promise.resolve({ success: false, error: 'Browser mock: metrics file unavailable' }),
  }
  w.terminal = {
    spawn: () => Promise.resolve({ success: false, error: 'Browser mock' }),
    attach: () => Promise.resolve({ success: false }),
    write: () => {},
    resize: () => {},
    kill: () => Promise.resolve({ success: false }),
    resolveRepoPath: () => Promise.resolve({ path: null }),
  }
  w.crew = {
    addProject: () => Promise.resolve({ success: false }),
    listProjects: () => Promise.resolve([]),
    removeProject: () => Promise.resolve(false),
    getSession: () => Promise.resolve(null),
    createSession: () => Promise.resolve(null),
    addMessage: () => Promise.resolve(null),
    updateSessionStatus: () => Promise.resolve(null),
    updateChangedFiles: () => Promise.resolve(null),
    clearSession: () => Promise.resolve(false),
    undoFile: () => Promise.resolve(false),
  }
  w.tempo = {
    getToday: () => Promise.resolve({ success: false }),
    getRange: () => Promise.resolve({ success: false }),
    getWeek: () => Promise.resolve({ success: false }),
    createWorklog: () => Promise.resolve({ success: false }),
    updateWorklog: () => Promise.resolve({ success: false }),
    deleteWorklog: () => Promise.resolve({ success: false }),
    getAccounts: () => Promise.resolve({ success: false }),
    getProjectAccounts: () => Promise.resolve({ success: false }),
    getCapexMap: () => Promise.resolve({ success: false }),
    getSchedule: () => Promise.resolve({ success: false }),
  }
  w.copilotSessions = {
    scan: () => Promise.resolve({ sessions: [], totalSize: 0 }),
    getSession: () => Promise.resolve(null),
    computeDigest: () => Promise.resolve(null),
  }
  w.todoist = {
    getUpcoming: () => Promise.resolve({ success: false }),
    getToday: () => Promise.resolve({ success: false }),
    completeTask: () => Promise.resolve({ success: false }),
    reopenTask: () => Promise.resolve({ success: false }),
    createTask: () => Promise.resolve({ success: false }),
    updateTask: () => Promise.resolve({ success: false }),
    deleteTask: () => Promise.resolve({ success: false }),
    getProjects: () => Promise.resolve({ success: false }),
  }
  w.finance = { fetchQuote: () => Promise.resolve({ success: false }) }
  w.slack = { nudgeAuthor: () => Promise.resolve({ success: false }) }
  w.filesystem = {
    readDir: () => Promise.resolve({ entries: [] }),
    readFile: () =>
      Promise.resolve({ content: '', language: 'text', size: 0, error: 'Browser mock' }),
  }
  w.ralph = {
    launch: () => Promise.resolve({ runId: '', status: 'error' }),
    stop: () => Promise.resolve({ success: false }),
    list: () => Promise.resolve([]),
    getStatus: () => Promise.resolve(null),
    getConfig: () => Promise.resolve({}),
    getScriptsPath: () => Promise.resolve(''),
    listTemplates: () => Promise.resolve([]),
    selectDirectory: () => Promise.resolve(null),
    onStatusChange: () => {},
    offStatusChange: () => {},
  }
  w.copilot = {
    execute: () => Promise.resolve({ resultId: null, success: false }),
    cancel: () => Promise.resolve({ success: false }),
    getActiveCount: () => Promise.resolve(0),
    listModels: () => Promise.resolve([]),
    chatSend: () => Promise.resolve(null),
    chatAbort: () => Promise.resolve({ success: false }),
    quickPrompt: () => Promise.resolve(''),
  }
}
