import { useCallback, useEffect, useRef, useState } from 'react'

export type ScheduleRetry = (key: string) => void
export type CanAttemptReconciliation = (key: string) => boolean

type OverrideRetryEvent = { key: string }

const OVERRIDE_RETRY_EVENT = 'buddy:usage-provider-override-retry'
const OVERRIDE_RETRY_CANCEL_EVENT = 'buddy:usage-provider-override-retry-cancel'
const OVERRIDE_WORK_DRAINED_EVENT = 'buddy:usage-provider-work-drained'
const RECONCILIATION_RETRY_MS = 1_000
const MAX_RECONCILIATION_RETRIES = 5

function publishRetryEvent(eventName: string, key: string) {
  window.dispatchEvent(new CustomEvent<OverrideRetryEvent>(eventName, { detail: { key } }))
}

export function cancelUsageProviderRetry(key: string) {
  publishRetryEvent(OVERRIDE_RETRY_CANCEL_EVENT, key)
}

export function scheduleUsageProviderRetry(key: string) {
  publishRetryEvent(OVERRIDE_RETRY_EVENT, key)
}

export function notifyUsageProviderWorkDrained(key: string) {
  publishRetryEvent(OVERRIDE_WORK_DRAINED_EVENT, key)
}

function clearRetryState(
  key: string,
  timers: Map<string, number>,
  attempts: Map<string, number>,
  exhausted: Set<string>
) {
  const timer = timers.get(key)
  if (timer !== undefined) window.clearTimeout(timer)
  timers.delete(key)
  attempts.delete(key)
  exhausted.delete(key)
}

function armRetry(
  key: string,
  timers: Map<string, number>,
  attempts: Map<string, number>,
  exhausted: Set<string>,
  notify: () => void
) {
  if (timers.has(key)) return
  const attempt = attempts.get(key) ?? 0
  if (attempt >= MAX_RECONCILIATION_RETRIES) {
    exhausted.add(key)
    return
  }
  attempts.set(key, attempt + 1)
  const timer = window.setTimeout(
    () => {
      timers.delete(key)
      notify()
    },
    RECONCILIATION_RETRY_MS * 2 ** attempt
  )
  timers.set(key, timer)
}

export function useUsageProviderRetry(
  retryContext: string
): [number, ScheduleRetry, CanAttemptReconciliation] {
  const [revision, setRevision] = useState(0)
  const timers = useRef(new Map<string, number>())
  const attempts = useRef(new Map<string, number>())
  const exhausted = useRef(new Set<string>())
  const previousContexts = useRef(new Map<string, string>())
  const scheduleRetry = useCallback((key: string) => {
    scheduleUsageProviderRetry(key)
  }, [])
  const canAttempt = useCallback(
    (key: string) => !timers.current.has(key) && !exhausted.current.has(key),
    []
  )

  useEffect(() => {
    const activeTimers = timers.current
    const activeAttempts = attempts.current
    const exhaustedKeys = exhausted.current
    const handleRetry = (event: Event) => {
      const { key } = (event as CustomEvent<OverrideRetryEvent>).detail
      armRetry(key, activeTimers, activeAttempts, exhaustedKeys, () => {
        setRevision(current => current + 1)
      })
    }
    const handleCancel = (event: Event) => {
      const { key } = (event as CustomEvent<OverrideRetryEvent>).detail
      clearRetryState(key, activeTimers, activeAttempts, exhaustedKeys)
    }
    const handleWorkDrained = () => {
      setRevision(current => current + 1)
    }
    window.addEventListener(OVERRIDE_RETRY_EVENT, handleRetry)
    window.addEventListener(OVERRIDE_RETRY_CANCEL_EVENT, handleCancel)
    window.addEventListener(OVERRIDE_WORK_DRAINED_EVENT, handleWorkDrained)
    return () => {
      window.removeEventListener(OVERRIDE_RETRY_EVENT, handleRetry)
      window.removeEventListener(OVERRIDE_RETRY_CANCEL_EVENT, handleCancel)
      window.removeEventListener(OVERRIDE_WORK_DRAINED_EVENT, handleWorkDrained)
      for (const timer of activeTimers.values()) window.clearTimeout(timer)
      activeTimers.clear()
    }
  }, [])

  useEffect(() => {
    const nextContexts = new Map(JSON.parse(retryContext) as Array<[string, string]>)
    const keys = new Set([...previousContexts.current.keys(), ...nextContexts.keys()])
    let contextChanged = false
    for (const key of keys) {
      if (previousContexts.current.get(key) !== nextContexts.get(key)) {
        clearRetryState(key, timers.current, attempts.current, exhausted.current)
        contextChanged = true
      }
    }
    previousContexts.current = nextContexts
    if (contextChanged) setRevision(current => current + 1)
  }, [retryContext])

  return [revision, scheduleRetry, canAttempt]
}
