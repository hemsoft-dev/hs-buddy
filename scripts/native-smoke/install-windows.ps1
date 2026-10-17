#requires -Version 7
[CmdletBinding()]
param([Parameter(Mandatory)] [string] $PackageDirectory, [Parameter(Mandatory)] [string] $Output)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'cleanup.ps1')
if ($env:GITHUB_ACTIONS -ne 'true' -or -not $env:RUNNER_TEMP) {
    throw 'Installer qualification requires an ephemeral GitHub Actions runner'
}
$metadata = Get-Content (Join-Path $PackageDirectory 'build.json') -Raw | ConvertFrom-Json
if ($metadata.sourceDirty -or $metadata.profile -ne 'release' -or $metadata.target -ne 'x86_64-pc-windows-msvc') {
    throw 'Expected a committed Windows MSVC release package'
}
$installer = Join-Path $PackageDirectory $metadata.package.file
if ((Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant() -ne $metadata.package.sha256) {
    throw 'Installer checksum mismatch'
}
$registration = 'HKCU:/Software/Microsoft/Windows/CurrentVersion/Uninstall/Buddy Native'
if ((Test-Path $registration) -or (Test-Path 'HKCU:/Software/HemSoft/Buddy Native')) {
    throw 'Preserve existing Buddy Native installation'
}
$install = Join-Path $env:RUNNER_TEMP ('buddy-native-installed-' + [Guid]::NewGuid().ToString('N'))
$installed = $false
$nativePrimaryError = $null
try {
    # No /R: silent installation must not auto-launch the app.
    $process = Start-Process -FilePath $installer -ArgumentList @('/S', "/D=$install") -WindowStyle Hidden -PassThru
    if (-not $process.WaitForExit(120000) -or $process.ExitCode -ne 0) { throw 'Installer failed or timed out' }
    $installed = $true
    $exe = Join-Path $install 'buddy-native.exe'
    if ((Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant() -ne $metadata.binarySha256) {
        throw 'Installed executable differs from qualified build'
    }
    $entry = Get-ItemProperty $registration
    if ($entry.DisplayVersion -ne $metadata.version -or $entry.InstallLocation.Trim('"') -ne $install) {
        throw 'Incorrect application registration'
    }
    $shortcuts = @(Get-ChildItem ([Environment]::GetFolderPath('StartMenu')) -Filter 'Buddy Native.lnk' -Recurse)
    if ($shortcuts.Count -ne 1) { throw 'Missing or ambiguous Start menu registration' }
    $shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcuts[0].FullName)
    if ($shortcut.TargetPath -ne $exe) { throw 'Start menu shortcut targets another executable' }
    & python (Join-Path $PSScriptRoot 'desktop.py') --executable $exe --output $Output --version $metadata.version --session windows
    if ($LASTEXITCODE -ne 0) { throw 'Installed desktop qualification failed' }
    $env:BUDDY_SETTINGS_FIXTURE = '1'
    & python (Join-Path $PSScriptRoot 'settings-desktop.py') --executable $exe --output (Join-Path $Output 'settings') --session windows
    if ($LASTEXITCODE -ne 0) { throw 'Installed settings qualification failed' }
    & python (Join-Path $PSScriptRoot 'github-desktop.py') --executable $exe --output (Join-Path $Output 'github') --session windows --version $metadata.version
    if ($LASTEXITCODE -ne 0) { throw 'Installed GitHub qualification failed' }
    & python (Join-Path $PSScriptRoot 'terminal-desktop.py') --executable $exe --output (Join-Path $Output 'terminal') --session windows --version $metadata.version
    if ($LASTEXITCODE -ne 0) { throw 'Installed terminal qualification failed' }
    & python (Join-Path $PSScriptRoot 'copilot-desktop.py') --executable $exe --output (Join-Path $Output 'copilot') --session windows --version $metadata.version
    if ($LASTEXITCODE -ne 0) { throw 'Installed Copilot qualification failed' }
    & python (Join-Path $PSScriptRoot 'todoist-desktop.py') --executable $exe --output (Join-Path $Output 'todoist') --session windows --version $metadata.version
    if ($LASTEXITCODE -ne 0) { throw 'Installed Todoist qualification failed' }
    $entry | Select-Object DisplayName,DisplayVersion,DisplayIcon,InstallLocation |
        ConvertTo-Json | Set-Content (Join-Path $Output 'registration.json')
}
catch { $nativePrimaryError = $_; throw }
finally {
    Invoke-NativeCleanup -PrimaryError $nativePrimaryError -Cleanup {
    if ($installed) {
        # The exact owned path is checked again before invoking its uninstaller.
        $resolved = [IO.Path]::GetFullPath($install)
        $boundary = [IO.Path]::GetFullPath($env:RUNNER_TEMP).TrimEnd('\') + '\'
        if (-not $resolved.StartsWith($boundary, [StringComparison]::OrdinalIgnoreCase)) {
            throw 'Uninstaller path escaped the runner workspace'
        }
        $uninstaller = Join-Path $resolved 'uninstall.exe'
        $cleanup = Start-Process -FilePath $uninstaller -ArgumentList '/S' -WindowStyle Hidden -PassThru
        if (-not $cleanup.WaitForExit(120000) -or $cleanup.ExitCode -ne 0) { throw 'Uninstaller failed or timed out' }
        $deadline = [DateTime]::UtcNow.AddSeconds(20)
        while ((Test-Path $registration) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 200 }
        if (Test-Path $registration) { throw 'Uninstaller left application registration behind' }
    }
    }
}
