#requires -Version 7
# One owned process on a never-shown desktop. No global input or user windows.
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Exe,
    [Parameter(Mandatory)] [string] $OutDir,
    [Parameter(Mandatory)] [string] $ConfigPath,
    [Parameter(Mandatory)] [string] $Actions,
    [string] $ConvexUrl = 'http://127.0.0.1:9'
)
$ErrorActionPreference = 'Stop'
$backend = [Uri]$ConvexUrl
if (-not $backend.IsLoopback -or $backend.Scheme -ne 'http' -or $backend.UserInfo) {
    throw 'Desktop fixtures require an isolated loopback Convex backend'
}
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class HiddenDesktop {
    [DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern IntPtr CreateDesktopW(string name, IntPtr device, IntPtr devmode, int flags, uint access, IntPtr sa);
    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool CloseDesktop(IntPtr desk);
    public delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr lparam);
    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool EnumDesktopWindows(IntPtr desk, EnumWindowsProc proc, IntPtr lparam);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetWindowTextW(IntPtr hwnd, StringBuilder text, int max);
    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll")]
    public static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool PostMessageW(IntPtr hwnd, uint msg, IntPtr wparam, IntPtr lparam);

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct STARTUPINFO {
        public int cb; public string lpReserved; public string lpDesktop; public string lpTitle;
        public int dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags;
        public short wShowWindow, cbReserved2; public IntPtr lpReserved2, hStdInput, hStdOutput, hStdError;
    }
    [StructLayout(LayoutKind.Sequential)]
    public struct PROCESS_INFORMATION { public IntPtr hProcess, hThread; public int dwProcessId, dwThreadId; }
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern bool CreateProcessW(string app, StringBuilder cmd, IntPtr pa, IntPtr ta, bool inherit,
        uint flags, IntPtr env, string cwd, ref STARTUPINFO si, out PROCESS_INFORMATION pi);
    [DllImport("kernel32.dll")]
    public static extern uint WaitForSingleObject(IntPtr h, uint ms);
    [DllImport("kernel32.dll")]
    public static extern bool GetExitCodeProcess(IntPtr h, out uint code);
    [DllImport("kernel32.dll")]
    public static extern bool TerminateProcess(IntPtr h, uint code);
    [DllImport("kernel32.dll")]
    public static extern bool CloseHandle(IntPtr h);

    public static List<IntPtr> TopLevelWindows(IntPtr desk, uint pid) {
        var found = new List<IntPtr>();
        EnumDesktopWindows(desk, (h, l) => {
            uint owner; GetWindowThreadProcessId(h, out owner);
            if (owner == pid && IsWindowVisible(h)) found.Add(h);
            return true;
        }, IntPtr.Zero);
        return found;
    }
    public static string Title(IntPtr h) { var sb = new StringBuilder(256); GetWindowTextW(h, sb, 256); return sb.ToString(); }
}
'@

function Start-OwnedProcess([string] $Application, [string] $Arguments) {
    # Paths are supplied as argv, never through a shell. Windows filenames
    # cannot contain a quote; reject it in the remaining caller-controlled text.
    if ($Application.Contains('"') -or $Application.Contains("`r") -or $Application.Contains("`n")) {
        throw 'Invalid application path'
    }
    $si = [HiddenDesktop+STARTUPINFO]::new()
    $si.cb = [Runtime.InteropServices.Marshal]::SizeOf($si)
    $si.lpDesktop = $desktopName
    $pi = [HiddenDesktop+PROCESS_INFORMATION]::new()
    $line = [Text.StringBuilder]::new('"' + $Application + '" ' + $Arguments)
    if (-not [HiddenDesktop]::CreateProcessW($Application, $line, 0, 0, $false, 0x08000000, 0, $OutDir, [ref]$si, [ref]$pi)) {
        throw "CreateProcess failed: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
    }
    [HiddenDesktop]::CloseHandle($pi.hThread) | Out-Null
    return $pi
}

$OutDir = [IO.Path]::GetFullPath($OutDir)
$Exe = (Resolve-Path -LiteralPath $Exe).Path
$ConfigPath = (Resolve-Path -LiteralPath $ConfigPath).Path
New-Item -ItemType Directory -Force $OutDir | Out-Null
$desktopName = "buddy-package-smoke-$PID-$([Guid]::NewGuid().ToString('N'))"
$desk = [HiddenDesktop]::CreateDesktopW($desktopName, 0, 0, 0, 0x10000000, 0)
if ($desk -eq [IntPtr]::Zero) { throw 'CreateDesktop failed' }
$app = $null
$helper = $null
$savedEnvironment = @{}
try {
    $environment = @{
        BUDDY_CONFIG_PATH = $ConfigPath
        BUDDY_CONVEX_URL = $ConvexUrl
        GH_CONFIG_DIR = (Join-Path $OutDir 'empty-gh')
        GH_TOKEN = $null
        GITHUB_TOKEN = $null
    }
    New-Item -ItemType Directory $environment.GH_CONFIG_DIR -Force | Out-Null
    foreach ($name in $environment.Keys) {
        $savedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
        [Environment]::SetEnvironmentVariable($name, $environment[$name], 'Process')
    }
    $app = Start-OwnedProcess $Exe ''
    $hwnd = $null
    $deadline = [DateTime]::UtcNow.AddSeconds(45)
    while ([DateTime]::UtcNow -lt $deadline) {
        if ([HiddenDesktop]::WaitForSingleObject($app.hProcess, 0) -eq 0) { throw 'App exited before rendering' }
        $hwnd = [HiddenDesktop]::TopLevelWindows($desk, [uint32]$app.dwProcessId) |
            Where-Object { [HiddenDesktop]::Title($_) -eq 'Buddy' } | Select-Object -First 1
        if ($hwnd) { break }
        Start-Sleep -Milliseconds 200
    }
    if (-not $hwnd) { throw 'Owned Buddy window did not appear' }
    Start-Sleep -Seconds 4
    $expectedIconPath = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../public/icon.ico'))
    foreach ($value in @($Actions, $OutDir, $PSScriptRoot, $expectedIconPath)) {
        if ($value.Contains('"') -or $value.Contains("`r") -or $value.Contains("`n")) { throw 'Invalid helper argument' }
    }
    $helperArguments = '-NoProfile -File "' + (Join-Path $PSScriptRoot 'windows-actions.ps1') +
        '" -Hwnd ' + $hwnd.ToInt64() + ' -Actions "' + $Actions + '" -Log "' + (Join-Path $OutDir 'actions.log') + '" -ExpectedIconPath "' + $expectedIconPath + '"'
    $helper = Start-OwnedProcess (Get-Command pwsh).Source $helperArguments
    if ([HiddenDesktop]::WaitForSingleObject($helper.hProcess, 120000) -ne 0) { throw 'Window actions timed out' }
    $helperExit = [uint32]0
    if (-not [HiddenDesktop]::GetExitCodeProcess($helper.hProcess, [ref]$helperExit) -or $helperExit -ne 0) {
        throw "Window actions failed with exit $helperExit"
    }
    if ([HiddenDesktop]::WaitForSingleObject($app.hProcess, 20000) -ne 0) { throw 'App did not exit cleanly' }
    $appExit = [uint32]0
    if (-not [HiddenDesktop]::GetExitCodeProcess($app.hProcess, [ref]$appExit) -or $appExit -ne 0) {
        throw "App failed with exit $appExit"
    }
    @{ processId = $app.dwProcessId; exitCode = $appExit; desktop = $desktopName } |
        ConvertTo-Json | Set-Content (Join-Path $OutDir 'process.json')
}
finally {
    foreach ($owned in @($helper, $app)) {
        if ($null -ne $owned) {
            if ([HiddenDesktop]::WaitForSingleObject($owned.hProcess, 0) -ne 0) {
                [HiddenDesktop]::TerminateProcess($owned.hProcess, 99) | Out-Null
            }
            [HiddenDesktop]::CloseHandle($owned.hProcess) | Out-Null
        }
    }
    foreach ($name in $savedEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name], 'Process')
    }
    [HiddenDesktop]::CloseDesktop($desk) | Out-Null
}


