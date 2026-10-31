import { describe, it, expect, vi, beforeEach } from 'vitest'
import { IPC_INVOKE, IPC_SEND, IPC_PUSH } from '../src/ipc/contracts'

const { mockOn, mockOff, mockSend, mockInvoke, exposedApis, exposureOrder } = vi.hoisted(() => ({
  mockOn: vi.fn(),
  mockOff: vi.fn(),
  mockSend: vi.fn(),
  mockInvoke: vi.fn(),
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  exposedApis: {} as Record<string, any>,
  exposureOrder: [] as string[],
}))

vi.mock('electron', () => ({
  contextBridge: {
    exposeInMainWorld: vi.fn((name: string, api: unknown) => {
      exposedApis[name] = api
      exposureOrder.push(name)
    }),
  },
  ipcRenderer: {
    on: (...args: unknown[]) => mockOn(...args),
    off: (...args: unknown[]) => mockOff(...args),
    send: (...args: unknown[]) => mockSend(...args),
    invoke: (...args: unknown[]) => mockInvoke(...args),
  },
}))

// Import triggers the module to execute and call exposeInMainWorld
import './preload'

describe('preload', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('exposes the readiness sentinel after every preload bridge', () => {
    expect(exposedApis.__buddyPreloadReady).toBe(true)
    expect(exposureOrder.at(-1)).toBe('__buddyPreloadReady')
  })

  describe('ipcRenderer bridge', () => {
    it('exposes ipcRenderer API with on/off/send/invoke', () => {
      expect(exposedApis.ipcRenderer).toBeDefined()
      expect(exposedApis.ipcRenderer.on).toBeTypeOf('function')
      expect(exposedApis.ipcRenderer.off).toBeTypeOf('function')
      expect(exposedApis.ipcRenderer.send).toBeTypeOf('function')
      expect(exposedApis.ipcRenderer.invoke).toBeTypeOf('function')
    })

    it('on() wraps the listener and registers with ipcRenderer.on', () => {
      const listener = vi.fn()
      exposedApis.ipcRenderer.on('test-channel', listener)
      expect(mockOn).toHaveBeenCalledWith('test-channel', expect.any(Function))
      // The wrapper should NOT be the original listener (it wraps it)
      const registeredWrapper = mockOn.mock.calls[0][1]
      expect(registeredWrapper).not.toBe(listener)
    })

    it('on() wrapper forwards events to the original listener', () => {
      const listener = vi.fn()
      exposedApis.ipcRenderer.on('data-channel', listener)
      const registeredWrapper = mockOn.mock.calls[0][1]
      const mockEvent = { sender: {} }
      registeredWrapper(mockEvent, 'arg1', 'arg2')
      expect(listener).toHaveBeenCalledWith(mockEvent, 'arg1', 'arg2')
    })

    it('off() removes the correct wrapper registered by on()', () => {
      const listener = vi.fn()
      exposedApis.ipcRenderer.on('remove-channel', listener)
      const registeredWrapper = mockOn.mock.calls[0][1]

      exposedApis.ipcRenderer.off('remove-channel', listener)
      expect(mockOff).toHaveBeenCalledWith('remove-channel', registeredWrapper)
    })

    it('off() does nothing when listener was never registered', () => {
      const unknownListener = vi.fn()
      exposedApis.ipcRenderer.off('unknown-channel', unknownListener)
      expect(mockOff).not.toHaveBeenCalled()
    })

    it('send() forwards channel and args to ipcRenderer.send', () => {
      exposedApis.ipcRenderer.send('msg-channel', 'payload1', 'payload2')
      expect(mockSend).toHaveBeenCalledWith('msg-channel', 'payload1', 'payload2')
    })

    it('invoke() forwards channel and args to ipcRenderer.invoke', () => {
      exposedApis.ipcRenderer.invoke('invoke-channel', 'data')
      expect(mockInvoke).toHaveBeenCalledWith('invoke-channel', 'data')
    })
  })

  describe('ipcRenderer subscription disposal', () => {
    it('returns an idempotent disposer for the captured wrapper', () => {
      const listener = vi.fn()
      const dispose = exposedApis.ipcRenderer.on('dispose-channel', listener)
      const wrapper = mockOn.mock.calls[0][1]
      dispose()
      dispose()
      exposedApis.ipcRenderer.off('dispose-channel', listener)
      expect(mockOff.mock.calls).toEqual([['dispose-channel', wrapper]])
    })

    it('disposes one subscription without removing another listener on the channel', () => {
      const first = vi.fn()
      const second = vi.fn()
      const disposeFirst = exposedApis.ipcRenderer.on('shared-channel', first)
      const firstWrapper = mockOn.mock.calls[0][1]
      const disposeSecond = exposedApis.ipcRenderer.on('shared-channel', second)
      const secondWrapper = mockOn.mock.calls[1][1]
      disposeFirst()
      expect(mockOff.mock.calls).toEqual([['shared-channel', firstWrapper]])
      disposeSecond()
      expect(mockOff.mock.calls[1]).toEqual(['shared-channel', secondWrapper])
    })

    it('keeps a newer registration discoverable when disposing an older one', () => {
      const listener = vi.fn()
      const disposeOld = exposedApis.ipcRenderer.on('reused-channel', listener)
      const oldWrapper = mockOn.mock.calls[0][1]
      exposedApis.ipcRenderer.on('reused-channel', listener)
      const newWrapper = mockOn.mock.calls[1][1]
      disposeOld()
      exposedApis.ipcRenderer.off('reused-channel', listener)
      expect(mockOff.mock.calls).toEqual([
        ['reused-channel', oldWrapper],
        ['reused-channel', newWrapper],
      ])
    })

    it('removes a captured older wrapper after the newer registration clears the channel', () => {
      const listener = vi.fn()
      const disposeOld = exposedApis.ipcRenderer.on('cleared-channel', listener)
      const oldWrapper = mockOn.mock.calls[0][1]
      const disposeNew = exposedApis.ipcRenderer.on('cleared-channel', listener)
      const newWrapper = mockOn.mock.calls[1][1]
      disposeNew()
      disposeOld()
      expect(mockOff.mock.calls).toEqual([
        ['cleared-channel', newWrapper],
        ['cleared-channel', oldWrapper],
      ])
    })
  })

  describe('shell bridge', () => {
    it('exposes shell API', () => {
      expect(exposedApis.shell).toBeDefined()
      expect(exposedApis.shell.openExternal).toBeTypeOf('function')
      expect(exposedApis.shell.openInAppBrowser).toBeTypeOf('function')
      expect(exposedApis.shell.fetchPageTitle).toBeTypeOf('function')
    })

    it('openExternal invokes shell:open-external', () => {
      exposedApis.shell.openExternal('https://example.com')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.SHELL_OPEN_EXTERNAL, 'https://example.com')
    })

    it('openInAppBrowser invokes shell:open-in-app-browser', () => {
      exposedApis.shell.openInAppBrowser('https://test.com', 'Title')
      expect(mockInvoke).toHaveBeenCalledWith(
        IPC_INVOKE.SHELL_OPEN_IN_APP_BROWSER,
        'https://test.com',
        'Title'
      )
    })
  })

  describe('github bridge', () => {
    it('exposes github API', () => {
      expect(exposedApis.github).toBeDefined()
      expect(exposedApis.github.getCliToken).toBeTypeOf('function')
      expect(exposedApis.github.getActiveAccount).toBeTypeOf('function')
    })

    it('getCliToken invokes github:get-cli-token', () => {
      exposedApis.github.getCliToken('testuser')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.GITHUB_GET_CLI_TOKEN, 'testuser')
    })

    it('getActiveAccount invokes github:get-active-account', () => {
      exposedApis.github.getActiveAccount()
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.GITHUB_GET_ACTIVE_ACCOUNT)
    })
  })

  describe('crew bridge', () => {
    it('exposes crew API', () => {
      expect(exposedApis.crew).toBeDefined()
      expect(exposedApis.crew.addProject).toBeTypeOf('function')
      expect(exposedApis.crew.listProjects).toBeTypeOf('function')
    })

    it('addProject invokes crew:add-project', () => {
      exposedApis.crew.addProject()
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.CREW_ADD_PROJECT)
    })

    it('removeProject invokes crew:remove-project with projectId', () => {
      exposedApis.crew.removeProject('proj-123')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.CREW_REMOVE_PROJECT, 'proj-123')
    })
  })

  describe('tempo bridge', () => {
    it('exposes tempo API', () => {
      expect(exposedApis.tempo).toBeDefined()
      expect(exposedApis.tempo.getToday).toBeTypeOf('function')
      expect(exposedApis.tempo.createWorklog).toBeTypeOf('function')
    })

    it('getToday invokes tempo:get-today', () => {
      exposedApis.tempo.getToday('2026-01-01')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.TEMPO_GET_TODAY, '2026-01-01')
    })
  })

  describe('todoist bridge', () => {
    it('exposes todoist API', () => {
      expect(exposedApis.todoist).toBeDefined()
      expect(exposedApis.todoist.getUpcoming).toBeTypeOf('function')
      expect(exposedApis.todoist.getToday).toBeTypeOf('function')
      expect(exposedApis.todoist.completeTask).toBeTypeOf('function')
      expect(exposedApis.todoist.createTask).toBeTypeOf('function')
      expect(exposedApis.todoist.deleteTask).toBeTypeOf('function')
    })

    it('getToday invokes todoist:get-today', () => {
      exposedApis.todoist.getToday()
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.TODOIST_GET_TODAY)
    })

    it('completeTask invokes todoist:complete-task with taskId', () => {
      exposedApis.todoist.completeTask('task-1')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.TODOIST_COMPLETE_TASK, 'task-1')
    })

    it('createTask invokes todoist:create-task with params', () => {
      const params = { content: 'Buy milk', due_date: '2026-01-01' }
      exposedApis.todoist.createTask(params)
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.TODOIST_CREATE_TASK, params)
    })
  })

  describe('finance bridge', () => {
    it('exposes finance API', () => {
      expect(exposedApis.finance).toBeDefined()
      expect(exposedApis.finance.fetchQuote).toBeTypeOf('function')
    })

    it('fetchQuote invokes finance:fetch-quote', () => {
      exposedApis.finance.fetchQuote('AAPL')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.FINANCE_FETCH_QUOTE, 'AAPL')
    })
  })

  describe('slack bridge', () => {
    it('exposes slack API', () => {
      expect(exposedApis.slack).toBeDefined()
      expect(exposedApis.slack.nudgeAuthor).toBeTypeOf('function')
    })

    it('nudgeAuthor invokes slack:nudge-author with params', () => {
      const params = { githubLogin: 'user1', prTitle: 'Fix bug', prUrl: 'https://github.com/pr/1' }
      exposedApis.slack.nudgeAuthor(params)
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.SLACK_NUDGE_AUTHOR, params)
    })
  })

  describe('filesystem bridge', () => {
    it('exposes filesystem API', () => {
      expect(exposedApis.filesystem).toBeDefined()
      expect(exposedApis.filesystem.readDir).toBeTypeOf('function')
      expect(exposedApis.filesystem.readFile).toBeTypeOf('function')
    })

    it('readDir invokes fs:read-dir', () => {
      exposedApis.filesystem.readDir('/tmp')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.FILESYSTEM_READ_DIR, '/tmp')
    })

    it('readFile invokes fs:read-file', () => {
      exposedApis.filesystem.readFile('/tmp/file.txt')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.FILESYSTEM_READ_FILE, '/tmp/file.txt')
    })
  })

  describe('terminal bridge', () => {
    it('exposes terminal API', () => {
      expect(exposedApis.terminal).toBeDefined()
      expect(exposedApis.terminal.spawn).toBeTypeOf('function')
      expect(exposedApis.terminal.attach).toBeTypeOf('function')
      expect(exposedApis.terminal.write).toBeTypeOf('function')
      expect(exposedApis.terminal.resize).toBeTypeOf('function')
      expect(exposedApis.terminal.kill).toBeTypeOf('function')
    })

    it('spawn invokes terminal:spawn', () => {
      exposedApis.terminal.spawn({ cwd: '/home' })
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.TERMINAL_SPAWN, { cwd: '/home' })
    })

    it('write sends terminal:write', () => {
      exposedApis.terminal.write('session-1', 'ls\n')
      expect(mockSend).toHaveBeenCalledWith(IPC_SEND.TERMINAL_WRITE, 'session-1', 'ls\n')
    })

    it('resize sends terminal:resize', () => {
      exposedApis.terminal.resize('session-1', 80, 24)
      expect(mockSend).toHaveBeenCalledWith(IPC_SEND.TERMINAL_RESIZE, 'session-1', 80, 24)
    })

    it('kill invokes terminal:kill', () => {
      exposedApis.terminal.kill('session-1')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.TERMINAL_KILL, 'session-1')
    })
  })

  describe('ralph bridge', () => {
    it('exposes ralph API', () => {
      expect(exposedApis.ralph).toBeDefined()
      expect(exposedApis.ralph.launch).toBeTypeOf('function')
      expect(exposedApis.ralph.stop).toBeTypeOf('function')
      expect(exposedApis.ralph.list).toBeTypeOf('function')
      expect(exposedApis.ralph.getStatus).toBeTypeOf('function')
      expect(exposedApis.ralph.onStatusChange).toBeTypeOf('function')
      expect(exposedApis.ralph.offStatusChange).toBeTypeOf('function')
    })

    it('launch invokes ralph:launch with config', () => {
      const config = { repoPath: '/repo', scriptType: 'fix' }
      exposedApis.ralph.launch(config)
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.RALPH_LAUNCH, config)
    })

    it('stop invokes ralph:stop', () => {
      exposedApis.ralph.stop('run-1')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.RALPH_STOP, 'run-1')
    })

    it('list invokes ralph:list', () => {
      exposedApis.ralph.list()
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.RALPH_LIST)
    })

    it('onStatusChange registers a listener and offStatusChange removes it', () => {
      const cb = vi.fn()
      exposedApis.ralph.onStatusChange(cb)
      expect(mockOn).toHaveBeenCalledWith(IPC_PUSH.RALPH_STATUS_UPDATE, expect.any(Function))

      exposedApis.ralph.offStatusChange(cb)
      // The wrapper registered by on should be passed to off
      const registeredWrapper = mockOn.mock.calls.find(
        (c: unknown[]) => c[0] === IPC_PUSH.RALPH_STATUS_UPDATE
      )?.[1]
      expect(mockOff).toHaveBeenCalledWith(IPC_PUSH.RALPH_STATUS_UPDATE, registeredWrapper)
    })
  })

  describe('copilot bridge', () => {
    it('exposes copilot API', () => {
      expect(exposedApis.copilot).toBeDefined()
      expect(exposedApis.copilot.execute).toBeTypeOf('function')
      expect(exposedApis.copilot.cancel).toBeTypeOf('function')
      expect(exposedApis.copilot.getActiveCount).toBeTypeOf('function')
      expect(exposedApis.copilot.listModels).toBeTypeOf('function')
      expect(exposedApis.copilot.chatSend).toBeTypeOf('function')
      expect(exposedApis.copilot.chatAbort).toBeTypeOf('function')
    })

    it('execute invokes copilot:execute with args', () => {
      const args = { prompt: 'hello', category: 'general' }
      exposedApis.copilot.execute(args)
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.COPILOT_EXECUTE, args)
    })

    it('cancel invokes copilot:cancel', () => {
      exposedApis.copilot.cancel('result-1')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.COPILOT_CANCEL, 'result-1')
    })

    it('listModels invokes copilot:list-models', () => {
      exposedApis.copilot.listModels('user1')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.COPILOT_LIST_MODELS, 'user1')
    })

    it('chatAbort invokes copilot:chat-abort', () => {
      exposedApis.copilot.chatAbort()
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.COPILOT_CHAT_ABORT)
    })
  })

  describe('copilotSessions bridge', () => {
    it('exposes copilotSessions API', () => {
      expect(exposedApis.copilotSessions).toBeDefined()
      expect(exposedApis.copilotSessions.scan).toBeTypeOf('function')
      expect(exposedApis.copilotSessions.getSession).toBeTypeOf('function')
      expect(exposedApis.copilotSessions.computeDigest).toBeTypeOf('function')
    })

    it('scan invokes copilot-sessions:scan', () => {
      exposedApis.copilotSessions.scan()
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.COPILOT_SESSIONS_SCAN)
    })

    it('getSession invokes copilot-sessions:get-session', () => {
      exposedApis.copilotSessions.getSession('/path/to/session')
      expect(mockInvoke).toHaveBeenCalledWith(
        IPC_INVOKE.COPILOT_SESSIONS_GET_SESSION,
        '/path/to/session'
      )
    })

    it('computeDigest invokes copilot-sessions:compute-digest', () => {
      exposedApis.copilotSessions.computeDigest('/path/to/session')
      expect(mockInvoke).toHaveBeenCalledWith(
        IPC_INVOKE.COPILOT_SESSIONS_COMPUTE_DIGEST,
        '/path/to/session'
      )
    })
  })

  // ── Additional bridge method invocation tests ────────────

  describe('github bridge - additional methods', () => {
    it('switchAccount invokes github:switch-account', () => {
      exposedApis.github.switchAccount('user2')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.GITHUB_SWITCH_ACCOUNT, 'user2')
    })

    it('getCopilotUsage invokes github:get-copilot-usage', () => {
      exposedApis.github.getCopilotUsage('my-org', 'user1')
      expect(mockInvoke).toHaveBeenCalledWith(
        IPC_INVOKE.GITHUB_GET_COPILOT_USAGE,
        'my-org',
        'user1'
      )
    })

    it('getCopilotQuota invokes github:get-copilot-quota', () => {
      exposedApis.github.getCopilotQuota('user1')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.GITHUB_GET_COPILOT_QUOTA, 'user1')
    })

    it('getCopilotBudget invokes github:get-copilot-budget', () => {
      exposedApis.github.getCopilotBudget('my-org', 'user1')
      expect(mockInvoke).toHaveBeenCalledWith(
        IPC_INVOKE.GITHUB_GET_COPILOT_BUDGET,
        'my-org',
        'user1'
      )
    })

    it('getCopilotMemberUsage invokes github:get-copilot-member-usage', () => {
      exposedApis.github.getCopilotMemberUsage('my-org', 'member1', 'user1')
      expect(mockInvoke).toHaveBeenCalledWith(
        IPC_INVOKE.GITHUB_GET_COPILOT_MEMBER_USAGE,
        'my-org',
        'member1',
        'user1'
      )
    })

    it('getUserPremiumRequests invokes github:get-user-premium-requests', () => {
      exposedApis.github.getUserPremiumRequests('my-org', 'member1', 'user1')
      expect(mockInvoke).toHaveBeenCalledWith(
        IPC_INVOKE.GITHUB_GET_USER_PREMIUM_REQUESTS,
        'my-org',
        'member1',
        'user1'
      )
    })

    it('getCopilotSeats invokes github:get-copilot-seats', () => {
      exposedApis.github.getCopilotSeats('my-org', 'user1')
      expect(mockInvoke).toHaveBeenCalledWith(
        IPC_INVOKE.GITHUB_GET_COPILOT_SEATS,
        'my-org',
        'user1'
      )
    })

    it('getBatchMonthlyRequests invokes github:get-batch-monthly-requests', () => {
      exposedApis.github.getBatchMonthlyRequests(['user1', 'user2'], 'owner', true)
      expect(mockInvoke).toHaveBeenCalledWith(
        IPC_INVOKE.GITHUB_GET_BATCH_MONTHLY_REQUESTS,
        ['user1', 'user2'],
        'owner',
        true
      )
    })

    it('getCopilotEnterpriseUsers invokes github:get-copilot-enterprise-users', () => {
      exposedApis.github.getCopilotEnterpriseUsers()
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.GITHUB_GET_COPILOT_ENTERPRISE_USERS)
    })
  })

  describe('codex bridge', () => {
    it('getUsage invokes codex:get-usage', () => {
      exposedApis.codex.getUsage()
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.CODEX_GET_USAGE)
    })
  })

  describe('crew bridge - additional methods', () => {
    it('listProjects invokes crew:list-projects', () => {
      exposedApis.crew.listProjects()
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.CREW_LIST_PROJECTS)
    })

    it('getSession invokes crew:get-session', () => {
      exposedApis.crew.getSession('proj-1')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.CREW_GET_SESSION, 'proj-1')
    })

    it('createSession invokes crew:create-session', () => {
      exposedApis.crew.createSession('proj-1')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.CREW_CREATE_SESSION, 'proj-1')
    })

    it('addMessage invokes crew:add-message', () => {
      const msg = { role: 'user', content: 'hello', timestamp: 1000 }
      exposedApis.crew.addMessage('proj-1', msg)
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.CREW_ADD_MESSAGE, 'proj-1', msg)
    })

    it('updateSessionStatus invokes crew:update-session-status', () => {
      exposedApis.crew.updateSessionStatus('proj-1', 'active')
      expect(mockInvoke).toHaveBeenCalledWith(
        IPC_INVOKE.CREW_UPDATE_SESSION_STATUS,
        'proj-1',
        'active'
      )
    })

    it('updateChangedFiles invokes crew:update-changed-files', () => {
      const files = [{ path: 'src/a.ts' }]
      exposedApis.crew.updateChangedFiles('proj-1', files)
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.CREW_UPDATE_CHANGED_FILES, 'proj-1', files)
    })

    it('clearSession invokes crew:clear-session', () => {
      exposedApis.crew.clearSession('proj-1')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.CREW_CLEAR_SESSION, 'proj-1')
    })

    it('undoFile invokes crew:undo-file', () => {
      exposedApis.crew.undoFile('proj-1', 'src/file.ts')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.CREW_UNDO_FILE, 'proj-1', 'src/file.ts')
    })
  })

  describe('tempo bridge - additional methods', () => {
    it('getRange invokes tempo:get-range', () => {
      exposedApis.tempo.getRange('2025-01-01', '2025-01-07')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.TEMPO_GET_RANGE, {
        from: '2025-01-01',
        to: '2025-01-07',
      })
    })

    it('getWeek invokes tempo:get-week', () => {
      exposedApis.tempo.getWeek('2025-01-06', '2025-01-10')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.TEMPO_GET_WEEK, {
        weekStart: '2025-01-06',
        weekEnd: '2025-01-10',
      })
    })

    it('createWorklog invokes tempo:create-worklog', () => {
      const payload = { issueKey: 'PROJ-1', hours: 2, date: '2025-01-01' }
      exposedApis.tempo.createWorklog(payload)
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.TEMPO_CREATE_WORKLOG, payload)
    })

    it('updateWorklog invokes tempo:update-worklog', () => {
      exposedApis.tempo.updateWorklog(42, { hours: 3 })
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.TEMPO_UPDATE_WORKLOG, {
        worklogId: 42,
        payload: { hours: 3 },
      })
    })

    it('deleteWorklog invokes tempo:delete-worklog', () => {
      exposedApis.tempo.deleteWorklog(42)
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.TEMPO_DELETE_WORKLOG, 42)
    })

    it('getAccounts invokes tempo:get-accounts', () => {
      exposedApis.tempo.getAccounts()
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.TEMPO_GET_ACCOUNTS)
    })

    it('getProjectAccounts invokes tempo:get-project-accounts', () => {
      exposedApis.tempo.getProjectAccounts('PROJ')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.TEMPO_GET_PROJECT_ACCOUNTS, 'PROJ')
    })

    it('getCapexMap invokes tempo:get-capex-map', () => {
      exposedApis.tempo.getCapexMap(['PROJ-1', 'PROJ-2'])
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.TEMPO_GET_CAPEX_MAP, ['PROJ-1', 'PROJ-2'])
    })

    it('getSchedule invokes tempo:get-schedule', () => {
      exposedApis.tempo.getSchedule('2025-01-01', '2025-01-31')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.TEMPO_GET_SCHEDULE, {
        from: '2025-01-01',
        to: '2025-01-31',
      })
    })
  })

  describe('todoist bridge - additional methods', () => {
    it('getUpcoming invokes todoist:get-upcoming', () => {
      exposedApis.todoist.getUpcoming(7)
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.TODOIST_GET_UPCOMING, 7)
    })

    it('reopenTask invokes todoist:reopen-task', () => {
      exposedApis.todoist.reopenTask('task-1')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.TODOIST_REOPEN_TASK, 'task-1')
    })

    it('updateTask invokes todoist:update-task', () => {
      exposedApis.todoist.updateTask('task-1', { content: 'Updated' })
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.TODOIST_UPDATE_TASK, {
        taskId: 'task-1',
        params: { content: 'Updated' },
      })
    })

    it('deleteTask invokes todoist:delete-task', () => {
      exposedApis.todoist.deleteTask('task-1')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.TODOIST_DELETE_TASK, 'task-1')
    })

    it('getProjects invokes todoist:get-projects', () => {
      exposedApis.todoist.getProjects()
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.TODOIST_GET_PROJECTS)
    })
  })

  describe('terminal bridge - additional methods', () => {
    it('attach invokes terminal:attach', () => {
      exposedApis.terminal.attach('session-1')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.TERMINAL_ATTACH, 'session-1')
    })

    it('resolveRepoPath invokes terminal:resolve-repo-path', () => {
      exposedApis.terminal.resolveRepoPath('owner', 'repo')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.TERMINAL_RESOLVE_REPO_PATH, {
        owner: 'owner',
        repo: 'repo',
      })
    })
  })

  describe('ralph bridge - additional methods', () => {
    it('getStatus invokes ralph:get-status', () => {
      exposedApis.ralph.getStatus('run-1')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.RALPH_GET_STATUS, 'run-1')
    })

    it('getConfig invokes ralph:get-config', () => {
      exposedApis.ralph.getConfig('models')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.RALPH_GET_CONFIG, 'models')
    })

    it('getScriptsPath invokes ralph:get-scripts-path', () => {
      exposedApis.ralph.getScriptsPath()
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.RALPH_GET_SCRIPTS_PATH)
    })

    it('listTemplates invokes ralph:list-templates', () => {
      exposedApis.ralph.listTemplates()
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.RALPH_LIST_TEMPLATES)
    })

    it('selectDirectory invokes ralph:select-directory', () => {
      exposedApis.ralph.selectDirectory('/default/path')
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.RALPH_SELECT_DIRECTORY, '/default/path')
    })
  })

  describe('copilot bridge - additional methods', () => {
    it('getActiveCount invokes copilot:active-count', () => {
      exposedApis.copilot.getActiveCount()
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.COPILOT_ACTIVE_COUNT)
    })

    it('chatSend invokes copilot:chat-send', () => {
      const args = {
        message: 'hello',
        context: 'test',
        conversationHistory: [],
        model: 'gpt-4',
      }
      exposedApis.copilot.chatSend(args)
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.COPILOT_CHAT_SEND, args)
    })

    it('quickPrompt invokes copilot:quick-prompt', () => {
      exposedApis.copilot.quickPrompt({ prompt: 'summarize', model: 'gpt-4' })
      expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.COPILOT_QUICK_PROMPT, {
        prompt: 'summarize',
        model: 'gpt-4',
      })
    })
  })

  describe('shell bridge - additional methods', () => {
    it('fetchPageTitle invokes shell:fetch-page-title', () => {
      exposedApis.shell.fetchPageTitle('https://example.com')
      expect(mockInvoke).toHaveBeenCalledWith(
        IPC_INVOKE.SHELL_FETCH_PAGE_TITLE,
        'https://example.com'
      )
    })
  })
})
