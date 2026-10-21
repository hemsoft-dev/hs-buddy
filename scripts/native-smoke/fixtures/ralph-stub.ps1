# Deterministic stand-in for ralph.ps1 in installed qualification. It changes nothing in the
# repository: it prints the real script's progress banners, runs the stub Copilot CLI found on
# PATH, and writes one receipt per start. The prompt file selects the behavior.
param(
    [int]$Max = 0,
    [string]$Prompt,
    [string]$Branch,
    [string]$Model,
    [string]$Provider,
    [string[]]$Agents,
    [switch]$Autopilot,
    [switch]$NoAudio,
    [switch]$NoPR,
    [switch]$SkipReview
)
$ErrorActionPreference = 'Stop'
$mode = (Get-Content -Raw -LiteralPath $Prompt).Trim()
$receipt = [ordered]@{
    mode = $mode
    max = $Max
    model = $Model
    noAudio = [bool]$NoAudio
    autopilot = [bool]$Autopilot
    agents = @($Agents)
    cwd = (Get-Location).Path
    pid = $PID
    child = $null
}
$path = Join-Path $env:BUDDY_RALPH_PROOF ("start-" + [Guid]::NewGuid().ToString('N') + ".json")
function Save-Receipt { $receipt | ConvertTo-Json | Set-Content -LiteralPath $path -Encoding utf8NoBOM }
Save-Receipt
Write-Host "== Iteration 1/$Max"
Write-Host "RALPH STUB $mode 資料界é"
# Found on the PATH Buddy passes, then called by its path: resolving a bare unknown name
# makes PowerShell scan every installed module first, which took over 20 s on a fresh
# macOS runner.
$names = if ($IsWindows) { @('copilot.cmd', 'copilot.exe') } else { @('copilot') }
$tool = $env:PATH -split [IO.Path]::PathSeparator |
    Where-Object { $_ } |
    ForEach-Object { $folder = $_; $names | ForEach-Object { Join-Path $folder $_ } } |
    Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
    Select-Object -First 1
if (-not $tool) { throw 'copilot is not on the PATH Buddy passed to the loop' }
Write-Host ("RALPH STUB TOOL " + (& $tool --version))
switch ($mode) {
    'quick' {
        Start-Sleep -Seconds 6
        for ($i = 2; $i -le $Max; $i++) {
            Write-Host "== Iteration $i/$Max"
            Start-Sleep -Seconds 3
        }
        Write-Host 'RALPH STUB COMPLETE'
        exit 0
    }
    'long' {
        $shell = (Get-Process -Id $PID).Path
        $child = Start-Process -FilePath $shell -PassThru -NoNewWindow -ArgumentList @(
            '-NoProfile', '-Command', 'while ($true) { Start-Sleep -Seconds 1 }')
        $receipt.child = $child.Id
        Save-Receipt
        Write-Host 'RALPH STUB RUNNING'
        while ($true) { Start-Sleep -Seconds 1 }
    }
    'fail' {
        Write-Host 'RALPH STUB FAILING'
        exit 3
    }
    default { throw "Unknown stub mode '$mode'" }
}
