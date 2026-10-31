/// <reference types="vite-plugin-electron/electron-env" />

declare namespace NodeJS {
  interface ProcessEnv {
    /**
     * The built directory structure
     *
     * ```tree
     * ├─┬─┬ dist
     * │ │ └── index.html
     * │ │
     * │ ├─┬ dist-electron
     * │ │ ├── main.js
     * │ │ └── preload.js
     * │
     * ```
     */
    APP_ROOT: string
    /** /dist/ or /public/ */
    VITE_PUBLIC: string
  }
}

// Used in Renderer process, expose in `preload.ts`

interface CopilotQuotaSnapshot {
  entitlement: number
  overage_count: number
  overage_permitted: boolean
  percent_remaining: number
  quota_id: string
  quota_remaining: number
  remaining: number
  unlimited: boolean
  timestamp_utc: string
}

interface Window {
  __buddyPreloadReady: true
  ipcRenderer: Omit<import('electron').IpcRenderer, 'on'> & {
    on: (...args: Parameters<import('electron').IpcRenderer['on']>) => () => void
  }
  shell: {
    openExternal: (url: string) => Promise<{ success: boolean; error?: string }>
    openInAppBrowser: (url: string, title?: string) => Promise<{ success: boolean; error?: string }>
    fetchPageTitle: (url: string) => Promise<{ success: boolean; title?: string; error?: string }>
  }
  crew: {
    addProject: () => Promise<import('../src/types/crew').CrewAddProjectResult>
    listProjects: () => Promise<import('../src/types/crew').CrewProject[]>
    removeProject: (projectId: string) => Promise<boolean>
    getSession: (projectId: string) => Promise<import('../src/types/crew').CrewSession | null>
    createSession: (projectId: string) => Promise<import('../src/types/crew').CrewSession>
    addMessage: (
      projectId: string,
      message: import('../src/types/crew').CrewChatMessage
    ) => Promise<import('../src/types/crew').CrewSession | null>
    updateSessionStatus: (
      projectId: string,
      status: import('../src/types/crew').CrewSession['status']
    ) => Promise<import('../src/types/crew').CrewSession | null>
    updateChangedFiles: (
      projectId: string,
      files: import('../src/types/crew').CrewChangedFile[]
    ) => Promise<import('../src/types/crew').CrewSession | null>
    clearSession: (projectId: string) => Promise<boolean>
    undoFile: (projectId: string, filePath: string) => Promise<boolean>
  }
  github: {
    getCliToken: (username?: string) => Promise<string>
    getActiveAccount: () => Promise<string | null>
    switchAccount: (username: string) => Promise<{ success: boolean; error?: string }>
    getCopilotUsage: (
      org: string,
      username?: string
    ) => Promise<{
      success: boolean
      error?: string
      data?: {
        org: string
        premiumRequests: number
        grossCost: number
        discount: number
        netCost: number
        businessSeats: number
        seatPlan: string
        seats: number
        billingMonth: number
        billingYear: number
        userCredits?: number | null
        allItems: Array<{
          date: string
          product: string
          sku: string
          quantity: number
          unitType: string
          pricePerUnit: number
          grossAmount: number
          discountAmount: number
          netAmount: number
          organizationName: string
          repositoryName: string
        }>
        fetchedAt: number
      }
    }>
    getCopilotQuota: (username: string) => Promise<{
      success: boolean
      error?: string
      data?: {
        login: string
        copilot_plan: string
        quota_reset_date: string
        quota_reset_date_utc: string
        organization_login_list: string[]
        quota_snapshots: {
          chat: CopilotQuotaSnapshot
          completions: CopilotQuotaSnapshot
          premium_interactions: CopilotQuotaSnapshot
        }
      }
    }>
    getCopilotBudget: (
      org: string,
      username?: string
    ) => Promise<{
      success: boolean
      error?: string
      data?: {
        org: string
        budgetAmount: number | null
        preventFurtherUsage: boolean
        spent: number
        gross: number
        spentUnavailable: boolean
        useQuotaOverage: boolean
        billingMonth: number
        billingYear: number
        fetchedAt: number
      }
    }>
    getCopilotMemberUsage: (
      org: string,
      memberLogin: string,
      username?: string
    ) => Promise<{
      success: boolean
      error?: string
      data?: {
        login: string
        planType: string | null
        lastActivityAt: string | null
        lastActivityEditor: string | null
        createdAt: string | null
        pendingCancellation: string | null
      } | null
    }>
    getUserPremiumRequests: (
      org: string,
      memberLogin: string,
      username?: string
    ) => Promise<{
      success: boolean
      code?: 'enterprise-not-configured'
      error?: string
      data?: {
        memberLogin: string
        org: string
        userMonthlyRequests: number
        userTodayRequests: number
        userMonthlyModels: Array<{ model: string; requests: number }>
        orgMonthlyRequests: number
        orgMonthlyNetCost: number
        billingYear: number
        billingMonth: number
      }
    }>
    getCopilotSeats: (
      org: string,
      username?: string
    ) => Promise<{
      success: boolean
      error?: string
      data?: {
        totalSeats: number
        fetchedSeats: number
        seats: Array<{
          login: string
          displayName: string | null
          planType: string | null
          lastActivityAt: string | null
          lastActivityEditor: string | null
          createdAt: string | null
          pendingCancellation: string | null
        }>
      }
    }>
    getBatchMonthlyRequests: (
      logins: string[],
      username?: string,
      skipDayProbing?: boolean
    ) => Promise<{
      success: boolean
      code?: 'enterprise-not-configured'
      error?: string
      data?: Record<string, { requests: number; lastActiveDate: string | null }>
    }>
  }
  codex: {
    getUsage: () => Promise<import('../src/types/codexUsage').CodexUsageResult>
  }
  finance: {
    fetchQuote: (symbol: string) => Promise<{
      success: boolean
      quote?: {
        symbol: string
        name: string
        price: number
        change: number
        changePercent: number
        previousClose: number
        marketOpen: boolean
      }
      error?: string
    }>
  }
  slack: {
    nudgeAuthor: (params: { githubLogin: string; prTitle: string; prUrl: string }) => Promise<{
      success: boolean
      error?: string
    }>
  }
  filesystem: {
    readDir: (dirPath: string) => Promise<{
      entries: Array<{ name: string; path: string; type: 'file' | 'directory'; size: number }>
      error?: string
    }>
    readFile: (filePath: string) => Promise<{
      content: string
      language: string
      size: number
      error?: string
    }>
  }
  terminal: {
    spawn: (opts: {
      cwd?: string
      cols?: number
      rows?: number
      startupCommand?: string
    }) => Promise<{ success: boolean; sessionId?: string; cwd?: string; error?: string }>
    attach: (sessionId: string) => Promise<{
      success: boolean
      buffer?: string
      cursor?: number
      alive?: boolean
      error?: string
    }>
    write: (sessionId: string, data: string) => void
    resize: (sessionId: string, cols: number, rows: number) => void
    kill: (sessionId: string) => Promise<{ success: boolean; error?: string }>
    resolveRepoPath: (owner: string, repo: string) => Promise<{ path: string | null }>
  }
  copilot: {
    execute: (args: {
      prompt: string
      category?: string
      metadata?: unknown
      model?: string
    }) => Promise<{ resultId: string | null; success: boolean; error?: string }>
    cancel: (resultId: string) => Promise<{ success: boolean; error?: string }>
    getActiveCount: () => Promise<number>
    listModels: (
      ghAccount?: string
    ) => Promise<
      | Array<{ id: string; name: string; isDisabled: boolean; billingMultiplier: number }>
      | { error: string }
    >
    chatSend: (args: {
      message: string
      context: string
      conversationHistory: Array<{ role: string; content: string }>
      model?: string
      ghAccount?: string
    }) => Promise<{ content?: string } | string | null>
    chatAbort: () => Promise<{ success: boolean; error?: string }>
    quickPrompt: (args: { prompt: string; model?: string }) => Promise<string>
  }
  tempo: {
    getToday: (
      date?: string
    ) => Promise<
      import('../src/types/tempo').TempoResult<import('../src/types/tempo').TempoDaySummary>
    >
    getRange: (
      from: string,
      to: string
    ) => Promise<
      import('../src/types/tempo').TempoResult<import('../src/types/tempo').TempoWorklog[]>
    >
    getWeek: (
      weekStart: string,
      weekEnd: string
    ) => Promise<
      import('../src/types/tempo').TempoResult<{
        worklogs: import('../src/types/tempo').TempoWorklog[]
        issueSummaries: import('../src/types/tempo').TempoIssueSummary[]
        totalHours: number
      }>
    >
    createWorklog: (
      payload: import('../src/types/tempo').CreateWorklogPayload
    ) => Promise<
      import('../src/types/tempo').TempoResult<import('../src/types/tempo').TempoWorklog>
    >
    updateWorklog: (
      worklogId: number,
      payload: import('../src/types/tempo').UpdateWorklogPayload
    ) => Promise<import('../src/types/tempo').TempoResult<void>>
    deleteWorklog: (worklogId: number) => Promise<import('../src/types/tempo').TempoResult<void>>
    getAccounts: () => Promise<
      import('../src/types/tempo').TempoResult<import('../src/types/tempo').TempoAccount[]>
    >
    getProjectAccounts: (
      projectKey: string
    ) => Promise<
      import('../src/types/tempo').TempoResult<{ key: string; name: string; isDefault: boolean }[]>
    >
    getCapexMap: (
      issueKeys: string[]
    ) => Promise<import('../src/types/tempo').TempoResult<Record<string, boolean>>>
    getSchedule: (
      from: string,
      to: string
    ) => Promise<
      import('../src/types/tempo').TempoResult<import('../src/types/tempo').TempoScheduleDay[]>
    >
  }
  copilotSessions: {
    scan: () => Promise<import('../src/types/copilotSession').SessionScanResult>
    getSession: (
      filePath: string
    ) => Promise<import('../src/types/copilotSession').CopilotSession | null>
    computeDigest: (
      filePath: string
    ) => Promise<import('../src/types/copilotSession').SessionDigest | null>
  }
  todoist: {
    getUpcoming: (days?: number) => Promise<{
      success: boolean
      error?: string
      data?: Record<string, import('../src/types/todoist').TodoistTask[]>
    }>
    getToday: () => Promise<{
      success: boolean
      error?: string
      data?: import('../src/types/todoist').TodoistTask[]
    }>
    completeTask: (taskId: string) => Promise<{ success: boolean; error?: string }>
    reopenTask: (taskId: string) => Promise<{ success: boolean; error?: string }>
    createTask: (params: {
      content: string
      due_date?: string
      priority?: number
      project_id?: string
      description?: string
    }) => Promise<{
      success: boolean
      error?: string
      data?: import('../src/types/todoist').TodoistTask
    }>
    updateTask: (
      taskId: string,
      params: { content?: string; due_date?: string; priority?: number; description?: string }
    ) => Promise<{
      success: boolean
      error?: string
      data?: import('../src/types/todoist').TodoistTask
    }>
    deleteTask: (taskId: string) => Promise<{ success: boolean; error?: string }>
    getProjects: () => Promise<{
      success: boolean
      error?: string
      data?: import('../src/types/todoist').TodoistProject[]
    }>
  }
  ralph: import('../src/types/ralph').RalphPreloadAPI
}
