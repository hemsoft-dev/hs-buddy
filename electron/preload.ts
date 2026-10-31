import { contextBridge, ipcRenderer } from 'electron'
import { IPC_INVOKE, IPC_SEND, IPC_PUSH } from '../src/ipc/contracts'

// Keep legacy off() bookkeeping for callers sharing a listener identity.
// Disposers capture their wrapper so cleanup survives contextBridge proxying.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type IpcListener = (event: Electron.IpcRendererEvent, ...args: any[]) => void
const ipcListenerWrappers = new Map<string, Map<IpcListener, IpcListener>>()

function isValidOptionalString(value: unknown): boolean {
  return value == null || typeof value === 'string'
}

function isValidOptionalBoolean(value: unknown): boolean {
  return value == null || typeof value === 'boolean'
}

function isValidLoginList(logins: unknown): logins is string[] {
  return (
    Array.isArray(logins) &&
    Array.from(logins).every(login => typeof login === 'string' && login.trim().length > 0)
  )
}

function isValidBatchMonthlyRequestArgs(
  logins: unknown,
  username: unknown,
  skipDayProbing: unknown
): logins is string[] {
  return (
    isValidLoginList(logins) &&
    isValidOptionalString(username) &&
    isValidOptionalBoolean(skipDayProbing)
  )
}

// --------- Expose some API to the Renderer process ---------
contextBridge.exposeInMainWorld('ipcRenderer', {
  on(...args: Parameters<typeof ipcRenderer.on>) {
    const [channel, listener] = args
    const wrapper = (event: Electron.IpcRendererEvent, ...rest: unknown[]) =>
      listener(event, ...rest)
    if (!ipcListenerWrappers.has(channel)) ipcListenerWrappers.set(channel, new Map())
    ipcListenerWrappers.get(channel)!.set(listener, wrapper)
    ipcRenderer.on(channel, wrapper)
    let active = true
    return () => {
      if (!active) return
      active = false
      const channelMap = ipcListenerWrappers.get(channel)
      if (channelMap?.get(listener) === wrapper) {
        channelMap.delete(listener)
        if (channelMap.size === 0) ipcListenerWrappers.delete(channel)
      }
      ipcRenderer.off(channel, wrapper)
    }
  },
  off(...args: Parameters<typeof ipcRenderer.off>) {
    const [channel, listener] = args
    const channelMap = ipcListenerWrappers.get(channel)
    const wrapper = channelMap?.get(listener)
    if (wrapper) {
      channelMap!.delete(listener)
      return ipcRenderer.off(channel, wrapper)
    }
    // No wrapper found — listener was never registered through this bridge or already removed.
    // Attempting ipcRenderer.off with the raw listener would be a silent no-op, so skip it.
  },
  send(...args: Parameters<typeof ipcRenderer.send>) {
    const [channel, ...omit] = args
    return ipcRenderer.send(channel, ...omit)
  },
  invoke(...args: Parameters<typeof ipcRenderer.invoke>) {
    const [channel, ...omit] = args
    return ipcRenderer.invoke(channel, ...omit)
  },
})

contextBridge.exposeInMainWorld('shell', {
  openExternal: (url: string) => ipcRenderer.invoke(IPC_INVOKE.SHELL_OPEN_EXTERNAL, url),
  openInAppBrowser: (url: string, title?: string) =>
    ipcRenderer.invoke(IPC_INVOKE.SHELL_OPEN_IN_APP_BROWSER, url, title),
  fetchPageTitle: (url: string) => ipcRenderer.invoke(IPC_INVOKE.SHELL_FETCH_PAGE_TITLE, url),
})

contextBridge.exposeInMainWorld('github', {
  getCliToken: (username?: string) => ipcRenderer.invoke(IPC_INVOKE.GITHUB_GET_CLI_TOKEN, username),
  getActiveAccount: () => ipcRenderer.invoke(IPC_INVOKE.GITHUB_GET_ACTIVE_ACCOUNT),
  switchAccount: (username: string) =>
    ipcRenderer.invoke(IPC_INVOKE.GITHUB_SWITCH_ACCOUNT, username),
  getCopilotUsage: (org: string, username?: string) =>
    ipcRenderer.invoke(IPC_INVOKE.GITHUB_GET_COPILOT_USAGE, org, username),
  getCopilotQuota: (username: string) =>
    ipcRenderer.invoke(IPC_INVOKE.GITHUB_GET_COPILOT_QUOTA, username),
  getCopilotBudget: (org: string, username?: string) =>
    ipcRenderer.invoke(IPC_INVOKE.GITHUB_GET_COPILOT_BUDGET, org, username),
  getCopilotMemberUsage: (org: string, memberLogin: string, username?: string) =>
    ipcRenderer.invoke(IPC_INVOKE.GITHUB_GET_COPILOT_MEMBER_USAGE, org, memberLogin, username),
  getUserPremiumRequests: (org: string, memberLogin: string, username?: string) =>
    ipcRenderer.invoke(IPC_INVOKE.GITHUB_GET_USER_PREMIUM_REQUESTS, org, memberLogin, username),
  getCopilotSeats: (org: string, username?: string) =>
    typeof org === 'string' &&
    org.trim().length > 0 &&
    (username == null || typeof username === 'string')
      ? ipcRenderer.invoke(IPC_INVOKE.GITHUB_GET_COPILOT_SEATS, org, username)
      : Promise.resolve({ success: false, error: 'Invalid arguments' }),
  getBatchMonthlyRequests: (logins: string[], username?: string, skipDayProbing?: boolean) =>
    isValidBatchMonthlyRequestArgs(logins, username, skipDayProbing)
      ? ipcRenderer.invoke(
          IPC_INVOKE.GITHUB_GET_BATCH_MONTHLY_REQUESTS,
          logins,
          username,
          skipDayProbing
        )
      : Promise.resolve({ success: false, error: 'Invalid arguments' }),
  getCopilotEnterpriseUsers: () =>
    ipcRenderer.invoke(IPC_INVOKE.GITHUB_GET_COPILOT_ENTERPRISE_USERS),
})

contextBridge.exposeInMainWorld('codex', {
  getUsage: () => ipcRenderer.invoke(IPC_INVOKE.CODEX_GET_USAGE),
})

contextBridge.exposeInMainWorld('crew', {
  addProject: () => ipcRenderer.invoke(IPC_INVOKE.CREW_ADD_PROJECT),
  listProjects: () => ipcRenderer.invoke(IPC_INVOKE.CREW_LIST_PROJECTS),
  removeProject: (projectId: string) =>
    ipcRenderer.invoke(IPC_INVOKE.CREW_REMOVE_PROJECT, projectId),
  getSession: (projectId: string) => ipcRenderer.invoke(IPC_INVOKE.CREW_GET_SESSION, projectId),
  createSession: (projectId: string) =>
    ipcRenderer.invoke(IPC_INVOKE.CREW_CREATE_SESSION, projectId),
  addMessage: (projectId: string, message: { role: string; content: string; timestamp: number }) =>
    ipcRenderer.invoke(IPC_INVOKE.CREW_ADD_MESSAGE, projectId, message),
  updateSessionStatus: (projectId: string, status: string) =>
    ipcRenderer.invoke(IPC_INVOKE.CREW_UPDATE_SESSION_STATUS, projectId, status),
  updateChangedFiles: (projectId: string, files: unknown[]) =>
    ipcRenderer.invoke(IPC_INVOKE.CREW_UPDATE_CHANGED_FILES, projectId, files),
  clearSession: (projectId: string) => ipcRenderer.invoke(IPC_INVOKE.CREW_CLEAR_SESSION, projectId),
  undoFile: (projectId: string, filePath: string) =>
    ipcRenderer.invoke(IPC_INVOKE.CREW_UNDO_FILE, projectId, filePath),
})

contextBridge.exposeInMainWorld('tempo', {
  getToday: (date?: string) => ipcRenderer.invoke(IPC_INVOKE.TEMPO_GET_TODAY, date),
  getRange: (from: string, to: string) =>
    ipcRenderer.invoke(IPC_INVOKE.TEMPO_GET_RANGE, { from, to }),
  getWeek: (weekStart: string, weekEnd: string) =>
    ipcRenderer.invoke(IPC_INVOKE.TEMPO_GET_WEEK, { weekStart, weekEnd }),
  createWorklog: (payload: {
    issueKey: string
    hours: number
    date: string
    startTime?: string
    description?: string
    accountKey?: string
  }) => ipcRenderer.invoke(IPC_INVOKE.TEMPO_CREATE_WORKLOG, payload),
  updateWorklog: (
    worklogId: number,
    payload: { hours?: number; startTime?: string; description?: string; accountKey?: string }
  ) => ipcRenderer.invoke(IPC_INVOKE.TEMPO_UPDATE_WORKLOG, { worklogId, payload }),
  deleteWorklog: (worklogId: number) =>
    ipcRenderer.invoke(IPC_INVOKE.TEMPO_DELETE_WORKLOG, worklogId),
  getAccounts: () => ipcRenderer.invoke(IPC_INVOKE.TEMPO_GET_ACCOUNTS),
  getProjectAccounts: (projectKey: string) =>
    ipcRenderer.invoke(IPC_INVOKE.TEMPO_GET_PROJECT_ACCOUNTS, projectKey),
  getCapexMap: (issueKeys: string[]) =>
    ipcRenderer.invoke(IPC_INVOKE.TEMPO_GET_CAPEX_MAP, issueKeys),
  getSchedule: (from: string, to: string) =>
    ipcRenderer.invoke(IPC_INVOKE.TEMPO_GET_SCHEDULE, { from, to }),
})

contextBridge.exposeInMainWorld('copilotSessions', {
  scan: () => ipcRenderer.invoke(IPC_INVOKE.COPILOT_SESSIONS_SCAN),
  getSession: (filePath: string) =>
    ipcRenderer.invoke(IPC_INVOKE.COPILOT_SESSIONS_GET_SESSION, filePath),
  computeDigest: (filePath: string) =>
    ipcRenderer.invoke(IPC_INVOKE.COPILOT_SESSIONS_COMPUTE_DIGEST, filePath),
})

contextBridge.exposeInMainWorld('todoist', {
  getUpcoming: (days?: number) => ipcRenderer.invoke(IPC_INVOKE.TODOIST_GET_UPCOMING, days),
  getToday: () => ipcRenderer.invoke(IPC_INVOKE.TODOIST_GET_TODAY),
  completeTask: (taskId: string) => ipcRenderer.invoke(IPC_INVOKE.TODOIST_COMPLETE_TASK, taskId),
  reopenTask: (taskId: string) => ipcRenderer.invoke(IPC_INVOKE.TODOIST_REOPEN_TASK, taskId),
  createTask: (params: {
    content: string
    due_date?: string
    priority?: number
    project_id?: string
    description?: string
  }) => ipcRenderer.invoke(IPC_INVOKE.TODOIST_CREATE_TASK, params),
  updateTask: (
    taskId: string,
    params: { content?: string; due_date?: string; priority?: number; description?: string }
  ) => ipcRenderer.invoke(IPC_INVOKE.TODOIST_UPDATE_TASK, { taskId, params }),
  deleteTask: (taskId: string) => ipcRenderer.invoke(IPC_INVOKE.TODOIST_DELETE_TASK, taskId),
  getProjects: () => ipcRenderer.invoke(IPC_INVOKE.TODOIST_GET_PROJECTS),
})

contextBridge.exposeInMainWorld('finance', {
  fetchQuote: (symbol: string) => ipcRenderer.invoke(IPC_INVOKE.FINANCE_FETCH_QUOTE, symbol),
})

contextBridge.exposeInMainWorld('slack', {
  nudgeAuthor: (params: { githubLogin: string; prTitle: string; prUrl: string }) =>
    ipcRenderer.invoke(IPC_INVOKE.SLACK_NUDGE_AUTHOR, params),
})

contextBridge.exposeInMainWorld('filesystem', {
  readDir: (dirPath: string) => ipcRenderer.invoke(IPC_INVOKE.FILESYSTEM_READ_DIR, dirPath),
  readFile: (filePath: string) => ipcRenderer.invoke(IPC_INVOKE.FILESYSTEM_READ_FILE, filePath),
})

contextBridge.exposeInMainWorld('terminal', {
  spawn: (opts: { cwd?: string; cols?: number; rows?: number; startupCommand?: string }) =>
    ipcRenderer.invoke(IPC_INVOKE.TERMINAL_SPAWN, opts),
  attach: (sessionId: string) => ipcRenderer.invoke(IPC_INVOKE.TERMINAL_ATTACH, sessionId),
  write: (sessionId: string, data: string) =>
    ipcRenderer.send(IPC_SEND.TERMINAL_WRITE, sessionId, data),
  resize: (sessionId: string, cols: number, rows: number) =>
    ipcRenderer.send(IPC_SEND.TERMINAL_RESIZE, sessionId, cols, rows),
  kill: (sessionId: string) => ipcRenderer.invoke(IPC_INVOKE.TERMINAL_KILL, sessionId),
  resolveRepoPath: (owner: string, repo: string) =>
    ipcRenderer.invoke(IPC_INVOKE.TERMINAL_RESOLVE_REPO_PATH, { owner, repo }),
})

contextBridge.exposeInMainWorld('ralph', {
  launch: (config: {
    repoPath: string
    scriptType: string
    templateScript?: string
    model?: string
    provider?: string
    agents?: string[]
    iterations?: number
    workUntil?: string
    branch?: string
    prompt?: string
    noAudio?: boolean
    skipReview?: boolean
    noPR?: boolean
  }) => ipcRenderer.invoke(IPC_INVOKE.RALPH_LAUNCH, config),
  stop: (runId: string) => ipcRenderer.invoke(IPC_INVOKE.RALPH_STOP, runId),
  list: () => ipcRenderer.invoke(IPC_INVOKE.RALPH_LIST),
  getStatus: (runId: string) => ipcRenderer.invoke(IPC_INVOKE.RALPH_GET_STATUS, runId),
  getConfig: (configType: string) => ipcRenderer.invoke(IPC_INVOKE.RALPH_GET_CONFIG, configType),
  getScriptsPath: () => ipcRenderer.invoke(IPC_INVOKE.RALPH_GET_SCRIPTS_PATH),
  listTemplates: () => ipcRenderer.invoke(IPC_INVOKE.RALPH_LIST_TEMPLATES),
  selectDirectory: (defaultPath?: string) =>
    ipcRenderer.invoke(IPC_INVOKE.RALPH_SELECT_DIRECTORY, defaultPath),
  onStatusChange: (callback: (...args: unknown[]) => void) => {
    const wrapper = (_event: Electron.IpcRendererEvent, ...rest: unknown[]) => callback(...rest)
    if (!ipcListenerWrappers.has(IPC_PUSH.RALPH_STATUS_UPDATE))
      ipcListenerWrappers.set(IPC_PUSH.RALPH_STATUS_UPDATE, new Map())
    // react-doctor-disable-next-line react-doctor/no-non-null-assertion-on-maybe-undefined-result -- The map is populated or its entry checked immediately above, with no asynchronous boundary before this access.
    ipcListenerWrappers.get(IPC_PUSH.RALPH_STATUS_UPDATE)!.set(callback, wrapper)
    ipcRenderer.on(IPC_PUSH.RALPH_STATUS_UPDATE, wrapper)
  },
  offStatusChange: (callback: (...args: unknown[]) => void) => {
    const wrapper = ipcListenerWrappers.get(IPC_PUSH.RALPH_STATUS_UPDATE)?.get(callback)
    if (wrapper) {
      // react-doctor-disable-next-line react-doctor/no-non-null-assertion-on-maybe-undefined-result -- The map is populated or its entry checked immediately above, with no asynchronous boundary before this access.
      ipcListenerWrappers.get(IPC_PUSH.RALPH_STATUS_UPDATE)!.delete(callback)
      ipcRenderer.off(IPC_PUSH.RALPH_STATUS_UPDATE, wrapper)
    }
  },
})

contextBridge.exposeInMainWorld('copilot', {
  execute: (args: { prompt: string; category?: string; metadata?: unknown; model?: string }) =>
    ipcRenderer.invoke(IPC_INVOKE.COPILOT_EXECUTE, args),
  cancel: (resultId: string) => ipcRenderer.invoke(IPC_INVOKE.COPILOT_CANCEL, resultId),
  getActiveCount: () => ipcRenderer.invoke(IPC_INVOKE.COPILOT_ACTIVE_COUNT),
  listModels: (ghAccount?: string) => ipcRenderer.invoke(IPC_INVOKE.COPILOT_LIST_MODELS, ghAccount),
  chatSend: (args: {
    message: string
    context: string
    conversationHistory: Array<{ role: string; content: string }>
    model?: string
    ghAccount?: string
  }) => ipcRenderer.invoke(IPC_INVOKE.COPILOT_CHAT_SEND, args),
  chatAbort: () => ipcRenderer.invoke(IPC_INVOKE.COPILOT_CHAT_ABORT),
  quickPrompt: (args: { prompt: string; model?: string }) =>
    ipcRenderer.invoke(IPC_INVOKE.COPILOT_QUICK_PROMPT, args) as Promise<string>,
})

contextBridge.exposeInMainWorld('__buddyPreloadReady', true)
