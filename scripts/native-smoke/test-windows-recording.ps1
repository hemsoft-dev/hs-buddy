#requires -Version 7
# Exercise the real recorder and stop blocks without accessing any desktop.
param([string]$Source = (Join-Path $PSScriptRoot 'windows-actions.ps1'))
$ErrorActionPreference = 'Stop'
$tokens = $null
$parseErrors = $null
$syntax = [Management.Automation.Language.Parser]::ParseFile($Source, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw 'Recorder source did not parse' }
$starts = @($syntax.FindAll({param($node) $node -is [Management.Automation.Language.CommandAst] -and $node.GetCommandName() -eq 'Start-ThreadJob'}, $true))
if ($starts.Count -ne 1) { throw 'Expected the production recorder job' }
$expressions = @($starts[0].CommandElements | Where-Object { $_ -is [Management.Automation.Language.ScriptBlockExpressionAst] })
if ($expressions.Count -ne 1) { throw 'Expected the production capture body' }
$capture = $expressions[0].ScriptBlock.GetScriptBlock()
$switches = @($syntax.FindAll({param($node) $node -is [Management.Automation.Language.SwitchStatementAst]}, $true))
$stops = @($switches | ForEach-Object { $_.Clauses } | Where-Object { $_.Item1.Value -eq 'record-stop' })
if ($stops.Count -ne 1) { throw 'Expected the production record-stop clause' }
$stopText = $stops[0].Item2.Extent.Text
$stop = [scriptblock]::Create($stopText.Substring(1, $stopText.Length - 2))
Add-Type -AssemblyName System.Drawing
# Deliberately fake pixels and window geometry. No user32 import or desktop call.
Add-Type -TypeDefinition @'
using System;
using System.Threading;
public static class Drive {
    public struct RECT { public int l,t,r,b; }
    public static int Calls;
    public static int FailAt;
    public static int FrameCount() { return Volatile.Read(ref Calls); }
    public static bool GetWindowRect(IntPtr window, ref RECT rect) {
        int frame=Interlocked.Increment(ref Calls);
        rect.r=2; rect.b=2;
        return FailAt == 0 || frame < FailAt;
    }
    public static bool PrintWindow(IntPtr window, IntPtr dc, int flags) { return true; }
}
'@
$temporary = Join-Path ([IO.Path]::GetTempPath()) ('buddy-recording-test-' + [Guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($temporary) | Out-Null
$jobs = [Collections.Generic.List[object]]::new()
function Start-TestRecorder([string]$Directory) {
    [IO.Directory]::CreateDirectory($Directory) | Out-Null
    $job = Start-ThreadJob -ArgumentList ([IntPtr]1),$Directory -InitializationScript {
        # Keep the production loop intact; accelerate only its frame interval.
        function global:Start-Sleep { param([int]$Milliseconds) [Threading.Thread]::Sleep(1) }
    } -ScriptBlock $capture
    $jobs.Add($job)
    return $job
}
function Stop-TestRecorder($Job, [string]$Directory) {
    $recorder = $Job
    $recordDirectory = $Directory
    . $stop
}
try {
    $long = Join-Path $temporary 'long'
    [Drive]::Calls = 0
    [Drive]::FailAt = 0
    $job = Start-TestRecorder $long
    $timer = [Diagnostics.Stopwatch]::StartNew()
    while ([Drive]::FrameCount() -lt 260 -and $job.State -notin @('Failed','Completed','Stopped') -and $timer.Elapsed.TotalSeconds -lt 15) {
        [Threading.Thread]::Sleep(10)
    }
    if ([Drive]::FrameCount() -lt 260 -or $job.State -ne 'Running') { throw 'Recorder ended before the bounded action sequence completed beyond 240 frames' }
    Stop-TestRecorder $job $long
    if ($job.State -ne 'Completed' -or @(Get-ChildItem -LiteralPath $long -Filter 'frame-*.png' -File).Count -lt 260) { throw 'Recorder did not save the complete interaction before stopping' }
    Write-Output 'Production recorder continues beyond 240 frames and stops on completion'

    $failure = Join-Path $temporary 'failure'
    [Drive]::Calls = 0
    [Drive]::FailAt = 3
    $job = Start-TestRecorder $failure
    Wait-Job $job -Timeout 5 | Out-Null
    if ($job.State -ne 'Failed') { throw 'Injected capture failure did not reach the recorder' }
    $observed = $null
    try { Stop-TestRecorder $job $failure }
    catch { $observed = $_ }
    if (-not $observed -or $observed.Exception.Message -notlike '*Recording window disappeared*') { throw 'Record-stop hid the original capture failure' }
    Write-Output 'Production record-stop preserves the original capture failure'
}
finally {
    foreach ($job in $jobs) {
        if (Get-Job -Id $job.Id -ErrorAction SilentlyContinue) {
            if ($job.State -eq 'Running') { Stop-Job $job }
            Remove-Job $job -Force
        }
    }
    # Only flat files created by these two owned fixtures are removed.
    foreach ($folder in @((Join-Path $temporary 'long'), (Join-Path $temporary 'failure'))) {
        $full = [IO.Path]::GetFullPath($folder)
        if (-not $full.StartsWith([IO.Path]::GetFullPath($temporary) + [IO.Path]::DirectorySeparatorChar)) { throw 'Fixture cleanup escaped its directory' }
        if (Test-Path -LiteralPath $full) {
            if (([IO.File]::GetAttributes($full) -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'Fixture cleanup refuses links' }
            foreach ($file in [IO.Directory]::GetFiles($full)) { [IO.File]::Delete($file) }
            [IO.Directory]::Delete($full)
        }
    }
    [IO.Directory]::Delete($temporary)
}
