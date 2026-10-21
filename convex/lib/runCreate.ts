import { convexToJson, type Value } from 'convex/values'
import type { Doc, Id } from '../_generated/dataModel'
import type { MutationCtx, QueryCtx } from '../_generated/server'
import { notFoundError } from './domain'
import { insertRun } from './runStore'
import { incrementStat } from './stats'

type CreateArgs = Pick<Doc<'runs'>, 'jobId' | 'triggeredBy' | 'scheduleId' | 'input'>

export async function createRun(ctx: MutationCtx, args: CreateArgs): Promise<Id<'runs'>> {
  if (!(await ctx.db.get('jobs', args.jobId))) throw notFoundError('Job', args.jobId)
  if (args.scheduleId) {
    const schedule = await ctx.db.get('schedules', args.scheduleId)
    if (!schedule) throw notFoundError('Schedule', args.scheduleId)
    if (schedule.jobId !== args.jobId)
      throw new Error(`Schedule ${args.scheduleId} does not belong to job ${args.jobId}`)
  }
  const id = await insertRun(ctx, { ...args, status: 'pending', startedAt: Date.now() })
  await incrementStat(ctx.db, 'runsTriggered')
  return id
}

export function validateRequestId(requestId: string): void {
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(requestId))
    throw new Error('A native request requires a UUID request identity')
}

type NativeTrigger = { kind: 'recorded' | 'retired'; requestId: string; runId: Id<'runs'> }

async function triggerReceipt(ctx: QueryCtx, requestId: string) {
  validateRequestId(requestId)
  const receipt = await ctx.db
    .query('automationReceipts')
    .withIndex('by_request', q => q.eq('requestId', requestId))
    .unique()
  if (receipt && receipt.kind !== 'trigger')
    throw new Error('Request identity belongs to an execution claim')
  return receipt
}

export async function getNativeTrigger(
  ctx: QueryCtx,
  requestId: string
): Promise<NativeTrigger | null> {
  const receipt = await triggerReceipt(ctx, requestId)
  if (!receipt) return null
  const run = await ctx.db.get('runs', receipt.runId)
  return { kind: run ? 'recorded' : 'retired', requestId, runId: receipt.runId }
}

function canonicalJson(value: ReturnType<typeof convexToJson>): string {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`
  if (value !== null && typeof value === 'object')
    return `{${Object.keys(value)
      .sort()
      .map(key => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
      .join(',')}}`
  return JSON.stringify(value)
}

function capturedInput(value: Value | undefined): string {
  return value === undefined ? 'absent' : canonicalJson(convexToJson(value))
}

export async function triggerNativeRun(
  ctx: MutationCtx,
  args: { requestId: string; jobId: Id<'jobs'>; input?: Value }
): Promise<NativeTrigger> {
  const receipt = await triggerReceipt(ctx, args.requestId)
  if (receipt) {
    if (receipt.jobId !== args.jobId) throw new Error('The captured trigger job does not match')
    const run = await ctx.db.get('runs', receipt.runId)
    if (!run) return { kind: 'retired', requestId: args.requestId, runId: receipt.runId }
    if (capturedInput(run.input) !== capturedInput(args.input))
      throw new Error('The captured trigger input does not match')
    return { kind: 'recorded', requestId: args.requestId, runId: receipt.runId }
  }
  const runId = await createRun(ctx, {
    jobId: args.jobId,
    input: args.input,
    triggeredBy: 'manual',
  })
  await ctx.db.insert('automationReceipts', {
    kind: 'trigger',
    requestId: args.requestId,
    jobId: args.jobId,
    runId,
  })
  return { kind: 'recorded', requestId: args.requestId, runId }
}
