#!/usr/bin/env pwsh
#Requires -Version 7.0
<#
.SYNOPSIS
  Start the Electron version of Buddy together with its local Convex backend.

.DESCRIPTION
  Reuses a Convex dev server that is already listening on port 3210. Otherwise it
  starts `bun run convex:dev` in the background, waits until Convex reports its
  functions ready, launches the app with `bun dev`, and stops that Convex server
  again when the app exits. Convex output goes to a log file in the temp folder.

.EXAMPLE
  ./run-electron.ps1               # start Convex if needed, then the app
.EXAMPLE
  ./run-electron.ps1 -Setup        # run `bun run setup` first (fresh clone or new dependencies)
.EXAMPLE
  ./run-electron.ps1 -KeepConvex   # leave the Convex server running after the app closes
#>
[CmdletBinding()]
param(
    [switch] $Setup,
    [switch] $KeepConvex,
    [ValidateRange(10, 600)]
    [int] $ConvexTimeoutSeconds = 120
)

$ErrorActionPreference = 'Stop'
$InformationPreference = 'Continue'
. (Join-Path $PSScriptRoot 'scripts/lib/PortUtils.ps1')

$script:ConvexApiPort = 3210
$script:ConvexLog = Join-Path ([IO.Path]::GetTempPath()) 'hs-buddy-convex-dev.log'
$script:ConvexErrLog = Join-Path ([IO.Path]::GetTempPath()) 'hs-buddy-convex-dev.err.log'
$script:ConvexStdin = Join-Path ([IO.Path]::GetTempPath()) 'hs-buddy-convex-dev.stdin'

function Write-Step([string] $Message, [string] $Color = 'Cyan') {
    Write-Host $Message -ForegroundColor $Color
}

function Assert-Prerequisite {
    if (-not (Get-Command bun -ErrorAction SilentlyContinue)) {
        throw 'bun not found. Install Bun (https://bun.sh), then re-open the shell.'
    }
    if (-not (Test-Path (Join-Path $PSScriptRoot '.env.local'))) {
        throw ('No .env.local yet, so Convex has never been configured here. Run ' +
            '`bun run convex:dev` once in this folder, answer its prompts (choose local ' +
            'development), stop it with Ctrl+C, then run this script again.')
    }
}

function Start-ConvexServer {
    # A local backend left behind with no dev watcher keeps the dashboard port alive
    # but never serves the API; clear it the same way scripts/runServer.ps1 does.
    $orphan = Get-Process -Name 'convex-local-backend' -ErrorAction SilentlyContinue
    if ($orphan) {
        Write-Step "Stopping orphaned convex-local-backend (PID $($orphan.Id -join ', '))..." 'Yellow'
        $orphan | Stop-Process -Force
        Start-Sleep -Seconds 2
    }

    foreach ($file in $script:ConvexLog, $script:ConvexErrLog) {
        if (Test-Path $file) { Remove-Item $file -Force }
    }

    # Give Convex an empty stdin instead of this console. With a console it sees a TTY
    # and can stop at an interactive question (such as "link this deployment to your
    # account?") that would sit unseen in the log; without one it runs non-interactively.
    Set-Content -Path $script:ConvexStdin -Value $null -NoNewline

    Write-Step "Starting Convex dev server (log: $script:ConvexErrLog)..."
    $bun = (Get-Command bun).Source
    return Start-Process -FilePath $bun -ArgumentList 'run', 'convex:dev' `
        -WorkingDirectory $PSScriptRoot -NoNewWindow -PassThru `
        -RedirectStandardInput $script:ConvexStdin `
        -RedirectStandardOutput $script:ConvexLog -RedirectStandardError $script:ConvexErrLog
}

function Get-ConvexLogText {
    $text = foreach ($file in $script:ConvexLog, $script:ConvexErrLog) {
        if (Test-Path $file) { Get-Content $file -Raw -ErrorAction SilentlyContinue }
    }
    return ($text -join "`n")
}

function Wait-ConvexReady([System.Diagnostics.Process] $Process, [int] $TimeoutSeconds) {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        if ($Process.HasExited) {
            throw "Convex dev server exited (code $($Process.ExitCode)) before it was ready.`n$(Get-ConvexLogText)"
        }
        $text = Get-ConvexLogText
        if ($text -match 'Convex functions ready') { return }
        # Convex prompts start a line with "? "; nobody can answer them here.
        $question = [regex]::Match($text, '(?m)^\? .*(\r?\n(?!\s*$).*)?')
        if ($question.Success) {
            $plain = $question.Value -replace "$([char]27)\[[0-9;]*[A-Za-z]", ''
            throw ("Convex is asking a question this script can't answer:`n  $plain`n" +
                'Run `bun run convex:dev` once in this folder to answer it, stop it with Ctrl+C, ' +
                'then run this script again.')
        }
        Start-Sleep -Milliseconds 500
    }
    throw "Convex was not ready after $TimeoutSeconds s. See $script:ConvexLog and $script:ConvexErrLog."
}

function Stop-ConvexServer([System.Diagnostics.Process] $Process) {
    if (-not $Process -or $Process.HasExited) { return }
    Write-Step 'Stopping the Convex dev server this script started...'
    if ($IsWindows) {
        # Process.Kill($true) ends bun but leaves its node child and the
        # convex-local-backend running on Windows; taskkill /T ends the whole tree.
        & taskkill.exe /PID $Process.Id /T /F 2>&1 | Out-Null
    }
    else {
        try { $Process.Kill($true) } catch { Write-Verbose "Convex process already gone: $_" }
    }
    $Process.WaitForExit(10000) | Out-Null
}

function Invoke-Main([bool] $RunSetup, [bool] $LeaveConvexRunning, [int] $TimeoutSeconds) {
    Assert-Prerequisite
    Push-Location $PSScriptRoot
    $convex = $null
    $appExit = 1
    try {
        if ($RunSetup -or -not (Test-Path (Join-Path $PSScriptRoot 'node_modules'))) {
            Write-Step 'Running bun run setup...'
            & bun run setup
            if ($LASTEXITCODE -ne 0) { throw "bun run setup failed with exit code $LASTEXITCODE" }
        }

        if (Test-PortOpen -Port $script:ConvexApiPort) {
            Write-Step "Using the Convex server already running on port $script:ConvexApiPort." 'Green'
        }
        else {
            $convex = Start-ConvexServer
            Wait-ConvexReady -Process $convex -TimeoutSeconds $TimeoutSeconds
            Write-Step 'Convex functions ready.' 'Green'
        }

        Write-Step 'Launching the Electron app (bun dev)...'
        & bun dev
        $appExit = $LASTEXITCODE
    }
    finally {
        if ($convex -and -not $LeaveConvexRunning) { Stop-ConvexServer -Process $convex }
        elseif ($convex) { Write-Step "Convex left running (PID $($convex.Id))." 'Yellow' }
        Pop-Location
    }
    exit $appExit
}

# Dot-sourcing the script (`. ./run-electron.ps1`) loads the functions without running it.
if ($MyInvocation.InvocationName -ne '.') {
    Invoke-Main -RunSetup $Setup.IsPresent -LeaveConvexRunning $KeepConvex.IsPresent `
        -TimeoutSeconds $ConvexTimeoutSeconds
}
