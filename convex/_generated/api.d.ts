/* eslint-disable */
/**
 * Generated `api` utility.
 *
 * THIS CODE IS AUTOMATICALLY GENERATED.
 *
 * To regenerate, run `npx convex dev`.
 * @module
 */

import type * as bookmarks from "../bookmarks.js";
import type * as buddyStats from "../buddyStats.js";
import type * as copilotNative from "../copilotNative.js";
import type * as copilotNativeWrites from "../copilotNativeWrites.js";
import type * as copilotResults from "../copilotResults.js";
import type * as copilotUsageHistory from "../copilotUsageHistory.js";
import type * as crons from "../crons.js";
import type * as featureIntakes from "../featureIntakes.js";
import type * as githubAccounts from "../githubAccounts.js";
import type * as jobs from "../jobs.js";
import type * as lib_constants from "../lib/constants.js";
import type * as lib_copilotResultCounts from "../lib/copilotResultCounts.js";
import type * as lib_cronUtils from "../lib/cronUtils.js";
import type * as lib_domain from "../lib/domain.js";
import type * as lib_offlineRecovery from "../lib/offlineRecovery.js";
import type * as lib_projections from "../lib/projections.js";
import type * as lib_runStore from "../lib/runStore.js";
import type * as lib_stats from "../lib/stats.js";
import type * as migrations from "../migrations.js";
import type * as prReviewRuns from "../prReviewRuns.js";
import type * as ralphRuns from "../ralphRuns.js";
import type * as repoBookmarks from "../repoBookmarks.js";
import type * as runs from "../runs.js";
import type * as scheduleScanner from "../scheduleScanner.js";
import type * as schedules from "../schedules.js";
import type * as sessionDigests from "../sessionDigests.js";
import type * as settings from "../settings.js";
import type * as terminalPrompts from "../terminalPrompts.js";
import type * as terminalWorkspaces from "../terminalWorkspaces.js";

import type {
  ApiFromModules,
  FilterApi,
  FunctionReference,
} from "convex/server";

declare const fullApi: ApiFromModules<{
  bookmarks: typeof bookmarks;
  buddyStats: typeof buddyStats;
  copilotNative: typeof copilotNative;
  copilotNativeWrites: typeof copilotNativeWrites;
  copilotResults: typeof copilotResults;
  copilotUsageHistory: typeof copilotUsageHistory;
  crons: typeof crons;
  featureIntakes: typeof featureIntakes;
  githubAccounts: typeof githubAccounts;
  jobs: typeof jobs;
  "lib/constants": typeof lib_constants;
  "lib/copilotResultCounts": typeof lib_copilotResultCounts;
  "lib/cronUtils": typeof lib_cronUtils;
  "lib/domain": typeof lib_domain;
  "lib/offlineRecovery": typeof lib_offlineRecovery;
  "lib/projections": typeof lib_projections;
  "lib/runStore": typeof lib_runStore;
  "lib/stats": typeof lib_stats;
  migrations: typeof migrations;
  prReviewRuns: typeof prReviewRuns;
  ralphRuns: typeof ralphRuns;
  repoBookmarks: typeof repoBookmarks;
  runs: typeof runs;
  scheduleScanner: typeof scheduleScanner;
  schedules: typeof schedules;
  sessionDigests: typeof sessionDigests;
  settings: typeof settings;
  terminalPrompts: typeof terminalPrompts;
  terminalWorkspaces: typeof terminalWorkspaces;
}>;

/**
 * A utility for referencing Convex functions in your app's public API.
 *
 * Usage:
 * ```js
 * const myFunctionReference = api.myModule.myFunction;
 * ```
 */
export declare const api: FilterApi<
  typeof fullApi,
  FunctionReference<any, "public">
>;

/**
 * A utility for referencing Convex functions in your app's internal API.
 *
 * Usage:
 * ```js
 * const myFunctionReference = internal.myModule.myFunction;
 * ```
 */
export declare const internal: FilterApi<
  typeof fullApi,
  FunctionReference<any, "internal">
>;

export declare const components: {
  runCounts: import("@convex-dev/aggregate/_generated/component.js").ComponentApi<"runCounts">;
  migrations: import("@convex-dev/migrations/_generated/component.js").ComponentApi<"migrations">;
};
