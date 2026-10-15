import { describe, expect, it, vi, beforeEach } from 'vitest'
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react'
import { axe } from '../../test/axe-helper'
import { SettingsAdvanced } from './SettingsAdvanced'

const { mockUseConfig, mockGetStorageStats, mockClearCache } = vi.hoisted(() => ({
  mockUseConfig: vi.fn(),
  mockGetStorageStats: vi.fn(),
  mockClearCache: vi.fn(),
}))

vi.mock('../../hooks/useConfig', () => ({
  useConfig: mockUseConfig,
}))

vi.mock('../../services/dataCache', () => ({
  dataCache: {
    getStorageStats: mockGetStorageStats,
    clear: mockClearCache,
  },
}))

function defaultMockValues() {
  return {
    api: {
      getStorePath: vi.fn().mockResolvedValue('/path/to/config.json'),
      openInEditor: vi.fn().mockResolvedValue(undefined),
      reset: vi.fn().mockResolvedValue({ success: true }),
    },
    refresh: vi.fn().mockResolvedValue(undefined),
    loading: false,
  }
}

describe('SettingsAdvanced', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockGetStorageStats.mockResolvedValue({ entryCount: 42, totalBytes: 3 * 1024 * 1024 })
    mockClearCache.mockResolvedValue(true)
    mockUseConfig.mockReturnValue(defaultMockValues())
  })

  it('renders Advanced heading', () => {
    render(<SettingsAdvanced />)
    expect(screen.getByText('Advanced')).toBeTruthy()
  })

  it('has no accessibility violations', async () => {
    const { container } = render(<SettingsAdvanced />)
    await screen.findByText('/path/to/config.json')
    const results = await axe(container)
    expect(results).toHaveNoViolations()
  })

  it('renders Configuration File section', () => {
    render(<SettingsAdvanced />)
    expect(screen.getByText('Configuration File')).toBeTruthy()
  })

  it('renders Reset Configuration section', () => {
    render(<SettingsAdvanced />)
    expect(screen.getByText('Reset Configuration')).toBeTruthy()
  })

  it('renders Open in Editor button', () => {
    render(<SettingsAdvanced />)
    expect(screen.getByText('Open in Editor')).toBeTruthy()
  })

  it('renders Reset to Defaults button', () => {
    render(<SettingsAdvanced />)
    expect(screen.getByText('Reset to Defaults')).toBeTruthy()
  })

  it('shows first-click confirmation on reset', () => {
    render(<SettingsAdvanced />)
    fireEvent.click(screen.getByText('Reset to Defaults'))
    expect(screen.getByText('Click Again to Confirm')).toBeTruthy()
  })

  it('shows About Storage section', () => {
    render(<SettingsAdvanced />)
    expect(screen.getByText('About Storage')).toBeTruthy()
  })

  it('shows persisted cache entry count and size', async () => {
    render(<SettingsAdvanced />)

    expect(await screen.findByText('42 entries · 3.00 MiB')).toBeTruthy()
  })

  it('clears cached data and refreshes the displayed stats', async () => {
    mockGetStorageStats
      .mockResolvedValueOnce({ entryCount: 42, totalBytes: 3 * 1024 * 1024 })
      .mockResolvedValueOnce({ entryCount: 0, totalBytes: 0 })
    render(<SettingsAdvanced />)
    await screen.findByText('42 entries · 3.00 MiB')

    fireEvent.click(screen.getByText('Clear Cached Data'))

    await waitFor(() => {
      expect(mockClearCache).toHaveBeenCalledTimes(1)
      expect(screen.getByText('0 entries · 0 B')).toBeTruthy()
    })
  })

  it('preserves displayed stats when clearing persisted data fails', async () => {
    mockClearCache.mockResolvedValue(false)
    render(<SettingsAdvanced />)
    await screen.findByText('42 entries · 3.00 MiB')

    fireEvent.click(screen.getByText('Clear Cached Data'))

    await waitFor(() => expect(mockClearCache).toHaveBeenCalledTimes(1))
    expect(mockGetStorageStats).toHaveBeenCalledTimes(1)
    expect(screen.getByText('42 entries · 3.00 MiB')).toBeTruthy()
    expect(screen.getByRole('alert')).toHaveTextContent('Failed to clear cached data')
  })

  it('shows security note about keychain', () => {
    render(<SettingsAdvanced />)
    expect(screen.getByText(/tokens are stored securely/)).toBeTruthy()
  })

  it('shows loading spinner when loading is true', () => {
    mockUseConfig.mockReturnValue({ ...defaultMockValues(), loading: true })
    render(<SettingsAdvanced />)
    expect(screen.getByText('Loading advanced settings…')).toBeTruthy()
  })

  it('shows "Opened!" after clicking Open in Editor', async () => {
    render(<SettingsAdvanced />)
    fireEvent.click(screen.getByText('Open in Editor'))
    await waitFor(() => {
      expect(screen.getByText('Opened!')).toBeTruthy()
    })
  })

  it('executes reset on second click of Reset to Defaults', async () => {
    const mock = defaultMockValues()
    mockUseConfig.mockReturnValue(mock)
    render(<SettingsAdvanced />)

    fireEvent.click(screen.getByText('Reset to Defaults'))
    expect(screen.getByText('Click Again to Confirm')).toBeTruthy()

    fireEvent.click(screen.getByText('Click Again to Confirm'))
    await waitFor(() => {
      expect(mock.api.reset).toHaveBeenCalled()
      expect(mock.refresh).toHaveBeenCalled()
    })
  })

  it.each([
    ['rejected secure store', () => Promise.reject(new Error('private-reset-fixture'))],
    ['handled failure', () => Promise.resolve({ success: false })],
    ['missing acknowledgment', () => Promise.resolve(undefined)],
  ])(
    'recovers a failed reset for %s without refreshing unchanged settings',
    async (_label, reply) => {
      const mock = defaultMockValues()
      mock.api.reset.mockImplementationOnce(reply)
      mockUseConfig.mockReturnValue(mock)
      render(<SettingsAdvanced />)
      fireEvent.click(screen.getByText('Reset to Defaults'))
      fireEvent.click(screen.getByText('Click Again to Confirm'))
      expect(await screen.findByRole('alert')).toHaveTextContent('Unlock secure storage')
      expect(screen.getByRole('alert')).not.toHaveTextContent('private-reset-fixture')
      expect(screen.getByText('Reset to Defaults')).toBeEnabled()
      expect(mock.refresh).not.toHaveBeenCalled()
      fireEvent.click(screen.getByText('Reset to Defaults'))
      fireEvent.click(screen.getByText('Click Again to Confirm'))
      await waitFor(() => expect(mock.refresh).toHaveBeenCalledOnce())
      expect(screen.queryByRole('alert')).not.toBeInTheDocument()
    }
  )

  it('distinguishes an acknowledged reset from a failed reload', async () => {
    const mock = defaultMockValues()
    mock.refresh.mockRejectedValueOnce(new Error('private-refresh-fixture'))
    mockUseConfig.mockReturnValue(mock)
    render(<SettingsAdvanced />)
    fireEvent.click(screen.getByText('Reset to Defaults'))
    fireEvent.click(screen.getByText('Click Again to Confirm'))
    expect(await screen.findByRole('alert')).toHaveTextContent('Settings were reset')
    expect(screen.getByRole('alert')).not.toHaveTextContent('private-refresh-fixture')
    expect(screen.getByText('Reset to Defaults')).toBeEnabled()
  })

  it('displays the config store path', async () => {
    render(<SettingsAdvanced />)
    await waitFor(() => {
      expect(screen.getByText('/path/to/config.json')).toBeTruthy()
    })
  })

  it('hides "Opened!" message after 2 seconds', async () => {
    vi.useFakeTimers()
    render(<SettingsAdvanced />)

    await act(async () => {
      fireEvent.click(screen.getByText('Open in Editor'))
      await vi.advanceTimersByTimeAsync(0)
    })
    expect(screen.getByText('Opened!')).toBeTruthy()

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000)
    })
    expect(screen.queryByText('Opened!')).toBeNull()
    expect(screen.getByText('Open in Editor')).toBeTruthy()

    vi.useRealTimers()
  })

  it('auto-cancels reset confirmation after 3 seconds', async () => {
    vi.useFakeTimers()
    render(<SettingsAdvanced />)

    await act(async () => {
      fireEvent.click(screen.getByText('Reset to Defaults'))
    })
    expect(screen.getByText('Click Again to Confirm')).toBeTruthy()

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000)
    })
    expect(screen.queryByText('Click Again to Confirm')).toBeNull()
    expect(screen.getByText('Reset to Defaults')).toBeTruthy()

    vi.useRealTimers()
  })
})
