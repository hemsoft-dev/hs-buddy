# ralph-pr.ps1 — PR comment resolver with fix loop and agent reviews.
# Version: 1.6.3
param(
    [int]$PRNumber,
    [int]$MaxIdleCycles = 3,
    [int]$WaitMinutes = 3,
    [string]$WorkUntil,
    [string]$OriginalPrompt,
    [string]$Model,
    [string]$Provider,
    [string]$DevAgent,
    [string[]]$Agents,
    [string]$StatsPath,
    [switch]$Autopilot,
    [switch]$NoAudio,
    [switch]$SkipReview,
    [switch]$AutoApprove,
    # Exit 3 unless the PR was merged (for callers that build on the merged work).
    [switch]$RequireMerge,
    [switch]$Help
)

# Normalize -Agents: split comma-delimited strings (e.g. -Agents "a,b,c")
if ($Agents) { $Agents = @($Agents | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ }) }

# --- Config: load shared model/agent configuration ---
. (Join-Path $PSScriptRoot 'lib' 'config.ps1')

# Resolve provider first (model default may depend on provider)
if (-not $Provider) { $Provider = Get-RalphDefaultProvider }
try {
    $script:ProviderConfig = Resolve-RalphProvider -Name $Provider
}
catch {
    Write-Host "Invalid provider '$Provider'. Valid: $((Get-RalphProviderNames) -join ', ')" -ForegroundColor Red
    exit 1
}
$script:ProviderName = $Provider

# Set BYOK environment variables if provider requires them
$script:SavedByokEnv = Set-RalphProviderEnv -ProviderName $Provider

# Resolve model — use explicit -Model if provided, else provider default, else global default
$effectiveModel = if ($PSBoundParameters.ContainsKey('Model') -and $Model -and $Model.Trim()) {
    $Model.Trim()
} else { $null }
try {
    $resolved = Resolve-RalphEffectiveModel -RequestedModel $effectiveModel -Provider $Provider
}
catch {
    Write-Host "Invalid model '$Model'. Valid names: $((Get-RalphModelNames) -join ', ')" -ForegroundColor Red
    exit 1
}
$script:ModelId         = $resolved.Id
$script:ModelMultiplier = $resolved.Multiplier
$script:ModelLabel      = $resolved.Label
$script:ModelEffort     = $resolved.Effort

# Validate and deduplicate agent roles — only review agents accepted
$parsedAgents = @()
if ($Agents) {
    $Agents = @($Agents | Select-Object -Unique)
    foreach ($spec in $Agents) {
        try {
            $pa = Parse-RalphAgentSpec -Spec $spec -Provider $Provider
        } catch {
            Write-Host $_.Exception.Message -ForegroundColor Red
            exit 1
        }
        if ($pa.Category -eq 'dev') {
            Write-Host "Dev agents not allowed in ralph-pr -Agents: $spec" -ForegroundColor Red
            Write-Host "Use -DevAgent for the fix loop agent. Review agents: $((Get-RalphReviewAgentRoles) -join ', ')" -ForegroundColor Yellow
            exit 1
        }
        $parsedAgents += $pa
    }

    # Deduplicate on resolved (Role, EffectiveProvider, ModelId)
    $deduped = @()
    $seen = @{}
    foreach ($pa in $parsedAgents) {
        $effProv = if ($pa.ProviderOverride) { $pa.ProviderOverride } else { $Provider }
        $key = "$($pa.Role)|$effProv|$($pa.ModelId)"
        if (-not $seen[$key]) {
            $seen[$key] = $true
            $deduped += $pa
        }
    }
    $parsedAgents = $deduped
}

# Resolve effective dev agent for the fix loop
if (-not $DevAgent) {
    $DevAgent = Get-RalphDefaultDevAgent
}
$devAgentResolved = Resolve-RalphAgent -Role $DevAgent -Provider $Provider
$script:DevAgentName = $devAgentResolved.Agent

[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding = [System.Text.Encoding]::UTF8
# Detect console availability once — false when launched from hs-buddy with piped stdio
$script:HasConsole = try { [Console]::KeyAvailable; $true } catch { $false }

if ($Help) {
    Write-Host ""
    Write-Host "ralph-pr.ps1 �� PR Comment Resolver" -ForegroundColor Cyan
    Write-Host "Monitors a PR for CI failures and unresolved comments, fixes them with Copilot CLI."
    Write-Host "Uses git worktrees for branch isolation."
    Write-Host ""
    Write-Host "PARAMETERS" -ForegroundColor Yellow
    Write-Host "  -PRNumber <int>        PR number (interactive picker if omitted)"
    Write-Host "  -MaxIdleCycles <int>   Consecutive idle cycles before exit (default: 3)"
    Write-Host "  -WaitMinutes <int>     Minutes between check cycles (default: 3)"
    Write-Host "  -WorkUntil <HH:mm>     Stop after this local time (e.g. -WorkUntil 08:00)"
    Write-Host "  -OriginalPrompt <str>  Original prompt from ralph.ps1 handoff (display only)"
    Write-Host "  -Model <name>          Model to use (default: per-provider, see providers.json)"
    Write-Host "  -Provider <name>       CLI provider: $((Get-RalphProviderNames) -join ', ') (default: $(Get-RalphDefaultProvider))"
    Write-Host "  -DevAgent <role>       Dev agent for fix loop (default: $(Get-RalphDefaultDevAgent))"
    Write-Host "  -Agents <specs>        Review agents (fix-loop mode): role or role@model"
    Write-Host "                         When specified, each agent reviews → dev agent fixes → CI check"
    Write-Host "                         Built-in Copilot review runs last (copilot provider only)"
    Write-Host "  -Autopilot             Skip all interactive prompts (auto-merge when clean)"
    Write-Host "  -SkipReview            Skip Copilot PR review requests (merge when CI passes)"
    Write-Host "                         Note: -SkipReview does NOT suppress explicit -Agents reviews."
    Write-Host "  -StatsPath <path>      Write PR phase stats JSON to this path (for parent rollup)"
    Write-Host "  -NoAudio               Suppress audio feedback"
    Write-Host "  -RequireMerge          Exit 3 unless the PR was merged (with -Autopilot -AutoApprove)"
    Write-Host "  -Help                  Show this help message"
    Write-Host ""
    Write-Host "MODELS" -ForegroundColor Yellow
    foreach ($name in (Get-RalphModelNames)) {
        try {
            $r = Resolve-RalphModel -Name $name
            Write-Host "  $($name.PadRight(24)) $($r.Label)" -ForegroundColor Gray
        } catch {}
    }
    Write-Host ""
    Write-Host "DEV AGENTS " -ForegroundColor Yellow -NoNewline
    Write-Host "(fix loop — default: $(Get-RalphDefaultDevAgent))" -ForegroundColor DarkGray
    foreach ($role in (Get-RalphDevAgentRoles)) {
        $a = Resolve-RalphAgent -Role $role
        Write-Host "  $($role.PadRight(26)) $($a.Description)" -ForegroundColor Gray
    }
    Write-Host ""
    Write-Host "REVIEW AGENTS " -ForegroundColor Yellow -NoNewline
    Write-Host "(fix-loop PR reviews — use role or role@model)" -ForegroundColor DarkGray
    foreach ($role in (Get-RalphReviewAgentRoles)) {
        $a = Resolve-RalphAgent -Role $role
        Write-Host "  $($role.PadRight(26)) $($a.Description)" -ForegroundColor Gray
    }
    Write-Host ""
    Write-Host "EXAMPLES" -ForegroundColor Yellow
    Write-Host "  ralph-pr"
    Write-Host "  ralph-pr -PRNumber 42"
    Write-Host "  ralph-pr -PRNumber 42 -WaitMinutes 5 -WorkUntil 08:00"
    Write-Host "  ralph-pr -PRNumber 42 -Autopilot"
    Write-Host "  ralph-pr -PRNumber 42 -DevAgent developer-principal"
    Write-Host "  ralph-pr -PRNumber 42 -Agents pr-review-quality,pr-review-security"
    Write-Host "  ralph-pr -PRNumber 42 -Agents pr-review-quality@opus47,pr-review-quality@sonnet"
    Write-Host ""
    Write-Host "AGENT REVIEW FLOW" -ForegroundColor Yellow
    Write-Host "  1. Fix CI failures + unresolved comments (initial phase)"
    Write-Host "  2. For each -Agents reviewer: review → dev agent fixes → re-check CI"
    Write-Host "  3. Built-in Copilot reviewer runs last for clean badge"
    Write-Host ""
    exit 0
}

# --- Git repo validation ---
$repoRoot = ([string](git rev-parse --show-toplevel 2>&1)).Trim() -replace '/', [IO.Path]::DirectorySeparatorChar
if ($LASTEXITCODE -ne 0) {
    Write-Host "Not inside a git repository. Run this from within a repo." -ForegroundColor Red
    exit 1
}
$repoName = Split-Path $repoRoot -Leaf
$repoParent = Split-Path $repoRoot -Parent

# --- Repo audio identification ---
$script:RepoAudioFile = Join-Path $repoRoot "assets" "ralph-loop" "$repoName.mp3"
if (-not $NoAudio -and -not (Test-Path $script:RepoAudioFile)) {
    Write-Host ""
    Write-Host "Repo audio identifier not found: $($script:RepoAudioFile)" -ForegroundColor Yellow
    $tts = Get-Command edge-tts -ErrorAction SilentlyContinue
    if ($tts) {
        Write-Host "  Generating with edge-tts..." -ForegroundColor Cyan
        $audioDir = Split-Path $script:RepoAudioFile -Parent
        if (-not (Test-Path $audioDir)) { New-Item -ItemType Directory -Path $audioDir -Force | Out-Null }
        $spokenName = $repoName -replace '[-_]', ' '
        & edge-tts --text $spokenName --write-media $script:RepoAudioFile 2>$null
        if (Test-Path $script:RepoAudioFile) {
            Write-Host "  Created: $($script:RepoAudioFile)" -ForegroundColor Green
        }
        else {
            Write-Host "  edge-tts failed. Pass -NoAudio to skip audio, or create the file manually." -ForegroundColor Red
            exit 1
        }
    }
    else {
        Write-Host "  edge-tts not installed. Install with: pip install edge-tts" -ForegroundColor Red
        Write-Host "  Or pass -NoAudio to skip audio." -ForegroundColor DarkGray
        exit 1
    }
}

# --- Save original directory ---
$originalDir = (Get-Location).Path
$logFile = Join-Path $originalDir "ralph-pr.log"

$runStart = Get-Date
$runStartStr = $runStart.ToString("yyyy-MM-dd HH:mm:ss")

# --- Resolve repo slug ---
$repoSlug = gh repo view --json nameWithOwner -q '.nameWithOwner' 2>$null
if (-not $repoSlug) {
    $remoteUrl = git --no-pager remote get-url origin 2>$null
    if ($remoteUrl -match '[:/]([^/]+/[^/]+?)(?:\.git)?$') {
        $repoSlug = $Matches[1]
    }
    else {
        Write-Host "Failed to resolve GitHub repo from this directory." -ForegroundColor Red
        if ($remoteUrl) {
            Write-Host "  Remote: $remoteUrl" -ForegroundColor DarkGray
        }
        exit 1
    }
}
$owner, $repo = $repoSlug -split '/'

# --- Match gh account to repo owner ---
# When the active gh account differs from the repo owner (e.g. Relias account
# active but running against a personal repo), API calls return 404. Detect
# this and set GH_TOKEN to the matching account's token for the script duration.
# If the owner is an org and the active user is a member, the current token works.
# Buddy Native runs a loop as the account its GitHub view showed: RALPH_GH_ACCOUNT names
# it and GH_TOKEN holds its token. That account is kept, never traded for the owner's.
$activeGhUser = gh api user --jq '.login' 2>$null
if ($env:RALPH_GH_ACCOUNT) {
    Write-Host "  gh account: $env:RALPH_GH_ACCOUNT (chosen in Buddy)" -ForegroundColor DarkGray
}
elseif ($activeGhUser -and $activeGhUser -ne $owner) {
    $ownerToken = gh auth token --user $owner 2>$null
    if ($ownerToken) {
        $env:GH_TOKEN = $ownerToken
        Write-Host "  gh account mismatch: active=$activeGhUser, repo owner=$owner" -ForegroundColor DarkGray
        Write-Host "  Using gh token for $owner (via GH_TOKEN)" -ForegroundColor DarkGray
    }
    else {
        # Check if owner is an org the active user belongs to
        $memberCheck = gh api "orgs/$owner/members/$activeGhUser" --silent 2>$null
        if ($LASTEXITCODE -eq 0) {
            Write-Host "  gh account $activeGhUser is a member of org $owner - token is valid" -ForegroundColor DarkGray
        }
        else {
            Write-Host "  Warning: active gh account ($activeGhUser) differs from repo owner ($owner)" -ForegroundColor Yellow
            Write-Host "  No token found for $owner. API calls may fail. Try: gh auth login --user $owner" -ForegroundColor Yellow
        }
    }
}

# --- PR selection ---
if (-not $PRNumber) {
    if ($Autopilot) {
        Write-Host "Autopilot mode requires -PRNumber. Cannot interactively select a PR." -ForegroundColor Red
        exit 1
    }
    $prJson = gh pr list --repo $repoSlug --limit 10 --state open --json number,title,author,headRefName 2>&1
    $prExitCode = $LASTEXITCODE
    if ($prExitCode -ne 0) {
        Write-Host "Failed to list PRs for $repoSlug (exit code $prExitCode)." -ForegroundColor Red
        Write-Host "  $($prJson | Out-String)".Trim() -ForegroundColor DarkGray
        Write-Host "  Your active gh account may not have access. Try: gh auth switch" -ForegroundColor DarkGray
        exit 1
    }
    $prs = if ($prJson) { $prJson | ConvertFrom-Json } else { @() }

    if ($prs.Count -eq 0) {
        Write-Host "No open PRs found in $repoSlug." -ForegroundColor Yellow
        exit 0
    }

    Write-Host ""
    Write-Host "Open PRs in $repoSlug" -ForegroundColor Cyan
    Write-Host ([string]::new([char]0x2500, 57))
    for ($i = 0; $i -lt $prs.Count; $i++) {
        $pr = $prs[$i]
        $num = "$($pr.number)".PadLeft(5)
        $branch = $pr.headRefName
        $author = $pr.author.login
        Write-Host "  [$($i + 1)] #$num  $($pr.title)" -ForegroundColor White
        Write-Host "             $author -> $branch" -ForegroundColor DarkGray
    }
    Write-Host ([string]::new([char]0x2500, 57))
    Write-Host ""

    do {
        $selection = Read-Host "Select a PR [1-$($prs.Count)]"
        $idx = 0
        $valid = [int]::TryParse($selection, [ref]$idx) -and $idx -ge 1 -and $idx -le $prs.Count
        if (-not $valid) {
            Write-Host "Invalid selection. Enter a number between 1 and $($prs.Count)." -ForegroundColor Red
        }
    } while (-not $valid)

    $PRNumber = $prs[$idx - 1].number
    Write-Host ""
    Write-Host "Selected PR #$PRNumber" -ForegroundColor Green
    Write-Host ""
}

# --- Get PR branch and set up worktree ---
$prInfo = (gh pr view $PRNumber --repo $repoSlug --json headRefName,state,isCrossRepository 2>&1)
if ($LASTEXITCODE -ne 0 -or -not $prInfo) {
    Write-Host "Failed to get PR info for PR #$PRNumber." -ForegroundColor Red
    exit 1
}
$prData = $prInfo | ConvertFrom-Json
$prBranch = $prData.headRefName
$prState = $prData.state

if (-not $prBranch) {
    Write-Host "Failed to get branch name for PR #$PRNumber." -ForegroundColor Red
    exit 1
}

if ($prState -ne 'OPEN') {
    Write-Host "PR #$PRNumber is $($prState.ToLower()) — cannot run ralph-pr on a non-open PR." -ForegroundColor Red
    exit 1
}

# Fixes are pushed to the PR's branch on origin, which a fork's branch is not.
if ($prData.isCrossRepository) {
    Write-Host "PR #$PRNumber comes from a fork. ralph-pr can only work on branches in $repoSlug." -ForegroundColor Red
    exit 1
}

# Fetch the branch to ensure we have the latest
$fetchOutput = git fetch origin $prBranch 2>&1
if ($LASTEXITCODE -ne 0) {
    Write-Host "Failed to fetch branch '$prBranch' from origin." -ForegroundColor Red
    Write-Host "  $fetchOutput" -ForegroundColor DarkGray
    Write-Host "  Ensure you have access to the remote and the branch exists." -ForegroundColor Yellow
    exit 1
}

$worktreeBase = Join-Path $repoParent "$repoName.worktrees"
$safeBranch = $prBranch -replace '[/\\]', '-'
$worktreeDir = Join-Path $worktreeBase $safeBranch

# Check existing worktrees for conflicts
$worktreeExists = $false
$branchAttachedElsewhere = $false
$attachedPath = ""
$currentWtPath = ""

$wtLines = @(git worktree list --porcelain 2>$null)
foreach ($line in $wtLines) {
    if ($line -match "^worktree (.+)$") {
        $currentWtPath = $Matches[1].Trim()
    }
    if ($line -match "^branch refs/heads/(.+)$") {
        $currentWtBranch = $Matches[1].Trim()
        if ($currentWtBranch -eq $prBranch) {
            $normalizedWtPath = ($currentWtPath -replace '/', [IO.Path]::DirectorySeparatorChar).TrimEnd([IO.Path]::DirectorySeparatorChar)
            $normalizedTarget = ($worktreeDir -replace '/', [IO.Path]::DirectorySeparatorChar).TrimEnd([IO.Path]::DirectorySeparatorChar)
            if ($normalizedWtPath -eq $normalizedTarget) {
                $worktreeExists = $true
            }
            else {
                $branchAttachedElsewhere = $true
                $attachedPath = $currentWtPath
            }
        }
    }
}

if ($branchAttachedElsewhere) {
    $normalizedAttached = ($attachedPath -replace '/', [IO.Path]::DirectorySeparatorChar).TrimEnd([IO.Path]::DirectorySeparatorChar)
    $normalizedRepoRoot = ($repoRoot -replace '/', [IO.Path]::DirectorySeparatorChar).TrimEnd([IO.Path]::DirectorySeparatorChar)
    if ($normalizedAttached -eq $normalizedRepoRoot) {
        if ($Autopilot) {
            # In autopilot mode (launched from UI), always use a separate worktree.
            # Working in the same directory as a running Vite dev server causes the
            # app to restart when the agent modifies electron/ files.
            Write-Host "Branch '$prBranch' checked out in current repo — switching to 'main' and creating worktree (autopilot)." -ForegroundColor Yellow
            git checkout main 2>&1 | Out-Null
            if ($LASTEXITCODE -ne 0) {
                # Never let an unattended agent work, commit or push in the user's own checkout.
                Write-Host "Could not switch the repository off '$prBranch' (no 'main' branch, or local changes)." -ForegroundColor Red
                Write-Host "  Check out another branch there, then run again; ralph-pr works in its own worktree." -ForegroundColor Yellow
                exit 1
            }
            # else: fall through to worktree creation below
        }
        else {
            Write-Host ""
            Write-Host "Branch '$prBranch' is already checked out in the current repo at: $repoRoot" -ForegroundColor Yellow
            Write-Host "  [1] Continue here (use current directory instead of a worktree)" -ForegroundColor White
            Write-Host "  [2] Create a separate worktree (switches this repo to 'main' first)" -ForegroundColor White
            Write-Host ""
            $choice = Read-Host "Choose [1/2]"
            if ($choice -eq '2') {
                Write-Host "Switching repo to 'main' and creating worktree..." -ForegroundColor Cyan
                git checkout main 2>&1 | Out-Null
                if ($LASTEXITCODE -ne 0) {
                    Write-Host "Failed to switch to 'main'. Resolve manually." -ForegroundColor Red
                    exit 1
                }
            }
            else {
                Write-Host "Continuing in current directory: $repoRoot" -ForegroundColor Green
                $worktreeDir = $repoRoot
                $worktreeExists = $true
            }
        }
    }
    else {
        Write-Host "Branch '$prBranch' is already checked out in a worktree at: $attachedPath" -ForegroundColor Red
        Write-Host "   Remove it first: git worktree remove '$attachedPath'" -ForegroundColor DarkGray
        exit 1
    }
}

if ($worktreeExists) {
    Write-Host "Reusing existing worktree at: $worktreeDir" -ForegroundColor Cyan
    git -C $worktreeDir pull --ff-only 2>&1 | Out-Null
}
else {
    if (-not (Test-Path $worktreeBase)) {
        New-Item -ItemType Directory -Path $worktreeBase -Force | Out-Null
    }
    $branchExistsLocally = git rev-parse --verify "refs/heads/$prBranch" 2>$null
    if ($LASTEXITCODE -eq 0) {
        git worktree add $worktreeDir $prBranch 2>&1
        # A local branch can be behind the PR; work from the PR's current head.
        if ($LASTEXITCODE -eq 0) {
            git -C $worktreeDir merge --ff-only "origin/$prBranch" 2>&1 | Out-Null
            if ($LASTEXITCODE -ne 0) {
                Write-Host "Local branch '$prBranch' has diverged from origin/$prBranch. Reconcile it, then run again." -ForegroundColor Red
                git worktree remove $worktreeDir --force 2>$null
                exit 1
            }
        }
    }
    else {
        git worktree add -b $prBranch $worktreeDir "origin/$prBranch" 2>&1
    }
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Failed to create worktree for branch '$prBranch' at: $worktreeDir" -ForegroundColor Red
        exit 1
    }
    Write-Host "Created worktree at: $worktreeDir" -ForegroundColor Green
}

# --- Parse deadline ---
$deadline = $null
if ($WorkUntil) {
    try {
        $parsed = [DateTime]::ParseExact($WorkUntil, 'HH:mm', $null)
        $deadline = Get-Date -Hour $parsed.Hour -Minute $parsed.Minute -Second 0
        if ($deadline -le (Get-Date)) { $deadline = $deadline.AddDays(1) }
    }
    catch {
        Write-Host "Invalid -WorkUntil format. Use HH:mm (e.g. 08:00)." -ForegroundColor Red
        exit 1
    }
}

$copilotLogin = "copilot-pull-request-reviewer"
$assetsDir = Join-Path $PSScriptRoot "assets"

function Invoke-Audio {
    param([string]$FileName)
    if ($script:NoAudio) { return }
    $filePath = Join-Path $assetsDir $FileName
    if (-not (Test-Path $filePath)) { return }
    $ffplay = Get-Command ffplay -ErrorAction SilentlyContinue
    if (-not $ffplay) { return }
    try {
        # Play repo identifier first (synchronous) so the user knows the source
        if ($script:RepoAudioFile -and (Test-Path $script:RepoAudioFile)) {
            Start-Process -FilePath $ffplay.Source -ArgumentList "-nodisp", "-autoexit", "-loglevel", "quiet", $script:RepoAudioFile -WindowStyle Hidden -Wait
        }
        # Then play the actual message (also synchronous — all audio must finish before script continues)
        Start-Process -FilePath $ffplay.Source -ArgumentList "-nodisp", "-autoexit", "-loglevel", "quiet", $filePath -WindowStyle Hidden -Wait
    }
    catch {
        Write-Warning "Failed to play audio '$FileName': $($_.Exception.Message)"
    }
}

$deadlineStr = if ($deadline) { ", deadline: $($deadline.ToString('yyyy-MM-dd HH:mm'))" } else { "" }
$deadlineDisplay = if ($deadline) { $deadline.ToString('yyyy-MM-dd HH:mm') } else { "(none)" }
$logTag = "pr-$PRNumber"

$promptDisplay = if ($OriginalPrompt) { $OriginalPrompt } else { "(not provided)" }

Write-Host ""
Write-Host "====================================" -ForegroundColor Cyan
Write-Host "== Ralph PR - Comment Resolver" -ForegroundColor Cyan
Write-Host "====================================" -ForegroundColor Cyan
Write-Host "  Started:         $runStartStr"
Write-Host "  PID:             $PID"
Write-Host "  Host:            $([Environment]::MachineName)"
Write-Host "  Invoked from:    $originalDir"
Write-Host "  PR:              #$PRNumber ($repoSlug) - https://github.com/$repoSlug/pull/$PRNumber"
Write-Host "  Branch:          $prBranch"
Write-Host "  Worktree:        $worktreeDir"
Write-Host "  Original prompt: $promptDisplay"
Write-Host "  Max idle cycles: $MaxIdleCycles"
Write-Host "  Wait interval:   $WaitMinutes min"
Write-Host "  WorkUntil:       $deadlineDisplay"
Write-Host "  Provider:        $script:ProviderName ($($script:ProviderConfig.Command))"
Write-Host "  Model:           $script:ModelId ($script:ModelLabel)"
$devAgentDisplay = "$DevAgent ($script:DevAgentName)"
$reviewDisplay = if ($parsedAgents.Count -gt 0) {
    ($parsedAgents | ForEach-Object { "$($_.Role)$(if ($_.ModelOverride) { "@$($_.ModelOverride)" } else { '' })" }) -join ', '
} else { '(none)' }
$reviewMode = if ($parsedAgents.Count -gt 0) { 'fix-loop' } else { 'advisory-only' }
Write-Host "  Dev agent:       $devAgentDisplay"
Write-Host "  Review agents:   $reviewDisplay ($reviewMode)"
Write-Host "  Flags:           Autopilot=$Autopilot, NoAudio=$NoAudio, SkipReview=$SkipReview"
Write-Host "====================================" -ForegroundColor Cyan

$startLogBlock = @"
[$logTag] Ralph PR started at $runStartStr for PR #$PRNumber ($repoSlug, branch: $prBranch, worktree: $worktreeDir$deadlineStr)
  PID:              $PID
  Host:             $([Environment]::MachineName)
  Script:           $PSCommandPath
  Invoked from:     $originalDir
  PR:               #$PRNumber (https://github.com/$repoSlug/pull/$PRNumber)
  Repo:             $repoSlug
  Branch:           $prBranch
  Worktree:         $worktreeDir
  Original prompt:  $promptDisplay
  Max idle cycles:  $MaxIdleCycles
  Wait interval:    ${WaitMinutes} min
  WorkUntil:        $deadlineDisplay
  Provider:         $script:ProviderName ($($script:ProviderConfig.Command))
  Model:            $script:ModelId ($script:ModelLabel)
  Dev agent:        $devAgentDisplay
  Review agents:    $reviewDisplay
  Autopilot:        $Autopilot
  SkipReview:       $SkipReview
  NoAudio:          $NoAudio
"@
$startLogBlock | Out-File $logFile -Append -Encoding UTF8

# --- Summary log (centralized cross-repo log) ---
$script:SummaryLogFile = Join-Path $PSScriptRoot "ralph-summary.log"
"$(Get-Date -Format 'yyyy-MM-dd HH:mm') -- ${repoName}: Ralph PR #$PRNumber processing started. $promptDisplay." | Out-File $script:SummaryLogFile -Append -Encoding UTF8

# A dev agent CLI that cannot run (sign-in, quota, startup) fails the same way every
# time, and the loop would invoke it forever. Three failures in a row stop the run.
$script:AgentFailureStreak = 0
function Test-AgentExit {
    param([int]$Code)
    if ($Code -eq 0) {
        $script:AgentFailureStreak = 0
        return
    }
    $script:AgentFailureStreak++
    Write-Host "The agent CLI exited with code $Code." -ForegroundColor Red
    "[$logTag] Agent CLI FAILED (exit code $Code), $($script:AgentFailureStreak) in a row" | Out-File $logFile -Append -Encoding UTF8
    if ($script:AgentFailureStreak -ge 3) {
        Write-Host "The agent CLI failed $($script:AgentFailureStreak) times in a row; stopping." -ForegroundColor Red
        "[$logTag] Stopped: the agent CLI failed $($script:AgentFailureStreak) times in a row." | Out-File $logFile -Append -Encoding UTF8
        exit 4
    }
}

function Write-Status {
    param([string]$Message)
    $timestamp = (Get-Date).ToString("yyyy-MM-dd HH:mm:ss")
    $line = "[$logTag] [$timestamp] $Message"
    Write-Host $line
    $line | Out-File $logFile -Append -Encoding UTF8
}

function Get-UnresolvedThreadCount {
    $totalUnresolved = 0
    $cursor = $null

    do {
        $afterClause = if ($null -ne $cursor) { ", after: `"$cursor`"" } else { "" }
        $query = @"
query {
  repository(owner: "$owner", name: "$repo") {
    pullRequest(number: $PRNumber) {
      reviewThreads(first: 100$afterClause) {
        nodes {
          isResolved
        }
        pageInfo {
          hasNextPage
          endCursor
        }
      }
    }
  }
}
"@
        $result = gh api graphql -f query="$query" 2>&1 | ConvertFrom-Json
        if ($result.errors) {
            Write-Warning "GraphQL query failed: $($result.errors | ConvertTo-Json -Compress)"
            return 0
        }

        if (-not $result.data -or -not $result.data.repository -or -not $result.data.repository.pullRequest) {
            Write-Warning "Unexpected GraphQL response structure"
            return 0
        }

        $reviewThreads = $result.data.repository.pullRequest.reviewThreads
        $threads = @($reviewThreads.nodes)
        $unresolved = @($threads | Where-Object { -not $_.isResolved })
        $totalUnresolved += $unresolved.Count
        $cursor = $reviewThreads.pageInfo.endCursor
    } while ($reviewThreads.pageInfo.hasNextPage)

    return $totalUnresolved
}

function Get-LastCommitDate {
    $commits = gh api "repos/$owner/$repo/pulls/$PRNumber/commits" --paginate 2>&1 | ConvertFrom-Json
    return [DateTimeOffset]::Parse($commits[-1].commit.committer.date).UtcDateTime
}

function Get-LastCopilotReviewDate {
    $reviews = gh api "repos/$owner/$repo/pulls/$PRNumber/reviews" --paginate 2>&1 | ConvertFrom-Json
    $copilotReviews = @($reviews | Where-Object { $_.user.login -like "${copilotLogin}*" })
    if ($copilotReviews.Count -eq 0) { return $null }
    $latest = $copilotReviews | Sort-Object { [DateTimeOffset]::Parse($_.submitted_at) } | Select-Object -Last 1
    return [DateTimeOffset]::Parse($latest.submitted_at).UtcDateTime
}

function Get-CIFailureList {
    $sha = gh pr view $PRNumber --repo $repoSlug --json headRefOid -q '.headRefOid' 2>&1
    $failures = @()

    $checkRuns = (gh api "repos/$owner/$repo/commits/$sha/check-runs" 2>&1 | ConvertFrom-Json).check_runs
    $checkRuns | Where-Object { $_.conclusion -eq 'failure' } | ForEach-Object {
        $failures += "$($_.name): $($_.output.title)"
    }

    $statusResult = gh api "repos/$owner/$repo/commits/$sha/status" 2>&1 | ConvertFrom-Json
    $statusResult.statuses | Where-Object { $_.state -in @('error', 'failure') } | ForEach-Object {
        $failures += "$($_.context): $($_.description)"
    }

    return $failures
}

function Get-CIPendingCount {
    $sha = gh pr view $PRNumber --repo $repoSlug --json headRefOid -q '.headRefOid' 2>&1
    $pending = 0

    $checkRuns = (gh api "repos/$owner/$repo/commits/$sha/check-runs" 2>&1 | ConvertFrom-Json).check_runs
    $pending += @($checkRuns | Where-Object { $_.status -in @('queued', 'in_progress') }).Count

    $statusResult = gh api "repos/$owner/$repo/commits/$sha/status" 2>&1 | ConvertFrom-Json
    $pending += @($statusResult.statuses | Where-Object { $_.state -eq 'pending' }).Count

    return $pending
}

function Request-CopilotReview {
    # Returns $true if the request was accepted by GitHub, $false otherwise
    $result = gh api "repos/$owner/$repo/pulls/$PRNumber/requested_reviewers" `
        -X POST -f "reviewers[]=${copilotLogin}[bot]" 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Status "WARNING: Copilot review request failed: $result"
        return $false
    }
    return $true
}

function Test-CopilotReviewRequested {
    # Check if Copilot is already in the pending requested reviewers list
    $reviewers = gh api "repos/$owner/$repo/pulls/$PRNumber/requested_reviewers" --jq '.users[].login' 2>&1
    if ($LASTEXITCODE -ne 0) { return $false }
    return ($reviewers -match '^Copilot$')
}

function Get-PRMergeableState {
    $state = (gh pr view $PRNumber --repo $repoSlug --json mergeable -q '.mergeable' 2>&1).Trim()
    return $state
}

function Resolve-MergeConflicts {
    Write-Status "PR has merge conflicts. Attempting rebase on origin/main..."
    git fetch origin main 2>&1 | Out-Null
    $rebaseOutput = git -c core.hooksPath=NUL rebase origin/main 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Status "Rebase failed — conflicts require manual resolution. Aborting rebase."
        git rebase --abort 2>&1 | Out-Null
        Write-Status "REBASE OUTPUT:`n$($rebaseOutput | Out-String)"
        return $false
    }
    Write-Status "Rebase succeeded. Force-pushing..."
    $pushOutput = git -c core.hooksPath=NUL push --force-with-lease 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Status "Force-push failed: $($pushOutput | Out-String)"
        return $false
    }
    Write-Status "Merge conflicts resolved and pushed. Waiting for CI and Copilot review."
    return $true
}

function ConvertFrom-TokenString {
    param([string]$Value)
    $v = $Value.Trim() -replace ',', ''
    if ($v -match '^([\d.]+)\s*M$') { return [double]$Matches[1] * 1000000 }
    if ($v -match '^([\d.]+)\s*k$') { return [double]$Matches[1] * 1000 }
    if ($v -match '^([\d.]+)$') { return [double]$Matches[1] }
    return 0
}

function ConvertTo-TokenString {
    param([double]$Value)
    if ($Value -ge 1000000) { return '{0:N1}M' -f ($Value / 1000000) }
    if ($Value -ge 1000) { return '{0:N1}k' -f ($Value / 1000) }
    return '{0:N0}' -f $Value
}

# --- Enter worktree ---
Push-Location $worktreeDir

try {
    $idleCount = 0
    $check = 0
    $workCycles = 0
    $prClean = $false
    $conflictAttempts = 0
    $copilotInvocations = 0
    $totalPremiumRequests = 0
    $totalAdded = 0
    $totalRemoved = 0
    $totalCost = 0.0
    $totalTokensIn = 0.0
    $totalTokensOut = 0.0

    # Agent review phase tracking
    $reviewPhase = 'initial'    # 'initial' → 'agent-reviews' → 'copilot-review'
    $agentReviewIndex = 0
    $agentInvocations = 0
    $agentTotalPremium = 0
    $agentTotalCost = 0.0
    $totalTokensCached = 0.0
    $copilotReviewPending = if ($script:ProviderConfig.SupportsNativePrReview) { Test-CopilotReviewRequested } else { $false }
    $copilotReviewWaitCycles = 0
    if ($copilotReviewPending) {
        Write-Status "Copilot review already requested — will wait for it."
    }

    while ($true) {
        if ($deadline -and (Get-Date) -ge $deadline) {
            Write-Status "Deadline reached ($($deadline.ToString('HH:mm'))). Stopping."
            break
        }

        $check++
        $actionTaken = $false
        $reviewRequested = $false
        $cycleStart = Get-Date
        $elapsed = $cycleStart - $runStart
        $elapsedStr = "{0:hh\:mm\:ss}" -f $elapsed

        Write-Host ""
        Write-Host "===================================="
        Write-Host "== Check $check  |  Elapsed: $elapsedStr"
        Write-Host "===================================="

        # --- Check for merge conflicts first ---
        Write-Status "Checking PR mergeable state..."
        $mergeState = Get-PRMergeableState
        if ($mergeState -eq "CONFLICTING") {
            Invoke-Audio "start-pr-processing.mp3"
            $resolved = Resolve-MergeConflicts
            if (-not $resolved) {
                $conflictAttempts++
                if ($conflictAttempts -ge 3) {
                    Write-Status "Failed to resolve merge conflicts after $conflictAttempts attempts. Exiting."
                    "[$logTag] MERGE CONFLICT: Gave up after $conflictAttempts copilot attempts." | Out-File $logFile -Append -Encoding UTF8
                    break
                }
                Write-Status "Handing off to Copilot to resolve merge conflicts (attempt $conflictAttempts)..."
                $conflictPrompt = "PR #$PRNumber in $repoSlug has merge conflicts with origin/main. Resolve them by rebasing branch '$prBranch' on origin/main, fixing all conflict markers in the affected files, then force-push. Working directory: $(Get-Location)"
                "[$logTag] Merge conflict — invoking copilot (attempt $conflictAttempts): $conflictPrompt" | Out-File $logFile -Append -Encoding UTF8
                $workCycles++
                $copilotInvocations++
                $cmdArgs = Build-RalphCommand -ModelId $script:ModelId -Agent $script:DevAgentName -Prompt $conflictPrompt -Yolo -Provider $script:ProviderName
                $cmdExe = $cmdArgs[0]; $cmdTail = $cmdArgs[1..($cmdArgs.Count-1)]
                & $cmdExe @cmdTail 2>&1 | Out-Null
                Test-AgentExit $LASTEXITCODE
                $actionTaken = $true
                continue
            }
            $conflictAttempts = 0
            $actionTaken = $true
            # Skip to next cycle — CI needs to re-run and Copilot will re-review
            Write-Status "Waiting $WaitMinutes minutes for CI to pick up rebased commits..."
            Invoke-Audio "waiting.mp3"
            $waitSeconds = $WaitMinutes * 60
            for ($s = 0; $s -lt $waitSeconds; $s += 1) {
                if ($script:HasConsole -and [Console]::KeyAvailable) {
                    $key = [Console]::ReadKey($true)
                    if ($key.Key -eq 'Enter') {
                        Write-Status "Skipping wait - resuming now."
                        break
                    }
                    if ($key.Key -eq 'C' -and ($key.Modifiers -band [ConsoleModifiers]::Control)) {
                        Write-Progress -Id 1 -Activity "Waiting for CI after rebase (Enter to skip)" -Completed
                        Write-Host "`nCtrl+C — exiting ralph-pr." -ForegroundColor Yellow
                        exit 0
                    }
                }
                $remaining = [math]::Max(0, $waitSeconds - $s)
                $pct = [math]::Round(($s / $waitSeconds) * 100)
                $mins = [math]::Floor($remaining / 60)
                $secs = $remaining % 60
                Write-Progress -Id 1 -Activity "Waiting for CI after rebase (Enter to skip)" `
                    -Status "$mins`:$("{0:D2}" -f $secs) remaining" `
                    -PercentComplete $pct
                Start-Sleep -Seconds 1
            }
            Write-Progress -Id 1 -Activity "Waiting for CI after rebase (Enter to skip)" -Completed
            continue
        }
        elseif ($mergeState -eq "UNKNOWN") {
            Write-Status "Mergeable state is UNKNOWN (GitHub still calculating). Will recheck next cycle."
        }

        Write-Status "Checking CI status and PR comments..."
        $ciFailures = Get-CIFailureList
        $snykFailures = @($ciFailures | Where-Object { $_ -match '(?i)snyk' })
        $actionableFailures = @($ciFailures | Where-Object { $_ -notmatch '(?i)snyk' })

        if ($snykFailures.Count -gt 0 -and $actionableFailures.Count -eq 0) {
            $snykList = $snykFailures -join "`n  - "
            Write-Status "Only Snyk CI failures detected (ignored — external service issue):`n  - $snykList"
        }

        $unresolvedCount = Get-UnresolvedThreadCount

        # --- Check if CI is still in progress ---
        if ($actionableFailures.Count -eq 0 -and $unresolvedCount -eq 0) {
            $pendingCI = Get-CIPendingCount
            if ($pendingCI -gt 0) {
                Write-Status "CI still in progress ($pendingCI check(s) pending). Waiting..."
                $actionTaken = $false
                $reviewRequested = $false
                $copilotReviewPending = $false
                # CI waiting is not a cycle — unlimited waits until CI completes
                Invoke-Audio "waiting.mp3"
                $waitSeconds = $WaitMinutes * 60
                for ($s = 0; $s -lt $waitSeconds; $s += 1) {
                    if ($script:HasConsole -and [Console]::KeyAvailable) {
                        $key = [Console]::ReadKey($true)
                        if ($key.Key -eq 'Enter') {
                            Write-Status "Skipping wait - resuming now."
                            break
                        }
                    }
                    Start-Sleep -Seconds 1
                }
                continue
            }
        }

        if ($actionableFailures.Count -gt 0 -or $unresolvedCount -gt 0) {
            $promptParts = @()
            $promptParts += @"
WORKTREE CONTEXT: You are inside the git worktree for branch ``$prBranch`` (PR #$PRNumber in $repoSlug).
Working directory: $(Get-Location)
IMPORTANT: Do NOT run ``git switch``, ``git checkout``, or create another worktree.
Work directly in this directory. All changes stay on branch ``$prBranch``.
"@

            if ($actionableFailures.Count -gt 0) {
                $failureList = $actionableFailures -join "`n  - "
                Write-Status "Found $($actionableFailures.Count) actionable CI failure(s):`n  - $failureList"
                if ($snykFailures.Count -gt 0) {
                    $snykList = $snykFailures -join "`n  - "
                    Write-Status "Also found $($snykFailures.Count) Snyk CI failure(s) (ignored):`n  - $snykList"
                }
                $promptParts += @"

CI FAILURES - The following CI checks are failing:
$($actionableFailures | ForEach-Object { "- $_" } | Out-String)
Investigate each failure by examining the CI logs and error output. Fix the root cause in the code - do not suppress warnings, skip tests, or weaken quality gates.
"@
            }

            if ($snykFailures.Count -gt 0) {
                $promptParts += @"

SNYK CI NOTE: The following Snyk checks are also reported as failing. Snyk is an external third-party security scanning service that occasionally has infrastructure issues beyond our control. These failures CANNOT be fixed in code and should be COMPLETELY IGNORED:
$($snykFailures | ForEach-Object { "- $_" } | Out-String)
Do NOT attempt to fix or investigate Snyk failures. They are not caused by code changes.
"@
            }

            if ($unresolvedCount -gt 0) {
                Write-Status "Found $unresolvedCount unresolved PR comment(s)."
                $promptParts += @"

UNRESOLVED PR COMMENTS - There are $unresolvedCount unresolved review comments on PR #$PRNumber.
Read each comment carefully and address the feedback in the code. After fixing the code for each comment, you MUST also resolve the review thread on GitHub. A comment is not done until BOTH the code is fixed AND the thread is marked as resolved. Use the GitHub CLI to resolve each thread: ``gh api graphql -f query='mutation { resolveReviewThread(input: {threadId: "THREAD_NODE_ID"}) { thread { isResolved } } }'``. To find thread IDs, query the PR's review threads via GraphQL.
"@
            }

            $promptParts += @"

IMPORTANT: Make all fixes first, run the build and tests locally to verify, then resolve every review thread on GitHub, then make a single commit and push at the very end. Do NOT commit multiple times. You are not done until all issues are addressed in code, all review threads are resolved on GitHub, and changes are committed and pushed.
"@

            $fullPrompt = $promptParts -join "`n"
            Invoke-Audio "start-pr-processing.mp3"

            $tempOut = [System.IO.Path]::GetTempFileName()
            $workCycles++
            $copilotInvocations++
            $cmdArgs = Build-RalphCommand -ModelId $script:ModelId -Agent $script:DevAgentName -Prompt $fullPrompt -Yolo -Provider $script:ProviderName
            $cmdExe = $cmdArgs[0]; $cmdTail = $cmdArgs[1..($cmdArgs.Count-1)]
            & $cmdExe @cmdTail 2>&1 | Tee-Object -FilePath $tempOut
            Test-AgentExit $LASTEXITCODE

            $captured = Get-Content $tempOut -Encoding UTF8
            $statsLines = $captured |
                Select-String -Pattern '^\s*(Changes|Requests|Tokens)\s' |
                ForEach-Object { $_.Line.Trim() }
            Remove-Item $tempOut -ErrorAction SilentlyContinue

            # --- Parse round stats and display summary ---
            $roundAdded = 0; $roundRemoved = 0
            $roundRequests = '(no stats)'; $roundTokens = '(no stats)'
            $roundPremium = 0
            $roundTokensIn = 0.0; $roundTokensOut = 0.0; $roundTokensCached = 0.0

            foreach ($stat in $statsLines) {
                if ($stat -match '^Changes\s+\+(\d+)\s+\-(\d+)') {
                    $roundAdded = [int]$Matches[1]
                    $roundRemoved = [int]$Matches[2]
                }
                elseif ($stat -match '^Requests\s+(.+)') {
                    $roundRequests = $Matches[1].Trim()
                    # Parse premium request count, e.g. "3 Premium (3m 30s)"
                    if ($stat -match '(\d+)\s+Premium') {
                        $roundPremium = [int]$Matches[1]
                    }
                }
                elseif ($stat -match '^Tokens\s+(.+)') {
                    $roundTokens = $Matches[1].Trim()
                    $tokenParts = $roundTokens -split ([char]0x2022) | ForEach-Object { $_.Trim() }
                    if ($tokenParts.Count -ge 3) {
                        $roundTokensIn = ConvertFrom-TokenString ($tokenParts[0] -replace '[^\d.,kM]', '')
                        $roundTokensOut = ConvertFrom-TokenString ($tokenParts[1] -replace '[^\d.,kM]', '')
                        $roundTokensCached = ConvertFrom-TokenString ($tokenParts[2] -replace '[^\d.,kM]', '')
                    }
                    else {
                        Write-Host "  (could not parse token totals from: $roundTokens)" -ForegroundColor DarkGray
                    }
                }
            }

            $roundCost = [math]::Round($roundPremium * 0.04 * $script:ModelMultiplier, 2)
            $totalAdded += $roundAdded
            $totalRemoved += $roundRemoved
            $totalPremiumRequests += $roundPremium
            $totalCost += $roundCost
            $totalTokensIn += $roundTokensIn
            $totalTokensOut += $roundTokensOut
            $totalTokensCached += $roundTokensCached

            $elapsed = (Get-Date) - $runStart
            $elapsedHrs = [math]::Floor($elapsed.TotalHours)
            $elapsedMins = $elapsed.Minutes
            $elapsedStr = if ($elapsedHrs -gt 0) { "${elapsedHrs}h ${elapsedMins}m" } else { "${elapsedMins}m" }

            $totInStr = ConvertTo-TokenString $totalTokensIn
            $totOutStr = ConvertTo-TokenString $totalTokensOut
            $totCachedStr = ConvertTo-TokenString $totalTokensCached

            $pluralS = if ($copilotInvocations -ne 1) { 's' } else { '' }
            $arrUp = [char]0x2191; $arrDn = [char]0x2193; $dot = [char]0x2022

            $sep = [string]::new('=', 60)
            $summaryLines = @(
                $sep
                "  Round $copilotInvocations"
                "  Changes        +$roundAdded -$roundRemoved"
                "  Requests       $roundRequests"
                "  Cost           `$$roundCost  ($roundPremium premium x `$$([math]::Round(0.04 * $script:ModelMultiplier, 4)))"
                "  Tokens         $roundTokens"
                ""
                "  Totals ($copilotInvocations round${pluralS})"
                "  Changes        +$totalAdded -$totalRemoved"
                "  Cost           `$$([math]::Round($totalCost, 2))  ($totalPremiumRequests premium requests)"
                "  Tokens         $arrUp $totInStr $dot $arrDn $totOutStr $dot $totCachedStr (cached)"
                "  Elapsed        $elapsedStr"
                $sep
            )

            foreach ($sl in $summaryLines) {
                if ($sl -eq $sep) { Write-Host $sl -ForegroundColor DarkCyan }
                elseif ($sl -match '^\s+Round \d' -or $sl -match '^\s+Totals') { Write-Host $sl -ForegroundColor Cyan }
                elseif ($sl -match 'Cost') { Write-Host $sl -ForegroundColor Yellow }
                else { Write-Host $sl }
            }

            $logSummary = $summaryLines | ForEach-Object { "[$logTag] $_" }
            ($logSummary -join "`n") | Out-File $logFile -Append -Encoding UTF8

            $remainingCount = Get-UnresolvedThreadCount
            if ($remainingCount -gt 0) {
                Write-Status "WARNING: $remainingCount unresolved comment(s) still remain. Retrying immediately..."
                continue
            }

            $actionTaken = $true
        }
        else {
            if ($snykFailures.Count -gt 0) {
                Write-Status "No actionable CI failures or unresolved comments. (Snyk failures ignored)"
            }
            else {
                Write-Status "No CI failures or unresolved comments."
            }

            $lastCommitDate = Get-LastCommitDate

            # --- Phase transitions: initial → agent-reviews → copilot-review ---
            if ($reviewPhase -eq 'initial') {
                if ($parsedAgents.Count -gt 0) {
                    $reviewPhase = 'agent-reviews'
                    Write-Status "Entering agent review phase ($($parsedAgents.Count) reviewer(s) with fix loops)"
                } else {
                    $reviewPhase = 'copilot-review'
                }
            }

            # --- Agent review phase: sequential review → fix ��� next ---
            # Intentional: sequential `if` (not elseif) allows fallthrough when phase transitions above
            if ($reviewPhase -eq 'agent-reviews') {
                Write-Host "    [diag] agentReviewIndex=$agentReviewIndex  parsedAgents.Count=$($parsedAgents.Count)  parsedAgents.Type=$($parsedAgents.GetType().Name)" -ForegroundColor DarkGray
                if ($agentReviewIndex -lt $parsedAgents.Count) {
                    $spec = $parsedAgents[$agentReviewIndex]
                    $agentInvocations++

                    $displayIndex = $agentReviewIndex + 1
                    Write-Host ""
                    Write-Host "====================================" -ForegroundColor Magenta
                    Write-Host "  AGENT REVIEW [$displayIndex/$($parsedAgents.Count)]" -ForegroundColor Magenta
                    Write-Host "  Role:  $($spec.Role)" -ForegroundColor Magenta
                    Write-Host "  Model: $($spec.ModelId) ($($spec.ModelLabel))" -ForegroundColor Magenta
                    Write-Host "  Agent: $($spec.Agent)" -ForegroundColor Magenta
                    Write-Host "====================================" -ForegroundColor Magenta

                    "[$logTag] Agent review [$displayIndex/$($parsedAgents.Count)]: $($spec.Role) ($($spec.Agent), $($spec.ModelId))" | Out-File $logFile -Append -Encoding UTF8

                    # Get current head SHA for stamping and marker matching
                    $headSha = (gh pr view $PRNumber --repo $repoSlug --json headRefOid -q '.headRefOid' 2>&1).Trim()
                    $reviewNonce = [guid]::NewGuid().ToString('N').Substring(0,8)
                    $reviewMarker = "<!-- ralph-review: role=$($spec.Role) model=$($spec.ModelId) sha=$headSha nonce=$reviewNonce -->"

                    # Agent writes structured findings to a temp file; Ralph posts the review
                    $findingsPath = Join-Path ([IO.Path]::GetTempPath()) "ralph-review-findings-$reviewNonce.json"

                    $reviewPrompt = @"
You are reviewing PR #$PRNumber in $repoSlug (branch: $prBranch, head SHA: $headSha).

YOUR ROLE: $($spec.Description)

INSTRUCTIONS:
1. Read the full PR diff with: gh pr diff $PRNumber
2. Read PR details with: gh pr view $PRNumber
3. Analyze the changes through the lens of your role described above.
4. Write your findings as a JSON file to: $findingsPath

The JSON file MUST have this exact structure:
{
  "summary": "Brief 2-3 sentence overall summary. Do NOT include any markdown headings (no ## or #). Plain text only.",
  "findings": [
    {
      "path": "relative/path/to/file.ts",
      "line": 42,
      "severity": "critical|warning|suggestion",
      "body": "[Critical] or [Warning] or [Suggestion] Specific actionable comment on this line."
    }
  ]
}

CRITICAL RULES FOR FINDINGS:
- ANY observation about a specific file — whether a bug, concern, suggestion, or improvement opportunity — MUST be a finding. Do not bury file-specific feedback in the summary.
- The summary should ONLY contain your overall impression in 2-3 sentences. No headings, no tables, no lists ��� just plain text. All specific feedback goes in findings.
- "path" must be a file path exactly as shown in the diff (e.g. src/utils/helper.ts)
- "line" must be the line number in the NEW version of the file (right side of diff). Look at @@ hunk headers to compute line numbers accurately. If you cannot determine the exact line, omit the "line" field but still include the finding.
- "severity": "critical" for bugs/security issues, "warning" for correctness concerns, "suggestion" for improvements
- Prefix each "body" with [Critical], [Warning], or [Suggestion] matching the severity.
- Only include findings on lines that appear in the diff or files touched by the diff.
- Each "body" should be specific and actionable — reference the code and explain what to change.
- Do NOT manufacture issues or pad findings. If a file genuinely has no issues, do not create a finding for it.
- But DO include suggestions and minor concerns as findings — these are valuable review feedback.
- If after thorough review you have zero findings, set "findings" to an empty array [].

DO NOT post any comments or reviews to the PR. DO NOT use gh pr comment or gh api to post anything.
DO NOT modify any code, commit, or push. This is a read-only review.
Your only output is the JSON findings file at the path above.
"@

                    $agentTempOut = [System.IO.Path]::GetTempFileName()
                    $reviewFailed = $false
                    $effectiveProvider = if ($spec.ProviderOverride) { $spec.ProviderOverride } else { $script:ProviderName }
                    $preReviewEnv = $null
                    if ($effectiveProvider -ne $script:ProviderName) {
                        $preReviewEnv = Set-RalphProviderEnv -ProviderName $effectiveProvider
                    }
                    try {
                        $reviewCmdArgs = Build-RalphCommand -ModelId $spec.ModelId -Agent $spec.Agent -Prompt $reviewPrompt -Yolo -Provider $effectiveProvider
                        $cmdExe = $reviewCmdArgs[0]; $cmdTail = $reviewCmdArgs[1..($reviewCmdArgs.Count-1)]
                        & $cmdExe @cmdTail 2>&1 | Tee-Object -FilePath $agentTempOut
                        if ($LASTEXITCODE -ne 0) {
                            Write-Host "    Agent $($spec.Role) exited with code $LASTEXITCODE" -ForegroundColor Red
                            "[$logTag] Agent $($spec.Role) FAILED (exit code $LASTEXITCODE)" | Out-File $logFile -Append -Encoding UTF8
                            $reviewFailed = $true
                        }
                    } finally {
                        if ($preReviewEnv) {
                            Restore-RalphProviderEnv -Saved $preReviewEnv
                        }
                    }

                    # Track agent review cost
                    $agentCaptured = Get-Content $agentTempOut -Encoding UTF8
                    $agentStats = $agentCaptured |
                        Select-String -Pattern '^\s*(Changes|Requests|Tokens)\s' |
                        ForEach-Object { $_.Line.Trim() }
                    Remove-Item $agentTempOut -ErrorAction SilentlyContinue

                    $agentRoundPremium = 0
                    foreach ($stat in $agentStats) {
                        if ($stat -match '(\d+)\s+Premium') {
                            $agentRoundPremium = [int]$Matches[1]
                        }
                    }
                    $agentCostPerReq = [math]::Round(0.04 * $spec.ModelMultiplier, 4)
                    $agentRoundCost = [math]::Round($agentRoundPremium * $agentCostPerReq, 2)
                    $agentTotalPremium += $agentRoundPremium
                    $agentTotalCost += $agentRoundCost

                    Write-Host "    Review cost: $agentRoundPremium premium, `$$agentRoundCost" -ForegroundColor DarkGray
                    "[$logTag] Agent $($spec.Role) review done. Premium: $agentRoundPremium, Cost: `$$agentRoundCost" | Out-File $logFile -Append -Encoding UTF8

                    if ($reviewFailed) {
                        Remove-Item $findingsPath -Force -ErrorAction SilentlyContinue
                        Write-Status "Agent $($spec.Role) failed. Skipping to next agent."
                        $agentReviewIndex++
                        $actionTaken = $true
                        continue
                    }

                    # Read findings file, post review with inline comments, build context for fix agent
                    $reviewBody = $null
                    $postedReviewId = $null
                    $findingsJson = $null

                    if (Test-Path $findingsPath) {
                        try { $findingsJson = Get-Content $findingsPath -Raw | ConvertFrom-Json }
                        catch { Write-Warning "Failed to parse agent findings JSON from $findingsPath" }
                        Remove-Item $findingsPath -Force -ErrorAction SilentlyContinue
                    }

                    if ($findingsJson) {
                        # Validate file paths against actual PR changed files
                        $changedFiles = @(gh pr diff $PRNumber --repo $repoSlug --name-only 2>&1)

                        $reviewTitle = "## PR-Reviewer: $($spec.Description)"
                        $reviewPayload = @{
                            body      = "$reviewMarker`n$reviewTitle`n`n$($findingsJson.summary)"
                            event     = "COMMENT"
                            commit_id = $headSha
                        }

                        $validComments = @()
                        $bodyExtras = ""
                        foreach ($f in @($findingsJson.findings)) {
                            if (-not $f.path -or -not $f.body) { continue }
                            if ($changedFiles -contains $f.path) {
                                $comment = @{ path = $f.path; body = $f.body; side = "RIGHT" }
                                if ($f.line -and [int]$f.line -gt 0) { $comment["line"] = [int]$f.line }
                                $validComments += $comment
                            } else {
                                # File not in diff — append to review body
                                $bodyExtras += "`n`n**$($f.path)**: $($f.body)"
                            }
                        }
                        if ($bodyExtras) { $reviewPayload.body += $bodyExtras }
                        if ($validComments.Count -gt 0) { $reviewPayload["comments"] = $validComments }

                        # Post the review via GitHub API
                        $payloadPath = Join-Path ([IO.Path]::GetTempPath()) "ralph-review-payload-$reviewNonce.json"
                        $reviewPayload | ConvertTo-Json -Depth 4 -Compress | Set-Content $payloadPath -Encoding UTF8
                        $apiResponse = gh api "repos/$owner/$repo/pulls/$PRNumber/reviews" --input $payloadPath 2>&1
                        Remove-Item $payloadPath -Force -ErrorAction SilentlyContinue

                        try {
                            $parsed = $apiResponse | ConvertFrom-Json
                            if ($parsed.id) {
                                $postedReviewId = $parsed.id
                                Write-Host "    Posted review with $($validComments.Count) inline comment(s)" -ForegroundColor DarkGray
                                "[$logTag] Posted PR review (id=$postedReviewId) with $($validComments.Count) inline comments" | Out-File $logFile -Append -Encoding UTF8
                            } else {
                                Write-Warning "Review API returned unexpected response"
                            }
                        } catch {
                            Write-Warning "Failed to parse review API response: $apiResponse"
                        }

                        # Build rich review body for fix agent (summary + inline comments as markdown)
                        $reviewBody = $findingsJson.summary
                        if ($validComments.Count -gt 0) {
                            $reviewBody += "`n`n---`n## Inline Review Comments`n"
                            foreach ($c in $validComments) {
                                $lineRef = if ($c["line"]) { " (line $($c["line"]))" } else { "" }
                                $reviewBody += "`n### ``$($c.path)``$lineRef`n$($c.body)`n"
                            }
                        }
                    }

                    # Fallback: check issue comments (backward compat if agent posted directly)
                    if (-not $reviewBody) {
                        $allComments = @(gh api "repos/$owner/$repo/issues/$PRNumber/comments" --paginate 2>&1 | ConvertFrom-Json)
                        $markerComment = $allComments | Where-Object { $_.body -like "*ralph-review*$($spec.Role)*" } | Select-Object -Last 1
                        if ($markerComment) {
                            $reviewBody = $markerComment.body
                            Write-Host "    (fallback: found review in issue comments)" -ForegroundColor DarkGray
                        }
                    }

                    if ($reviewBody) {
                        # Run dev agent to address the review findings
                        Write-Status "Running dev agent to address review findings..."
                        $fixPrompt = @"
WORKTREE CONTEXT: You are inside the git worktree for branch ``$prBranch`` (PR #$PRNumber in $repoSlug).
Working directory: $(Get-Location)
IMPORTANT: Do NOT run ``git switch``, ``git checkout``, or create another worktree.

A code reviewer ($($spec.Role)) just reviewed this PR and posted the following feedback:

---
$reviewBody
---

Address every actionable issue raised in the review above.
- If the reviewer indicated everything looks good with no issues to fix, make no changes and do not commit.
- Ignore non-actionable praise or general observations.
- Do not make speculative changes beyond what the reviewer requested.

If there are issues to fix:
1. Make all fixes
2. Run the build and tests locally to verify
3. Make a single commit and push
"@
                        $fixTempOut = [System.IO.Path]::GetTempFileName()
                        $workCycles++
                        $copilotInvocations++
                        $fixCmdArgs = Build-RalphCommand -ModelId $script:ModelId -Agent $script:DevAgentName -Prompt $fixPrompt -Yolo -Provider $script:ProviderName
                        $cmdExe = $fixCmdArgs[0]; $cmdTail = $fixCmdArgs[1..($fixCmdArgs.Count-1)]
                        & $cmdExe @cmdTail 2>&1 | Tee-Object -FilePath $fixTempOut
                        Test-AgentExit $LASTEXITCODE

                        # Track fix cycle stats (even if exit code is nonzero)
                        $fixCaptured = Get-Content $fixTempOut -Encoding UTF8
                        $fixStats = $fixCaptured |
                            Select-String -Pattern '^\s*(Changes|Requests|Tokens)\s' |
                            ForEach-Object { $_.Line.Trim() }
                        Remove-Item $fixTempOut -ErrorAction SilentlyContinue

                        $fixPremium = 0
                        foreach ($stat in $fixStats) {
                            if ($stat -match '(\d+)\s+Premium') {
                                $fixPremium = [int]$Matches[1]
                            }
                        }
                        $fixCost = [math]::Round($fixPremium * (0.04 * $script:ModelMultiplier), 2)
                        $totalPremiumRequests += $fixPremium
                        $totalCost += $fixCost

                        Write-Host "    Fix cost: $fixPremium premium, `$$fixCost" -ForegroundColor DarkGray

                        # Resolve agent review threads to prevent re-entering generic fix loop
                        if ($postedReviewId) {
                            $cursor = $null
                            $threadsToResolve = @()
                            do {
                                $afterClause = if ($null -ne $cursor) { ", after: `"$cursor`"" } else { "" }
                                $tq = @"
query { repository(owner: "$owner", name: "$repo") { pullRequest(number: $PRNumber) { reviewThreads(first: 100$afterClause) { nodes { id isResolved comments(first: 1) { nodes { pullRequestReview { databaseId } } } } pageInfo { hasNextPage endCursor } } } } }
"@
                                $tr = gh api graphql -f query="$tq" 2>&1 | ConvertFrom-Json
                                if ($tr.data) {
                                    $nodes = @($tr.data.repository.pullRequest.reviewThreads.nodes)
                                    $threadsToResolve += @($nodes | Where-Object {
                                        -not $_.isResolved -and
                                        $_.comments -and $_.comments.nodes -and $_.comments.nodes.Count -gt 0 -and
                                        $_.comments.nodes[0].pullRequestReview -and
                                        $_.comments.nodes[0].pullRequestReview.databaseId -eq $postedReviewId
                                    })
                                    $cursor = $tr.data.repository.pullRequest.reviewThreads.pageInfo.endCursor
                                }
                            } while ($tr.data -and $tr.data.repository.pullRequest.reviewThreads.pageInfo.hasNextPage)

                            foreach ($thread in $threadsToResolve) {
                                gh api graphql -f query="mutation { resolveReviewThread(input: {threadId: `"$($thread.id)`"}) { thread { isResolved } } }" 2>&1 | Out-Null
                            }
                            if ($threadsToResolve.Count -gt 0) {
                                Write-Host "    Resolved $($threadsToResolve.Count) agent review thread(s)" -ForegroundColor DarkGray
                                "[$logTag] Resolved $($threadsToResolve.Count) agent review threads for review $postedReviewId" | Out-File $logFile -Append -Encoding UTF8
                            }
                        }
                    } else {
                        Write-Status "Reviewer $($spec.Role) did not produce findings. Skipping fix cycle."
                    }

                    # Advance to next agent only after successful attempt
                    $agentReviewIndex++
                    $actionTaken = $true
                    continue  # Re-check CI + threads before next agent
                }
                else {
                    # All agent reviews complete
                    $reviewPhase = 'copilot-review'
                    Write-Status "All $($parsedAgents.Count) agent review(s) complete. Moving to Copilot review."

                    if ($agentInvocations -gt 0) {
                        Write-Host ""
                        Write-Host "====================================" -ForegroundColor Magenta
                        Write-Host "  AGENT REVIEWS COMPLETE" -ForegroundColor Magenta
                        Write-Host "  Invocations:     $agentInvocations"
                        Write-Host "  Premium reqs:    $agentTotalPremium"
                        Write-Host "  Est. cost:       `$$([math]::Round($agentTotalCost, 2))" -ForegroundColor Yellow
                        Write-Host "====================================" -ForegroundColor Magenta
                        "[$logTag] Agent reviews complete. $agentInvocations invocations, $agentTotalPremium premium, est. cost `$$([math]::Round($agentTotalCost, 2))." | Out-File $logFile -Append -Encoding UTF8
                    }
                    # Force a fresh cycle before copilot review so CI can process any fix-cycle commits
                    $actionTaken = $true
                    continue
                }
            }

            # --- Copilot review phase (always last, only if provider supports it) ---
            if ($reviewPhase -eq 'copilot-review') {
                if (-not $script:ProviderConfig.SupportsNativePrReview) {
                    Write-Status "Provider '$script:ProviderName' does not support native PR review. PR is clean!"
                    $prClean = $true
                    break
                }
                if ($SkipReview) {
                    Write-Status "SkipReview: skipping Copilot review. PR is clean!"
                    $prClean = $true
                    break
                }

                # Re-fetch commit date — fix cycles may have pushed new commits since $lastCommitDate was set
                $freshCommitDate = Get-LastCommitDate
                $lastReviewDate = Get-LastCopilotReviewDate

                if ($null -eq $lastReviewDate) {
                    if (-not $copilotReviewPending) {
                        Write-Status "No Copilot review found yet. Requesting one..."
                        Invoke-Audio "start-pr-review.mp3"
                        $requestOk = Request-CopilotReview
                        if ($requestOk) {
                            Write-Status "Copilot review requested."
                            $copilotReviewPending = $true
                            $copilotReviewWaitCycles = 0
                        } else {
                            Write-Status "Copilot review request failed — treating PR as clean (no review available)."
                            $script:ReviewUnavailable = $true
                            $prClean = $true
                            break
                        }
                    } else {
                        $copilotReviewWaitCycles++
                        if ($copilotReviewWaitCycles -ge 5) {
                            Write-Status "Copilot review not received after $copilotReviewWaitCycles cycles (~15 min). Skipping review — treating PR as clean."
                            $script:ReviewUnavailable = $true
                            $copilotReviewPending = $false
                            $prClean = $true
                            break
                        }
                        Write-Status "Copilot review still pending ($copilotReviewWaitCycles/5 cycles). Waiting..."
                    }
                    $actionTaken = $true
                    $reviewRequested = $true
                }
                elseif ($freshCommitDate -gt $lastReviewDate) {
                    Write-Status "New commit after last review (commit: $($freshCommitDate.ToString('HH:mm:ss')) > review: $($lastReviewDate.ToString('HH:mm:ss'))). Requesting Copilot review..."
                    Invoke-Audio "start-pr-review.mp3"
                    $requestOk = Request-CopilotReview
                    if ($requestOk) {
                        Write-Status "Copilot review requested."
                        $copilotReviewPending = $true
                        $copilotReviewWaitCycles = 0
                    } else {
                        Write-Status "Copilot review request failed — treating PR as clean."
                        $script:ReviewUnavailable = $true
                        $prClean = $true
                        break
                    }
                    $actionTaken = $true
                    $reviewRequested = $true
                }
                else {
                    Write-Status "Copilot review complete with 0 unresolved comments. PR is clean!"
                    $copilotReviewPending = $false
                    $prClean = $true
                    break
                }
            }
        }

        if ($actionTaken) {
            $idleCount = 0
        }
        else {
            $idleCount++
            if ($idleCount -ge $MaxIdleCycles) {
                Write-Status "No action for $MaxIdleCycles consecutive cycles. Exiting."
                break
            }
        }

        # Short wait after real work (fixes pushed), full wait when idle or review just requested
        if ($actionTaken -and -not $reviewRequested) {
            $waitSeconds = 10
            Write-Status "Action taken — short 10s cooldown before next check..."
        }
        else {
            $waitSeconds = $WaitMinutes * 60
            Write-Status "Waiting $WaitMinutes minutes... (Idle: $idleCount/$MaxIdleCycles) - press Enter to skip"
        }
        Invoke-Audio "waiting.mp3"
        for ($s = 0; $s -lt $waitSeconds; $s += 1) {
            if ($script:HasConsole -and [Console]::KeyAvailable) {
                $key = [Console]::ReadKey($true)
                if ($key.Key -eq 'Enter') {
                    Write-Status "Skipping wait - resuming now."
                    break
                }
                if ($key.Key -eq 'C' -and ($key.Modifiers -band [ConsoleModifiers]::Control)) {
                    Write-Progress -Id 1 -Activity "Waiting before next cycle (Enter to skip)" -Completed
                    Write-Host "`nCtrl+C — exiting ralph-pr." -ForegroundColor Yellow
                    exit 0
                }
            }
            $remaining = [math]::Max(0, $waitSeconds - $s)
            $pct = [math]::Round(($s / $waitSeconds) * 100)
            $mins = [math]::Floor($remaining / 60)
            $secs = $remaining % 60
            Write-Progress -Id 1 -Activity "Waiting before next cycle (Enter to skip)" `
                -Status "$mins`:$("{0:D2}" -f $secs) remaining" `
                -PercentComplete $pct
            Start-Sleep -Seconds 1
        }
        Write-Progress -Id 1 -Activity "Waiting before next cycle (Enter to skip)" -Completed
    }
}
finally {
    Pop-Location
}

$endTime = Get-Date
$totalDuration = $endTime - $runStart
$totalDurStr = "{0:hh\:mm\:ss}" -f $totalDuration

$costPerRequest = [math]::Round(0.04 * $script:ModelMultiplier, 4)
$costTotal = [math]::Round($totalPremiumRequests * $costPerRequest, 2)
$grandTotalPremium = $totalPremiumRequests + $agentTotalPremium
$grandTotalCost = [math]::Round($costTotal + $agentTotalCost, 2)

Write-Host ""
Write-Host "====================================" -ForegroundColor Cyan
Write-Host "  RALPH PR SUMMARY" -ForegroundColor Cyan
Write-Host "====================================" -ForegroundColor Cyan
Write-Host "  Finished:            $($endTime.ToString('yyyy-MM-dd HH:mm:ss'))"
Write-Host "  Total duration:      $totalDurStr"
Write-Host "  Work cycles:         $workCycles"
Write-Host "  Total checks:        $check"
Write-Host "  Copilot invocations: $copilotInvocations"
Write-Host "  Dev-fix premium:     $totalPremiumRequests ($script:ModelId)" -ForegroundColor Cyan
Write-Host "  Dev-fix cost:        `$$costTotal" -ForegroundColor Yellow
if ($agentInvocations -gt 0) {
    Write-Host "  Agent reviews:       $agentInvocations invocations" -ForegroundColor Magenta
    Write-Host "  Agent premium:       $agentTotalPremium (mixed models)" -ForegroundColor Magenta
    Write-Host "  Agent cost:          `$$([math]::Round($agentTotalCost, 2))" -ForegroundColor Yellow
}
Write-Host "  GRAND TOTAL:         `$$grandTotalCost  ($grandTotalPremium premium)" -ForegroundColor Yellow
Write-Host "  Original prompt:     $promptDisplay"
Write-Host "  PR:                  #$PRNumber ($repoSlug) - https://github.com/$repoSlug/pull/$PRNumber"
Write-Host "  Branch:              $prBranch"
Write-Host "  Worktree:            $worktreeDir"
Write-Host "====================================" -ForegroundColor Cyan

$summaryBlock = @"

[$logTag] ====================================
[$logTag] RALPH PR SUMMARY
[$logTag] ====================================
[$logTag]   Finished:            $($endTime.ToString('yyyy-MM-dd HH:mm:ss'))
[$logTag]   Total duration:      $totalDurStr
[$logTag]   Work cycles:         $workCycles
[$logTag]   Total checks:        $check
[$logTag]   Copilot invocations: $copilotInvocations
[$logTag]   Dev-fix premium:     $totalPremiumRequests ($script:ModelId)
[$logTag]   Dev-fix cost:        `$$costTotal
[$logTag]   Agent reviews:       $agentInvocations invocations
[$logTag]   Agent premium:       $agentTotalPremium
[$logTag]   Agent cost:          `$$([math]::Round($agentTotalCost, 2))
[$logTag]   GRAND TOTAL:         `$$grandTotalCost ($grandTotalPremium premium)
[$logTag]   Original prompt:     $promptDisplay
[$logTag]   PR:                  #$PRNumber (https://github.com/$repoSlug/pull/$PRNumber)
[$logTag]   Branch:              $prBranch
[$logTag]   Worktree:            $worktreeDir
[$logTag] ====================================
"@
$summaryBlock | Out-File $logFile -Append -Encoding UTF8

# --- Write stats file for parent rollup ---
if ($StatsPath) {
    @{
        prPhasePremium  = $grandTotalPremium
        prPhaseCost     = $grandTotalCost
        devFixPremium   = $totalPremiumRequests
        devFixCost      = $costTotal
        agentPremium    = $agentTotalPremium
        agentCost       = $agentTotalCost
        agentInvocations = $agentInvocations
        workCycles      = $workCycles
        checks          = $check
        copilotInvocations = $copilotInvocations
        tokensIn        = $totalTokensIn
        tokensOut       = $totalTokensOut
        tokensCached    = $totalTokensCached
        linesAdded      = $totalAdded
        linesRemoved    = $totalRemoved
    } | ConvertTo-Json | Set-Content -Path $StatsPath -Encoding UTF8
}

# --- Summary log entry ---
$dot = [char]0x2022
$tokInStr = ConvertTo-TokenString $totalTokensIn
$tokOutStr = ConvertTo-TokenString $totalTokensOut
$tokCachedStr = ConvertTo-TokenString $totalTokensCached
"$($endTime.ToString('yyyy-MM-dd HH:mm')) -- ${repoName}: Ralph PR #$PRNumber processing complete. $workCycles work cycles, $check checks. Total duration $totalDurStr. Total tokens: $tokInStr in $dot $tokOutStr out $dot $tokCachedStr cached. $promptDisplay." | Out-File $script:SummaryLogFile -Append -Encoding UTF8

# --- Completion audio ---
Invoke-Audio "ralph-pr-loop-completed.mp3"

# A merge a caller requires (-RequireMerge, Buddy's merge option) needs a completed review:
# without one the PR is neither approved nor merged, and the run fails below.
if ($RequireMerge -and $script:ReviewUnavailable -and $prClean) {
    Write-Host "No review completed, so PR #$PRNumber is neither approved nor merged." -ForegroundColor Yellow
    "[$logTag] No completed review; skipped approve and merge." | Out-File $logFile -Append -Encoding UTF8
    $prClean = $false
}

# --- Auto-approve PR if requested and clean ---
if ($prClean -and $AutoApprove) {
    Write-Host ""
    Write-Host "Auto-approving PR #$PRNumber..." -ForegroundColor Cyan
    $approveOutput = gh pr review $PRNumber --repo $repoSlug --approve --body "Approved by Ralph (auto-approve)" 2>&1
    if ($LASTEXITCODE -eq 0) {
        Write-Host "PR #$PRNumber approved." -ForegroundColor Green
        "[$logTag] PR #$PRNumber auto-approved." | Out-File $logFile -Append -Encoding UTF8
    }
    else {
        Write-Host "Auto-approve failed: $approveOutput" -ForegroundColor Yellow
        "[$logTag] Auto-approve failed: $approveOutput" | Out-File $logFile -Append -Encoding UTF8
    }
}

# --- Offer to merge if PR is clean ---
if ($prClean) {
    Write-Host ""
    Write-Host "PR #$PRNumber is clean and ready to merge!" -ForegroundColor Green
    Write-Host ""

    "[$logTag] PR #$PRNumber is clean. Offering merge." | Out-File $logFile -Append -Encoding UTF8

    $doMerge = $false
    if ($Autopilot -and $AutoApprove) {
        Write-Host "Autopilot: merging automatically..." -ForegroundColor Cyan
        $doMerge = $true
    }
    elseif ($Autopilot -and -not $AutoApprove) {
        Write-Host "Auto-approve is disabled — skipping auto-merge." -ForegroundColor DarkGray
        "[$logTag] Auto-approve off — skipped auto-merge." | Out-File $logFile -Append -Encoding UTF8
    }
    else {
        try {
            $response = Read-Host "Merge now? [y/N]"
            if ($response -match '^[Yy]') {
                $doMerge = $true
            }
            else {
                Write-Host "Skipped merge." -ForegroundColor DarkGray
                "[$logTag] User declined merge." | Out-File $logFile -Append -Encoding UTF8
            }
        }
        catch {
            Write-Host "Could not read input. Skipping merge offer." -ForegroundColor DarkGray
        }
    }

    if ($doMerge) {
        Write-Host "Merging PR #$PRNumber via gh..." -ForegroundColor Cyan
        "[$logTag] Merging PR #$PRNumber with gh pr merge." | Out-File $logFile -Append -Encoding UTF8

        # Leave the worktree before cleanup
        Set-Location $originalDir

        $mergeOutput = gh pr merge $PRNumber --repo $repoSlug --squash --delete-branch --admin 2>&1
        $mergeOutput | Out-File $logFile -Append -Encoding UTF8

        if ($LASTEXITCODE -eq 0) {
            $script:Merged = $true
            Write-Host "PR #$PRNumber merged successfully." -ForegroundColor Green
            "[$logTag] PR #$PRNumber merged successfully." | Out-File $logFile -Append -Encoding UTF8

            # Deterministic cleanup of this PR's worktree only. A reused checkout (for example
            # the repository itself, when the PR branch was checked out there) is never removed.
            $separator = [IO.Path]::DirectorySeparatorChar
            $ownedBase = [IO.Path]::GetFullPath($worktreeBase).TrimEnd($separator) + $separator
            $ownedWorktree = [IO.Path]::GetFullPath($worktreeDir).StartsWith($ownedBase, [StringComparison]::OrdinalIgnoreCase)
            if ((Test-Path $worktreeDir) -and $ownedWorktree) {
                Write-Host "Removing worktree: $worktreeDir" -ForegroundColor DarkGray
                git -C $originalDir worktree remove $worktreeDir --force 2>$null
                if ($LASTEXITCODE -ne 0) { Remove-Item -Recurse -Force $worktreeDir -ErrorAction SilentlyContinue }
                "[$logTag] Removed worktree: $worktreeDir" | Out-File $logFile -Append -Encoding UTF8
            }
            elseif (-not $ownedWorktree) {
                Write-Host "Kept $worktreeDir`: it is not a Ralph worktree." -ForegroundColor DarkGray
            }

            # Clean up local branch if it still exists
            $branchExists = git -C $originalDir rev-parse --verify $prBranch 2>$null
            if ($LASTEXITCODE -eq 0) {
                git -C $originalDir branch -D $prBranch 2>$null
                "[$logTag] Deleted local branch: $prBranch" | Out-File $logFile -Append -Encoding UTF8
            }

            # Update main. A -RequireMerge caller (Buddy) refreshes the checkout itself and
            # never wants its branch switched.
            if (-not $RequireMerge) {
                Write-Host "Updating main..." -ForegroundColor DarkGray
                git -C $originalDir checkout main 2>$null
                git -C $originalDir pull --ff-only 2>$null
                Write-Host "Done." -ForegroundColor Green
            }
        }
        else {
            Write-Host "Merge failed. Check output above." -ForegroundColor Red
            Write-Host $mergeOutput
            "[$logTag] Merge failed." | Out-File $logFile -Append -Encoding UTF8
        }
    }
}

# --- Restore BYOK environment variables ---
if ($script:SavedByokEnv -and $script:SavedByokEnv.Count -gt 0) {
    Restore-RalphProviderEnv -Saved $script:SavedByokEnv
}

# A caller that builds on the merged work (repeat cycles) must not continue without it.
if ($RequireMerge -and -not $script:Merged) {
    Write-Host "A merge was required, but PR #$PRNumber was not merged." -ForegroundColor Red
    "[$logTag] Required merge did not happen." | Out-File $logFile -Append -Encoding UTF8
    exit 3
}

exit 0