# ralph-run-all.ps1 — Sequential autopilot orchestrator.
# Version: 1.2.0
# Between each script, pulls latest main so the next run branches from fresh code.
# Stops on the first failure.
param(
    [switch]$Pick,
    [string]$Model,
    [string]$Provider,
    [string[]]$Agents,
    [string]$WorkUntil,
    [switch]$NoAudio,
    [switch]$SkipReview,
    [switch]$Once,
    [switch]$Help
)

$ErrorActionPreference = 'Stop'

if ($Help) {
    Write-Host ""
    Write-Host "ralph-run-all.ps1 - Sequential Autopilot Orchestrator" -ForegroundColor Cyan
    Write-Host "Runs all ralph-*.ps1 scripts in the scripts/ directory sequentially."
    Write-Host "Each script runs in full autopilot mode — PRs are auto-merged."
    Write-Host ""
    Write-Host "PARAMETERS" -ForegroundColor Yellow
    Write-Host "  -Pick                  Choose which scripts to run (default: run all)"
    Write-Host "  -Model <name>          Model to pass through (validated by ralph.ps1)"
    Write-Host "  -Provider <name>       CLI provider: copilot, opencode (validated by ralph.ps1)"
    Write-Host "  -Agents <specs>        Agent specs: role or role@model (validated by ralph.ps1)"
    Write-Host "                         Dev agents control the work loop; review agents run PR reviews"
    Write-Host "  -WorkUntil <HH:mm>     Stop after this local time (passed to each script)"
    Write-Host "  -NoAudio               Suppress audio feedback"
    Write-Host "  -SkipReview            Skip Copilot PR review requests"
    Write-Host "  -Once                  Run only one work iteration (passed to each script)"
    Write-Host "  -Help                  Show this help message"
    Write-Host ""
    Write-Host "EXAMPLES" -ForegroundColor Yellow
    Write-Host "  ralph-run-all"
    Write-Host "  ralph-run-all -Pick"
    Write-Host "  ralph-run-all -Model sonnet -WorkUntil 08:00"
    Write-Host "  ralph-run-all -Agents pr-review-quality,pr-review-security"
    Write-Host "  ralph-run-all -NoAudio"
    Write-Host ""
    exit 0
}

# --- Verify git state ---
$repoRoot = ([string](git rev-parse --show-toplevel 2>&1)).Trim() -replace '/', [IO.Path]::DirectorySeparatorChar
if ($LASTEXITCODE -ne 0) {
    Write-Host "Not inside a git repository." -ForegroundColor Red
    exit 1
}
$currentBranch = (git rev-parse --abbrev-ref HEAD 2>&1).Trim()

# --- Discover scripts from this repo's scripts/ directory ---
$scriptsDir = $PSScriptRoot
$myName = Split-Path -Leaf $MyInvocation.MyCommand.Path

$allScripts = Get-ChildItem -Path $scriptsDir -Filter "ralph-*.ps1" -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -ne $myName } |
    Sort-Object Name

if ($allScripts.Count -eq 0) {
    Write-Host "No ralph-*.ps1 scripts found in $scriptsDir" -ForegroundColor Red
    exit 1
}

$scripts = $allScripts

# --- Pick mode ---
if ($Pick) {
    Write-Host ""
    Write-Host "Available scripts:" -ForegroundColor Yellow
    for ($i = 0; $i -lt $allScripts.Count; $i++) {
        Write-Host "  [$($i + 1)] $($allScripts[$i].Name)"
    }
    Write-Host ""
    $selection = Read-Host "Enter numbers to run (comma-separated, e.g. 1,3) or 'all'"
    if ($selection -eq 'all') {
        # keep all
    }
    else {
        $indices = $selection -split ',' | ForEach-Object { [int]$_.Trim() - 1 }
        $indices = $indices | Where-Object { $_ -ge 0 -and $_ -lt $allScripts.Count }
        if ($indices.Count -eq 0) {
            Write-Host "No valid selections." -ForegroundColor Yellow
            exit 1
        }
        $scripts = $indices | ForEach-Object { $allScripts[$_] }
    }
    Write-Host ""
}

# --- Banner ---
$runStart = Get-Date
Write-Host ""
Write-Host "====================================" -ForegroundColor Cyan
Write-Host "  RALPH RUN-ALL" -ForegroundColor Cyan
Write-Host "====================================" -ForegroundColor Cyan
Write-Host "  Started:   $($runStart.ToString('yyyy-MM-dd HH:mm:ss'))"
Write-Host "  Scripts:   $($scripts.Count)"
for ($i = 0; $i -lt $scripts.Count; $i++) {
    Write-Host "    $($i + 1). $($scripts[$i].Name)" -ForegroundColor White
}
Write-Host "  Branch:    $currentBranch"
Write-Host "  Repo:      $repoRoot"
if ($Model) { Write-Host "  Model:     $Model" }
if ($WorkUntil) { Write-Host "  WorkUntil: $WorkUntil" }
Write-Host "====================================" -ForegroundColor Cyan
Write-Host ""

# --- Run scripts sequentially ---
$results = @()
$failed = $false

for ($i = 0; $i -lt $scripts.Count; $i++) {
    $script = $scripts[$i]
    $stepStart = Get-Date

    # Check deadline
    if ($WorkUntil) {
        try {
            $parsed = [DateTime]::ParseExact($WorkUntil, 'HH:mm', $null)
            $deadline = Get-Date -Hour $parsed.Hour -Minute $parsed.Minute -Second 0
            if ($deadline -le $runStart) { $deadline = $deadline.AddDays(1) }
            if ((Get-Date) -ge $deadline) {
                Write-Host "Deadline reached ($WorkUntil). Stopping before $($script.Name)." -ForegroundColor Yellow
                break
            }
        }
        catch { }
    }

    Write-Host ""
    Write-Host "====================================" -ForegroundColor Magenta
    Write-Host "== [$($i + 1)/$($scripts.Count)] $($script.Name)"
    Write-Host "====================================" -ForegroundColor Magenta
    Write-Host ""

    # Build args — always pass -Autopilot
    $scriptArgs = @{ Autopilot = $true }
    if ($NoAudio) { $scriptArgs['NoAudio'] = $true }
    if ($SkipReview) { $scriptArgs['SkipReview'] = $true }
    if ($Model) { $scriptArgs['Model'] = $Model }
    if ($Provider) { $scriptArgs['Provider'] = $Provider }
    if ($Agents) { $scriptArgs['Agents'] = $Agents }
    if ($WorkUntil) { $scriptArgs['WorkUntil'] = $WorkUntil }
    if ($Once) { $scriptArgs['Once'] = $true }

    # Invoke the script
    & $script.FullName @scriptArgs
    $exitCode = $LASTEXITCODE

    $stepEnd = Get-Date
    $stepDuration = $stepEnd - $stepStart
    $stepDurStr = "{0:hh\:mm\:ss}" -f $stepDuration

    $status = if ($exitCode -eq 0) { "OK" } else { "FAILED (exit $exitCode)" }
    $results += [PSCustomObject]@{
        Script   = $script.Name
        Status   = $status
        Duration = $stepDurStr
        ExitCode = $exitCode
    }

    if ($exitCode -ne 0) {
        Write-Host ""
        Write-Host "$($script.Name) failed with exit code $exitCode. Stopping." -ForegroundColor Red
        $failed = $true
        break
    }

    Write-Host ""
    Write-Host "$($script.Name) completed successfully ($stepDurStr)." -ForegroundColor Green

    # Pull latest main before next script
    if ($i -lt $scripts.Count - 1) {
        Write-Host ""
        Write-Host "Pulling latest main before next script..." -ForegroundColor Cyan
        git fetch origin main 2>&1 | Out-Null
        git checkout main 2>&1 | Out-Null
        git pull --ff-only 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) {
            Write-Host "Warning: git pull on main failed. Continuing anyway." -ForegroundColor Yellow
        }
        else {
            Write-Host "Main is up to date." -ForegroundColor Green
        }
    }
}

# --- Summary ---
$runEnd = Get-Date
$totalDuration = $runEnd - $runStart
$totalDurStr = "{0:hh\:mm\:ss}" -f $totalDuration
$successCount = ($results | Where-Object { $_.ExitCode -eq 0 }).Count

Write-Host ""
Write-Host "====================================" -ForegroundColor Cyan
Write-Host "  RALPH RUN-ALL SUMMARY" -ForegroundColor Cyan
Write-Host "====================================" -ForegroundColor Cyan
Write-Host "  Finished:  $($runEnd.ToString('yyyy-MM-dd HH:mm:ss'))"
Write-Host "  Duration:  $totalDurStr"
Write-Host "  Results:   $successCount/$($results.Count) succeeded"
Write-Host ""
foreach ($r in $results) {
    $color = if ($r.ExitCode -eq 0) { "Green" } else { "Red" }
    Write-Host "    $($r.Status.PadRight(20)) $($r.Duration)  $($r.Script)" -ForegroundColor $color
}
Write-Host ""
Write-Host "====================================" -ForegroundColor Cyan

if ($failed) { exit 1 }
