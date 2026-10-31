import { isValidGitHubAccountSlug } from '../../shared/githubAccountIdentity'
import {
  getUsageProviderOverrideKey,
  type UsageProviderOverrides,
} from '../utils/usageProviderOverrides'
import type { ConvexGitHubAccount } from './githubAccountProviderTypes'

export type ObservedAccountGroup = {
  account: Pick<ConvexGitHubAccount, 'username' | 'org'>
  ids: Set<string>
}

type PendingRemoteClear = {
  account: Pick<ConvexGitHubAccount, 'username' | 'org'>
  documentIds: string
}

export type PendingRemoteClears = Map<string, PendingRemoteClear>

function hasValidIdentity(account: ConvexGitHubAccount) {
  return isValidGitHubAccountSlug(account.username) && isValidGitHubAccountSlug(account.org)
}

function collectAccountGroups(accounts: ConvexGitHubAccount[]) {
  const groups = new Map<string, ObservedAccountGroup>()
  for (const account of accounts) {
    if (!hasValidIdentity(account)) continue
    const key = getUsageProviderOverrideKey(account)
    const group = groups.get(key) ?? {
      account: { username: account.username, org: account.org },
      ids: new Set<string>(),
    }
    group.ids.add(account._id ?? key)
    groups.set(key, group)
  }
  return groups
}

function recordRemovedDocuments(
  previous: Map<string, ObservedAccountGroup>,
  groups: Map<string, ObservedAccountGroup>,
  pendingClears: PendingRemoteClears
) {
  for (const [key, previousGroup] of previous) {
    const currentGroup = groups.get(key)
    const retainsDocument =
      currentGroup && [...previousGroup.ids].some(id => currentGroup.ids.has(id))
    if (!retainsDocument) {
      pendingClears.set(key, {
        account: currentGroup?.account ?? previousGroup.account,
        documentIds: getDocumentIds(currentGroup),
      })
    }
  }
}

function refreshPendingGenerations(
  groups: Map<string, ObservedAccountGroup>,
  pendingClears: PendingRemoteClears
) {
  for (const [key, pendingClear] of pendingClears) {
    const currentGroup = groups.get(key)
    const documentIds = getDocumentIds(currentGroup)
    if (documentIds !== pendingClear.documentIds) {
      pendingClears.set(key, {
        account: currentGroup?.account ?? pendingClear.account,
        documentIds,
      })
    }
  }
}

export function observeRemoteAccounts(
  accounts: ConvexGitHubAccount[],
  previous: Map<string, ObservedAccountGroup>,
  pendingClears: PendingRemoteClears
) {
  const groups = collectAccountGroups(accounts)
  recordRemovedDocuments(previous, groups, pendingClears)
  refreshPendingGenerations(groups, pendingClears)
  const documentSnapshot = [...groups]
    .map(([key, group]) => [key, [...group.ids].sort()] as const)
    .sort(([left], [right]) => left.localeCompare(right))
  return { groups, documentSnapshot }
}

function getDocumentIds(group: ObservedAccountGroup | undefined) {
  return group ? [...group.ids].sort().join('\0') : ''
}

type OverrideResult = { success: boolean; error?: string }
type ClearOverride = (
  account: Pick<ConvexGitHubAccount, 'username' | 'org'>
) => Promise<OverrideResult>
type SerializeClear = (
  account: Pick<ConvexGitHubAccount, 'username' | 'org'>,
  operation: (isSuperseded: () => boolean) => Promise<OverrideResult>
) => Promise<OverrideResult>

export async function clearPendingRemoteOverrides(
  pendingClears: PendingRemoteClears,
  clearOverride: ClearOverride,
  serializeClear: SerializeClear
): Promise<OverrideResult> {
  const entries = [...pendingClears]
  const results = await Promise.all(
    entries.map(async ([key, pendingClear]) => ({
      key,
      pendingClear,
      result: await serializeClear(pendingClear.account, async isSuperseded => {
        if (pendingClears.get(key) !== pendingClear || isSuperseded()) return { success: true }
        const result = await clearOverride(pendingClear.account)
        return isSuperseded() ? { success: true } : result
      }),
    }))
  )
  for (const { key, pendingClear, result } of results) {
    if (result.success && pendingClears.get(key) === pendingClear) pendingClears.delete(key)
  }
  return results.find(({ result }) => !result.success)?.result ?? { success: true }
}

export function retainConnectedOverrides(
  current: UsageProviderOverrides,
  accountKeys: Set<string>,
  changedKeys: Set<string>
) {
  const next: UsageProviderOverrides = {}
  let changed = false
  for (const [key, provider] of Object.entries(current)) {
    if (accountKeys.has(key)) next[key] = provider
    else {
      changed = true
      changedKeys.add(key)
    }
  }
  return changed ? next : current
}
