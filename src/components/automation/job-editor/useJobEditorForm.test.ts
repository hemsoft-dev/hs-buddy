import { describe, it, expect, vi, beforeEach } from 'vitest'
import { renderHook, act, waitFor } from '@testing-library/react'

const { mockUseJob, mockCreate, mockUpdate, mockIncrementStat, mockUseCopilotSettings } =
  vi.hoisted(() => ({
    mockUseJob: vi.fn(),
    mockCreate: vi.fn(),
    mockUpdate: vi.fn(),
    mockIncrementStat: vi.fn(),
    mockUseCopilotSettings: vi.fn(),
  }))

vi.mock('../../../hooks/useConvex', () => ({
  useJob: mockUseJob,
  useJobMutations: () => ({ create: mockCreate, update: mockUpdate }),
  useBuddyStatsMutations: () => ({ increment: mockIncrementStat }),
}))

vi.mock('../../../hooks/useConfig', () => ({
  useCopilotSettings: mockUseCopilotSettings,
}))

import { useJobEditorForm } from './useJobEditorForm'

const onSaved = vi.fn()
const onClose = vi.fn()

function renderDefault(jobId?: string, duplicateFrom?: Parameters<typeof useJobEditorForm>[1]) {
  return renderHook(() => useJobEditorForm(jobId, duplicateFrom, onSaved, onClose))
}

describe('useJobEditorForm', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockUseJob.mockReturnValue(undefined)
    mockCreate.mockResolvedValue(undefined)
    mockUpdate.mockResolvedValue(undefined)
    mockIncrementStat.mockResolvedValue(undefined)
    mockUseCopilotSettings.mockReturnValue({ ghAccount: 'defaultUser', model: 'gpt-4' })
  })

  it('keeps explicit environment when duplicating a job through the older editor', async () => {
    const env = { BUDDY_FIXTURE_VALUE: '雪 exact' }
    const source = {
      name: 'Captured',
      workerType: 'exec' as const,
      config: { command: 'fixture', env },
    }
    const { result } = renderDefault(undefined, source)
    await waitFor(() => expect(result.current.name).toBe('Captured (Copy)'))
    await act(() => result.current.handleSave())
    expect(mockCreate).toHaveBeenCalledWith(
      expect.objectContaining({ config: expect.objectContaining({ env }) })
    )
  })

  it.each([null, false, 0])(
    'retains the explicit skill parameter %s when editing',
    async params => {
      mockUseJob.mockReturnValue({
        name: 'Skill',
        workerType: 'skill',
        config: { skillName: 'fixture', params },
      })
      const { result } = renderDefault('job-one')
      await waitFor(() => expect(result.current.name).toBe('Skill'))
      await act(() => result.current.handleSave())
      expect(mockUpdate).toHaveBeenCalledWith(
        expect.objectContaining({ config: expect.objectContaining({ params }) })
      )
    }
  )

  // ── 1. Default state ─────────────────────────────────────────────
  describe('default state', () => {
    it('returns empty/default values for a new job', async () => {
      const { result } = renderDefault()
      // ghAccount/model get populated from copilot settings via useEffect
      await waitFor(() => {
        expect(result.current.ghAccount).toBe('defaultUser')
        expect(result.current.model).toBe('gpt-4')
      })
      expect(result.current.name).toBe('')
      expect(result.current.description).toBe('')
      expect(result.current.workerType).toBe('exec')
      expect(result.current.command).toBe('')
      expect(result.current.cwd).toBe('')
      expect(result.current.timeout).toBe(60000)
      expect(result.current.shell).toBe('powershell')
      expect(result.current.prompt).toBe('')
      expect(result.current.targetRepo).toBe('')
      expect(result.current.skillName).toBe('')
      expect(result.current.skillAction).toBe('')
      expect(result.current.skillParams).toBe('')
      expect(result.current.saving).toBe(false)
      expect(result.current.error).toBeNull()
      expect(result.current.isEditing).toBe(false)
    })
  })

  // ── 2. Copilot settings defaults ─────────────────────────────────
  describe('copilot settings defaults', () => {
    it('applies default ghAccount and model when not editing and no duplicateFrom', async () => {
      const { result } = renderDefault()
      await waitFor(() => {
        expect(result.current.ghAccount).toBe('defaultUser')
        expect(result.current.model).toBe('gpt-4')
      })
    })

    it('does not apply defaults when editing', async () => {
      mockUseJob.mockReturnValue({
        name: 'Existing',
        workerType: 'exec',
        config: { command: 'echo hi' },
      })
      const { result } = renderDefault('job-123')
      await waitFor(() => {
        expect(result.current.name).toBe('Existing')
      })
      // ghAccount should not be overwritten to the copilot default
      // because the effect guards on isEditing
      expect(result.current.ghAccount).toBe('')
    })

    it('does not apply defaults when duplicateFrom is provided', async () => {
      const dup = {
        name: 'Source',
        workerType: 'ai' as const,
        config: { prompt: 'do stuff', model: 'claude' },
      }
      const { result } = renderDefault(undefined, dup)
      await waitFor(() => {
        expect(result.current.name).toBe('Source (Copy)')
      })
      // model comes from duplicateFrom, not from copilot settings
      expect(result.current.model).toBe('claude')
    })
  })

  // ── 3. Populate from existingJob (edit mode) ─────────────────────
  describe('populate from existingJob', () => {
    it('loads all exec fields from an existing job', async () => {
      mockUseJob.mockReturnValue({
        name: 'Build',
        description: 'Runs build',
        workerType: 'exec',
        config: { command: 'npm run build', cwd: '/app', timeout: 30000, shell: 'bash' },
      })
      const { result } = renderDefault('job-1')
      await waitFor(() => {
        expect(result.current.name).toBe('Build')
      })
      expect(result.current.description).toBe('Runs build')
      expect(result.current.workerType).toBe('exec')
      expect(result.current.command).toBe('npm run build')
      expect(result.current.cwd).toBe('/app')
      expect(result.current.timeout).toBe(30000)
      expect(result.current.shell).toBe('bash')
    })

    it('loads ai fields including targetRepo', async () => {
      mockUseJob.mockReturnValue({
        name: 'Summarize',
        workerType: 'ai',
        config: { prompt: 'Summarize PR', model: 'gpt-4', repoOwner: 'acme', repoName: 'app' },
      })
      const { result } = renderDefault('job-2')
      await waitFor(() => {
        expect(result.current.name).toBe('Summarize')
      })
      expect(result.current.prompt).toBe('Summarize PR')
      expect(result.current.model).toBe('gpt-4')
      expect(result.current.targetRepo).toBe('acme/app')
    })

    it('loads skill fields including params as JSON string', async () => {
      mockUseJob.mockReturnValue({
        name: 'Run Skill',
        workerType: 'skill',
        config: { skillName: 'deploy', action: 'start', params: { env: 'prod' } },
      })
      const { result } = renderDefault('job-3')
      await waitFor(() => {
        expect(result.current.name).toBe('Run Skill')
      })
      expect(result.current.skillName).toBe('deploy')
      expect(result.current.skillAction).toBe('start')
      expect(result.current.skillParams).toBe(JSON.stringify({ env: 'prod' }, null, 2))
    })
  })

  // ── 4. Populate from duplicateFrom with "(Copy)" suffix ──────────
  describe('populate from duplicateFrom', () => {
    it('appends (Copy) to the name', async () => {
      const dup = {
        name: 'My Job',
        description: 'desc',
        workerType: 'exec' as const,
        config: { command: 'ls' },
      }
      const { result } = renderDefault(undefined, dup)
      await waitFor(() => {
        expect(result.current.name).toBe('My Job (Copy)')
      })
      expect(result.current.description).toBe('desc')
      expect(result.current.command).toBe('ls')
    })
  })

  // ── 5. isEditing ─────────────────────────────────────────────────
  describe('isEditing', () => {
    it('returns true when jobId is provided', () => {
      const { result } = renderDefault('job-42')
      expect(result.current.isEditing).toBe(true)
    })

    it('returns false when jobId is undefined', () => {
      const { result } = renderDefault()
      expect(result.current.isEditing).toBe(false)
    })
  })

  // ── 6. handleSave validation ─────────────────────────────────────
  describe('handleSave validation', () => {
    it('sets error when name is empty', async () => {
      const { result } = renderDefault()
      await act(() => result.current.handleSave())
      expect(result.current.error).toBe('Job name is required')
      expect(mockCreate).not.toHaveBeenCalled()
    })

    it('sets error when command is empty for exec worker', async () => {
      const { result } = renderDefault()
      act(() => result.current.setName('Test'))
      await act(() => result.current.handleSave())
      expect(result.current.error).toBe('Command is required for exec jobs')
    })

    it('sets error when prompt is empty for ai worker', async () => {
      const { result } = renderDefault()
      act(() => {
        result.current.setName('Test')
        result.current.setWorkerType('ai')
      })
      await act(() => result.current.handleSave())
      expect(result.current.error).toBe('Prompt is required for AI jobs')
    })

    it('sets error when skillName is empty for skill worker', async () => {
      const { result } = renderDefault()
      act(() => {
        result.current.setName('Test')
        result.current.setWorkerType('skill')
      })
      await act(() => result.current.handleSave())
      expect(result.current.error).toBe('Skill name is required for skill jobs')
    })
  })

  // ── 7. handleSave success – create ───────────────────────────────
  describe('handleSave success (create)', () => {
    it('calls create, incrementStat, onSaved, and onClose', async () => {
      const { result } = renderDefault()
      act(() => {
        result.current.setName('New Job')
        result.current.setCommand('echo hello')
      })
      await act(() => result.current.handleSave())

      expect(mockCreate).toHaveBeenCalledWith({
        name: 'New Job',
        description: undefined,
        workerType: 'exec',
        config: { command: 'echo hello', cwd: undefined, timeout: 60000, shell: 'powershell' },
      })
      expect(mockIncrementStat).toHaveBeenCalledWith({ field: 'jobsCreated' })
      expect(onSaved).toHaveBeenCalled()
      expect(onClose).toHaveBeenCalled()
      expect(result.current.saving).toBe(false)
      expect(result.current.error).toBeNull()
    })
  })

  // ── 8. handleSave success – update ───────────────────────────────
  describe('handleSave success (update)', () => {
    it('calls update (not create) for an existing job', async () => {
      mockUseJob.mockReturnValue({
        name: 'Old',
        workerType: 'exec',
        config: { command: 'old cmd' },
      })
      const { result } = renderDefault('job-99')
      await waitFor(() => {
        expect(result.current.name).toBe('Old')
      })

      act(() => {
        result.current.setName('Updated')
        result.current.setCommand('new cmd')
      })
      await act(() => result.current.handleSave())

      expect(mockUpdate).toHaveBeenCalledWith(
        expect.objectContaining({ id: 'job-99', name: 'Updated' })
      )
      expect(mockCreate).not.toHaveBeenCalled()
      expect(mockIncrementStat).not.toHaveBeenCalled()
      expect(onSaved).toHaveBeenCalled()
      expect(onClose).toHaveBeenCalled()
    })
  })

  // ── 9. handleSave error handling ─────────────────────────────────
  describe('handleSave error handling', () => {
    it('captures error message from thrown Error', async () => {
      mockCreate.mockRejectedValueOnce(new Error('Network failure'))
      const { result } = renderDefault()
      act(() => {
        result.current.setName('Fail Job')
        result.current.setCommand('cmd')
      })
      await act(() => result.current.handleSave())

      expect(result.current.error).toBe('Network failure')
      expect(result.current.saving).toBe(false)
    })

    it('uses fallback message for non-Error throws', async () => {
      mockCreate.mockRejectedValueOnce('something bad')
      const { result } = renderDefault()
      act(() => {
        result.current.setName('Fail Job')
        result.current.setCommand('cmd')
      })
      await act(() => result.current.handleSave())

      expect(result.current.error).toBe('something bad')
    })
  })

  // ── 10. buildConfig for each worker type ─────────────────────────
  describe('buildConfig via handleSave', () => {
    it('builds exec config with trimmed values', async () => {
      const { result } = renderDefault()
      act(() => {
        result.current.setName('Exec')
        result.current.setCommand('  echo hi  ')
        result.current.setCwd('  /home  ')
        result.current.setTimeout(5000)
        result.current.setShell('bash')
      })
      await act(() => result.current.handleSave())

      expect(mockCreate).toHaveBeenCalledWith(
        expect.objectContaining({
          config: { command: 'echo hi', cwd: '/home', timeout: 5000, shell: 'bash' },
        })
      )
    })

    it('builds exec config with undefined cwd when empty', async () => {
      const { result } = renderDefault()
      act(() => {
        result.current.setName('Exec')
        result.current.setCommand('echo hi')
        result.current.setCwd('')
      })
      await act(() => result.current.handleSave())

      expect(mockCreate).toHaveBeenCalledWith(
        expect.objectContaining({
          config: expect.objectContaining({ cwd: undefined }),
        })
      )
    })

    it('builds ai config with repo split', async () => {
      const { result } = renderDefault()
      act(() => {
        result.current.setName('AI Job')
        result.current.setWorkerType('ai')
        result.current.setPrompt('analyze code')
        result.current.setModel('  claude-3  ')
        result.current.setTargetRepo('acme/repo')
      })
      await act(() => result.current.handleSave())

      expect(mockCreate).toHaveBeenCalledWith(
        expect.objectContaining({
          config: {
            prompt: 'analyze code',
            model: 'claude-3',
            repoOwner: 'acme',
            repoName: 'repo',
          },
        })
      )
    })

    it('builds ai config with undefined repo parts when targetRepo is empty', async () => {
      const { result } = renderDefault()
      act(() => {
        result.current.setName('AI Job')
        result.current.setWorkerType('ai')
        result.current.setPrompt('do something')
      })
      await act(() => result.current.handleSave())

      expect(mockCreate).toHaveBeenCalledWith(
        expect.objectContaining({
          config: expect.objectContaining({ repoOwner: undefined, repoName: undefined }),
        })
      )
    })

    it('builds skill config with parsed JSON params', async () => {
      const { result } = renderDefault()
      act(() => {
        result.current.setName('Skill Job')
        result.current.setWorkerType('skill')
        result.current.setSkillName('deploy')
        result.current.setSkillAction('run')
        result.current.setSkillParams('{"env":"staging"}')
      })
      await act(() => result.current.handleSave())

      expect(mockCreate).toHaveBeenCalledWith(
        expect.objectContaining({
          config: {
            skillName: 'deploy',
            action: 'run',
            params: { env: 'staging' },
          },
        })
      )
    })

    it('builds skill config with undefined params when skillParams is empty', async () => {
      const { result } = renderDefault()
      act(() => {
        result.current.setName('Skill Job')
        result.current.setWorkerType('skill')
        result.current.setSkillName('deploy')
      })
      await act(() => result.current.handleSave())

      expect(mockCreate).toHaveBeenCalledWith(
        expect.objectContaining({
          config: expect.objectContaining({ params: undefined }),
        })
      )
    })
  })

  // ── 11. buildConfig skill with invalid JSON ──────────────────────
  describe('buildConfig with invalid JSON', () => {
    it('sets error when skill params contain invalid JSON', async () => {
      const { result } = renderDefault()
      act(() => {
        result.current.setName('Bad Params')
        result.current.setWorkerType('skill')
        result.current.setSkillName('deploy')
        result.current.setSkillParams('{not valid json}')
      })
      await act(() => result.current.handleSave())

      expect(result.current.error).toBe('Invalid JSON in parameters')
      expect(mockCreate).not.toHaveBeenCalled()
      expect(result.current.saving).toBe(false)
    })
  })
})
