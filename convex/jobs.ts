import { v } from 'convex/values'
import { mutation, query } from './_generated/server'
import { notFoundError } from './lib/domain'
import { jobConfigValidator } from './lib/jobConfig'
import { buildUpdateData } from '../shared/utils/convexPatchUtils'

/**
 * Job CRUD operations
 */

const workerTypeValidator = v.union(v.literal('exec'), v.literal('ai'), v.literal('skill'))

// List all jobs
export const list = query({
  args: {},
  handler: async ctx => {
    return ctx.db.query('jobs').collect()
  },
})

// List jobs by worker type
export const listByType = query({
  args: {
    workerType: workerTypeValidator,
  },
  handler: async (ctx, args) => {
    return ctx.db
      .query('jobs')
      .withIndex('by_worker_type', q => q.eq('workerType', args.workerType))
      .collect()
  },
})

// Get single job by ID
export const get = query({
  args: { id: v.id('jobs') },
  handler: async (ctx, args) => {
    return ctx.db.get('jobs', args.id)
  },
})

// Get job by name
export const getByName = query({
  args: { name: v.string() },
  handler: async (ctx, args) => {
    return ctx.db
      .query('jobs')
      .withIndex('by_name', q => q.eq('name', args.name))
      .first()
  },
})

// Create new job
export const create = mutation({
  args: {
    name: v.string(),
    description: v.optional(v.string()),
    workerType: workerTypeValidator,
    config: jobConfigValidator,
    inputParams: v.optional(
      v.array(
        v.object({
          name: v.string(),
          type: v.union(v.literal('string'), v.literal('number'), v.literal('boolean')),
          defaultValue: v.optional(v.any()),
          required: v.boolean(),
          description: v.optional(v.string()),
        })
      )
    ),
  },
  handler: async (ctx, args) => {
    // Check for duplicate name
    const existing = await ctx.db
      .query('jobs')
      .withIndex('by_name', q => q.eq('name', args.name))
      .first()

    if (existing) {
      throw new Error(`Job with name "${args.name}" already exists`)
    }

    const now = Date.now()
    const id = await ctx.db.insert('jobs', {
      name: args.name,
      description: args.description,
      workerType: args.workerType,
      config: args.config,
      inputParams: args.inputParams,
      createdAt: now,
      updatedAt: now,
    })

    return id
  },
})

// Update existing job
export const update = mutation({
  args: {
    id: v.id('jobs'),
    name: v.optional(v.string()),
    description: v.optional(v.string()),
    workerType: v.optional(workerTypeValidator),
    config: v.optional(jobConfigValidator),
    inputParams: v.optional(
      v.array(
        v.object({
          name: v.string(),
          type: v.union(v.literal('string'), v.literal('number'), v.literal('boolean')),
          defaultValue: v.optional(v.any()),
          required: v.boolean(),
          description: v.optional(v.string()),
        })
      )
    ),
  },
  handler: async (ctx, args) => {
    const { id, ...updates } = args

    const existing = await ctx.db.get('jobs', id)
    if (!existing) {
      throw notFoundError('Job', id)
    }

    // If renaming, check for duplicates
    if (updates.name && updates.name !== existing.name) {
      const duplicate = await ctx.db
        .query('jobs')
        .withIndex('by_name', q => q.eq('name', updates.name!))
        .first()

      if (duplicate) {
        throw new Error(`Job with name "${updates.name}" already exists`)
      }
    }

    // Older Electron editors do not expose environment. Preserve it on those
    // config replacements; an explicit empty env object clears overrides.
    const preserved =
      updates.config && updates.config.env === undefined && existing.config.env
        ? { ...updates, config: { ...updates.config, env: existing.config.env } }
        : updates
    await ctx.db.patch('jobs', id, buildUpdateData(preserved, Date.now()))
    return id
  },
})

// Delete job (and associated schedules)
export const remove = mutation({
  args: { id: v.id('jobs') },
  handler: async (ctx, args) => {
    const existing = await ctx.db.get('jobs', args.id)
    if (!existing) {
      throw notFoundError('Job', args.id)
    }

    // Delete associated schedules
    const schedules = await ctx.db
      .query('schedules')
      .withIndex('by_job', q => q.eq('jobId', args.id))
      .collect()

    for (const schedule of schedules) {
      // react-doctor-disable-next-line react-doctor/async-await-in-loop -- Associated schedules are deleted in one mutation before deleting the parent job.
      await ctx.db.delete('schedules', schedule._id)
    }

    await ctx.db.delete('jobs', args.id)
    return args.id
  },
})
