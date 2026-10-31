import type { UsageProvider } from '../types/config'

export type ConvexGitHubAccount = {
  _id?: string
  username: string
  org: string
  repoRoot?: string
  usageProvider?: UsageProvider
  createdAt?: number
  updatedAt?: number
}
