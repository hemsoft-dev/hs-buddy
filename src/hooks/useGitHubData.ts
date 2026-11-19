/**
 * Generic hook for GitHub data fetching with caching, task queue, and abort handling.
 *
 * Consolidates the repeated fetch/loading/error/cache pattern found across
 * RepoDetailPanel, RepoCommitDetailPanel, RepoIssueDetailPanel, RepoIssueList,
 * PRFilesChangedPanel, and PRChecksPanel.
 */

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { useGitHubAccounts } from './useConfig'
import { useLatest } from './useLatest'
import { useTaskQueue } from './useTaskQueue'
import { GitHubClient } from '../api/github/client'
import { dataCache } from '../services/dataCache'
import { getErrorMessage, isAbortError, throwIfAborted } from '../utils/errorUtils'

function handleFetchError(
  err: unknown,
  requestId: number,
  currentRequestId: number,
  setError: (e: string) => void
): void {
  if (isAbortError(err)) return
  if (requestId !== currentRequestId) return
  setError(getErrorMessage(err))
}

function resolveNextCacheState<T>(cacheKey: string | null): { data: T | null; hasCache: boolean } {
  const next = cacheKey ? dataCache.get<T>(cacheKey) : null
  return { data: next?.data ?? null, hasCache: next !== null }
}

async function getCachedData<T>(cacheKey: string) {
  return await dataCache.getOrLoad<T>(cacheKey)
}

function isActiveGitHubRequest(requestId: number, requestIdRef: { current: number }): boolean {
  return requestId === requestIdRef.current
}

function applyGitHubDataResult<T>(
  requestId: number,
  requestIdRef: { current: number },
  result: T,
  setData: (data: T) => void
): void {
  if (!isActiveGitHubRequest(requestId, requestIdRef)) return
  setData(result)
}

function finishGitHubLoading(
  requestId: number,
  requestIdRef: { current: number },
  setLoading: (value: boolean) => void
): void {
  if (isActiveGitHubRequest(requestId, requestIdRef)) {
    setLoading(false)
  }
}

interface UseGitHubDataOptions<T> {
  /**
   * Cache key for dataCache. When this changes, data resets and a new fetch starts.
   * Pass `null` to disable fetching entirely.
   */
  cacheKey: string | null

  /** Task queue job name (for debugging/deduplication). */
  taskName: string

  /** Serialize work that reads or mutates the same cache-backed resource. */
  serializationKey?: string

  /**
   * The fetch function. Receives a pre-authenticated GitHubClient and an AbortSignal.
   * Kept in a ref internally — does not need to be memoized by the caller.
   */
  fetchFn: (client: GitHubClient, signal: AbortSignal) => Promise<T>
}

interface UseGitHubDataResult<T> {
  data: T | null
  loading: boolean
  error: string | null
  /** Force a fresh fetch, bypassing cache. Returns a promise for await in tests/timers. */
  refresh: () => Promise<void>
}

export function useGitHubData<T>({
  cacheKey,
  taskName,
  serializationKey,
  fetchFn,
}: UseGitHubDataOptions<T>): UseGitHubDataResult<T> {
  const { accounts } = useGitHubAccounts()
  const { enqueue } = useTaskQueue('github')
  const enqueueRef = useLatest(enqueue)
  const fetchFnRef = useLatest(fetchFn)
  const requestIdRef = useRef(0)

  // Seed initial state from cache
  const cachedEntry = cacheKey ? dataCache.get<T>(cacheKey) : null
  const [data, setData] = useState<T | null>(cachedEntry?.data ?? null)
  const [loading, setLoading] = useState(cacheKey !== null && !cachedEntry)
  const [error, setError] = useState<string | null>(null)

  // Reset this render before children can observe the previous identity's data.
  const [renderedKey, setRenderedKey] = useState(cacheKey)
  if (renderedKey !== cacheKey) {
    setRenderedKey(cacheKey)
    const { data: nextData, hasCache } = resolveNextCacheState<T>(cacheKey)
    setData(nextData)
    setLoading(cacheKey !== null && !hasCache)
    setError(null)
  }

  // Invalidate the old request when the new identity commits.
  const prevKeyRef = useRef(cacheKey)
  useLayoutEffect(() => {
    if (prevKeyRef.current !== cacheKey) {
      prevKeyRef.current = cacheKey
      // Invalidate any in-flight request from the previous cacheKey
      requestIdRef.current += 1
    }
  }, [cacheKey])

  const doFetch = useCallback(
    async (forceRefresh: boolean) => {
      if (cacheKey === null) return

      // Stale-request protection: only the latest request writes state
      const requestId = ++requestIdRef.current

      if (!forceRefresh) {
        const cached = await getCachedData<T>(cacheKey)
        if (!isActiveGitHubRequest(requestId, requestIdRef)) return
        if (cached) {
          setData(cached.data)
          setLoading(false)
          setError(null)
          return
        }
      }

      setLoading(true)
      setError(null)

      try {
        const result = await enqueueRef.current(
          async signal => {
            throwIfAborted(signal)
            const client = new GitHubClient({ accounts }, 7)
            const result = await fetchFnRef.current(client, signal)
            throwIfAborted(signal)
            if (isActiveGitHubRequest(requestId, requestIdRef)) {
              dataCache.set(cacheKey, result)
            }
            return result
          },
          { name: taskName, serializationKey }
        )

        applyGitHubDataResult(requestId, requestIdRef, result, setData)
      } catch (err: unknown) {
        handleFetchError(err, requestId, requestIdRef.current, setError)
      } finally {
        finishGitHubLoading(requestId, requestIdRef, setLoading)
      }
    },
    [accounts, cacheKey, taskName, serializationKey, enqueueRef, fetchFnRef]
  )

  // Initial fetch (and re-fetch when deps change)
  useEffect(() => {
    doFetch(false)
  }, [doFetch])

  const refresh = useCallback(() => doFetch(true), [doFetch])

  return { data, loading, error, refresh }
}
