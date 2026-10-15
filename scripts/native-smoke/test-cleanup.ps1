#requires -Version 7
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'cleanup.ps1')
$temporary = Join-Path ([IO.Path]::GetTempPath()) ('buddy-cleanup-test-' + [Guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($temporary) | Out-Null
try {
    $state = @{ Calls = 0 }
    $cleanup = {
        $state.Calls++
        # Exercise a real write failure, without installing or launching anything.
        [IO.File]::WriteAllText((Join-Path $temporary 'missing/result.txt'), 'owned fixture')
    }
    $original = [InvalidOperationException]::new('Owned primary qualification failure')
    $primary = $null
    $observed = $null
    try {
        try { throw $original }
        catch { $primary = $_; throw }
        finally { Invoke-NativeCleanup -Cleanup $cleanup -PrimaryError $primary }
    }
    catch { $observed = $_ }
    if (-not [object]::ReferenceEquals($observed.Exception, $original)) { throw 'Cleanup masked the original exception' }
    if (-not $original.Data['BuddyNativeCleanupFailure']) { throw 'Secondary cleanup error was lost' }
    if ($state.Calls -ne 1) { throw 'Cleanup was replayed' }

    $state.Calls = 0
    $observed = $null
    try { Invoke-NativeCleanup -Cleanup $cleanup }
    catch { $observed = $_ }
    if (-not $observed -or $state.Calls -ne 1) { throw 'A required cleanup failure must fail an otherwise successful run' }
    Write-Output 'Native cleanup real-I/O failure guards passed'
}
finally { [IO.Directory]::Delete($temporary) }
