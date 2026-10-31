import { isValidGitHubAccountSlug } from '../../shared/githubAccountIdentity'
import type { GitHubAccount, UsageProvider } from '../types/config'
import {
  getUsageProviderOverrideKey,
  type UsageProviderOverrides,
} from '../utils/usageProviderOverrides'
import { retainConnectedOverrides } from './githubAccountMirrorObservation'
import type { ConvexGitHubAccount } from './githubAccountProviderTypes'

export type { ConvexGitHubAccount } from './githubAccountProviderTypes'

export function markOverrideChange(
  key: string,
  provider: UsageProvider | null,
  changedOverrideKeys: Set<string>,
  changedDefaultOverrideKeys: Set<string>
) {
  changedOverrideKeys.add(key)
  if (provider !== null) changedDefaultOverrideKeys.add(key)
}

export function toDurableAccounts(accounts: ConvexGitHubAccount[]): GitHubAccount[] {
  const accountGroups = new Map<string, ConvexGitHubAccount[]>()
  for (const account of accounts.filter(
    candidate =>
      isValidGitHubAccountSlug(candidate.username) && isValidGitHubAccountSlug(candidate.org)
  )) {
    const key = getUsageProviderOverrideKey(account)
    accountGroups.set(key, [...(accountGroups.get(key) ?? []), account])
  }
  return [...accountGroups.values()].map(group => {
    const canonical = [...group].sort((left, right) => {
      const createdDifference = (left.createdAt ?? 0) - (right.createdAt ?? 0)
      return createdDifference || (left._id ?? '').localeCompare(right._id ?? '')
    })[0]
    const ordered = [...group].sort((left, right) => {
      const updatedDifference =
        (left.updatedAt ?? left.createdAt ?? 0) - (right.updatedAt ?? right.createdAt ?? 0)
      return updatedDifference || (left._id ?? '').localeCompare(right._id ?? '')
    })
    return ordered.reduce<GitHubAccount>(
      (durable, account) => {
        if (account.repoRoot !== undefined) durable.repoRoot = account.repoRoot
        if (account.usageProvider !== undefined) durable.usageProvider = account.usageProvider
        return durable
      },
      { username: canonical.username, org: canonical.org }
    )
  })
}

function applyOverride(account: GitHubAccount, overrides: UsageProviderOverrides): GitHubAccount {
  const localProvider = overrides[getUsageProviderOverrideKey(account)]
  const usageProvider = localProvider ?? account.usageProvider
  return { ...account, ...(usageProvider ? { usageProvider } : {}) }
}

export function getConnectedOverrides(
  accounts: ConvexGitHubAccount[],
  overrides: UsageProviderOverrides,
  defaultOverrides: UsageProviderOverrides
): UsageProviderOverrides {
  const explicitCodexOwner = accounts.find(account => {
    const override = overrides[getUsageProviderOverrideKey(account)]
    return account.usageProvider === 'codex' && override !== 'copilot'
  })
  let codexOwnerKey = explicitCodexOwner ? getUsageProviderOverrideKey(explicitCodexOwner) : null
  const connectedOverrides: UsageProviderOverrides = {}

  for (const account of accounts) {
    const key = getUsageProviderOverrideKey(account)
    const provider = overrides[key]
    if (!provider) continue
    if (defaultOverrides[key] === provider && account.usageProvider !== undefined) continue
    if (provider === 'codex' && codexOwnerKey && codexOwnerKey !== key) continue
    connectedOverrides[key] = provider
    if (provider === 'codex') codexOwnerKey = key
  }
  return connectedOverrides
}

export function resolveAccountsFromSources(
  convexAccounts: ConvexGitHubAccount[] | undefined,
  electronStoreAccounts: GitHubAccount[],
  overrides: UsageProviderOverrides,
  defaultOverrides: UsageProviderOverrides
): GitHubAccount[] {
  if (!convexAccounts) {
    return electronStoreAccounts.map(account => applyOverride(account, overrides))
  }
  const connectedOverrides = getConnectedOverrides(convexAccounts, overrides, defaultOverrides)
  return convexAccounts.map(account =>
    applyOverride(
      {
        username: account.username,
        org: account.org,
        ...(account.repoRoot === undefined ? {} : { repoRoot: account.repoRoot }),
        ...(account.usageProvider ? { usageProvider: account.usageProvider } : {}),
      },
      connectedOverrides
    )
  )
}

export function mergeUsageProviderOverrideSnapshot(
  snapshot: UsageProviderOverrides,
  current: UsageProviderOverrides,
  changedKeys: Set<string>
): UsageProviderOverrides {
  const merged = { ...snapshot }
  for (const key of changedKeys) {
    const provider = current[key]
    if (provider) merged[key] = provider
    else delete merged[key]
  }
  return merged
}

export function mergeConnectedOverrides(
  snapshot: UsageProviderOverrides | undefined,
  defaultSnapshot: UsageProviderOverrides | undefined,
  current: UsageProviderOverrides,
  accountKeys: Set<string>,
  changedKeys: Set<string>,
  changedDefaultKeys: Set<string>
): UsageProviderOverrides {
  for (const [key, provider] of Object.entries(defaultSnapshot ?? {})) {
    if (snapshot?.[key] === provider && !changedDefaultKeys.has(key)) changedKeys.delete(key)
  }
  const merged = snapshot
    ? mergeUsageProviderOverrideSnapshot(snapshot, current, changedKeys)
    : current
  return retainConnectedOverrides(merged, accountKeys, changedKeys)
}

export function retainDefaultOverrides(
  snapshot: UsageProviderOverrides | undefined,
  current: UsageProviderOverrides,
  accountKeys: Set<string>,
  changedKeys: Set<string>
): UsageProviderOverrides {
  for (const key of changedKeys) {
    if (!accountKeys.has(key)) changedKeys.delete(key)
  }
  const merged = snapshot
    ? mergeUsageProviderOverrideSnapshot(snapshot, current, changedKeys)
    : current
  return Object.fromEntries(Object.entries(merged).filter(([key]) => accountKeys.has(key)))
}
