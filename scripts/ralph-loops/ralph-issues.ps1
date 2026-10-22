# ralph-issues.ps1 — Iterative codebase scanner that creates GitHub Issues.
# Version: 1.2.0
param(
    [int]$Max = 0,
    [string]$Prompt,
    [string[]]$Labels,
    [string]$WorkUntil,
    [string]$Model,
    [string]$Provider,
    [string[]]$Agents,
    [switch]$Once,
    [switch]$DryRun,
    [switch]$NoAudio,
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

# Validate and resolve dev agent
$script:DevAgentRole = $null
$script:DevAgentName = $null

if ($Agents) {
    $Agents = @($Agents | Select-Object -Unique)

    $parsedSpecs = @()
    foreach ($spec in $Agents) {
        try {
            $parsedSpecs += Parse-RalphAgentSpec -Spec $spec -Provider $Provider
        } catch {
            Write-Host $_.Exception.Message -ForegroundColor Red
            exit 1
        }
    }

    # Reject model overrides on dev agents (use -Model flag instead)
    $devWithOverride = @($parsedSpecs | Where-Object { $_.Category -eq 'dev' -and $_.ModelOverride })
    if ($devWithOverride.Count -gt 0) {
        Write-Host "Model override (@model) not supported for dev agents. Use -Model flag instead." -ForegroundColor Red
        exit 1
    }

    $devSpecs = @($parsedSpecs | Where-Object { $_.Category -eq 'dev' })

    if ($devSpecs.Count -gt 1) {
        Write-Host "Only one dev agent allowed at a time. Got: $($devSpecs.Role -join ', ')" -ForegroundColor Red
        exit 1
    }
    if ($devSpecs.Count -eq 1) {
        $script:DevAgentRole = $devSpecs[0].Role
        $script:DevAgentName = $devSpecs[0].Agent
    }

    # Warn about review agents (not used in issues mode)
    $reviewSpecs = @($parsedSpecs | Where-Object { $_.Category -eq 'review' })
    if ($reviewSpecs.Count -gt 0) {
        Write-Host "Review agents are ignored in issues mode (no PR created): $($reviewSpecs.Role -join ', ')" -ForegroundColor Yellow
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
    Write-Host "ralph-issues.ps1 — Iterative Codebase Scanner → GitHub Issues" -ForegroundColor Cyan
    Write-Host "Scans a codebase in a loop, finds improvements, and creates GitHub Issues."
    Write-Host "Checks for existing open issues before creating duplicates."
    Write-Host ""
    Write-Host "PARAMETERS" -ForegroundColor Yellow
    Write-Host "  -Max <int>             Number of scan iterations (default: 1, or unlimited with -WorkUntil)"
    Write-Host "  -Once                  Run a single scan iteration (shortcut for -Max 1)"
    Write-Host "  -Prompt <string>       Scan prompt (file path or literal text)"
    Write-Host "  -Labels <strings>      Labels to apply to created issues (comma-separated)"
    Write-Host "  -Model <name>          Model to use (validated against models.json)"
    Write-Host "  -Provider <name>       CLI provider: $((Get-RalphProviderNames) -join ', ') (default: $(Get-RalphDefaultProvider))"
    Write-Host "  -Agents <specs>        Dev agent spec (only dev agents used; review agents ignored)"
    Write-Host "  -WorkUntil <HH:mm>     Stop after this local time (e.g. -WorkUntil 08:00)"
    Write-Host "  -DryRun                Scan and report findings but do NOT create issues"
    Write-Host "  -NoAudio               Suppress audio feedback"
    Write-Host "  -Help                  Show this help message"
    Write-Host ""
    Write-Host "WORKFLOW" -ForegroundColor Yellow
    Write-Host "  1. Scans the codebase (default branch, no worktree needed)"
    Write-Host "  2. Identifies issues, improvements, or violations"
    Write-Host "  3. Searches existing open issues for duplicates (gh issue list --search)"
    Write-Host "  4. Creates new GitHub Issues for unique findings (gh issue create)"
    Write-Host "  5. Repeats until -Max iterations or no new issues found"
    Write-Host ""
    Write-Host "EXAMPLES" -ForegroundColor Yellow
    Write-Host "  ralph-issues"
    Write-Host "  ralph-issues -Prompt 'Find security vulnerabilities and create issues'"
    Write-Host "  ralph-issues -Labels 'tech-debt,automated' -Max 3"
    Write-Host "  ralph-issues -Prompt prompts/scan-quality.md -WorkUntil 08:00"
    Write-Host "  ralph-issues -DryRun -Prompt 'Find all TODO/FIXME comments'"
    Write-Host "  ralph-issues -Agents developer-principal -Model opus"
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

# --- Validate gh CLI is available ---
$ghCmd = Get-Command gh -ErrorAction SilentlyContinue
if (-not $ghCmd) {
    Write-Host "GitHub CLI (gh) not found. Install it: https://cli.github.com/" -ForegroundColor Red
    exit 1
}

# --- Resolve repo slug for issue tracking ---
$repoSlug = gh repo view --json nameWithOwner -q '.nameWithOwner' 2>$null
if (-not $repoSlug) {
    $remoteUrl = git --no-pager remote get-url origin 2>$null
    if ($remoteUrl -match '[:/]([^/]+/[^/]+?)(?:\.git)?$') {
        $repoSlug = $Matches[1]
    }
}
if (-not $repoSlug) {
    Write-Host "Cannot determine repo slug. Ensure 'origin' remote is configured." -ForegroundColor Red
    exit 1
}

# Match gh account to repo owner
$rsOwner = ($repoSlug -split '/')[0]
# Buddy Native runs a loop as the account its GitHub view showed: RALPH_GH_ACCOUNT names
# it and GH_TOKEN holds its token. That account is kept, never traded for the owner's.
$rsActiveUser = gh api user --jq '.login' 2>$null
if (-not $env:RALPH_GH_ACCOUNT -and $rsActiveUser -and $rsActiveUser -ne $rsOwner) {
    $rsToken = gh auth token --user $rsOwner 2>$null
    if ($rsToken) {
        $env:GH_TOKEN = $rsToken
    }
}

# --- Repo audio identification ---
$script:NoAudio = $NoAudio
$script:RepoAudioFile = Join-Path $repoRoot "assets" "ralph-loop" "$repoName.mp3"

function Play-Audio([string]$FileName) {
    if ($script:NoAudio) { return }
    $messagePath = Join-Path $PSScriptRoot "assets" $FileName
    if (-not (Test-Path $messagePath)) { return }
    $ffplay = Get-Command ffplay -ErrorAction SilentlyContinue
    if (-not $ffplay) { return }
    try {
        if ($script:RepoAudioFile -and (Test-Path $script:RepoAudioFile)) {
            Start-Process -FilePath $ffplay.Source -ArgumentList "-nodisp", "-autoexit", "-loglevel", "quiet", $script:RepoAudioFile -WindowStyle Hidden -Wait
        }
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

# --- Save original directory and resolve prompt paths ---
$originalDir = (Get-Location).Path

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

# --- Build label args for prompt ---
$labelClause = ""
if ($Labels) {
    $labelList = ($Labels | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ }) -join ','
    $labelClause = " Apply these labels to every issue you create: $labelList (use --label flag with gh issue create)."
}

$dryRunClause = ""
if ($DryRun) {
    $dryRunClause = " DRY RUN MODE: List all findings you would create as issues (title + body), but do NOT actually run 'gh issue create'. Just report what you found."
}

# --- Logging ---
$logFile = Join-Path $originalDir "ralph-issues.log"
$maxLabel = if ($Max -eq [int]::MaxValue) { "unlimited" } else { "$Max" }
$runStart = Get-Date
$runStartStr = $runStart.ToString("yyyy-MM-dd HH:mm:ss")
$deadlineStr = if ($deadline) { ", deadline: $($deadline.ToString('yyyy-MM-dd HH:mm'))" } else { "" }
$deadlineDisplay = if ($deadline) { $deadline.ToString('yyyy-MM-dd HH:mm') } else { "(none)" }
$promptDisplay = if ($Prompt) {
    $resolvedPath = if ([System.IO.Path]::IsPathRooted($Prompt)) { $Prompt } else { Join-Path $originalDir $Prompt }
    if (Test-Path $resolvedPath) { "$Prompt (file: $resolvedPath)" } else { "$Prompt (literal)" }
} else { "(default scan)" }

# Brief content preview of prompt
$promptBrief = if ($Prompt) {
    $resolvedPath = if ([System.IO.Path]::IsPathRooted($Prompt)) { $Prompt } else { Join-Path $originalDir $Prompt }
    if (Test-Path $resolvedPath) {
        $firstLine = (Get-Content $resolvedPath -TotalCount 1).Trim()
        if ($firstLine.Length -gt 100) { $firstLine.Substring(0, 97) + "..." } else { $firstLine }
    } else {
        if ($Prompt.Length -gt 100) { $Prompt.Substring(0, 97) + "..." } else { $Prompt }
    }
} else { "Scan codebase and create improvement issues" }

Write-Host ""
Write-Host "====================================" -ForegroundColor Cyan
Write-Host "== Ralph Issues - Codebase Scanner" -ForegroundColor Cyan
Write-Host "====================================" -ForegroundColor Cyan
Write-Host "  Started:         $runStartStr"
Write-Host "  PID:             $PID"
Write-Host "  Host:            $([Environment]::MachineName)"
Write-Host "  Repo:            $repoSlug"
Write-Host "  Directory:       $originalDir"
Write-Host "  Max iterations:  $maxLabel"
Write-Host "  WorkUntil:       $deadlineDisplay"
Write-Host "  Prompt:          $promptDisplay"
Write-Host "  Labels:          $(if ($Labels) { $Labels -join ', ' } else { '(none)' })"
Write-Host "  Provider:        $script:ProviderName ($($script:ProviderConfig.Command))"
Write-Host "  Model:           $script:ModelId ($script:ModelLabel)"
Write-Host "  Dev agent:       $script:DevAgentRole ($script:DevAgentName)"
Write-Host "  DryRun:          $DryRun"
Write-Host "====================================" -ForegroundColor Cyan

$startLogBlock = @"
[issues] Ralph Issues scan started at $runStartStr ($maxLabel iterations$deadlineStr)
  PID:              $PID
  Host:             $([Environment]::MachineName)
  Repo:             $repoSlug
  Directory:        $originalDir
  Max iterations:   $maxLabel
  WorkUntil:        $deadlineDisplay
  Prompt:           $promptDisplay
  Labels:           $(if ($Labels) { $Labels -join ', ' } else { '(none)' })
  Provider:         $script:ProviderName ($($script:ProviderConfig.Command))
  Model:            $script:ModelId ($script:ModelLabel)
  Dev agent:        $script:DevAgentRole ($script:DevAgentName)
  DryRun:           $DryRun
"@
$startLogBlock | Out-File $logFile -Append -Encoding UTF8

# --- Summary log (centralized cross-repo log) ---
$script:SummaryLogFile = Join-Path $PSScriptRoot "ralph-summary.log"
"$(Get-Date -Format 'yyyy-MM-dd HH:mm') -- ${repoName}: Ralph Issues scan started: $promptBrief" | Out-File $script:SummaryLogFile -Append -Encoding UTF8

# --- Count open issues (for change detection) ---
function Get-OpenIssueCount {
    $count = gh issue list --repo $repoSlug --state open --limit 1 --json number -q 'length' 2>$null
    if ($LASTEXITCODE -eq 0 -and $count -match '^\d+$') { return [int]$count }
    # Fallback: count with higher limit
    $issues = gh issue list --repo $repoSlug --state open --limit 500 --json number -q 'length' 2>$null
    if ($LASTEXITCODE -eq 0 -and $issues -match '^\d+$') { return [int]$issues }
    return -1
}

# --- Main loop ---
$completedIterations = 0
$copilotInvocations = 0
# Detect console availability once — false when launched from hs-buddy with piped stdio
$script:HasConsole = try { [Console]::KeyAvailable; $true } catch { $false }
$agentFailures = 0
$totalPremiumRequests = 0
$totalTokensIn = 0.0
$totalTokensOut = 0.0
$totalTokensCached = 0.0
$noNewIssueCount = 0
$totalIssuesCreated = 0
$earlyExitReason = $null

Write-Host "  (Press Q or Escape between iterations to exit early)" -ForegroundColor DarkGray

for ($i = 1; $i -le $Max; $i++) {
    if ($deadline -and (Get-Date) -ge $deadline) {
        Write-Host ""
        Write-Host "Deadline reached ($($deadline.ToString('HH:mm'))). Stopping." -ForegroundColor Yellow
        break
    }

    # --- Early exit: hotkey check ---
    while ($script:HasConsole -and [Console]::KeyAvailable) {
        $key = [Console]::ReadKey($true)
        if ($key.Key -eq 'Q' -or $key.Key -eq 'Escape') {
            $earlyExitReason = "User pressed $($key.Key)"
            break
        }
    }
    if ($earlyExitReason) {
        Write-Host ""
        Write-Host "** Early exit: $earlyExitReason **" -ForegroundColor Yellow
        "[issues] EARLY EXIT: $earlyExitReason" | Out-File $logFile -Append -Encoding UTF8
        break
    }

    $iterStart = Get-Date
    $elapsed = $iterStart - $runStart
    $elapsedStr = "{0:hh\:mm\:ss}" -f $elapsed

    Write-Host ""
    Write-Host "===================================="
    Write-Host "== Scan Iteration $i/$maxLabel"
    Write-Host "== Repo: $repoSlug"
    Write-Host "== Prompt: $promptBrief"
    Write-Host "== Started: $($iterStart.ToString('HH:mm:ss'))  |  Elapsed: $elapsedStr"
    Write-Host "===================================="

    Play-Audio "ralph-processing.mp3"

    $promptText = Resolve-PromptText

    # Count issues before this iteration
    $issuesBefore = Get-OpenIssueCount

    # Build the scan prompt
    $scanContext = @"
ISSUE SCANNER CONTEXT: You are scanning a codebase to find improvements and create GitHub Issues.
Repository: $repoSlug
Working directory: $(Get-Location)

IMPORTANT RULES:
1. Do NOT modify any source code. Do NOT create branches or commits. This is a READ-ONLY scan.
2. Analyze the codebase and identify issues, improvements, or violations.
3. Before creating any issue, ALWAYS check if a similar open issue already exists:
   gh issue list --repo $repoSlug --state open --search "relevant keywords" --json title,number,url -q '.[] | "#\(.number) \(.title)"'
4. Only create an issue if NO existing open issue covers the same topic.
5. When creating issues, use this format:
   gh issue create --repo $repoSlug --title "Clear, specific title" --body "Detailed description with file paths, line numbers, and suggested fix"$( if ($labelClause) { " --label `"$labelList`"" } )
6. Create focused, actionable issues (one issue per finding — not mega-issues).
7. Include relevant file paths and code snippets in the issue body.
$dryRunClause

COMPLETION SIGNAL: When you have thoroughly scanned the codebase and cannot find any more new issues to create (that don't already exist), include the exact marker RALPH_TASK_COMPLETE on its own line in your final output.

"@

    if ($i -gt 1) {
        $effectivePrompt = @"
${scanContext}Continue scanning from where you left off. Check what issues already exist to avoid duplicates:
  gh issue list --repo $repoSlug --state open --limit 50 --json title,number -q '.[] | "#\(.number) \(.title)"'
Focus on areas you haven't scanned yet. Look for different categories of issues than previous iterations.
Original scan goal: $promptBrief
"@
    }
    elseif ($promptText) {
        $effectivePrompt = "${scanContext}${promptText}${labelClause}${dryRunClause}"
    }
    else {
        $effectivePrompt = @"
${scanContext}Scan this codebase for improvements and create GitHub Issues for each finding.
Look for: code quality issues, potential bugs, missing tests, security concerns, performance problems, technical debt, TODO/FIXME items, missing documentation, and architectural improvements.
Prioritize findings by severity (critical > high > medium > low).${labelClause}${dryRunClause}
"@
    }

    $tempOut = [System.IO.Path]::GetTempFileName()
    $copilotInvocations++
    $buildArgs = @{
        ModelId  = $script:ModelId
        Agent    = $script:DevAgentName
        Prompt   = $effectivePrompt
        Yolo     = $true
        Provider = $script:ProviderName
    }
    if ($i -gt 1) { $buildArgs['Continue'] = $true }
    $cmdArgs = Build-RalphCommand @buildArgs
    $cmdExe = $cmdArgs[0]; $cmdTail = $cmdArgs[1..($cmdArgs.Count-1)]
    & $cmdExe @cmdTail 2>&1 | Tee-Object -FilePath $tempOut
    if ($LASTEXITCODE -ne 0) {
        $agentFailures++
        Write-Host "The agent CLI exited with code $LASTEXITCODE." -ForegroundColor Red
    }

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

    # Count issues created this iteration
    $issuesAfter = Get-OpenIssueCount
    $iterIssuesCreated = if ($issuesBefore -ge 0 -and $issuesAfter -ge 0) { $issuesAfter - $issuesBefore } else { 0 }
    # Also detect issues from output (gh issue create outputs URLs)
    $issueUrls = @($captured | Select-String -Pattern 'https://github\.com/.+/issues/\d+' | ForEach-Object { $_.Matches[0].Value } | Select-Object -Unique)
    if ($issueUrls.Count -gt $iterIssuesCreated) { $iterIssuesCreated = $issueUrls.Count }
    $totalIssuesCreated += $iterIssuesCreated

    Write-Host "------------------------------------"
    Write-Host "== Scan $i/$maxLabel completed in $iterDurStr  |  Total: $totalStr"
    Write-Host "== Issues created this iteration: $iterIssuesCreated  |  Total: $totalIssuesCreated"
    Write-Host "------------------------------------"

    $logEntry = @"

[issues] ==== Scan Iteration $i/$maxLabel ====
Prompt:            $promptBrief
Started:           $($iterStart.ToString('yyyy-MM-dd HH:mm:ss'))
Finished:          $($iterEnd.ToString('yyyy-MM-dd HH:mm:ss'))
Duration:          $iterDurStr
Total elapsed:     $totalStr
Issues created:    $iterIssuesCreated (total: $totalIssuesCreated)
$statsBlock
"@
    $logEntry | Out-File $logFile -Append -Encoding UTF8
    $completedIterations = $i

    # --- Early exit: sentinel marker in copilot output ---
    if ($captured -match 'RALPH_TASK_COMPLETE') {
        $earlyExitReason = "Scan complete — no more issues to create (RALPH_TASK_COMPLETE)"
        Write-Host ""
        Write-Host "** Early exit: $earlyExitReason **" -ForegroundColor Green
        "[issues] EARLY EXIT: $earlyExitReason" | Out-File $logFile -Append -Encoding UTF8
        break
    }

    # --- Early exit: no new issues detection ---
    if ($iterIssuesCreated -le 0 -and -not $DryRun) {
        $noNewIssueCount++
        Write-Host "  No new issues created ($noNewIssueCount consecutive)." -ForegroundColor DarkYellow
        if ($noNewIssueCount -ge 2) {
            $earlyExitReason = "No new issues for $noNewIssueCount consecutive iterations"
            Write-Host ""
            Write-Host "** Early exit: $earlyExitReason **" -ForegroundColor Yellow
            "[issues] EARLY EXIT: $earlyExitReason" | Out-File $logFile -Append -Encoding UTF8
            break
        }
    }
    else {
        $noNewIssueCount = 0
    }
}

# --- Summary ---
$endTime = Get-Date
$totalDuration = $endTime - $runStart
$totalDurStr = "{0:hh\:mm\:ss}" -f $totalDuration

$costPerRequest = [math]::Round(0.04 * $script:ModelMultiplier, 4)
$costTotal = [math]::Round($totalPremiumRequests * $costPerRequest, 2)

$summary = @"

[issues] ====================================
[issues] RALPH ISSUES SCAN SUMMARY
[issues] ====================================
[issues]   Finished:          $($endTime.ToString('yyyy-MM-dd HH:mm:ss'))
[issues]   Total duration:    $totalDurStr
[issues]   Iterations:        $completedIterations / $maxLabel
[issues]   Copilot invocations: $copilotInvocations
[issues]   Issues created:    $totalIssuesCreated
[issues]   Premium requests:  $totalPremiumRequests
[issues]   Provider:          $script:ProviderName ($($script:ProviderConfig.Command))
[issues]   Model:             $script:ModelId ($script:ModelLabel)
[issues]   Est. cost:         `$$costTotal  ($totalPremiumRequests x `$$costPerRequest)
[issues]   Prompt:            $promptBrief
[issues]   Repo:              $repoSlug
[issues]   DryRun:            $DryRun
"@
if ($earlyExitReason) {
    $summary += "`n[issues]   Early exit:        $earlyExitReason"
}
$summary += "`n[issues] ===================================="

$summary | Out-File $logFile -Append -Encoding UTF8
"$(Get-Date -Format 'yyyy-MM-dd HH:mm') -- ${repoName}: Ralph Issues scan finished: $totalIssuesCreated issues, $totalDurStr, `$$costTotal" | Out-File $script:SummaryLogFile -Append -Encoding UTF8

Write-Host ""
Write-Host "====================================" -ForegroundColor Cyan
Write-Host "== RALPH ISSUES SCAN SUMMARY" -ForegroundColor Cyan
Write-Host "====================================" -ForegroundColor Cyan
Write-Host "  Duration:        $totalDurStr"
Write-Host "  Iterations:      $completedIterations / $maxLabel"
Write-Host "  Issues created:  $totalIssuesCreated" -ForegroundColor $(if ($totalIssuesCreated -gt 0) { 'Green' } else { 'Yellow' })
Write-Host "  Premium reqs:    $totalPremiumRequests"
Write-Host "  Est. cost:       `$$costTotal"
if ($earlyExitReason) {
    Write-Host "  Exit reason:     $earlyExitReason" -ForegroundColor DarkGray
}
Write-Host "====================================" -ForegroundColor Cyan

# Restore BYOK environment
if ($script:SavedByokEnv) {
    Restore-RalphProviderEnv -Saved $script:SavedByokEnv
}

Play-Audio "ralph-complete.mp3"

if ($copilotInvocations -gt 0 -and $agentFailures -eq $copilotInvocations) {
    Write-Host "The agent CLI failed on every scan; no issues were looked for." -ForegroundColor Red
    exit 4
}
