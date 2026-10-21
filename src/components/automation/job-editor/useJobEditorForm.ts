import { useState, useEffect } from 'react'
import {
  useJob,
  useJobMutations,
  useBuddyStatsMutations,
  type JobId,
} from '../../../hooks/useConvex'
import { useCopilotSettings } from '../../../hooks/useConfig'
import { getUserFacingErrorMessage } from '../../../utils/errorUtils'

export interface JobConfig {
  // exec-worker
  command?: string
  cwd?: string
  env?: Record<string, string>
  timeout?: number
  shell?: 'powershell' | 'bash' | 'cmd'
  // ai-worker
  prompt?: string
  model?: string
  maxTokens?: number
  temperature?: number
  repoOwner?: string
  repoName?: string
  // skill-worker
  skillName?: string
  action?: string
  params?: unknown
}

interface UseJobEditorFormSource {
  name: string
  description?: string
  workerType: 'exec' | 'ai' | 'skill'
  config: JobConfig
}

interface ConfigSetters {
  setCommand: (v: string) => void
  setCwd: (v: string) => void
  setTimeout: (v: number) => void
  setShell: (v: 'powershell' | 'bash' | 'cmd') => void
  setPrompt: (v: string) => void
  setModel: (v: string) => void
  setTargetRepo: (v: string) => void
  setSkillName: (v: string) => void
  setSkillAction: (v: string) => void
  setSkillParams: (v: string) => void
}

function loadExecConfig(config: JobConfig, setters: ConfigSetters): void {
  if (config.command) setters.setCommand(config.command)
  if (config.cwd) setters.setCwd(config.cwd)
  if (config.timeout) setters.setTimeout(config.timeout)
  if (config.shell) setters.setShell(config.shell)
}

function loadAiConfig(config: JobConfig, setters: ConfigSetters): void {
  if (config.prompt) setters.setPrompt(config.prompt)
  if (config.model) setters.setModel(config.model)
  if (config.repoOwner && config.repoName) {
    setters.setTargetRepo(`${config.repoOwner}/${config.repoName}`)
  }
}

function loadSkillConfig(config: JobConfig, setters: ConfigSetters): void {
  if (config.skillName) setters.setSkillName(config.skillName)
  if (config.action) setters.setSkillAction(config.action)
  if (config.params !== undefined) setters.setSkillParams(JSON.stringify(config.params, null, 2))
}

function loadSourceConfig(config: JobConfig, setters: ConfigSetters): void {
  loadExecConfig(config, setters)
  loadAiConfig(config, setters)
  loadSkillConfig(config, setters)
}

const WORKER_VALIDATIONS: Record<string, { field: string; message: string }> = {
  exec: { field: 'command', message: 'Command is required for exec jobs' },
  ai: { field: 'prompt', message: 'Prompt is required for AI jobs' },
  skill: { field: 'skillName', message: 'Skill name is required for skill jobs' },
}

function validateJobForm(
  name: string,
  workerType: 'exec' | 'ai' | 'skill',
  command: string,
  prompt: string,
  skillName: string
): string | null {
  if (!name.trim()) return 'Job name is required'
  const fields: Record<string, string> = { command, prompt, skillName }
  const rule = WORKER_VALIDATIONS[workerType]
  if (rule && !fields[rule.field]?.trim()) return rule.message
  return null
}

interface SourceFieldSetters extends ConfigSetters {
  setName: (v: string) => void
  setDescription: (v: string) => void
  setWorkerType: (v: 'exec' | 'ai' | 'skill') => void
}

function getOptionalDescription(description?: string): string {
  return description || ''
}

function populateJobFormFromSource(
  source: UseJobEditorFormSource,
  duplicateFrom: UseJobEditorFormSource | undefined,
  setters: SourceFieldSetters
): void {
  setters.setName(duplicateFrom ? `${source.name} (Copy)` : source.name)
  setters.setDescription(getOptionalDescription(source.description))
  setters.setWorkerType(source.workerType)
  loadSourceConfig(source.config, setters)
}

async function persistJobChanges({
  isEditing,
  jobId,
  name,
  description,
  workerType,
  config,
  create,
  update,
  incrementStat,
}: {
  isEditing: boolean
  jobId: string | undefined
  name: string
  description: string
  workerType: 'exec' | 'ai' | 'skill'
  config: JobConfig
  create: ReturnType<typeof useJobMutations>['create']
  update: ReturnType<typeof useJobMutations>['update']
  incrementStat: ReturnType<typeof useBuddyStatsMutations>['increment']
}) {
  const trimmedDescription = description.trim() || undefined
  if (isEditing && jobId) {
    await update({
      id: jobId as JobId,
      name: name.trim(),
      description: trimmedDescription,
      workerType,
      config,
    })
    return
  }

  await create({
    name: name.trim(),
    description: trimmedDescription,
    workerType,
    config,
  })
  /* v8 ignore start */
  incrementStat({ field: 'jobsCreated' }).catch(() => {})
  /* v8 ignore stop */
}

export function useJobEditorForm(
  jobId: string | undefined,
  duplicateFrom: UseJobEditorFormSource | undefined,
  onSaved: (() => void) | undefined,
  onClose: () => void
) {
  const existingJob = useJob(jobId as JobId | undefined)
  const { create, update } = useJobMutations()
  const { increment: incrementStat } = useBuddyStatsMutations()
  const { ghAccount: defaultGhAccount, model: defaultModel } = useCopilotSettings()

  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [workerType, setWorkerType] = useState<'exec' | 'ai' | 'skill'>('exec')
  const [command, setCommand] = useState('')
  const [cwd, setCwd] = useState('')
  const [timeout, setTimeout] = useState(60000)
  const [shell, setShell] = useState<'powershell' | 'bash' | 'cmd'>('powershell')
  const [prompt, setPrompt] = useState('')
  const isEditing = !!jobId
  const shouldUseDefaults = !isEditing && !duplicateFrom
  const [ghAccount, setGhAccount] = useState(() => (shouldUseDefaults ? defaultGhAccount : ''))
  const [model, setModel] = useState(() => (shouldUseDefaults ? defaultModel : ''))
  const [targetRepo, setTargetRepo] = useState('')
  const [skillName, setSkillName] = useState('')
  const [skillAction, setSkillAction] = useState('')
  const [skillParams, setSkillParams] = useState('')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const defaultsKey = `${shouldUseDefaults}:${defaultGhAccount}:${defaultModel}`
  const [appliedDefaultsKey, setAppliedDefaultsKey] = useState(defaultsKey)

  /* v8 ignore start */
  if (appliedDefaultsKey !== defaultsKey) {
    setAppliedDefaultsKey(defaultsKey)
    if (shouldUseDefaults) {
      setGhAccount(defaultGhAccount)
      setModel(defaultModel)
    }
  }
  /* v8 ignore stop */

  useEffect(() => {
    const source = existingJob || duplicateFrom
    if (!source) return
    populateJobFormFromSource(source, duplicateFrom, {
      setName,
      setDescription,
      setWorkerType,
      setCommand,
      setCwd,
      setTimeout,
      setShell,
      setPrompt,
      setModel,
      setTargetRepo,
      setSkillName,
      setSkillAction,
      setSkillParams,
    })
  }, [existingJob, duplicateFrom])

  const buildExecJobConfig = (): JobConfig => ({
    command: command.trim(),
    cwd: cwd.trim() || undefined,
    /* v8 ignore start */ timeout: timeout || undefined,
    /* v8 ignore stop */ shell,
  })

  const buildAiJobConfig = (): JobConfig => {
    const [repoOwner, repoName] = targetRepo ? targetRepo.split('/') : [undefined, undefined]
    return {
      prompt: prompt.trim(),
      /* v8 ignore start */ model: model.trim() || undefined,
      /* v8 ignore stop */ repoOwner,
      repoName,
    }
  }

  const buildSkillJobConfig = (): JobConfig => {
    let params: unknown = undefined
    if (skillParams.trim()) {
      try {
        params = JSON.parse(skillParams)
      } catch (_: unknown) {
        throw new Error('Invalid JSON in parameters', { cause: _ })
      }
    }
    return { skillName: skillName.trim(), action: skillAction.trim() || undefined, params }
  }

  const buildConfig = (): JobConfig => {
    const builders: Record<string, () => JobConfig> = {
      exec: buildExecJobConfig,
      ai: buildAiJobConfig,
      skill: buildSkillJobConfig,
    }
    /* v8 ignore start */
    return (builders[workerType] ?? (() => ({})))()
    /* v8 ignore stop */
  }

  const handleSave = async () => {
    const validationError = validateJobForm(name, workerType, command, prompt, skillName)
    if (validationError) {
      setError(validationError)
      return
    }
    setError(null)
    setSaving(true)
    try {
      const source = existingJob || duplicateFrom
      const config = { ...source?.config, ...buildConfig() }
      await persistJobChanges({
        isEditing,
        jobId,
        name,
        description,
        workerType,
        config,
        create,
        update,
        incrementStat,
      })
      onSaved?.()
      onClose()
    } catch (err: unknown) {
      setError(getUserFacingErrorMessage(err, 'Failed to save job'))
    } finally {
      setSaving(false)
    }
  }

  return {
    name,
    setName,
    description,
    setDescription,
    workerType,
    setWorkerType,
    command,
    setCommand,
    cwd,
    setCwd,
    timeout,
    setTimeout,
    shell,
    setShell,
    prompt,
    setPrompt,
    ghAccount,
    setGhAccount,
    model,
    setModel,
    targetRepo,
    setTargetRepo,
    skillName,
    setSkillName,
    skillAction,
    setSkillAction,
    skillParams,
    setSkillParams,
    saving,
    error,
    isEditing,
    handleSave,
  }
}
