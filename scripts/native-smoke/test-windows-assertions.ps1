#requires -Version 7
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
. (Join-Path $PSScriptRoot 'windows-assertions.ps1')
$source = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../public/icon.ico'))
$icon = [System.Drawing.Icon]::new($source, [System.Drawing.Size]::new(32, 32))
try {
    $valid = Assert-NativeIcon -Handle $icon.Handle -ExpectedIconPath $source
    if (-not $valid.verified -or $valid.width -ne 32) { throw 'Bundled Buddy icon did not qualify' }
    foreach ($invalid in @([IntPtr]::Zero, [System.Drawing.SystemIcons]::Warning.Handle)) {
        $rejected = $false
        try { Assert-NativeIcon -Handle $invalid -ExpectedIconPath $source | Out-Null }
        catch { $rejected = $true }
        if (-not $rejected) { throw 'A missing or unrelated icon qualified' }
    }
    Write-Output 'Native icon guards accept Buddy and reject missing or unrelated icons'
}
finally { $icon.Dispose() }
