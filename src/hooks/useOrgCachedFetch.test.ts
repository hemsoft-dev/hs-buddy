import { act, renderHook } from '@testing-library/react'
import { describe, it, expect, vi } from 'vitest'
import { useLayoutEffect } from 'react'
import {
  applyResolvedOrgCache,
  applyResolvedOrgCacheIfCurrent,
  isStaleOrgFetch,
  applyOrgFetchResult,
  handleOrgFetchErrorIfCurrent,
  useOrgCachedFetch,
} from './useOrgCachedFetch'
import { getTaskQueue } from '../services/taskQueue'
import { dataCache } from '../services/dataCache'
import type { useTaskQueue } from './useTaskQueue'

vi.mock('../services/dataCache', () => ({
  dataCache: { get: vi.fn(() => null), getOrLoad: vi.fn(async () => null), set: vi.fn() },
}))

vi.mock('../api/github', () => ({
  GitHubClient: vi.fn(),
}))

vi.mock('../api/github/client', async () => {
  const github = await vi.importMock<typeof import('../api/github')>('../api/github')
  return { GitHubClient: github.GitHubClient }
})

vi.mock('../services/taskQueue', () => ({
  getTaskQueue: vi.fn(),
}))

describe('applyResolvedOrgCache', () => {
  it('returns false and does not call setters when cached is null', () => {
    const setData = vi.fn()
    const setError = vi.fn()
    const setPhase = vi.fn()
    expect(applyResolvedOrgCache(null, setData, setError, setPhase)).toBe(false)
    expect(setData).not.toHaveBeenCalled()
    expect(setError).not.toHaveBeenCalled()
    expect(setPhase).not.toHaveBeenCalled()
  })

  it('returns true and applies cached data when not null', () => {
    const setData = vi.fn()
    const setError = vi.fn()
    const setPhase = vi.fn()
    const data = { repos: ['repo1'] }
    expect(applyResolvedOrgCache(data, setData, setError, setPhase)).toBe(true)
    expect(setData).toHaveBeenCalledWith(data)
    expect(setError).toHaveBeenCalledWith(null)
    expect(setPhase).toHaveBeenCalledWith('ready')
  })
})

it('commits only the new organization data after an identity change', () => {
  const committed: Array<string | null> = []
  const { rerender } = renderHook(
    ({ cacheKey }) => {
      const state = useOrgCachedFetch({
        accounts: [],
        org: cacheKey,
        cacheKey,
        initialData: cacheKey,
        enqueue: vi.fn() as ReturnType<typeof useTaskQueue>['enqueue'],
        taskName: 'identity',
        fetchFn: vi.fn(),
      })
      useLayoutEffect(() => {
        committed.push(state.data)
      }, [cacheKey, state.data])
    },
    { initialProps: { cacheKey: 'first' } }
  )
  rerender({ cacheKey: 'second' })
  expect(committed).toEqual(['first', 'second'])
})

describe('useOrgCachedFetch', () => {
  it('keeps a cached refresh visibly busy while its task is queued', async () => {
    let resolveEnqueue: ((value: { repositories: never[] }) => void) | undefined
    const enqueuePromise = new Promise<{ repositories: never[] }>(resolve => {
      resolveEnqueue = resolve
    })
    const enqueue = vi.fn().mockReturnValue(enqueuePromise)
    vi.mocked(getTaskQueue).mockReturnValue({
      hasTaskWithName: vi.fn().mockReturnValue(true),
    } as unknown as ReturnType<typeof getTaskQueue>)
    const { result } = renderHook(() =>
      useOrgCachedFetch({
        accounts: [],
        org: 'HemSoft',
        enqueue: enqueue as ReturnType<typeof useTaskQueue>['enqueue'],
        cacheKey: 'org-overview:HemSoft',
        taskName: 'org-detail-overview-HemSoft',
        initialData: { repositories: [] },
        fetchFn: vi.fn(),
      })
    )

    let fetchPromise: Promise<void> | undefined
    await act(async () => {
      fetchPromise = result.current.fetch(true)
      await Promise.resolve()
    })

    expect(result.current.phase).toBe('refreshing')
    expect(result.current.error).toBeNull()
    expect(enqueue).toHaveBeenCalledWith(expect.any(Function), {
      name: 'org-detail-overview-HemSoft',
      priority: 1,
      deduplicate: true,
    })

    await act(async () => {
      resolveEnqueue!({ repositories: [] })
      await fetchPromise
    })
    expect(result.current.phase).toBe('ready')
  })

  it('updates the shared cache before releasing a deduplicated task', async () => {
    const fetched = { repositories: ['hs-buddy'] }
    let cacheWasUpdatedInsideTask = false
    vi.mocked(dataCache.set).mockClear()
    vi.mocked(getTaskQueue).mockReturnValue({
      hasTaskWithName: vi.fn().mockReturnValue(false),
    } as unknown as ReturnType<typeof getTaskQueue>)
    const enqueue = vi.fn(
      async (task: (signal: AbortSignal) => Promise<{ repositories: string[] }>) => {
        const result = await task(new AbortController().signal)
        cacheWasUpdatedInsideTask = vi
          .mocked(dataCache.set)
          .mock.calls.some(call => call[0] === 'org-overview:HemSoft' && call[1] === fetched)
        return result
      }
    )
    const { result } = renderHook(() =>
      useOrgCachedFetch({
        accounts: [],
        org: 'HemSoft',
        enqueue: enqueue as ReturnType<typeof useTaskQueue>['enqueue'],
        cacheKey: 'org-overview:HemSoft',
        taskName: 'org-detail-overview-HemSoft',
        fetchFn: vi.fn().mockResolvedValue(fetched),
      })
    )

    await act(async () => {
      await result.current.fetch()
    })

    expect(cacheWasUpdatedInsideTask).toBe(true)
  })
})

describe('useOrgCachedFetch shared task ownership', () => {
  it('publishes a shared result to its original cache after the owning hook navigates', async () => {
    let resolveFetch: ((value: { repositories: string[] }) => void) | undefined
    const fetchResult = new Promise<{ repositories: string[] }>(resolve => {
      resolveFetch = resolve
    })
    vi.mocked(dataCache.set).mockClear()
    vi.mocked(getTaskQueue).mockReturnValue({
      hasTaskWithName: vi.fn().mockReturnValue(false),
    } as unknown as ReturnType<typeof getTaskQueue>)
    const enqueue = vi.fn(async (task: (signal: AbortSignal) => Promise<unknown>) =>
      task(new AbortController().signal)
    )
    const { result, rerender } = renderHook(
      ({ org, cacheKey }) =>
        useOrgCachedFetch({
          accounts: [],
          org,
          enqueue: enqueue as ReturnType<typeof useTaskQueue>['enqueue'],
          cacheKey,
          taskName: `org-detail-overview-${org}`,
          fetchFn: vi.fn().mockReturnValue(fetchResult),
        }),
      { initialProps: { org: 'HemSoft', cacheKey: 'org-overview:HemSoft' } }
    )

    let pendingFetch: Promise<void> | undefined
    await act(async () => {
      pendingFetch = result.current.fetch()
      await Promise.resolve()
    })
    rerender({ org: 'other-org', cacheKey: 'org-overview:other-org' })

    await act(async () => {
      resolveFetch!({ repositories: ['hs-buddy'] })
      await pendingFetch
    })

    expect(dataCache.set).toHaveBeenCalledWith('org-overview:HemSoft', {
      repositories: ['hs-buddy'],
    })
    expect(result.current.data).toBeNull()
  })
})

describe('applyResolvedOrgCacheIfCurrent', () => {
  it('discards a cached result after navigation changes the key', () => {
    const setData = vi.fn()
    const setError = vi.fn()
    const setPhase = vi.fn()

    expect(
      applyResolvedOrgCacheIfCurrent(
        'org-members:old',
        { current: 'org-members:new' },
        { members: ['stale'] },
        setData,
        setError,
        setPhase
      )
    ).toBe(true)
    expect(setData).not.toHaveBeenCalled()
  })

  it('applies a cached result when the key is still current', () => {
    const setData = vi.fn()
    const setError = vi.fn()
    const setPhase = vi.fn()
    const cached = { members: ['current'] }

    expect(
      applyResolvedOrgCacheIfCurrent(
        'org-members:current',
        { current: 'org-members:current' },
        cached,
        setData,
        setError,
        setPhase
      )
    ).toBe(true)
    expect(setData).toHaveBeenCalledWith(cached)
  })
})

describe('isStaleOrgFetch', () => {
  it('returns true when keys differ', () => {
    expect(isStaleOrgFetch('key-a', { current: 'key-b' })).toBe(true)
  })

  it('returns false when keys match', () => {
    expect(isStaleOrgFetch('key-a', { current: 'key-a' })).toBe(false)
  })
})

describe('applyOrgFetchResult', () => {
  it('does nothing when fetch is stale', () => {
    const setData = vi.fn()
    const setPhase = vi.fn()
    const normalize = vi.fn()
    applyOrgFetchResult('key-a', { current: 'key-b' }, normalize, 'result', setData, setPhase)
    expect(normalize).not.toHaveBeenCalled()
    expect(setData).not.toHaveBeenCalled()
  })

  it('normalizes and applies when fetch is current', () => {
    const setData = vi.fn()
    const setPhase = vi.fn()
    const normalize = vi.fn(d => d)
    applyOrgFetchResult('key-a', { current: 'key-a' }, normalize, 'result', setData, setPhase)
    expect(normalize).toHaveBeenCalledWith('result')
    // startTransition calls the callback synchronously in test env
    expect(setData).toHaveBeenCalledWith('result')
    expect(setPhase).toHaveBeenCalledWith('ready')
  })
})

describe('handleOrgFetchErrorIfCurrent', () => {
  it('does nothing when fetch is stale', () => {
    const setPhase = vi.fn()
    const setError = vi.fn()
    handleOrgFetchErrorIfCurrent(
      new Error('fail'),
      'key-a',
      { current: 'key-b' },
      setPhase,
      setError
    )
    expect(setPhase).not.toHaveBeenCalled()
    expect(setError).not.toHaveBeenCalled()
  })

  it('sets error phase when fetch is current', () => {
    const setPhase = vi.fn()
    const setError = vi.fn()
    handleOrgFetchErrorIfCurrent(
      new Error('fail'),
      'key-a',
      { current: 'key-a' },
      setPhase,
      setError
    )
    expect(setPhase).toHaveBeenCalledWith('error')
    expect(setError).toHaveBeenCalledWith('fail')
  })
})
