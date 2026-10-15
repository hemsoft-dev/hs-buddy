function Invoke-NativeCleanup {
    [CmdletBinding()]
    param([Parameter(Mandatory)] [scriptblock] $Cleanup, [System.Management.Automation.ErrorRecord] $PrimaryError)
    try { & $Cleanup }
    catch {
        if (-not $PrimaryError) { throw }
        $PrimaryError.Exception.Data['BuddyNativeCleanupFailure'] = $_
        # Reporting a secondary failure must never replace the original failure.
        try { Write-Warning 'Native cleanup also failed; the original qualification error is retained.' -WarningAction Continue }
        catch { }
    }
}
