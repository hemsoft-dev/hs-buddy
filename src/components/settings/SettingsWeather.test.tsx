import { beforeEach, describe, expect, it, vi } from 'vitest'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { SettingsWeather } from './SettingsWeather'
import { IPC_INVOKE } from '../../ipc/contracts'

describe('SettingsWeather', () => {
  const mockInvoke = vi.fn()
  beforeEach(() => {
    mockInvoke.mockReset()
    window.ipcRenderer = { invoke: mockInvoke } as unknown as typeof window.ipcRenderer
  })

  async function load(configured = false) {
    mockInvoke.mockResolvedValueOnce({ configured })
    render(<SettingsWeather />)
    return screen.findByPlaceholderText(/Enter your Google Cloud API key/)
  }

  it('shows loading while secure status is pending', () => {
    mockInvoke.mockReturnValue(new Promise(() => {}))
    render(<SettingsWeather />)
    expect(screen.getByText('Loading weather settings…')).toBeInTheDocument()
  })

  it('loads configured status without filling or exposing a saved key', async () => {
    mockInvoke.mockResolvedValueOnce({ configured: true, key: 'unexpected-private-fixture' })
    render(<SettingsWeather />)
    const input = await screen.findByPlaceholderText(/Enter your Google Cloud API key/)
    expect(input).toHaveValue('')
    expect(input).toHaveAttribute('type', 'password')
    expect(screen.queryByDisplayValue('unexpected-private-fixture')).not.toBeInTheDocument()
    expect(screen.getByText(/Configured:/)).toBeInTheDocument()
    expect(screen.getByText('Save Key')).toBeDisabled()
    expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.CONFIG_GET_POLLEN_API_KEY)
  })

  it('shows unconfigured status without a saved key', async () => {
    await load()
    expect(screen.getByText(/Not configured:/)).toBeInTheDocument()
    expect(screen.queryByText('Clear Key')).not.toBeInTheDocument()
  })

  it.each([
    ['rejected IPC', () => Promise.reject(new Error('private-fixture-error'))],
    ['failed status', () => Promise.resolve({ configured: false, error: 'private-fixture-error' })],
    ['malformed legacy response', () => Promise.resolve('private-fixture-error')],
    ['missing response', () => Promise.resolve(null)],
  ])('reports an actionable safe load error for %s', async (_label, reply) => {
    mockInvoke.mockImplementationOnce(reply)
    render(<SettingsWeather />)
    expect(await screen.findByRole('alert')).toHaveTextContent('Unlock secure storage')
    expect(screen.getByRole('alert')).not.toHaveTextContent('private-fixture-error')
    expect(screen.getByPlaceholderText(/Enter your Google Cloud API key/)).toHaveValue('')
    expect(screen.getByText('Clear Key')).not.toBeDisabled()
  })

  it('clears a migration conflict without requiring a replacement key', async () => {
    mockInvoke.mockResolvedValueOnce({ configured: false, error: 'migration conflict' })
    render(<SettingsWeather />)
    await screen.findByRole('alert')
    const input = screen.getByPlaceholderText(/Enter your Google Cloud API key/)
    fireEvent.change(input, { target: { value: ' ' } })
    expect(screen.getByText(/Saved key status is unknown/)).toBeInTheDocument()
    expect(screen.getByText('Save Key')).toBeDisabled()
    mockInvoke.mockResolvedValueOnce({ success: true })
    fireEvent.click(screen.getByText('Clear Key'))
    await waitFor(() => expect(screen.getByText(/Not configured:/)).toBeInTheDocument())
    expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.CONFIG_SET_POLLEN_API_KEY, '')
    expect(input).toHaveValue('')
    expect(screen.queryByText('Clear Key')).not.toBeInTheDocument()
  })

  it('shows and hides only the user-entered draft', async () => {
    const input = await load(true)
    fireEvent.change(input, { target: { value: 'draft-fixture' } })
    fireEvent.click(screen.getByTitle('Show key'))
    expect(input).toHaveAttribute('type', 'text')
    expect(input).toHaveValue('draft-fixture')
    fireEvent.click(screen.getByTitle('Hide key'))
    expect(input).toHaveAttribute('type', 'password')
  })

  it('clears the draft and visibility after a confirmed secure save', async () => {
    const input = await load()
    fireEvent.change(input, { target: { value: '  new-fixture-key  ' } })
    fireEvent.click(screen.getByTitle('Show key'))
    mockInvoke.mockResolvedValueOnce({ success: true })
    fireEvent.click(screen.getByText('Save Key'))
    await waitFor(() => expect(screen.getByText(/Configured:/)).toBeInTheDocument())
    expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.CONFIG_SET_POLLEN_API_KEY, 'new-fixture-key')
    expect(input).toHaveValue('')
    expect(input).toHaveAttribute('type', 'password')
    expect(screen.getByText('Save Key')).toBeDisabled()
  })

  it.each([
    ['unsuccessful result', () => Promise.resolve({ success: false, error: 'new-fixture-key' })],
    ['rejected IPC', () => Promise.reject(new Error('new-fixture-key'))],
    ['missing acknowledgment', () => Promise.resolve(undefined)],
  ])('retains a masked retry draft and hides raw errors for %s', async (_label, reply) => {
    const input = await load()
    fireEvent.change(input, { target: { value: 'new-fixture-key' } })
    mockInvoke.mockImplementationOnce(reply)
    fireEvent.click(screen.getByText('Save Key'))
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not save the key')
    expect(screen.getByRole('alert')).not.toHaveTextContent('new-fixture-key')
    expect(input).toHaveValue('new-fixture-key')
    expect(input).toHaveAttribute('type', 'password')
    expect(screen.getByText('Save Key')).not.toBeDisabled()
  })

  it('updates configured status only after clear succeeds', async () => {
    const input = await load(true)
    mockInvoke.mockResolvedValueOnce({ success: true })
    fireEvent.click(screen.getByText('Clear Key'))
    await waitFor(() => expect(screen.getByText(/Not configured:/)).toBeInTheDocument())
    expect(mockInvoke).toHaveBeenCalledWith(IPC_INVOKE.CONFIG_SET_POLLEN_API_KEY, '')
    expect(input).toHaveValue('')
    expect(screen.queryByText('Clear Key')).not.toBeInTheDocument()
  })

  it.each([
    [
      'unsuccessful result',
      () => Promise.resolve({ success: false, error: 'private-fixture-error' }),
    ],
    ['rejected IPC', () => Promise.reject(new Error('private-fixture-error'))],
  ])('preserves the configured key and shows safe clear failure for %s', async (_label, reply) => {
    await load(true)
    mockInvoke.mockImplementationOnce(reply)
    fireEvent.click(screen.getByText('Clear Key'))
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not clear the key')
    expect(screen.getByRole('alert')).not.toHaveTextContent('private-fixture-error')
    expect(screen.getByText('Clear Key')).not.toBeDisabled()
  })

  it('disables editing and visibility changes while a save is pending', async () => {
    const input = await load()
    fireEvent.change(input, { target: { value: 'new-fixture-key' } })
    let acknowledge!: (value: { success: true }) => void
    mockInvoke.mockImplementationOnce(
      () =>
        new Promise(resolve => {
          acknowledge = resolve
        })
    )
    fireEvent.click(screen.getByText('Save Key'))
    expect(input).toBeDisabled()
    expect(screen.getByTitle('Show key')).toBeDisabled()
    expect(screen.getByText('Saving…')).toBeInTheDocument()
    await act(async () => {
      acknowledge({ success: true })
    })
    expect(input).not.toBeDisabled()
    expect(input).toHaveValue('')
  })

  it('keeps whitespace drafts unsavable and clears a prior error when typing', async () => {
    const input = await load()
    fireEvent.change(input, { target: { value: '   ' } })
    expect(screen.getByText('Save Key')).toBeDisabled()
    fireEvent.change(input, { target: { value: 'fixture' } })
    mockInvoke.mockRejectedValueOnce(new Error('failure'))
    fireEvent.click(screen.getByText('Save Key'))
    await screen.findByRole('alert')
    fireEvent.change(input, { target: { value: 'retry-fixture' } })
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('opens the existing Google API setup link through IPC', async () => {
    await load()
    fireEvent.click(screen.getByText(/Enable the Google Pollen API/))
    expect(mockInvoke).toHaveBeenCalledWith(
      'shell:open-external',
      'https://console.cloud.google.com/apis/library/pollen.googleapis.com'
    )
  })

  it.each([false, true])(
    'ignores a late status reply after leaving the form, rejected=%s',
    async rejected => {
      let complete!: (value: unknown) => void
      let fail!: (error: Error) => void
      mockInvoke.mockImplementationOnce(
        () =>
          new Promise((resolve, reject) => {
            complete = resolve
            fail = reject
          })
      )
      const first = render(<SettingsWeather />)
      first.unmount()
      const input = await load(false)
      await act(async () => {
        if (rejected) fail(new Error('private-fixture-error'))
        else complete({ configured: true })
      })
      expect(screen.getByText(/Not configured:/)).toBeInTheDocument()
      expect(input).toHaveValue('')
      expect(screen.queryByRole('alert')).not.toBeInTheDocument()
    }
  )
})
