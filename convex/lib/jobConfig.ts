import { v } from 'convex/values'
import { shellValidator } from './domain'

/** The schema and both CRUD mutations must accept the same optional settings. */
export const jobConfigValidator = v.object({
  command: v.optional(v.string()),
  cwd: v.optional(v.string()),
  env: v.optional(v.record(v.string(), v.string())),
  timeout: v.optional(v.number()),
  shell: v.optional(shellValidator),
  prompt: v.optional(v.string()),
  model: v.optional(v.string()),
  maxTokens: v.optional(v.number()),
  temperature: v.optional(v.number()),
  repoOwner: v.optional(v.string()),
  repoName: v.optional(v.string()),
  skillName: v.optional(v.string()),
  action: v.optional(v.string()),
  params: v.optional(v.any()),
})
