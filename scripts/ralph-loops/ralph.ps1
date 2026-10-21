# ralph.ps1 — Iterative autopilot work loop with PR creation and handoff.
# Version: 1.5.2
param(
    [int]$Max = 0,
    [string]$Prompt,
    [string]$Branch,
    [string]$WorkUntil,
    [string]$Model,
    [string]$Provider,
    [string[]]$Agents,
    [string]$StatsPath,
    [string]$ReviewProduct,
    [string]$ReviewMode,
    [switch]$Once,
    [switch]$CleanupWorktree,
    [switch]$NoPR,
    [switch]$Autopilot,
    [switch]$NoAudio,
    [switch]$SkipReview,
    # Passed on to ralph-pr.ps1 as -AutoApprove: with -Autopilot it merges a clean PR.
    [switch]$MergeWhenClean,
    [switch]$Install,
    [switch]$All,
    [switch]$Force,
    [switch]$Pick,
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

# Validate and deduplicate agent roles, split by category
$script:DevAgentRole = $null
$script:DevAgentName = $null
$reviewAgents = @()

if ($Agents) {
    $Agents = @($Agents | Select-Object -Unique)

    # Parse and validate all specs
    $parsedSpecs = @()
    foreach ($spec in $Agents) {
        try {
            $parsedSpecs += Parse-RalphAgentSpec -Spec $spec -Provider $Provider
        } catch {
            Write-Host $_.Exception.Message -ForegroundColor Red
            exit 1
        }
    }

    # Reject model/provider overrides on dev agents (use -Model / -Provider flags instead)
    $devWithOverride = @($parsedSpecs | Where-Object { $_.Category -eq 'dev' -and ($_.ModelOverride -or $_.ProviderOverride) })
    if ($devWithOverride.Count -gt 0) {
        Write-Host "Overrides (@model or @provider:model) not supported for dev agents. Use -Model / -Provider flags instead." -ForegroundColor Red
        Write-Host "Invalid: $($devWithOverride.OriginalSpec -join ', ')" -ForegroundColor Yellow
        exit 1
    }

    $devSpecs = @($parsedSpecs | Where-Object { $_.Category -eq 'dev' })
    $reviewSpecs = @($parsedSpecs | Where-Object { $_.Category -eq 'review' })

    if ($devSpecs.Count -gt 1) {
        Write-Host "Only one dev agent allowed at a time. Got: $($devSpecs.Role -join ', ')" -ForegroundColor Red
        Write-Host "Dev agents: $((Get-RalphDevAgentRoles) -join ', ')" -ForegroundColor Yellow
        exit 1
    }
    if ($devSpecs.Count -eq 1) {
        $script:DevAgentRole = $devSpecs[0].Role
        $script:DevAgentName = $devSpecs[0].Agent
    }

    # Deduplicate review specs on resolved (Role, EffectiveProvider, ModelId)
    $seenReview = @{}
    foreach ($rs in $reviewSpecs) {
        $effProv = if ($rs.ProviderOverride) { $rs.ProviderOverride } else { $Provider }
        $key = "$($rs.Role)|$effProv|$($rs.ModelId)"
        if (-not $seenReview[$key]) {
            $seenReview[$key] = $true
            $reviewAgents += $rs.OriginalSpec
        }
    }
}

# Resolve effective dev agent (explicit or default from config)
if (-not $script:DevAgentRole) {
    $script:DevAgentRole = Get-RalphDefaultDevAgent
    $script:DevAgentName = (Resolve-RalphAgent -Role $script:DevAgentRole -Provider $Provider).Agent
}

[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding = [System.Text.Encoding]::UTF8

if ($Help) {
    Write-Host ""
    Write-Host "ralph.ps1 — Iterative Autopilot" -ForegroundColor Cyan
    Write-Host "Runs Copilot CLI in a loop using git worktrees for branch isolation."
    Write-Host ""
    Write-Host "PARAMETERS" -ForegroundColor Yellow
    Write-Host "  -Max <int>             Number of iterations (default: 10, or unlimited with -WorkUntil)"
    Write-Host "  -Once                  Run a single iteration (shortcut for -Max 1)"
    Write-Host "  -Prompt <string>       Prompt file path or literal text"
    Write-Host "  -Branch <string>       Target branch (auto-generated if omitted)"
    Write-Host "  -WorkUntil <HH:mm>     Stop after this local time (e.g. -WorkUntil 08:00)"
    Write-Host "  -Model <name>          Model to use (default: per-provider, see providers.json)"
    Write-Host "  -Provider <name>       CLI provider: $((Get-RalphProviderNames) -join ', ') (default: $(Get-RalphDefaultProvider))"
    Write-Host "  -Agents <specs>        Agent roles (comma-separated), supports role@model"
    Write-Host "                         Dev agents go to work loop, review agents go to ralph-pr"
    Write-Host "  -StatsPath <path>      Write run stats JSON to this path (for parent rollup)"
    Write-Host "  -ReviewProduct <name>  Accepted for wrapper compatibility"
    Write-Host "  -ReviewMode <name>     Accepted for wrapper compatibility"
    Write-Host "  -CleanupWorktree       Remove worktree after completion if clean"
    Write-Host "  -NoPR                  Skip PR creation and ralph-pr handoff"
    Write-Host "  -Autopilot             Skip interactive prompts, auto-merge PR when clean"
    Write-Host "  -NoAudio               Suppress audio feedback"
    Write-Host "  -SkipReview            Skip Copilot PR review requests"
    Write-Host "  -MergeWhenClean        With -Autopilot, merge the PR once clean (gh pr merge --admin)"
    Write-Host "  -Install              Install ralph-loop scripts and audio into the current repo"
    Write-Host "    -All                   Install all, skip existing (use with -Install)"
    Write-Host "    -Force                 Install all, overwrite existing (use with -Install)"
    Write-Host "    -Pick                  Choose which scripts to install (use with -Install)"
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
    Write-Host "AGENTS" -ForegroundColor Yellow
    Write-Host "  Dev agents " -ForegroundColor White -NoNewline
    Write-Host "(work loop — max 1, default: $(Get-RalphDefaultDevAgent))" -ForegroundColor DarkGray
    foreach ($role in (Get-RalphDevAgentRoles)) {
        $a = Resolve-RalphAgent -Role $role
        Write-Host "    $($role.PadRight(26)) $($a.Description)" -ForegroundColor Gray
    }
    Write-Host ""
    Write-Host "  Review agents " -ForegroundColor White -NoNewline
    Write-Host "(passed to ralph-pr — fix-loop reviews, supports role@model)" -ForegroundColor DarkGray
    foreach ($role in (Get-RalphReviewAgentRoles)) {
        $a = Resolve-RalphAgent -Role $role
        Write-Host "    $($role.PadRight(26)) $($a.Description)" -ForegroundColor Gray
    }
    Write-Host ""
    Write-Host "EXAMPLES" -ForegroundColor Yellow
    Write-Host "  ralph"
    Write-Host "  ralph -Max 5 -Branch feature/my-task"
    Write-Host "  ralph -Prompt C:\prompts\task.md -Branch feature/task -WorkUntil 08:00"
    Write-Host "  ralph -Prompt 'Add error handling' -Branch feature/errors -CleanupWorktree"
    Write-Host "  ralph -Agents developer-principal,pr-review-security,auditor-crap-score"
    Write-Host "  ralph -Agents simplisticate -Prompt 'Reduce CRAP scores'"
    Write-Host "  ralph -Agents pr-review-quality@opus47,pr-review-quality@sonnet  # multi-model review"
    Write-Host "  ralph -Install                     # interactive install into current repo"
    Write-Host "  ralph -Install -All                # install all, skip existing"
    Write-Host "  ralph -Install -Force              # install all, overwrite existing"
    Write-Host "  ralph -Install -Pick               # choose which scripts to install"
    Write-Host ""
    exit 0
}

# --- Install mode: set up consumer repo with ralph-loop scripts and audio ---
if ($Install) {
    $ErrorActionPreference = 'Stop'
    $templateDir = Join-Path $PSScriptRoot "scripts"
    $installRepoRoot = (git rev-parse --show-toplevel 2>$null)
    if (-not $installRepoRoot) {
        Write-Host "ERROR: Not inside a git repository." -ForegroundColor Red
        exit 1
    }
    $installRepoRoot = $installRepoRoot -replace '/', [IO.Path]::DirectorySeparatorChar
    $installRepoName = Split-Path $installRepoRoot -Leaf
    $targetScriptsDir = Join-Path $installRepoRoot "scripts"

    Write-Host ""
    Write-Host "ralph -Install — setting up ralph-loop scripts" -ForegroundColor Cyan
    Write-Host "  Repo:       $installRepoName"
    Write-Host "  Repo root:  $installRepoRoot"
    Write-Host "  Target:     $targetScriptsDir"
    Write-Host ""

    # --- Ensure target scripts directory exists ---
    if (-not (Test-Path $targetScriptsDir)) {
        New-Item -ItemType Directory -Path $targetScriptsDir -Force | Out-Null
        Write-Host "  Created $targetScriptsDir" -ForegroundColor Green
    }

    # --- Discover available templates ---
    $templates = Get-ChildItem -Path $templateDir -Filter "ralph-*.ps1" | Sort-Object Name

    if ($Pick) {
        Write-Host "Available scripts:" -ForegroundColor Yellow
        for ($i = 0; $i -lt $templates.Count; $i++) {
            Write-Host "  [$($i + 1)] $($templates[$i].Name)"
        }
        Write-Host ""
        $selection = Read-Host "Enter numbers to install (comma-separated, e.g. 1,3) or 'all'"
        if ($selection -eq 'all') {
            # keep all templates
        }
        else {
            $indices = $selection -split ',' | ForEach-Object { [int]$_.Trim() - 1 }
            $indices = $indices | Where-Object { $_ -ge 0 -and $_ -lt $templates.Count }
            if ($indices.Count -eq 0) {
                Write-Host "No valid selections." -ForegroundColor Yellow
                exit 1
            }
            $templates = $indices | ForEach-Object { $templates[$_] }
        }
        Write-Host ""
    }

    # --- Install scripts ---
    $installed = 0
    $skipped = 0
    $overwritten = 0

    foreach ($template in $templates) {
        $dest = Join-Path $targetScriptsDir $template.Name
        $exists = Test-Path $dest

        if ($exists -and -not $Force -and -not $All) {
            $answer = Read-Host "  $($template.Name) already exists. Overwrite? [y/N]"
            if ($answer -notmatch '^[Yy]') {
                Write-Host "    Skipped" -ForegroundColor DarkGray
                $skipped++
                continue
            }
            $overwritten++
        }
        elseif ($exists -and -not $Force) {
            Write-Host "  $($template.Name) already exists — skipping (use -Force to overwrite)" -ForegroundColor DarkGray
            $skipped++
            continue
        }
        elseif ($exists) {
            $overwritten++
        }

        Copy-Item -Path $template.FullName -Destination $dest -Force
        if ($exists) {
            Write-Host "  $($template.Name) — overwritten" -ForegroundColor Yellow
        }
        else {
            Write-Host "  $($template.Name) — installed" -ForegroundColor Green
            $installed++
        }
    }

    # --- Repo audio identifier ---
    $audioDir = Join-Path $installRepoRoot "assets" "ralph-loop"
    $audioFile = Join-Path $audioDir "$installRepoName.mp3"

    if (-not (Test-Path $audioFile)) {
        Write-Host ""
        $tts = Get-Command edge-tts -ErrorAction SilentlyContinue
        if ($tts) {
            Write-Host "  Generating repo audio: $audioFile" -ForegroundColor Cyan
            if (-not (Test-Path $audioDir)) { New-Item -ItemType Directory -Path $audioDir -Force | Out-Null }
            $spokenName = $installRepoName -replace '[-_]', ' '
            & edge-tts --text "$spokenName" --write-media "$audioFile" 2>$null
            if (Test-Path $audioFile) {
                Write-Host "  Audio created: $audioFile" -ForegroundColor Green
            }
            else {
                Write-Host "  edge-tts generation failed — create $audioFile manually" -ForegroundColor Yellow
            }
        }
        else {
            Write-Host "  Repo audio not found: $audioFile" -ForegroundColor Yellow
            Write-Host "  Install edge-tts (pip install edge-tts) to auto-generate, or create it manually." -ForegroundColor DarkGray
        }
    }
    else {
        Write-Host "  Repo audio already exists: $audioFile" -ForegroundColor DarkGray
    }

    # --- Summary ---
    Write-Host ""
    Write-Host "Done. Installed: $installed  Overwritten: $overwritten  Skipped: $skipped" -ForegroundColor Cyan
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
$subPrefix = (git rev-parse --show-prefix 2>$null).Trim().TrimEnd('/')

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

# --- Save original directory and resolve prompt paths before entering worktree ---
$originalDir = (Get-Location).Path

function Play-Audio([string]$FileName) {
    if ($script:NoAudio) { return }
    $messagePath = Join-Path $PSScriptRoot "assets" $FileName
    if (-not (Test-Path $messagePath)) { return }
    $ffplay = Get-Command ffplay -ErrorAction SilentlyContinue
    if (-not $ffplay) { return }
    try {
        # Play repo identifier first (synchronous) so the user knows the source
        if ($script:RepoAudioFile -and (Test-Path $script:RepoAudioFile)) {
            Start-Process -FilePath $ffplay.Source -ArgumentList "-nodisp", "-autoexit", "-loglevel", "quiet", $script:RepoAudioFile -WindowStyle Hidden -Wait
        }
        # Then play the actual message (also synchronous — all audio must finish before script continues)
        Start-Process -FilePath $ffplay.Source -ArgumentList "-nodisp", "-autoexit", "-loglevel", "quiet", $messagePath -WindowStyle Hidden -Wait
    }
    catch {
        Write-Warning "Failed to play audio '$FileName': $($_.Exception.Message)"
    }
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

function Resolve-PromptText {
    if ($Prompt) {
        $resolvedPath = if ([System.IO.Path]::IsPathRooted($Prompt)) { $Prompt } else { Join-Path $originalDir $Prompt }
        if (Test-Path $resolvedPath) {
            $script:promptSource = $resolvedPath
            return Get-Content $resolvedPath -Raw
        }
        else {
            $script:promptSource = "command-line string"
            return $Prompt
        }
    }
    else {
        $script:promptSource = "default"
        return $null
    }
}

# --- Resolve branch once before the loop ---
$hash = -join ((0..7) | ForEach-Object { '{0:x}' -f (Get-Random -Max 16) })
if (-not $Branch) {
    $Branch = "feature/ralph-$hash"
}
elseif ($Branch -notmatch '-[0-9a-f]{8}$') {
    $Branch = "$Branch-$hash"
}

# --- Derive human-readable log tag from branch name ---
$logTag = $Branch -replace '^feature/', '' -replace '-[0-9a-f]{8}$', ''

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

# --- Resolve effective Max ---
if ($Once) {
    $Max = 1
}
elseif ($Max -le 0 -and -not $WorkUntil) {
    $Max = 1
}
elseif ($Max -le 0 -and $WorkUntil) {
    $Max = [int]::MaxValue
}

# --- Set up worktree ---
$worktreeBase = Join-Path $repoParent "$repoName.worktrees"
$safeBranch = $Branch -replace '[/\\]', '-'
$worktreeDir = Join-Path $worktreeBase $safeBranch
$createdWorktree = $false

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
        if ($currentWtBranch -eq $Branch) {
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
    Write-Host "Branch '$Branch' is already checked out in a worktree at: $attachedPath" -ForegroundColor Red
    Write-Host "   Remove it first: git worktree remove '$attachedPath'" -ForegroundColor DarkGray
    exit 1
}

if ($worktreeExists) {
    Write-Host "Reusing existing worktree at: $worktreeDir" -ForegroundColor Cyan
}
else {
    if (-not (Test-Path $worktreeBase)) {
        New-Item -ItemType Directory -Path $worktreeBase -Force | Out-Null
    }
    $branchExists = git rev-parse --verify "refs/heads/$Branch" 2>$null
    if ($LASTEXITCODE -eq 0) {
        git worktree add $worktreeDir $Branch 2>&1
    }
    else {
        git worktree add -b $Branch $worktreeDir HEAD 2>&1
    }
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Failed to create worktree for branch '$Branch' at: $worktreeDir" -ForegroundColor Red
        exit 1
    }
    $createdWorktree = $true
    Write-Host "Created worktree at: $worktreeDir" -ForegroundColor Green
}

# Target directory preserves subdirectory offset within the worktree
$targetDir = if ($subPrefix) { Join-Path $worktreeDir $subPrefix } else { $worktreeDir }

# Absolute log path in original directory
$logFile = Join-Path $originalDir "ralph.log"

$maxLabel = if ($Max -eq [int]::MaxValue) { "unlimited" } else { "$Max" }

$runStart = Get-Date
$runStartStr = $runStart.ToString("yyyy-MM-dd HH:mm:ss")
$deadlineStr = if ($deadline) { ", deadline: $($deadline.ToString('yyyy-MM-dd HH:mm'))" } else { "" }
$deadlineDisplay = if ($deadline) { $deadline.ToString('yyyy-MM-dd HH:mm') } else { "(none)" }
$promptDisplay = if ($Prompt) {
    $resolvedPath = if ([System.IO.Path]::IsPathRooted($Prompt)) { $Prompt } else { Join-Path $originalDir $Prompt }
    if (Test-Path $resolvedPath) { "$Prompt (file: $resolvedPath)" } else { "$Prompt (literal)" }
} else { "(default)" }

# Brief content preview of prompt (first line, max 100 chars) for compact headers
$promptBrief = if ($Prompt) {
    $resolvedPath = if ([System.IO.Path]::IsPathRooted($Prompt)) { $Prompt } else { Join-Path $originalDir $Prompt }
    if (Test-Path $resolvedPath) {
        $firstLine = (Get-Content $resolvedPath -TotalCount 1).Trim()
        if ($firstLine.Length -gt 100) { $firstLine.Substring(0, 97) + "..." } else { $firstLine }
    } else {
        if ($Prompt.Length -gt 100) { $Prompt.Substring(0, 97) + "..." } else { $Prompt }
    }
} else { "Improve test coverage" }

Write-Host ""
Write-Host "====================================" -ForegroundColor Cyan
Write-Host "== Ralph - Iterative Autopilot" -ForegroundColor Cyan
Write-Host "====================================" -ForegroundColor Cyan
Write-Host "  Started:         $runStartStr"
Write-Host "  PID:             $PID"
Write-Host "  Host:            $([Environment]::MachineName)"
Write-Host "  Invoked from:    $originalDir"
Write-Host "  Branch:          $Branch"
Write-Host "  Worktree:        $worktreeDir"
Write-Host "  Max iterations:  $maxLabel"
Write-Host "  WorkUntil:       $deadlineDisplay"
Write-Host "  Prompt:          $promptDisplay"
Write-Host "  Provider:        $script:ProviderName ($($script:ProviderConfig.Command))"
Write-Host "  Model:           $script:ModelId ($script:ModelLabel)"
$devAgentDisplay = "$script:DevAgentRole ($script:DevAgentName)"
$reviewDisplay = if ($reviewAgents.Count -gt 0) { $reviewAgents -join ', ' } else { '(none)' }
Write-Host "  Dev agent:       $devAgentDisplay"
Write-Host "  Review agents:   $reviewDisplay"
Write-Host "  Flags:           NoPR=$NoPR CleanupWorktree=$CleanupWorktree NoAudio=$NoAudio"
Write-Host "====================================" -ForegroundColor Cyan

$startLogBlock = @"
[$logTag] Ralph run started at $runStartStr ($maxLabel iterations, branch: $Branch, worktree: $worktreeDir$deadlineStr)
  PID:              $PID
  Host:             $([Environment]::MachineName)
  Script:           $PSCommandPath
  Invoked from:     $originalDir
  Branch:           $Branch
  Worktree:         $worktreeDir
  Max iterations:   $maxLabel
  WorkUntil:        $deadlineDisplay
  Prompt:           $promptDisplay
  Provider:         $script:ProviderName ($($script:ProviderConfig.Command))
  Model:            $script:ModelId ($script:ModelLabel)
  Dev agent:        $devAgentDisplay
  Review agents:    $reviewDisplay
  NoPR:             $NoPR
  CleanupWorktree:  $CleanupWorktree
  NoAudio:          $NoAudio
"@
$startLogBlock | Out-File $logFile -Append -Encoding UTF8

# --- Summary log (centralized cross-repo log) ---
$script:SummaryLogFile = Join-Path $PSScriptRoot "ralph-summary.log"
"$(Get-Date -Format 'yyyy-MM-dd HH:mm') -- ${repoName}: Ralph loop started: $promptBrief" | Out-File $script:SummaryLogFile -Append -Encoding UTF8

# --- Enter worktree ---
Push-Location $targetDir

try {
    $completedIterations = 0
    $copilotInvocations = 0
    $totalPremiumRequests = 0
    $totalTokensIn = 0.0
    $totalTokensOut = 0.0
    $totalTokensCached = 0.0
    $noChangeCount = 0
    $earlyExitReason = $null
    Write-Host "  (Press Q or Escape between iterations to exit early)" -ForegroundColor DarkGray
    for ($i = 1; $i -le $Max; $i++) {
        if ($deadline -and (Get-Date) -ge $deadline) {
            Write-Host ""
            Write-Host "Deadline reached ($($deadline.ToString('HH:mm'))). Stopping." -ForegroundColor Yellow
            break
        }

        # --- Early exit: hotkey check (Q or Escape pressed between iterations) ---
        # Skip when no console is attached (e.g. launched from hs-buddy with piped stdio)
        $hasConsole = try { [Console]::KeyAvailable; $true } catch { $false }
        if ($hasConsole) {
        while ([Console]::KeyAvailable) {
            $key = [Console]::ReadKey($true)
            if ($key.Key -eq 'Q' -or $key.Key -eq 'Escape') {
                $earlyExitReason = "User pressed $($key.Key)"
                break
            }
        }
        }
        if ($earlyExitReason) {
            Write-Host ""
            Write-Host "** Early exit: $earlyExitReason **" -ForegroundColor Yellow
            "[$logTag] EARLY EXIT: $earlyExitReason" | Out-File $logFile -Append -Encoding UTF8
            break
        }

        $iterStart = Get-Date
        $elapsed = $iterStart - $runStart
        $elapsedStr = "{0:hh\:mm\:ss}" -f $elapsed

        Write-Host ""
        Write-Host "===================================="
        Write-Host "== Iteration $i/$maxLabel"
        Write-Host "== Branch: $Branch"
        Write-Host "== Prompt: $promptBrief"
        Write-Host "== Started: $($iterStart.ToString('HH:mm:ss'))  |  Elapsed: $elapsedStr"
        Write-Host "===================================="

        Play-Audio "ralph-processing.mp3"

        $promptText = Resolve-PromptText
        if ($i -eq 1) { Write-Host "Prompt source: $promptSource" }

        # Prepend worktree context so Copilot knows where it is
        $worktreeContext = @"
WORKTREE CONTEXT: You are already inside the correct git worktree for branch ``$Branch``.
Working directory: $(Get-Location)
IMPORTANT: Do NOT run ``git switch``, ``git checkout``, or create another worktree.
Work directly in this directory. Make changes, test locally, commit on the current branch, and push when finished.
COMPLETION SIGNAL: If you determine the task goal has been fully achieved and no further improvements can be made, include the exact marker RALPH_TASK_COMPLETE on its own line in your final output.

"@

        # Iteration 2+: continue previous session with a shorter prompt
        $useContinue = $i -gt 1
        if ($useContinue) {
            $continuationPrompt = @"
${worktreeContext}Continue from where you left off. Check ``git log --oneline -5`` to see your prior commits.
Build on your previous work — do NOT repeat discovery you already did.
Original goal: $promptBrief
"@
            $effectivePrompt = $continuationPrompt
            Write-Host "  (continuing previous session)" -ForegroundColor DarkGray
        }
        elseif ($promptText) {
            $effectivePrompt = "${worktreeContext}${promptText}"
        }
        else {
            $effectivePrompt = "${worktreeContext}Improve test coverage. Make changes, run tests, commit, and push."
        }

        $headBefore = git rev-parse HEAD 2>$null
        $tempOut = [System.IO.Path]::GetTempFileName()
        $copilotInvocations++
        $buildArgs = @{
            ModelId  = $script:ModelId
            Agent    = $script:DevAgentName
            Prompt   = $effectivePrompt
            Yolo     = $true
            Provider = $script:ProviderName
        }
        if ($useContinue) { $buildArgs['Continue'] = $true }
        $cmdArgs = Build-RalphCommand @buildArgs
        $cmdExe = $cmdArgs[0]; $cmdTail = $cmdArgs[1..($cmdArgs.Count-1)]
        & $cmdExe @cmdTail 2>&1 | Tee-Object -FilePath $tempOut

        $iterEnd = Get-Date
        $iterDuration = $iterEnd - $iterStart
        $iterDurStr = "{0:hh\:mm\:ss}" -f $iterDuration
        $totalElapsed = $iterEnd - $runStart
        $totalStr = "{0:hh\:mm\:ss}" -f $totalElapsed

        $captured = Get-Content $tempOut -Encoding UTF8
        $statsLines = $captured | Select-String -Pattern '^\s*(Changes|Requests|Tokens)\s' | ForEach-Object { $_.Line.Trim() }
        $statsBlock = if ($statsLines) { ($statsLines -join "`n") } else { "(no stats captured)" }
        Remove-Item $tempOut -ErrorAction SilentlyContinue

        # Parse premium request count and token stats
        $iterPremium = 0
        foreach ($stat in $statsLines) {
            if ($stat -match '(\d+)\s+Premium') {
                $iterPremium = [int]$Matches[1]
            }
            elseif ($stat -match '^Tokens\s+(.+)') {
                $tokenText = $Matches[1].Trim()
                $tokenParts = $tokenText -split ([char]0x2022) | ForEach-Object { $_.Trim() }
                if ($tokenParts.Count -ge 3) {
                    $totalTokensIn += ConvertFrom-TokenString ($tokenParts[0] -replace '[^\d.,kM]', '')
                    $totalTokensOut += ConvertFrom-TokenString ($tokenParts[1] -replace '[^\d.,kM]', '')
                    $totalTokensCached += ConvertFrom-TokenString ($tokenParts[2] -replace '[^\d.,kM]', '')
                }
            }
        }
        $totalPremiumRequests += $iterPremium

        Write-Host "------------------------------------"
        Write-Host "== Iteration $i/$maxLabel completed in $iterDurStr  |  Total: $totalStr"
        Write-Host "------------------------------------"

        $logEntry = @"

[$logTag] ==== Iteration $i/$maxLabel ====
Prompt:   $promptBrief
Started:  $($iterStart.ToString('yyyy-MM-dd HH:mm:ss'))
Finished: $($iterEnd.ToString('yyyy-MM-dd HH:mm:ss'))
Duration: $iterDurStr
Total:    $totalStr
$statsBlock
"@
        $logEntry | Out-File $logFile -Append -Encoding UTF8
        $completedIterations = $i

        # --- Early exit: sentinel marker in copilot output ---
        if ($captured -match 'RALPH_TASK_COMPLETE') {
            $earlyExitReason = "Task marked complete by Copilot (RALPH_TASK_COMPLETE)"
            Write-Host ""
            Write-Host "** Early exit: $earlyExitReason **" -ForegroundColor Green
            "[$logTag] EARLY EXIT: $earlyExitReason" | Out-File $logFile -Append -Encoding UTF8
            break
        }

        # --- Early exit: no-change detection ---
        $headAfter = git rev-parse HEAD 2>$null
        if ($headBefore -eq $headAfter) {
            $noChangeCount++
            Write-Host "  No changes detected ($noChangeCount consecutive)." -ForegroundColor DarkYellow
            if ($noChangeCount -ge 2) {
                $earlyExitReason = "No changes for $noChangeCount consecutive iterations"
                Write-Host ""
                Write-Host "** Early exit: $earlyExitReason **" -ForegroundColor Yellow
                "[$logTag] EARLY EXIT: $earlyExitReason" | Out-File $logFile -Append -Encoding UTF8
                break
            }
        }
        else {
            $noChangeCount = 0
        }
    }
}
finally {
    Pop-Location
}

# --- PR creation and handoff to ralph-pr ---
$handedOff = $false
if (-not $NoPR) {
    # Ensure branch is pushed
    Write-Host ""
    Write-Host "Pushing branch '$Branch' to origin..." -ForegroundColor Cyan
    git -C $worktreeDir push origin $Branch 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Push failed or nothing to push. Skipping PR creation." -ForegroundColor Yellow
    }
    else {
        # Resolve repo slug
        $repoSlug = gh repo view --json nameWithOwner -q '.nameWithOwner' 2>$null
        if (-not $repoSlug) {
            $remoteUrl = git --no-pager remote get-url origin 2>$null
            if ($remoteUrl -match '[:/]([^/]+/[^/]+?)(?:\.git)?$') {
                $repoSlug = $Matches[1]
            }
        }

        if ($repoSlug) {
            # Match gh account to repo owner (same fix as ralph-pr.ps1)
            $rsOwner = ($repoSlug -split '/')[0]
            $rsActiveUser = gh api user --jq '.login' 2>$null
            if ($rsActiveUser -and $rsActiveUser -ne $rsOwner) {
                $rsToken = gh auth token --user $rsOwner 2>$null
                if ($rsToken) {
                    $env:GH_TOKEN = $rsToken
                    Write-Host "  Using gh token for $rsOwner (repo owner)" -ForegroundColor DarkGray
                }
            }

            # Check for existing PR on this branch
            $existingPRNum = gh pr view $Branch --repo $repoSlug --json number -q '.number' 2>$null
            $newPRNumber = $null

            if ($existingPRNum -and $LASTEXITCODE -eq 0) {
                $newPRNumber = [int]$existingPRNum
                Write-Host "PR #$newPRNumber already exists for branch '$Branch'." -ForegroundColor Cyan
            }
            else {
                Write-Host "Creating PR for branch '$Branch'..." -ForegroundColor Cyan
                $prOutput = gh pr create --repo $repoSlug --head $Branch --fill 2>&1
                if ($LASTEXITCODE -eq 0) {
                    if ($prOutput -match '/pull/(\d+)') {
                        $newPRNumber = [int]$Matches[1]
                    }
                    Write-Host "Created PR #$newPRNumber" -ForegroundColor Green
                }
                else {
                    Write-Host "Failed to create PR: $prOutput" -ForegroundColor Yellow
                }
            }

            if ($newPRNumber) {
                "[$logTag] PR #$newPRNumber created/found for branch $Branch" | Out-File $logFile -Append -Encoding UTF8

                Write-Host "Waiting 30 seconds for PR to be fully available..." -ForegroundColor DarkGray
                Start-Sleep -Seconds 30

                $prScriptPath = Join-Path $PSScriptRoot "ralph-pr.ps1"
                $prStatsPath = Join-Path ([IO.Path]::GetTempPath()) "ralph-pr-stats-$([guid]::NewGuid().ToString('N').Substring(0,8)).json"
                $prArgs = @{ PRNumber = $newPRNumber; Model = $script:ModelId; OriginalPrompt = $promptBrief; DevAgent = $script:DevAgentRole; Provider = $Provider; StatsPath = $prStatsPath }
                if ($WorkUntil) { $prArgs['WorkUntil'] = $WorkUntil }
                if ($NoAudio) { $prArgs['NoAudio'] = $true }
                if ($Autopilot) { $prArgs['Autopilot'] = $true }
                if ($SkipReview) { $prArgs['SkipReview'] = $true }
                if ($MergeWhenClean) {
                    $prArgs['AutoApprove'] = $true
                    $prArgs['RequireMerge'] = $true
                }
                if ($reviewAgents.Count -gt 0) { $prArgs['Agents'] = $reviewAgents }

                Write-Host ""
                Write-Host "===================================="
                Write-Host "Handing off to ralph-pr.ps1 for PR #$newPRNumber..."
                Write-Host "===================================="

                Play-Audio "ralph-processing.mp3"
                & $prScriptPath @prArgs
                $prExitCode = $LASTEXITCODE
                $handedOff = $true

                # Import PR phase stats for summary rollup
                $prStats = $null
                if (Test-Path $prStatsPath) {
                    try { $prStats = Get-Content $prStatsPath -Raw | ConvertFrom-Json }
                    catch { Write-Warning "Failed to read PR stats from $prStatsPath" }
                    Remove-Item $prStatsPath -Force -ErrorAction SilentlyContinue
                }
            }
        }
        else {
            Write-Host "Could not resolve repo slug. Skipping PR creation." -ForegroundColor Yellow
        }
    }
}

# Cleanup worktree if requested (only if we created it, it is clean, and we didn't hand off)
if ($CleanupWorktree -and $createdWorktree -and -not $handedOff) {
    $wtStatus = git -C $worktreeDir status --porcelain 2>&1
    if ($wtStatus) {
        Write-Host "Worktree has uncommitted changes - skipping cleanup." -ForegroundColor Yellow
    }
    else {
        git worktree remove $worktreeDir 2>&1
        if ($LASTEXITCODE -eq 0) {
            Write-Host "Worktree removed: $worktreeDir" -ForegroundColor Cyan
        }
        else {
            Write-Host "Failed to remove worktree: $worktreeDir" -ForegroundColor Yellow
        }
    }
}

$endTime = Get-Date
$totalDuration = $endTime - $runStart
$totalDurStr = "{0:hh\:mm\:ss}" -f $totalDuration

$costPerRequest = [math]::Round(0.04 * $script:ModelMultiplier, 4)
$costTotal = [math]::Round($totalPremiumRequests * $costPerRequest, 2)

# Compute grand totals including PR phase
$grandPremium = $totalPremiumRequests
$grandCost = $costTotal
if ($prStats) {
    $grandPremium += [int]$prStats.prPhasePremium
    $grandCost = [math]::Round($grandCost + [double]$prStats.prPhaseCost, 2)
}

$grandTokensIn = $totalTokensIn
$grandTokensOut = $totalTokensOut
$grandTokensCached = $totalTokensCached
if ($prStats) {
    $grandTokensIn += [double]$prStats.tokensIn
    $grandTokensOut += [double]$prStats.tokensOut
    $grandTokensCached += [double]$prStats.tokensCached
}

$summary = @"

[$logTag] ====================================
[$logTag] RALPH RUN SUMMARY
[$logTag] ====================================
[$logTag]   Finished:          $($endTime.ToString('yyyy-MM-dd HH:mm:ss'))
[$logTag]   Total duration:    $totalDurStr
[$logTag]   Iterations:        $completedIterations / $maxLabel
[$logTag]   Copilot invocations: $copilotInvocations
[$logTag]   Premium requests:  $totalPremiumRequests
[$logTag]   Provider:          $script:ProviderName ($($script:ProviderConfig.Command))
[$logTag]   Model:             $script:ModelId ($script:ModelLabel)
[$logTag]   Est. cost:         `$$costTotal  ($totalPremiumRequests x `$$costPerRequest)
[$logTag]   Prompt:            $promptBrief
[$logTag]   Branch:            $Branch
[$logTag]   Worktree:          $worktreeDir
"@
if ($handedOff) {
    $summary += "`n[$logTag]   PR:                #$newPRNumber (handed off to ralph-pr)"
}
if ($prStats) {
    $summary += "`n[$logTag]   PR phase premium:  $([int]$prStats.prPhasePremium)"
    $summary += "`n[$logTag]   PR phase cost:     `$$([math]::Round([double]$prStats.prPhaseCost, 2))"
    $summary += "`n[$logTag]   ---"
    $summary += "`n[$logTag]   Grand total premium: $grandPremium"
    $summary += "`n[$logTag]   Grand total cost:  `$$grandCost"
}
if ($earlyExitReason) {
    $summary += "`n[$logTag]   Early exit:        $earlyExitReason"
}
$summary += "`n[$logTag] ===================================="

Write-Host ""
Write-Host "====================================" -ForegroundColor Cyan
Write-Host "  RALPH RUN SUMMARY" -ForegroundColor Cyan
Write-Host "====================================" -ForegroundColor Cyan
Write-Host "  Finished:            $($endTime.ToString('yyyy-MM-dd HH:mm:ss'))"
Write-Host "  Total duration:      $totalDurStr"
Write-Host "  Iterations:          $completedIterations / $maxLabel"
Write-Host "  Copilot invocations: $copilotInvocations"
Write-Host "  Premium requests:    $totalPremiumRequests"
Write-Host "  Provider:            $script:ProviderName ($($script:ProviderConfig.Command))" -ForegroundColor Cyan
Write-Host "  Model:               $script:ModelId ($script:ModelLabel)" -ForegroundColor Cyan
Write-Host "  Est. cost:           `$$costTotal  ($totalPremiumRequests x `$$costPerRequest)" -ForegroundColor Yellow
Write-Host "  Prompt:              $promptBrief"
Write-Host "  Branch:              $Branch"
Write-Host "  Worktree:            $worktreeDir"
if ($handedOff) {
    Write-Host "  PR:                  #$newPRNumber (handed off to ralph-pr)"
}
if ($prStats) {
    Write-Host "  PR phase premium:    $([int]$prStats.prPhasePremium)" -ForegroundColor DarkGray
    Write-Host "  PR phase cost:       `$$([math]::Round([double]$prStats.prPhaseCost, 2))" -ForegroundColor DarkGray
    Write-Host "  ---"
    Write-Host "  Grand total premium: $grandPremium" -ForegroundColor White
    Write-Host "  Grand total cost:    `$$grandCost" -ForegroundColor Yellow
}
if ($earlyExitReason) {
    Write-Host "  Early exit:          $earlyExitReason" -ForegroundColor Yellow
}
Write-Host "====================================" -ForegroundColor Cyan

$summary | Out-File $logFile -Append -Encoding UTF8

# --- Write stats file for parent rollup ---
if ($StatsPath) {
    @{
        premiumRequests = $grandPremium
        estimatedCost = $grandCost
        tokenCost = $grandCost
        tokensIn = $grandTokensIn
        tokensOut = $grandTokensOut
        tokensCached = $grandTokensCached
        modelId = $script:ModelId
        provider = $script:ProviderName
        iterations = $completedIterations
        copilotInvocations = $copilotInvocations
        prNumber = $newPRNumber
        handedOff = $handedOff
    } | ConvertTo-Json | Set-Content -Path $StatsPath -Encoding UTF8
}

# --- Summary log entry ---
$dot = [char]0x2022
$tokInStr = ConvertTo-TokenString $totalTokensIn
$tokOutStr = ConvertTo-TokenString $totalTokensOut
$tokCachedStr = ConvertTo-TokenString $totalTokensCached
$exitNote = if ($earlyExitReason) { " Early exit: $earlyExitReason." } else { "" }
"$($endTime.ToString('yyyy-MM-dd HH:mm')) -- ${repoName}: Ralph loop ended. $completedIterations turns. Total duration $totalDurStr. Total tokens: $tokInStr in $dot $tokOutStr out $dot $tokCachedStr cached.$exitNote $promptBrief." | Out-File $script:SummaryLogFile -Append -Encoding UTF8

# --- Completion audio ---
if (-not $handedOff) {
    Play-Audio "ralph-loop-completed.mp3"
}

# --- Restore BYOK environment variables ---
if ($script:SavedByokEnv -and $script:SavedByokEnv.Count -gt 0) {
    Restore-RalphProviderEnv -Saved $script:SavedByokEnv
}

# --- Exit with PR phase exit code if applicable, otherwise success ---
if ($handedOff -and $prExitCode) {
    exit $prExitCode
}
exit 0
