# Native Copilot workspace

The Rust app has Chat, Saved results, Usage and Sessions inside its Copilot section.
Electron continues to use the same configuration and Convex records.

## Runtime and authentication

The native app uses `github-copilot-sdk` **1.0.23-preview.2** and its bundled
Copilot runtime **1.0.99-2**. The SDK embeds the executable for each supported
target; users do not need a separately installed Copilot CLI. Buddy extracts and
verifies that runtime through its own `--prepare-copilot-runtime` helper. The
helper establishes process ownership before starting the SDK, then checks the
resolved program and hashes before returning it to the parent.

Install GitHub CLI and sign in with the account you intend to use. Select that
account in Chat and load available models, or enter a model ID. Buddy requests
that account's credential from `gh` and verifies the SDK's authenticated login
before creating a session or listing models. Model discovery does not send a
prompt. The native settings model picker uses this same packaged runtime.

Each turn has an isolated temporary state directory and an explicit credential.
The SDK cannot inherit ambient GitHub/provider credentials, preload settings,
debug logging or automatic login. Sessions advertise no tools and deny tool
permission requests. They cannot edit a checkout, run shell commands or post
GitHub comments. SDK, Convex and WebSocket transport diagnostics are excluded from the app
logger. Normal shutdown stops the owned process before removing its temporary
state. Abrupt process termination can leave temporary files; this is not secure
erasure. The verified executable cache remains available for later turns.

## Chat and saved results

**Send message** keeps the turn in the current conversation. **Run and save**
first creates a pending result and claims its send through Convex. A saved turn
cannot start inference before that acknowledgment. Request IDs and immutable
completion digests prevent storage reconciliation from sending the prompt again.

Cancel retains partial text and stops the owned runtime. A cancelled saved turn
uses the existing failed-result status with a cancellation reason. Storage-only
retry writes the captured outcome to the existing result; it never starts another
turn. If storage remains unconfirmed, the user can keep the conversation locally
and continue, then inspect saved results separately. Do not interpret a checked
request as proof of a completed answer.

Results load in bounded pages, with status and category filters and full records
read on selection. Prompts, answers and saved errors are available separately.
Deleting a result requires confirmation and removes its linked PR review record.
An uncertain deletion retains the displayed record until it is refreshed.

Conversation prompts are limited to 256 KiB. Output is bounded to 4 MiB, with a
768 KiB combined prompt/output limit for saved records. Conversation context is
sent again on subsequent explicit turns. Switching accounts requires starting a
new conversation. Markdown is selectable; automatic images and raw HTML are
suppressed, and only explicit HTTP/HTTPS links can open externally.

## Pull request reviews

Choose **Review with Buddy assistant…** from a native GitHub PR view. Preparation
captures that view's account, repository, PR, head, description, patches and
review threads through read-only GitHub calls. It verifies the head and counts
again after loading. Changed, incomplete, duplicate or oversized context is
rejected. Binary or unavailable patches remain explicit coverage limits.

The captured context stays outside the editable instructions field. Choose a
model, add optional instructions and select **Run and save**. The result and its
existing `prReviewRuns` record retain the captured head and thread statistics.
This action stores a local assistant review in Convex; it does not submit a
GitHub review. Saved details identify the captured head and account and do not
claim freshness against a PR that may have advanced.

## Usage and enterprise exports

Usage respects the configured per-account Copilot/Codex provider overrides.
Copilot allowance, billed spend and budget availability are distinct values.
Organization billing retains the existing active-login fallback when a selected
credential is unavailable; personal allowance always requires the selected
account's credential. Enterprise budget fallback uses the configured enterprise
slug and bounded pagination. Projections retain the existing UTC billing-period
rules. Unavailable spend or budgets are not displayed as zero.
Refreshing hides earlier live figures, and failed reads leave those figures
unavailable. Explicitly loaded billing history remains separate from live usage.

A successful billing read makes one immutable snapshot-save attempt. A lost save
reply is reported as unconfirmed and is not automatically retried. Billing history
reads the existing `copilotUsageHistory` account/organization/period index in pages.
The reader accepts finite Convex floats, including encoded negative zero from
empty billing sums. Nonfinite values and other encoded types remain invalid.

The local Codex ChatGPT login belongs to the first configured Codex usage account.
Buddy reads `CODEX_HOME/auth.json`, or the normal home `.codex/auth.json`, only
when usage is requested. It sends a bounded read-only request to the fixed
ChatGPT usage endpoint, rejects redirects, and displays static authentication or
service errors. Additional Codex usage accounts are told which assignment to fix.

Enterprise users read a local CodexBar JSON export. Set an absolute
`COPILOT_METRICS_FILE`, or choose the file in Usage. Windows retains the existing
CodexBar default location; macOS and Linux require an explicit export path.
Search and page through user totals, models and failures, or inspect/copy a user's
source JSON. The export's organization and period are shown independently from
the selected live usage account. Reads are bounded to 8 MiB and never modify the
export.

## VS Code sessions

Sessions scans an explicitly chosen VS Code or VS Code Insiders User directory,
including standard platform locations and custom/portable installations. Scanning
and detail reads run off the UI thread through a read-only directory capability.
Issued file keys remain bound to that source. Links cannot escape it, and file
identity is checked again when details open.
An entry whose identity cannot be read within its bounded preview is omitted
and counted as skipped. It cannot issue a detail key for a later replacement file.

Scans are bounded to 512 workspace entries, 2,000 file entries and 8 MiB of preview
reads; detail parsing is bounded to 16 MiB. Limit and malformed-record warnings
remain visible. Request-level token/duration/tool totals and efficiency metrics
describe readable session data. The legacy cost estimate is an approximation,
not a provider invoice or current token price. Sessions are never deleted or
rewritten by this view.

The request pager stays below the independently scrolling detail pane. Loaded
source and error messages reserve their own space and wrap at compact sizes,
so a failed scan preserves both the previous dataset and usable request paging.

## Qualification

`scripts/native-smoke/copilot-desktop.py` runs only on disposable GitHub Actions
hosts with a fresh owned profile, synthetic exports/sessions, the guarded loopback
GitHub fixture and an isolated anonymous Convex backend. The installed executable's
private `--qualify-copilot` command uses the actual turn runner and bundled SDK.
Its provider intercepts synthetic model traffic locally and has no upstream
forwarder. A nonce marker, confined paths, exact synthetic account/model and
loopback backend are required before credential or persistence work begins.

The workflow exercises streaming, cancellation, result persistence/read-back,
unauthenticated rejection and ownership with a retained session handle. Native
flows capture chat/results, PR review, provider/history/export and session states
in light/dark themes and compact windows. Process cleanup is checked separately
from an unrelated sentinel. These fixtures never use a real account credential
or a paid model. Passing compilation alone does not constitute installed UI or
SDK qualification; current-head workflow receipts and captures supply that proof.
The session fixture contains 21 requests and exercises content scrolling plus
enabled Next/Previous request controls at 900 x 650 while a missing-directory
error and the original loaded source remain visible. A headless layout regression
also checks wrapped Unicode source text, scroll isolation, pointer paging and
Tab/Enter activation against the production view.
Hosted Codex cases also use guarded loopback snapshots and a deterministic
missing-login response, without reading a local Codex credential.
The chat fixture scrolls its owned conversation to expose completion and
cancellation feedback, then requires the exact visible status before advancing.
The offline provider holds after its first delta until the driver writes the
fresh streaming capture. That handshake is confined to the owned CI directory,
expires after 60 seconds and stops on cancellation. Existing captures cannot
release a new stream; ordinary SDK traffic and deadlines are unchanged.
It does not infer a successful save from a finished stream or a scroll action.
After resizing, the chat fixture finds the visible Assistant label and scrolls
that owned conversation in small steps before requiring cancellation and partial
output in the same capture. Its former fixed wheel position falls over the
message editor at compact sizes.
Failed runtime preparation reports ownership or reap stages, error kinds and OS
codes only in the guarded offline qualifier. Private paths, arguments and error
messages stay out of those diagnostics. Cleanup failures remain failures.
