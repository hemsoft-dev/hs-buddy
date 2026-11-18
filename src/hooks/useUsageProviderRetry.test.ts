import { act, renderHook } from '@testing-library/react'
import { afterEach, expect, it, vi } from 'vitest'
import { cancelUsageProviderRetry, useUsageProviderRetry } from './useUsageProviderRetry'

afterEach(() => vi.useRealTimers())

it('deduplicates retries and cancels every owned timer on unmount', () => {
  vi.useFakeTimers()
  const { result, unmount } = renderHook(() => useUsageProviderRetry('[]'))
  act(() => {
    result.current[1]('first')
    result.current[1]('first')
    result.current[1]('second')
  })
  expect(vi.getTimerCount()).toBe(2)
  expect(result.current[2]('first')).toBe(false)
  unmount()
  expect(vi.getTimerCount()).toBe(0)
})

it('retains bounded backoff and allows a cancelled key to retry', () => {
  vi.useFakeTimers()
  const { result, unmount } = renderHook(() => useUsageProviderRetry('[]'))
  for (let attempt = 0; attempt < 5; attempt++) {
    act(() => result.current[1]('account'))
    act(() => vi.advanceTimersByTime(1_000 * 2 ** attempt))
    expect(result.current[0]).toBe(attempt + 1)
  }
  act(() => result.current[1]('account'))
  expect(result.current[2]('account')).toBe(false)
  expect(vi.getTimerCount()).toBe(0)
  act(() => cancelUsageProviderRetry('account'))
  expect(result.current[2]('account')).toBe(true)
  act(() => result.current[1]('account'))
  expect(vi.getTimerCount()).toBe(1)
  unmount()
  expect(vi.getTimerCount()).toBe(0)
})
