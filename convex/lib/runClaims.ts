import type { MutationCtx, QueryCtx } from '../_generated/server'
import type { Doc, Id } from '../_generated/dataModel'
import { isPendingOrRunning, notFoundError } from './domain'
import { patchRun } from './runStore'
import { incrementStat } from './stats'
import { validateRequestId } from './runCreate'

export function publicRun(run: Doc<'runs'>): Omit<Doc<'runs'>, 'nativeClaimRequestId'> {
  const visible = { ...run }
  delete visible.nativeClaimRequestId
  return visible
}

export async function retireClaimPayload(ctx: MutationCtx, run: Doc<'runs'>): Promise<void> {
  const requestId = run.nativeClaimRequestId
  if (!requestId) return
  const claim = await ctx.db
    .query('automationReceipts')
    .withIndex('by_request', q => q.eq('requestId', requestId))
    .unique()
  if (claim?.kind === 'claim')
    await ctx.db.patch('automationReceipts', claim._id, { job: undefined })
}

type NativeClaim =
  | { kind: 'retired'; requestId: string; runId: Id<'runs'> }
  | { kind: 'claimed'; requestId: string; run: ReturnType<typeof publicRun>; job: Doc<'jobs'> }

export async function getNativeClaim(
  ctx: QueryCtx,
  requestId: string
): Promise<NativeClaim | null> {
  validateRequestId(requestId)
  const claim = await ctx.db
    .query('automationReceipts')
    .withIndex('by_request', q => q.eq('requestId', requestId))
    .unique()
  if (!claim) return null
  if (claim.kind !== 'claim') throw new Error('Request identity belongs to a manual trigger')
  const run = await ctx.db.get('runs', claim.runId)
  if (!run || !claim.job) return { kind: 'retired', requestId, runId: claim.runId }
  return { kind: 'claimed', requestId, run: publicRun(run), job: claim.job as Doc<'jobs'> }
}

export async function claimNativeRun(
  ctx: MutationCtx,
  requestId: string,
  runId: Id<'runs'>
): Promise<NativeClaim> {
  const original = await getNativeClaim(ctx, requestId)
  if (original) {
    const originalId = original.kind === 'claimed' ? original.run._id : original.runId
    if (originalId !== runId) throw new Error('The captured claim candidate does not match')
    return original
  }
  const run = await ctx.db.get('runs', runId)
  if (!run || run.status !== 'pending') {
    await ctx.db.insert('automationReceipts', { kind: 'claim', requestId, runId })
    return { kind: 'retired', requestId, runId }
  }
  const job = await ctx.db.get('jobs', run.jobId)
  await patchRun(ctx, run._id, { status: 'running', nativeClaimRequestId: requestId })
  await ctx.db.insert('automationReceipts', {
    kind: 'claim',
    requestId,
    runId: run._id,
    job: job ?? undefined,
  })
  if (!job) {
    await finalizeRun(
      ctx,
      run._id,
      'failed',
      'runsFailed',
      { error: 'The claimed job no longer exists' },
      requestId
    )
    return { kind: 'retired', requestId, runId: run._id }
  }
  return { kind: 'claimed', requestId, run: publicRun({ ...run, status: 'running' }), job }
}

export async function finalizeRun(
  ctx: MutationCtx,
  runId: Id<'runs'>,
  status: 'completed' | 'failed',
  statKey: 'runsCompleted' | 'runsFailed',
  extraPatch: Record<string, unknown>,
  claimRequestId?: string
) {
  const run = await ctx.db.get('runs', runId)
  if (!run) throw notFoundError('Run', runId)
  if (run.nativeClaimRequestId !== claimRequestId)
    throw new Error('Run claim ownership does not match')
  if (!isPendingOrRunning(run.status)) {
    // Cancellation is terminal, but the captured worker can still publish its
    // final bounded output once. Late completion must never change the status
    // or increment completion/failure counters.
    if (
      run.status === 'cancelled' &&
      run.nativeClaimRequestId &&
      run.output === undefined &&
      extraPatch.output !== undefined
    ) {
      await patchRun(ctx, runId, { output: extraPatch.output })
    }
    return
  }
  const completedAt = Date.now()
  await patchRun(ctx, runId, {
    status,
    ...extraPatch,
    completedAt,
    duration: completedAt - run.startedAt,
  })
  await incrementStat(ctx.db, statKey)
  if (run.scheduleId) {
    const schedule = await ctx.db.get('schedules', run.scheduleId)
    if (schedule)
      await ctx.db.patch('schedules', schedule._id, {
        lastRunAt: completedAt,
        lastRunStatus: status,
      })
  }
}
