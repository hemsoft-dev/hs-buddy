import { describe, it, expect, vi, beforeEach } from 'vitest'
import { renderHook, waitFor } from '@testing-library/react'
import { useLayoutEffect } from 'react'
import { useGitHubData } from './useGitHubData'
import { dataCache } from '../services/dataCache'

const mockEnqueue = vi.fn()
const mockAccounts = vi.fn()

vi.mock('./useConfig', () => ({
  useGitHubAccounts: () => mockAccounts(),
}))

vi.mock('./useTaskQueue', () => ({
  useTaskQueue: () => ({ enqueue: mockEnqueue }),
}))

vi.mock('../api/github', () => ({
  GitHubClient: vi.fn(function () {
    return {}
  }),
}))

vi.mock('../api/github/client', async () => {
  const github = await vi.importMock<typeof import('../api/github')>('../api/github')
  return { GitHubClient: github.GitHubClient }
})

vi.mock('../utils/errorUtils', () => ({
  getErrorMessage: (e: unknown) => (e instanceof Error ? e.message : String(e)),
  isAbortError: (e: unknown) =>
    e instanceof DOMException && (e as DOMException).name === 'AbortError',
  throwIfAborted: (signal: AbortSignal) => {
    if (signal.aborted) throw new DOMException('Aborted', 'AbortError')
  },
}))

const stableAccounts = [{ username: 'user1', org: 'myorg' }]

beforeEach(() => {
  vi.clearAllMocks()
  mockAccounts.mockReturnValue({ accounts: stableAccounts })
  // Clear dataCache internal state
  vi.spyOn(dataCache, 'get').mockReturnValue(null)
  vi.spyOn(dataCache, 'getOrLoad').mockImplementation(async key => dataCache.get(key))
  vi.spyOn(dataCache, 'set').mockImplementation(() => {})
})

describe('useGitHubData state', () => {
  it('never commits the previous identity to a layout observer', () => {
    vi.mocked(dataCache.get).mockImplementation(key => ({ data: key, fetchedAt: Date.now() }))
    const committed: Array<string | null> = []
    const { rerender } = renderHook(
      ({ cacheKey }) => {
        const state = useGitHubData<string>({ cacheKey, taskName: 'identity', fetchFn: vi.fn() })
        useLayoutEffect(() => {
          committed.push(state.data)
        }, [cacheKey, state.data])
      },
      { initialProps: { cacheKey: 'first' } }
    )
    rerender({ cacheKey: 'second' })
    expect(committed).toEqual(['first', 'second'])
  })

  it('clears error when serving cached data on re-fetch', async () => {
    // Start: no cache, fetch fails → error state set
    mockEnqueue.mockRejectedValueOnce(new Error('Network fail'))

    const fetchFn = vi.fn()
    const { result, rerender } = renderHook(() =>
      useGitHubData({ cacheKey: 'key-1', taskName: 'test', fetchFn })
    )

    await waitFor(() => {
      expect(result.current.error).toBe('Network fail')
    })

    // Cache now has data; change accounts to force doFetch recreation → re-fetch
    vi.mocked(dataCache.get).mockReturnValue({ data: 'cached-data', fetchedAt: Date.now() })
    mockAccounts.mockReturnValue({ accounts: [{ username: 'new-user', org: 'new-org' }] })

    rerender()

    await waitFor(() => {
      expect(result.current.error).toBeNull()
      expect(result.current.data).toBe('cached-data')
    })
  })

  it('resets state when cacheKey changes to null (line 65 branch)', async () => {
    // Start with a valid key and cached data
    vi.mocked(dataCache.get).mockReturnValue({ data: 'initial', fetchedAt: Date.now() })
    mockEnqueue.mockResolvedValue('fetched')

    const fetchFn = vi.fn()
    const { result, rerender } = renderHook(
      ({ cacheKey }: { cacheKey: string | null }) =>
        useGitHubData({ cacheKey, taskName: 'test', fetchFn }),
      { initialProps: { cacheKey: 'key-1' as string | null } }
    )

    await waitFor(() => {
      expect(result.current.data).toBe('initial')
    })

    // Change cacheKey to null — triggers the false branch on line 65
    vi.mocked(dataCache.get).mockReturnValue(null)
    rerender({ cacheKey: null })

    await waitFor(() => {
      expect(result.current.data).toBeNull()
      expect(result.current.loading).toBe(false)
    })
  })
})

describe('useGitHubData serialization', () => {
  it('holds serialization through the cache write', async () => {
    const fetchFn = vi.fn().mockResolvedValue('fresh-data')
    mockEnqueue.mockImplementation(
      async (
        execute: (signal: AbortSignal) => Promise<unknown>,
        options: { name: string; serializationKey?: string }
      ) => {
        expect(options).toEqual({ name: 'test', serializationKey: 'shared-resource' })
        const result = await execute(new AbortController().signal)
        expect(dataCache.set).toHaveBeenCalledWith('key-1', 'fresh-data')
        return result
      }
    )

    const { result } = renderHook(() =>
      useGitHubData({
        cacheKey: 'key-1',
        taskName: 'test',
        serializationKey: 'shared-resource',
        fetchFn,
      })
    )

    await waitFor(() => {
      expect(result.current.data).toBe('fresh-data')
    })
  })

  it('ignores a cache lookup that resolves after the cache key changes', async () => {
    let resolveOldLookup!: (value: { data: string; fetchedAt: number }) => void
    const oldLookup = new Promise<{ data: string; fetchedAt: number }>(resolve => {
      resolveOldLookup = resolve
    })
    vi.mocked(dataCache.getOrLoad).mockImplementation(async key => {
      if (key === 'key-1') return oldLookup
      return { data: 'current-data', fetchedAt: Date.now() }
    })

    const fetchFn = vi.fn()
    const { result, rerender } = renderHook(
      ({ cacheKey }: { cacheKey: string }) =>
        useGitHubData({ cacheKey, taskName: 'test', fetchFn }),
      { initialProps: { cacheKey: 'key-1' } }
    )
    await waitFor(() => expect(dataCache.getOrLoad).toHaveBeenCalledWith('key-1'))

    rerender({ cacheKey: 'key-2' })
    resolveOldLookup({ data: 'obsolete-data', fetchedAt: Date.now() })

    await waitFor(() => {
      expect(result.current.data).toBe('current-data')
    })
    expect(fetchFn).not.toHaveBeenCalled()
  })

  it('does not cache a GitHub result after the cache key changes', async () => {
    let resolveOldFetch!: (value: string) => void
    const oldFetch = new Promise<string>(resolve => {
      resolveOldFetch = resolve
    })
    vi.mocked(dataCache.getOrLoad).mockImplementation(async key => {
      if (key === 'key-2') return { data: 'current-data', fetchedAt: Date.now() }
      return null
    })
    mockEnqueue.mockImplementation(async execute => execute(new AbortController().signal))
    const fetchFn = vi.fn().mockReturnValue(oldFetch)

    const { result, rerender } = renderHook(
      ({ cacheKey }: { cacheKey: string }) =>
        useGitHubData({ cacheKey, taskName: 'test', fetchFn }),
      { initialProps: { cacheKey: 'key-1' } }
    )
    await waitFor(() => expect(fetchFn).toHaveBeenCalledOnce())

    rerender({ cacheKey: 'key-2' })
    resolveOldFetch('obsolete-data')

    await waitFor(() => {
      expect(result.current.data).toBe('current-data')
    })
    expect(dataCache.set).not.toHaveBeenCalledWith('key-1', 'obsolete-data')
  })
})
