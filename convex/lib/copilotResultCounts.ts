import type { MutationCtx } from '../_generated/server'

type ResultStatus = 'pending' | 'running' | 'completed' | 'failed'
type ActiveStatus = 'pending' | 'running'

function activeStatus(status: ResultStatus | undefined): ActiveStatus | undefined {
  return status === 'pending' || status === 'running' ? status : undefined
}

async function adjust(ctx: MutationCtx, status: ActiveStatus, delta: number) {
  const counts = await ctx.db
    .query('copilotResultCounts')
    .withIndex('by_key', q => q.eq('key', 'default'))
    .unique()
  const next = { pending: counts?.pending ?? 0, running: counts?.running ?? 0 }
  next[status] = Math.max(0, next[status] + delta)
  if (counts) await ctx.db.patch(counts._id, next)
  else await ctx.db.insert('copilotResultCounts', { key: 'default', ...next })
}

// Every caller runs inside the same transaction as its result transition.
export async function transitionResultCount(
  ctx: MutationCtx,
  previous: ResultStatus | undefined,
  next: ResultStatus | undefined
) {
  const from = activeStatus(previous)
  const to = activeStatus(next)
  if (from === to) return
  if (from) await adjust(ctx, from, -1)
  if (to) await adjust(ctx, to, 1)
}
