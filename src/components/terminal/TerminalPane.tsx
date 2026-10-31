import { useEffect, useRef } from 'react'
import type { MutableRefObject } from 'react'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { Unicode11Addon } from '@xterm/addon-unicode11'
import {
  getSessionId,
  setSessionId,
  removeSession,
  setTerminalPasteHandler,
  removeTerminalPasteHandler,
} from './terminalSessions'
import { IPC_PUSH } from '../../ipc/contracts'
import '@xterm/xterm/css/xterm.css'
import './TerminalPane.css'

interface TerminalPaneProps {
  /** Unique key for this terminal view (used to persist session across tab switches) */
  viewKey: string
  cwd?: string
  startupCommand?: string
  onExit?: (exitCode: number) => void
  onCwdChange?: (newCwd: string) => void
}

function resolveSpawnDimensions(dims: { cols?: number; rows?: number } | undefined): {
  cols: number
  rows: number
} {
  return {
    /* v8 ignore start */
    cols: dims?.cols || 120,
    rows: dims?.rows || 30,
    /* v8 ignore stop */
  }
}

const NERD_FONT_FAMILY = "'CaskaydiaCove NFM', 'CaskaydiaCove NF', 'FiraCode Nerd Font Mono'"
const FALLBACK_FAMILY = "'Cascadia Code', 'Cascadia Mono', Consolas, 'Courier New', monospace"
const FULL_FONT_FAMILY = `${NERD_FONT_FAMILY}, ${FALLBACK_FAMILY}`

const TERMINAL_THEME = {
  background: '#0c0c0c',
  foreground: '#cccccc',
  cursor: '#ffffff',
  selectionBackground: '#264f78',
  black: '#000000',
  red: '#cd3131',
  green: '#0dbc79',
  yellow: '#e5e510',
  blue: '#2472c8',
  magenta: '#bc3fbc',
  cyan: '#11a8cd',
  white: '#e5e5e5',
  brightBlack: '#666666',
  brightRed: '#f14c4c',
  brightGreen: '#23d18b',
  brightYellow: '#f5f543',
  brightBlue: '#3b8eea',
  brightMagenta: '#d670d6',
  brightCyan: '#29b8db',
  brightWhite: '#e5e5e5',
} as const

function applyCursorRowHighlight(container: HTMLElement, term: Terminal) {
  const rowContainer = container.querySelector('.xterm-rows')
  if (!rowContainer) return
  const prev = rowContainer.querySelector('.xterm-cursor-row')
  if (prev) prev.classList.remove('xterm-cursor-row')
  const cursorY = term.buffer.active.cursorY
  const row = rowContainer.children[cursorY]
  if (row) row.classList.add('xterm-cursor-row')
}

function createTerminalInstance(container: HTMLElement): { term: Terminal; fitAddon: FitAddon } {
  const term = new Terminal({
    cursorBlink: true,
    fontSize: 14,
    fontFamily: FULL_FONT_FAMILY,
    theme: TERMINAL_THEME,
    allowProposedApi: true,
  })

  const fitAddon = new FitAddon()
  term.loadAddon(fitAddon)

  const unicode11Addon = new Unicode11Addon()
  term.loadAddon(unicode11Addon)

  term.open(container)
  term.unicode.activeVersion = '11'

  return { term, fitAddon }
}

interface TerminalAttachResult {
  success: boolean
  buffer?: string
  cursor?: number | null
  alive?: boolean
}

interface TerminalSpawnResult {
  success: boolean
  sessionId?: string
  cwd?: string
  error?: string
}

interface SuccessfulTerminalSpawnResult extends TerminalSpawnResult {
  success: true
  sessionId: string
}

function applyExistingSessionResult(
  term: Terminal,
  result: TerminalAttachResult,
  attachCursorRef: MutableRefObject<number>
) {
  if (result.buffer) term.write(result.buffer)
  if (result.cursor != null) attachCursorRef.current = result.cursor
  if (result.alive === false) term.writeln('\r\n\x1b[90m[Process has exited]\x1b[0m')
}

function applyAttachResult(
  term: Terminal,
  result: TerminalAttachResult,
  attachCursorRef: MutableRefObject<number>
) {
  if (!result.success) return
  if (result.buffer) term.write(result.buffer)
  if (result.cursor != null) attachCursorRef.current = result.cursor
}

function hasSpawnSession(result: TerminalSpawnResult): result is SuccessfulTerminalSpawnResult {
  return result.success && Boolean(result.sessionId)
}

function writeSpawnFailure(term: Terminal, result: TerminalSpawnResult) {
  term.writeln(`\r\n\x1b[31mFailed to spawn terminal: ${result.error || 'Unknown error'}\x1b[0m`)
}

function applySpawnSuccess(
  viewKey: string,
  result: SuccessfulTerminalSpawnResult,
  sessionIdRef: MutableRefObject<string | null>,
  onCwdChange: TerminalPaneProps['onCwdChange']
) {
  sessionIdRef.current = result.sessionId
  setSessionId(viewKey, result.sessionId)
  if (result.cwd) onCwdChange?.(result.cwd)
}

function getResizeDimensions(fit: FitAddon): { cols: number; rows: number } | null {
  const dimensions = fit.proposeDimensions()
  if (!dimensions?.cols || !dimensions.rows) return null
  return { cols: dimensions.cols, rows: dimensions.rows }
}

function hasResizeChanged(
  last: { cols: number; rows: number } | null,
  next: { cols: number; rows: number }
): boolean {
  return !last || last.cols !== next.cols || last.rows !== next.rows
}

interface TerminalEventHandlers {
  onData: (_event: unknown, sid: string, data: string, seq?: number) => void
  onExit: (_event: unknown, sid: string, exitCode: number) => void
  onCwdChanged: (_event: unknown, sid: string, newCwd: string) => void
}

function subscribeToTerminalEvents({
  onData,
  onExit,
  onCwdChanged,
}: TerminalEventHandlers): () => void {
  const removeData = window.ipcRenderer.on(IPC_PUSH.TERMINAL_DATA, onData)
  const removeExit = window.ipcRenderer.on(IPC_PUSH.TERMINAL_EXIT, onExit)
  const removeCwd = window.ipcRenderer.on(IPC_PUSH.TERMINAL_CWD_CHANGED, onCwdChanged)

  return () => {
    removeData()
    removeExit()
    removeCwd()
  }
}

export function TerminalPane({
  viewKey,
  cwd,
  startupCommand,
  onExit,
  onCwdChange,
}: TerminalPaneProps) {
  const containerRef = useRef<HTMLDivElement>(null)
  const termRef = useRef<Terminal | null>(null)
  const fitRef = useRef<FitAddon | null>(null)
  const sessionIdRef = useRef<string | null>(null)
  const lastResizeRef = useRef<{ cols: number; rows: number } | null>(null)
  /** Cursor from the last attach — data events with seq ≤ this are already in the buffer */
  const attachCursorRef = useRef<number>(0)

  useEffect(() => {
    // Per-invocation flag — set to false in cleanup so stale async flows
    // from React StrictMode's double-mount don't register duplicate listeners.
    let active = true
    let unsubscribeTerminalEvents: (() => void) | null = null
    const container = containerRef.current
    /* v8 ignore start */
    if (!container) return
    /* v8 ignore stop */

    const { term, fitAddon } = createTerminalInstance(container)

    const highlight = () => applyCursorRowHighlight(container, term)
    const cursorMoveDisposable = term.onCursorMove(highlight)
    const renderDisposable = term.onRender(highlight)

    // Force xterm.js to re-measure glyphs once the Nerd Font is available.
    void document.fonts.load(`14px ${NERD_FONT_FAMILY}`).then(() => {
      if (active && termRef.current) {
        termRef.current.options.fontFamily = FULL_FONT_FAMILY
      }
    })

    termRef.current = term
    fitRef.current = fitAddon
    setTerminalPasteHandler(viewKey, data => term.paste(data))

    // Initial fit
    requestAnimationFrame(() => {
      try {
        fitAddon.fit()
      } catch (_: unknown) {
        // Ignore initial fit errors
      }
    })

    const dims = fitAddon.proposeDimensions()

    async function handleExistingSession(term: Terminal, existingSessionId: string) {
      sessionIdRef.current = existingSessionId
      const result = await window.terminal.attach(existingSessionId)

      /* v8 ignore start */
      if (!active) return
      /* v8 ignore stop */

      if (!result.success) {
        removeSession(viewKey)
        await spawnNew()
        return
      }

      applyExistingSessionResult(term, result, attachCursorRef)
    }

    async function initSession() {
      try {
        const existingSessionId = getSessionId(viewKey)

        if (existingSessionId) {
          await handleExistingSession(term, existingSessionId)
        } else {
          /* v8 ignore start */
          if (!active) return
          /* v8 ignore stop */
          await spawnNew()
        }
      } catch (error: unknown) {
        if (!active) return
        term.writeln('\r\n\x1b[31m[Failed to initialize terminal session]\x1b[0m')
        console.error('Failed to initialize terminal session', error)
      }
    }

    /* v8 ignore start -- only called from v8-ignored deactivation guard */
    function killOrphanedSession(result: { success: boolean; sessionId?: string }) {
      if (result.success && result.sessionId) {
        void window.terminal.kill(result.sessionId)
      }
    }
    /* v8 ignore stop */

    async function spawnNew() {
      const result = await window.terminal.spawn({
        cwd,
        ...resolveSpawnDimensions(dims),
        startupCommand,
      })

      // If deactivated while spawn was in flight, kill the orphaned PTY immediately
      if (!active) {
        /* v8 ignore start */
        killOrphanedSession(result)
        return
        /* v8 ignore stop */
      }

      if (!hasSpawnSession(result)) {
        writeSpawnFailure(term, result)
        return
      }

      applySpawnSuccess(viewKey, result, sessionIdRef, onCwdChange)
      await applyAttachBuffer(term, result.sessionId)
    }

    async function applyAttachBuffer(term: Terminal, sid: string) {
      const attachResult = await window.terminal.attach(sid)
      /* v8 ignore start */
      if (!active) return
      /* v8 ignore stop */
      applyAttachResult(term, attachResult, attachCursorRef)
    }

    // Forward keystrokes via fire-and-forget (no OTel span per keystroke)
    const inputDisposable = term.onData((data: string) => {
      const sid = sessionIdRef.current
      if (sid) {
        window.terminal.write(sid, data)
      }
    })

    // Listen for PTY output — skip events already covered by the attach buffer
    const onData = (_event: unknown, sid: string, data: string, seq?: number) => {
      if (sid === sessionIdRef.current && termRef.current) {
        if (seq != null && seq <= attachCursorRef.current) return
        termRef.current.write(data)
      }
    }

    const onSessionExit = (_event: unknown, sid: string, exitCode: number) => {
      /* v8 ignore start */
      if (sid === sessionIdRef.current && termRef.current) {
        /* v8 ignore stop */
        termRef.current.writeln(`\r\n\x1b[90m[Process exited with code ${exitCode}]\x1b[0m`)
        onExit?.(exitCode)
      }
    }

    /* v8 ignore start */
    const onCwdChanged = (_event: unknown, sid: string, newCwd: string) => {
      if (sid === sessionIdRef.current) {
        onCwdChange?.(newCwd)
        /* v8 ignore stop */
      }
    }

    // Defer data/exit listener registration until after initSession completes
    // so the attach buffer is fully replayed and attachCursorRef is set,
    // preventing duplicate output from racing IPC events.
    void (async () => {
      await initSession()
      if (!active) return
      unsubscribeTerminalEvents = subscribeToTerminalEvents({
        onData,
        onExit: onSessionExit,
        onCwdChanged,
      })
    })()

    // Debounced resize
    let resizeTimer: ReturnType<typeof setTimeout> | null = null
    const handleResize = () => {
      const fit = fitRef.current
      const sid = sessionIdRef.current
      /* v8 ignore start */
      if (!fit || !sid) return
      /* v8 ignore stop */
      try {
        fit.fit()
        applyResizeDimensions(fit, sid)
      } catch (_: unknown) {
        // Ignore
      }
    }
    function applyResizeDimensions(fit: FitAddon, sid: string) {
      const d = getResizeDimensions(fit)
      if (!d) return
      const last = lastResizeRef.current
      if (!hasResizeChanged(last, d)) return
      lastResizeRef.current = { cols: d.cols, rows: d.rows }
      window.terminal.resize(sid, d.cols, d.rows)
    }
    const resizeObserver = new ResizeObserver(() => {
      if (resizeTimer) clearTimeout(resizeTimer)
      resizeTimer = setTimeout(handleResize, 100)
    })
    resizeObserver.observe(container)

    return () => {
      active = false
      resizeObserver.disconnect()
      if (resizeTimer) clearTimeout(resizeTimer)
      cursorMoveDisposable.dispose()
      renderDisposable.dispose()
      inputDisposable.dispose()
      unsubscribeTerminalEvents?.()

      // Do NOT kill PTY here — session survives tab switches.
      // PTY is killed only via killTerminalSession() on explicit tab close.
      removeTerminalPasteHandler(viewKey)
      term.dispose()
      termRef.current = null
      fitRef.current = null
    }
    // react-doctor-disable-next-line react-doctor/exhaustive-deps -- Terminal lifecycle is keyed by the mounted view; adding props would recreate and disconnect the PTY session.
  }, []) // eslint-disable-line react-hooks/exhaustive-deps -- Terminal lifecycle is keyed by the mounted view; adding props would recreate and disconnect the PTY session.

  return (
    <div className="terminal-pane">
      <div className="terminal-pane-body" ref={containerRef} />
    </div>
  )
}
