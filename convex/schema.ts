import { defineSchema, defineTable } from 'convex/server'
import { v } from 'convex/values'

/**
 * Convex Schema for Buddy
 *
 * Tables:
 * - githubAccounts: GitHub account configurations
 * - settings: Application settings (non-UI, synced across sessions)
 * - jobs: Task definitions (name, worker type, config)
 * - schedules: Cron config, enabled state, linked job
 * - runs: Execution history (status, duration, output)
 */

export default defineSchema({
  /**
   * GitHub account configurations
   * Note: Tokens are NOT stored - uses GitHub CLI (gh auth) for auth
   */
  githubAccounts: defineTable({
    username: v.string(),
    org: v.string(),
    repoRoot: v.optional(v.string()),
    usageProvider: v.optional(v.union(v.literal('copilot'), v.literal('codex'))),
    createdAt: v.number(),
    updatedAt: v.number(),
  })
    .index('by_username', ['username'])
    .index('by_org', ['org']),

  /**
   * Application settings (non-UI settings that should persist)
   * Singleton table - only one document with key "default"
   */
  settings: defineTable({
    key: v.literal('default'),
    pr: v.object({
      refreshInterval: v.number(), // minutes
      autoRefresh: v.boolean(),
      recentlyMergedDays: v.number(),
    }),
    copilot: v.optional(
      v.object({
        ghAccount: v.string(), // GitHub CLI account username (empty = active)
        model: v.string(), // LLM model name (default chat)
        premiumModel: v.optional(v.string()), // Model for high-stakes tasks (address comments)
      })
    ),
    viewModes: v.optional(v.record(v.string(), v.union(v.literal('card'), v.literal('list')))),
    terminalPanelHeight: v.optional(v.number()),
    terminalTabs: v.optional(
      v.array(
        v.object({
          title: v.string(),
          cwd: v.string(),
          repoSlug: v.optional(v.string()),
          color: v.optional(v.string()),
        })
      )
    ),
    createdAt: v.number(),
    updatedAt: v.number(),
  }).index('by_key', ['key']),

  /**
   * Reusable terminal prompt snippets for Copilot CLI sessions.
   * Shared across machines via Convex instead of local terminal settings.
   */
  terminalPrompts: defineTable({
    title: v.string(),
    content: v.string(),
    sortOrder: v.optional(v.number()),
    lastUsedAt: v.optional(v.number()),
    createdAt: v.number(),
    updatedAt: v.number(),
    revision: v.optional(v.number()),
  })
    .index('by_created', ['createdAt'])
    .index('by_last_used', ['lastUsedAt']),

  // Durable request receipts outlive prompt deletion. A delayed retry must not
  // recreate a deleted prompt or apply an old edit to a newer revision.
  terminalPromptWrites: defineTable({
    requestId: v.string(),
    digest: v.string(),
    result: v.object({
      status: v.union(v.literal('saved'), v.literal('deleted'), v.literal('conflict')),
      id: v.id('terminalPrompts'),
      revision: v.number(),
    }),
    createdAt: v.number(),
  }).index('by_request', ['requestId']),

  /**
   * Job definitions - the tasks that can be scheduled or run manually
   */
  jobs: defineTable({
    name: v.string(),
    description: v.optional(v.string()),
    workerType: v.union(
      v.literal('exec'), // Shell commands (PowerShell, bash)
      v.literal('ai'), // LLM prompts
      v.literal('skill') // Claude skills
    ),
    // Worker-specific configuration
    config: v.object({
      // exec-worker
      command: v.optional(v.string()),
      cwd: v.optional(v.string()),
      timeout: v.optional(v.number()),
      shell: v.optional(v.union(v.literal('powershell'), v.literal('bash'), v.literal('cmd'))),
      // ai-worker
      prompt: v.optional(v.string()),
      model: v.optional(v.string()),
      maxTokens: v.optional(v.number()),
      temperature: v.optional(v.number()),
      // ai/skill target repo (from bookmarks)
      repoOwner: v.optional(v.string()),
      repoName: v.optional(v.string()),
      // skill-worker
      skillName: v.optional(v.string()),
      action: v.optional(v.string()),
      params: v.optional(v.any()),
    }),
    // Input parameters that can be provided at runtime
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
    createdAt: v.number(),
    updatedAt: v.number(),
  })
    .index('by_name', ['name'])
    .index('by_worker_type', ['workerType']),

  /**
   * Schedules - cron-based triggers for jobs
   */
  schedules: defineTable({
    jobId: v.id('jobs'),
    name: v.string(),
    description: v.optional(v.string()),
    cron: v.string(), // Standard 5-field cron: "0 9 * * 1-5"
    timezone: v.optional(v.string()), // e.g., "America/New_York"
    enabled: v.boolean(),
    // Runtime params to pass to job
    params: v.optional(v.any()),
    // How to handle missed executions
    missedPolicy: v.union(
      v.literal('catchup'), // Run all missed
      v.literal('skip'), // Skip missed, continue normally
      v.literal('last') // Run only the most recent missed
    ),
    lastRunAt: v.optional(v.number()),
    lastRunStatus: v.optional(v.union(v.literal('completed'), v.literal('failed'))),
    nextRunAt: v.optional(v.number()),
    createdAt: v.number(),
    updatedAt: v.number(),
  })
    .index('by_enabled', ['enabled'])
    .index('by_next_run', ['nextRunAt'])
    .index('by_job', ['jobId']),

  /**
   * Repo bookmarks - folder-organized GitHub repo bookmarks
   */
  repoBookmarks: defineTable({
    folder: v.string(), // Folder name (e.g., "Relias", "Home")
    owner: v.string(), // GitHub org or user (e.g., "relias-engineering")
    repo: v.string(), // Repo name (e.g., "ai-skills")
    url: v.string(), // Full URL to the repo
    description: v.optional(v.string()),
    createdAt: v.number(),
    updatedAt: v.number(),
  })
    .index('by_folder', ['folder'])
    .index('by_owner_repo', ['owner', 'repo']),

  /**
   * Buddy stats - centralized usage statistics (singleton, keyed "default")
   * Stored in Convex (not electron-store) for multi-client aggregation
   * (desktop, mobile, web all share the same stats)
   */
  buddyStats: defineTable({
    key: v.literal('default'),

    // Lifetime counters
    appLaunches: v.number(),
    tabsOpened: v.number(),
    prsViewed: v.number(),
    prsReviewed: v.number(),
    prsMergedWatched: v.number(),
    reposBrowsed: v.number(),
    repoDetailViews: v.number(),
    jobsCreated: v.number(),
    runsTriggered: v.number(),
    runsCompleted: v.number(),
    runsFailed: v.number(),
    schedulesCreated: v.number(),
    bookmarksCreated: v.number(),
    settingsChanged: v.number(),
    searchesPerformed: v.number(),
    copilotPrReviews: v.optional(v.number()),

    // Time tracking
    firstLaunchDate: v.number(), // Epoch ms — set once on first launch
    totalUptimeMs: v.number(), // Cumulative session time
    lastSessionStart: v.optional(v.number()), // Epoch ms — current session start

    // Timestamps
    createdAt: v.number(),
    updatedAt: v.number(),
  }).index('by_key', ['key']),

  /**
   * Copilot usage history - immutable timestamped snapshots of Copilot
   * billing/spend data per tracked account.  Never updated or deleted;
   * each row is a point-in-time record.
   */
  copilotUsageHistory: defineTable({
    accountUsername: v.string(),
    org: v.string(),
    billingYear: v.number(),
    billingMonth: v.number(),
    premiumRequests: v.number(),
    grossCost: v.number(),
    discount: v.number(),
    netCost: v.number(),
    businessSeats: v.number(),
    budgetAmount: v.optional(v.number()),
    spent: v.number(),
    snapshotAt: v.number(),
  })
    .index('by_account_period', ['accountUsername', 'org', 'billingYear', 'billingMonth'])
    .index('by_org', ['org'])
    .index('by_org_period', ['org', 'billingYear', 'billingMonth'])
    .index('by_snapshot', ['snapshotAt']),

  /**
   * Copilot SDK results - captured output from Copilot SDK prompts
   * Generic: supports PR reviews, code analysis, or any free-text prompt.
   */
  copilotResults: defineTable({
    nativeRequestId: v.optional(v.string()),
    nativeFinishDigest: v.optional(v.string()),
    prompt: v.string(),
    status: v.union(
      v.literal('pending'),
      v.literal('running'),
      v.literal('completed'),
      v.literal('failed')
    ),
    result: v.optional(v.string()), // Markdown output from Copilot
    error: v.optional(v.string()),
    model: v.optional(v.string()),
    category: v.optional(v.string()), // "pr-review", "general", etc.
    metadata: v.optional(v.any()), // { prUrl, owner, repo, prNumber } for PR reviews
    duration: v.optional(v.number()), // milliseconds
    createdAt: v.number(),
    completedAt: v.optional(v.number()),
  })
    .index('by_status', ['status'])
    .index('by_category', ['category'])
    .index('by_created', ['createdAt']),

  // Retain request identities after result deletion, without retaining prompts.
  copilotNativeRequests: defineTable({
    requestId: v.string(),
    payloadDigest: v.string(),
    resultId: v.id('copilotResults'),
  }).index('by_request', ['requestId']),

  /**
   * Exact active Copilot result counts used by activity badges.
   * Kept separate from copilotResults so count queries stay bounded.
   */
  copilotResultCounts: defineTable({
    key: v.literal('default'),
    pending: v.number(),
    running: v.number(),
  }).index('by_key', ['key']),

  /**
   * PR review runs - links Copilot review executions to a specific PR snapshot.
   */
  prReviewRuns: defineTable({
    owner: v.string(),
    repo: v.string(),
    prNumber: v.number(),
    prUrl: v.string(),
    prTitle: v.string(),
    status: v.union(
      v.literal('pending'),
      v.literal('running'),
      v.literal('completed'),
      v.literal('failed')
    ),
    resultId: v.id('copilotResults'),
    prompt: v.string(),
    model: v.optional(v.string()),
    ghAccount: v.optional(v.string()),
    reviewedHeadSha: v.optional(v.string()),
    reviewedThreadStats: v.optional(
      v.object({
        total: v.number(),
        unresolved: v.number(),
        outdated: v.number(),
      })
    ),
    error: v.optional(v.string()),
    createdAt: v.number(),
    updatedAt: v.number(),
    completedAt: v.optional(v.number()),
  })
    .index('by_pr', ['owner', 'repo', 'prNumber'])
    .index('by_result', ['resultId'])
    .index('by_status', ['status'])
    .index('by_created', ['createdAt']),

  /**
   * Feature intake normalization records
   * Maps external ticket IDs (Jira/GitHub/manual/etc.) to canonical GitHub issue drafts.
   */
  featureIntakes: defineTable({
    source: v.union(
      v.literal('jira'),
      v.literal('github-issue'),
      v.literal('manual'),
      v.literal('other')
    ),
    externalId: v.string(),
    externalUrl: v.optional(v.string()),
    requestedBy: v.optional(v.string()),
    title: v.string(),
    problem: v.string(),
    requestedOutcome: v.optional(v.string()),
    acceptanceCriteria: v.array(v.string()),
    riskLabel: v.union(
      v.literal('risk:trivial'),
      v.literal('risk:low'),
      v.literal('risk:medium'),
      v.literal('risk:high'),
      v.literal('risk:critical')
    ),
    canonicalKey: v.string(),
    canonicalIssueTitle: v.string(),
    canonicalIssueBody: v.string(),
    canonicalIssueLabels: v.array(v.string()),
    status: v.union(v.literal('draft'), v.literal('linked'), v.literal('duplicate')),
    duplicateOfId: v.optional(v.id('featureIntakes')),
    canonicalIssueNumber: v.optional(v.number()),
    canonicalIssueUrl: v.optional(v.string()),
    metadata: v.optional(v.any()),
    createdAt: v.number(),
    updatedAt: v.number(),
  })
    .index('by_source_external', ['source', 'externalId'])
    .index('by_canonical_key', ['canonicalKey'])
    .index('by_status', ['status'])
    .index('by_created', ['createdAt']),

  /**
   * Runs - execution history for jobs
   */
  runs: defineTable({
    jobId: v.id('jobs'),
    scheduleId: v.optional(v.id('schedules')),
    status: v.union(
      v.literal('pending'),
      v.literal('running'),
      v.literal('completed'),
      v.literal('failed'),
      v.literal('cancelled')
    ),
    triggeredBy: v.union(v.literal('manual'), v.literal('schedule'), v.literal('api')),
    // Input provided for this run
    input: v.optional(v.any()),
    // Output from the worker
    output: v.optional(v.any()),
    // For large outputs, store in Convex File Storage
    outputFileId: v.optional(v.id('_storage')),
    // Error message if failed
    error: v.optional(v.string()),
    // Timing
    startedAt: v.number(),
    completedAt: v.optional(v.number()),
    duration: v.optional(v.number()), // milliseconds
    // Set atomically with the run-count aggregate. Missing means the historical
    // backfill has not processed this run yet.
    runCountVersion: v.optional(v.literal(1)),
  })
    .index('by_job', ['jobId'])
    .index('by_job_count_version', ['jobId', 'runCountVersion'])
    .index('by_schedule', ['scheduleId'])
    .index('by_status', ['status'])
    .index('by_started', ['startedAt'])
    .index('by_run_count_version', ['runCountVersion']),

  /**
   * Session digests — efficiency metrics computed from Copilot session JSONL files.
   * Idempotent by sessionId — upserted on each digest computation run.
   */
  sessionDigests: defineTable({
    sessionId: v.string(),
    workspaceName: v.string(),
    model: v.optional(v.string()),
    agentMode: v.optional(v.string()),
    requestCount: v.number(),
    totalPromptTokens: v.number(),
    totalOutputTokens: v.number(),
    totalToolCalls: v.number(),
    totalDurationMs: v.number(),
    tokenEfficiency: v.number(),
    toolDensity: v.number(),
    searchChurn: v.number(),
    estimatedCost: v.number(),
    dominantTools: v.array(v.string()),
    firstPrompt: v.optional(v.string()),
    sessionDate: v.number(),
    digestedAt: v.number(),
  })
    .index('by_workspace', ['workspaceName', 'sessionDate'])
    .index('by_session', ['sessionId'])
    .index('by_date', ['sessionDate']),

  /**
   * Bookmarks — categorized URL/link collection with tagging and quick-launch
   */
  bookmarks: defineTable({
    url: v.string(),
    title: v.string(),
    description: v.optional(v.string()),
    faviconUrl: v.optional(v.string()),
    category: v.string(),
    tags: v.optional(v.array(v.string())),
    sortOrder: v.number(),
    lastVisitedAt: v.optional(v.number()),
    createdAt: v.number(),
    updatedAt: v.number(),
  })
    .index('by_category', ['category'])
    .index('by_url', ['url'])
    .index('by_url_category', ['url', 'category']),

  /**
   * Ralph Loops run history — tracks autonomous AI work loop executions.
   * Active runs are managed in-memory by ralphService; completed runs persist here.
   */
  ralphRuns: defineTable({
    runId: v.string(), // UUID generated by ralphService
    repoPath: v.string(),
    repoSlug: v.optional(v.string()), // owner/repo if detectable
    branch: v.optional(v.string()),
    scriptType: v.union(
      v.literal('ralph'),
      v.literal('ralph-pr'),
      v.literal('ralph-issues'),
      v.literal('template')
    ),
    templateScript: v.optional(v.string()),
    model: v.optional(v.string()),
    provider: v.optional(v.string()),
    agents: v.optional(v.array(v.string())),
    status: v.union(
      v.literal('pending'),
      v.literal('running'),
      v.literal('completed'),
      v.literal('failed'),
      v.literal('cancelled'),
      v.literal('orphaned')
    ),
    phase: v.optional(v.string()),
    iterations: v.optional(v.number()), // requested
    completedIterations: v.optional(v.number()),
    prUrl: v.optional(v.string()),
    exitCode: v.optional(v.number()),
    error: v.optional(v.string()),
    costMultiplier: v.optional(v.number()),
    startedAt: v.number(),
    completedAt: v.optional(v.number()),
    duration: v.optional(v.number()), // ms
    createdAt: v.number(),
    updatedAt: v.number(),
  })
    .index('by_run_id', ['runId'])
    .index('by_status', ['status'])
    .index('by_repo', ['repoPath', 'startedAt'])
    .index('by_started', ['startedAt']),

  /**
   * Terminal workspaces — tree-based terminal session organizer.
   * Singleton keyed "default". Stores the full tree structure with
   * recursive split layouts (via v.any() since Convex lacks recursive validators).
   */
  terminalWorkspaces: defineTable({
    key: v.literal('default'),
    revision: v.optional(v.number()),
    nativeWrite: v.optional(
      v.object({
        requestId: v.string(),
        digest: v.string(),
        baseRevision: v.number(),
        revision: v.number(),
      })
    ),
    /** Flat array of tree nodes with parentId references. Layout is recursive TerminalLayout. */
    nodes: v.array(
      v.object({
        id: v.string(),
        name: v.string(),
        color: v.optional(v.string()),
        parentId: v.optional(v.string()),
        sortOrder: v.number(),
        layout: v.any(), // Recursive TerminalLayout (pane | split)
      })
    ),
    activeNodeId: v.optional(v.string()),
    createdAt: v.number(),
    updatedAt: v.number(),
  }).index('by_key', ['key']),
})
