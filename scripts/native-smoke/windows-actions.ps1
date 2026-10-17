#requires -Version 7
# Runs ON the hidden desktop. Drives one window with posted messages and a
# thread-attached key state (never SendInput), and captures it with PrintWindow.
# Actions (comma separated): ctrlkey:<vk>  wheel:<delta>  ctrlwheel:<delta>
#   click:<x>:<y> (client px)  capture:<file.png>  sleep:<ms>  close
param([Parameter(Mandatory)] [long] $Hwnd, [Parameter(Mandatory)] [string] $Actions,
    [Parameter(Mandatory)] [string] $Log, [Parameter(Mandatory)] [string] $ExpectedIconPath)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
. (Join-Path $PSScriptRoot 'windows-assertions.ps1')
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class Drive {
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, IntPtr pid);
    [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool attach);
    [DllImport("user32.dll")] public static extern bool GetKeyboardState(byte[] state);
    [DllImport("user32.dll")] public static extern bool SetKeyboardState(byte[] state);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern uint MapVirtualKeyW(uint code, uint mapType);
    [DllImport("user32.dll")] public static extern IntPtr SetActiveWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr SetFocus(IntPtr h);
    [StructLayout(LayoutKind.Sequential)] public struct MSG { public IntPtr h; public uint m; public IntPtr w, l; public uint t; public int x, y; }
    [DllImport("user32.dll")] public static extern bool PeekMessageW(out MSG msg, IntPtr h, uint a, uint b, uint remove);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int l, t, r, b; }
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int x, y; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
[DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int attribute, out RECT r, int size);
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int command);
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern int GetSystemMetrics(int index);
    [DllImport("user32.dll", EntryPoint = "GetClassLongPtrW")] public static extern IntPtr GetClassLongPtr(IntPtr h, int index);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
    [DllImport("user32.dll", SetLastError = true)] public static extern IntPtr SendMessageTimeoutW(IntPtr h, uint message, IntPtr wparam, ref RECT rect, uint flags, uint timeout, out UIntPtr result);

    static uint target;
    public static void Attach(IntPtr h) {
        MSG m; PeekMessageW(out m, IntPtr.Zero, 0, 0, 0);
        target = GetWindowThreadProcessId(h, IntPtr.Zero);
        AttachThreadInput(GetCurrentThreadId(), target, true);
    }
    public static void Detach() { AttachThreadInput(GetCurrentThreadId(), target, false); }
    public static void Ctrl(bool down) {
        var s = new byte[256]; GetKeyboardState(s);
        byte v = (byte)(down ? 0x80 : 0); s[0x11] = v; s[0xA2] = v; SetKeyboardState(s);
    }
    public static void Shift(bool down) {
        var s = new byte[256]; GetKeyboardState(s);
        byte v = (byte)(down ? 0x80 : 0); s[0x10] = v; s[0xA0] = v; SetKeyboardState(s);
    }
    public static IntPtr MakeLParam(int lo, int hi) { return (IntPtr)((hi << 16) | (lo & 0xFFFF)); }
}
'@
function Note($s) { if ($Log) { Add-Content $Log $s } }
[Drive]::SetThreadDpiAwarenessContext([IntPtr](-4)) | Out-Null
$h = [IntPtr]$Hwnd
$recordFrame = 0
$recorder = $null
$recordDirectory = $null
[Drive]::Attach($h)
function Save-OwnedImage([string] $Path) {
    $rect = New-Object Drive+RECT
    [Drive]::GetWindowRect($h, [ref]$rect) | Out-Null
    $bmp = [System.Drawing.Bitmap]::new(($rect.r-$rect.l), ($rect.b-$rect.t), [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
    $graphics = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $graphics.GetHdc()
    try {
        if (-not [Drive]::PrintWindow($h, $hdc, 2)) { throw 'Owned window capture failed' }
    } finally { $graphics.ReleaseHdc($hdc); $graphics.Dispose() }
    try { $bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png) }
    finally { $bmp.Dispose() }
}
function Click-OwnedPoint([int] $X, [int] $Y) {
    $lp = [Drive]::MakeLParam($X, $Y)
    [Drive]::PostMessageW($h, 0x200, 0, $lp) | Out-Null
    Start-Sleep -Milliseconds 150
    [Drive]::PostMessageW($h, 0x201, 1, $lp) | Out-Null
    Start-Sleep -Milliseconds 80
    [Drive]::PostMessageW($h, 0x202, 0, $lp) | Out-Null
    Start-Sleep -Milliseconds 600
}
function Scroll-OwnedWindow([int] $Delta, [hashtable] $Position) {
    $pt = New-Object Drive+POINT
    $pt.x = if ($Position) { $Position.X } else { 600 }
    $pt.y = if ($Position) { $Position.Y } else { 400 }
    [Drive]::ClientToScreen($h, [ref]$pt) | Out-Null
    $wparam = [IntPtr]([int64]($Delta -band 0xFFFF) -shl 16)
    [Drive]::PostMessageW($h, 0x20A, $wparam, [Drive]::MakeLParam($pt.x, $pt.y)) | Out-Null
    Start-Sleep -Milliseconds 700
}
function Write-OwnedText([string] $Encoded) {
    $text = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($Encoded))
    if ($text.Length -gt 8192) { throw 'Fixture text exceeds the bounded input size' }
    foreach ($character in $text.ToCharArray()) {
        [Drive]::PostMessageW($h, 0x102, [IntPtr]([int]$character), [IntPtr]1) | Out-Null
        Start-Sleep -Milliseconds 10
    }
    Start-Sleep -Milliseconds 400
}
function Find-OwnedLabelPoint([string] $Encoded, [string] $Mode) {
    $image = Join-Path ([IO.Path]::GetDirectoryName($Log)) 'settings-locator.png'
    Save-OwnedImage $image
    $label = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($Encoded))
    $arguments = @((Join-Path $PSScriptRoot 'settings-ocr.py'), '--image', $image, '--label', $label)
    if ($Mode -eq 'navigation') { $arguments += '--navigation' }
    if ($Mode -eq 'tab') { $arguments += '--tab' }
    if ($Mode -in @('input', 'field', 'leftfield', 'rightfield', 'save')) { $arguments += '--field' }
    $raw = & python @arguments
    if ($LASTEXITCODE -ne 0) { throw 'Owned settings label was not found unambiguously' }
    $box = $raw | ConvertFrom-Json
    $rect = New-Object Drive+RECT
    [Drive]::GetWindowRect($h, [ref]$rect) | Out-Null
    $origin = New-Object Drive+POINT
    [Drive]::ClientToScreen($h, [ref]$origin) | Out-Null
    $scale = [Drive]::GetDpiForWindow($h) / 96.0
    $x = $box.left + $box.width / 2
    $y = $box.top + $box.height / 2
    if ($Mode -in @('input', 'field', 'leftfield', 'rightfield', 'save')) { $y = $box.top + $box.height + 20 * $scale; $x = $box.left + 20 * $scale }
    if ($Mode -eq 'leftfield') { $x = ($rect.r-$rect.l) / 4 }
    if ($Mode -eq 'rightfield') { $x = 3 * ($rect.r-$rect.l) / 4 }
    if ($Mode -eq 'save') { $x = ($rect.r-$rect.l) - 49 * $scale }
    return @{ X=[int]($rect.l + $x - $origin.x); Y=[int]($rect.t + $y - $origin.y) }
}
try {
    foreach ($action in $Actions.Split(',')) {
        $parts = $action.Split(':')
        switch ($parts[0]) {
            'record-start' {
                if ($recorder) { throw 'A recording is already active' }
                $recordDirectory = $parts[1..($parts.Length - 1)] -join ':'
                if (Test-Path -LiteralPath $recordDirectory) { throw 'Recording needs a fresh directory' }
                New-Item -ItemType Directory -Path $recordDirectory | Out-Null
                $recorder = Start-ThreadJob -ArgumentList $h,$recordDirectory -ScriptBlock {
                    param($window,$directory)
                    for ($frame = 0; $frame -lt 240; $frame++) {
                        if (Test-Path -LiteralPath (Join-Path $directory 'complete')) { return }
                        $rect = New-Object Drive+RECT
                        if (-not [Drive]::GetWindowRect($window, [ref]$rect)) { throw 'Recording window disappeared' }
                        $bmp = [System.Drawing.Bitmap]::new(($rect.r-$rect.l), ($rect.b-$rect.t), [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
                        try {
                            $graphics = [System.Drawing.Graphics]::FromImage($bmp)
                            try {
                                $dc = $graphics.GetHdc()
                                try {
                                    if (-not [Drive]::PrintWindow($window, $dc, 2)) { throw 'Recording capture failed' }
                                } finally { $graphics.ReleaseHdc($dc) }
                            } finally { $graphics.Dispose() }
                            $bmp.Save((Join-Path $directory ('frame-{0:D4}.png' -f $frame)), [System.Drawing.Imaging.ImageFormat]::Png)
                        } finally { $bmp.Dispose() }
                        Start-Sleep -Milliseconds 250
                    }
                    throw 'Recording exceeded 60 seconds'
                }
            }
            'record-stop' {
                if (-not $recorder) { throw 'No recording is active' }
                New-Item -ItemType File (Join-Path $recordDirectory 'complete') | Out-Null
                $finished = Wait-Job $recorder -Timeout 10
                if (-not $finished -or $recorder.State -ne 'Completed') { throw 'Recording did not complete' }
                Receive-Job $recorder -ErrorAction Stop | Out-Null
                Remove-Job $recorder
                $recorder = $null
            }
            'sleep' { Start-Sleep -Milliseconds ([int]$parts[1]) }
            { $_ -in 'waittext64', 'waitmodeltext64', 'waitscrolltext64', 'revealtext64' } {
                # Explicit scroll waits may reveal content on this owned HWND.
                # Never advance until the exact status is visibly observed.
                $scrollPoint = if ($parts[0] -eq 'revealtext64') { Find-OwnedLabelPoint $parts[2] 'button' } else { $null }
                $scrollDelta = if ($parts[0] -eq 'revealtext64') { -120 } else { -1200 }
                # Model discovery includes the existing bounded cold SDK bootstrap.
                $timeoutSeconds = if ($parts[0] -eq 'waitmodeltext64') { 200 } else { 15 }
                $timer = [Diagnostics.Stopwatch]::StartNew()
                while ($true) {
                    try {
                        Find-OwnedLabelPoint $parts[1] 'button' | Out-Null
                        break
                    } catch {
                        if ($timer.Elapsed.TotalSeconds -ge $timeoutSeconds) { throw }
                        if ($parts[0] -in 'waitscrolltext64', 'revealtext64') { Scroll-OwnedWindow $scrollDelta $scrollPoint }
                        Start-Sleep -Milliseconds 100
                    }
                }
            }
            'ctrlkey' {
                $vk = [int]$parts[1]
                [Drive]::Ctrl($true)
                [Drive]::PostMessageW($h, 0x100, [IntPtr]0x11, [IntPtr]0x001D0001) | Out-Null
                [Drive]::PostMessageW($h, 0x100, [IntPtr]$vk, [IntPtr]0x00000001) | Out-Null
                Start-Sleep -Milliseconds 400
                [Drive]::PostMessageW($h, 0x101, [IntPtr]$vk, [IntPtr]0xC0000001) | Out-Null
                Start-Sleep -Milliseconds 100
                [Drive]::Ctrl($false)
                [Drive]::PostMessageW($h, 0x101, [IntPtr]0x11, [IntPtr]0xC01D0001) | Out-Null
                Start-Sleep -Milliseconds 400
            }
            'ctrlshiftkey' {
                $vk = [int]$parts[1]
                [Drive]::Ctrl($true); [Drive]::Shift($true)
                [Drive]::PostMessageW($h, 0x100, [IntPtr]0x11, [IntPtr]0x001D0001) | Out-Null
                [Drive]::PostMessageW($h, 0x100, [IntPtr]0x10, [IntPtr]0x002A0001) | Out-Null
                [Drive]::PostMessageW($h, 0x100, [IntPtr]$vk, [IntPtr]0x00000001) | Out-Null
                Start-Sleep -Milliseconds 400
                [Drive]::PostMessageW($h, 0x101, [IntPtr]$vk, [IntPtr]0xC0000001) | Out-Null
                Start-Sleep -Milliseconds 100
                [Drive]::Shift($false); [Drive]::Ctrl($false)
                [Drive]::PostMessageW($h, 0x101, [IntPtr]0x10, [IntPtr]0xC02A0001) | Out-Null
                [Drive]::PostMessageW($h, 0x101, [IntPtr]0x11, [IntPtr]0xC01D0001) | Out-Null
                Start-Sleep -Milliseconds 400
            }
            'focus' {
                [Drive]::SetActiveWindow($h) | Out-Null
                [Drive]::SetFocus($h) | Out-Null
                Start-Sleep -Milliseconds 400
            }
            'text64' {
                # Fixture text goes only to this owned HWND. No clipboard,
                # global input, shell expansion, or payload logging.
                Write-OwnedText $parts[1]
            }
            { $_ -in 'nav64', 'tab64', 'click64', 'input64', 'field64', 'leftfield64', 'rightfield64', 'save64' } {
                $mode = switch ($parts[0]) { 'nav64' {'navigation'} 'tab64' {'tab'} 'input64' {'input'} 'field64' {'field'} 'leftfield64' {'leftfield'} 'rightfield64' {'rightfield'} 'save64' {'save'} default {'button'} }
                $point = Find-OwnedLabelPoint $parts[1] $mode
                Click-OwnedPoint $point.X $point.Y
                if ($mode -in @('input', 'field', 'leftfield', 'rightfield')) {
                    [Drive]::Ctrl($true)
                    [Drive]::PostMessageW($h, 0x100, [IntPtr]65, [IntPtr]0x1E0001) | Out-Null
                    Start-Sleep -Milliseconds 100
                    [Drive]::PostMessageW($h, 0x101, [IntPtr]65, [IntPtr]0xC01E0001L) | Out-Null
                    Start-Sleep -Milliseconds 400
                    [Drive]::Ctrl($false)
                    Write-OwnedText $parts[2]
                }
            }
            { $_ -in 'key', 'gpuikey' } {
                $vk = [int]$parts[1]
                $keyBits = ([long][Drive]::MapVirtualKeyW($vk, 0) -shl 16) -bor 1
                $message = if ($parts[0] -eq 'gpuikey') { 0x408 } else { 0x100 }
                [Drive]::PostMessageW($h, $message, [IntPtr]$vk, [IntPtr]$keyBits) | Out-Null
                Start-Sleep -Milliseconds 100
                [Drive]::PostMessageW($h, 0x101, [IntPtr]$vk, [IntPtr]($keyBits -bor 0xC0000000L)) | Out-Null
                Start-Sleep -Milliseconds 400
            }
            { $_ -in 'wheel', 'ctrlwheel' } {
                $delta = [int]$parts[1]
                $pt = New-Object Drive+POINT; $pt.x = 600; $pt.y = 400
                [Drive]::ClientToScreen($h, [ref]$pt) | Out-Null
                $ctrl = $_ -eq 'ctrlwheel'
                if ($ctrl) { [Drive]::Ctrl($true) }
                $wparam = [IntPtr](([int64]($delta -band 0xFFFF) -shl 16) -bor ($(if ($ctrl) { 0x8 } else { 0 })))
                [Drive]::PostMessageW($h, 0x20A, $wparam, [Drive]::MakeLParam($pt.x, $pt.y)) | Out-Null
                Start-Sleep -Milliseconds 400
                if ($ctrl) { [Drive]::Ctrl($false) }
                Start-Sleep -Milliseconds 300
            }
            'click' {
                $lp = [Drive]::MakeLParam([int]$parts[1], [int]$parts[2])
                [Drive]::PostMessageW($h, 0x200, 0, $lp) | Out-Null
                Start-Sleep -Milliseconds 150
                [Drive]::PostMessageW($h, 0x201, 1, $lp) | Out-Null
                Start-Sleep -Milliseconds 80
                [Drive]::PostMessageW($h, 0x202, 0, $lp) | Out-Null
                Start-Sleep -Milliseconds 600
            }
            'move' {
                $lp = [Drive]::MakeLParam([int]$parts[1], [int]$parts[2])
                [Drive]::PostMessageW($h, 0x200, 0, $lp) | Out-Null
                Start-Sleep -Milliseconds 400
            }
            'capture' {
                $path = $parts[1..($parts.Length - 1)] -join ':'
                $rect = New-Object Drive+RECT
                [Drive]::GetWindowRect($h, [ref]$rect) | Out-Null
                $w = $rect.r - $rect.l; $ht = $rect.b - $rect.t
                $bmp = [System.Drawing.Bitmap]::new($w, $ht, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
                $g = [System.Drawing.Graphics]::FromImage($bmp)
                $hdc = $g.GetHdc()
                $ok = [Drive]::PrintWindow($h, $hdc, 2)
                $g.ReleaseHdc($hdc); $g.Dispose()
                $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
                Note ("capture {0}: ok={1} {2}x{3} dpi={4}" -f $path, $ok, $w, $ht, [Drive]::GetDpiForWindow($h))
            }
            'record' {
                $seconds = [int]$parts[1]
                $path = $parts[2..($parts.Length - 1)] -join ':'
                New-Item -ItemType Directory -Path $path -Force | Out-Null
                for ($frame = 0; $frame -lt $seconds * 5; $frame++) {
                    $rect = New-Object Drive+RECT
                    [Drive]::GetWindowRect($h, [ref]$rect) | Out-Null
                    $bmp = [System.Drawing.Bitmap]::new(($rect.r-$rect.l), ($rect.b-$rect.t), [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
                    $g = [System.Drawing.Graphics]::FromImage($bmp)
                    $hdc = $g.GetHdc()
                    if (-not [Drive]::PrintWindow($h, $hdc, 2)) { throw 'Recording capture failed' }
                    $g.ReleaseHdc($hdc); $g.Dispose()
                    $bmp.Save((Join-Path $path ('frame-{0:D4}.png' -f $recordFrame)), [System.Drawing.Imaging.ImageFormat]::Png)
                    $bmp.Dispose(); $recordFrame++
                    Start-Sleep -Milliseconds 200
                }
                Note "recorded $seconds seconds, $recordFrame frames"
            }
            'classicon' {
                # The window class icons GPUI registered (taskbar/Alt+Tab use these).
                $path = $parts[1..($parts.Length - 1)] -join ':'
                $big = [Drive]::GetClassLongPtr($h, -14)    # GCLP_HICON
                $small = [Drive]::GetClassLongPtr($h, -34)  # GCLP_HICONSM
                $validated = @{
                    big = Assert-NativeIcon -Handle $big -ExpectedIconPath $ExpectedIconPath
                    small = Assert-NativeIcon -Handle $small -ExpectedIconPath $ExpectedIconPath
                    expectedSha256 = (Get-FileHash -LiteralPath $ExpectedIconPath -Algorithm SHA256).Hash.ToLowerInvariant()
                }
                $validated | ConvertTo-Json -Depth 4 | Set-Content (Join-Path ([IO.Path]::GetDirectoryName($Log)) 'registered-window-icons.json')
                $bmp = [System.Drawing.Bitmap]::new(420, 220)
                $g = [System.Drawing.Graphics]::FromImage($bmp)
                $g.Clear([System.Drawing.Color]::FromArgb(32, 32, 32))
                $font = [System.Drawing.Font]::new('Segoe UI', 12)
                if ($big -ne [IntPtr]::Zero) { $g.DrawIcon([System.Drawing.Icon]::FromHandle($big), [System.Drawing.Rectangle]::new(60, 50, 96, 96)) }
                if ($small -ne [IntPtr]::Zero) { $g.DrawIcon([System.Drawing.Icon]::FromHandle($small), [System.Drawing.Rectangle]::new(290, 82, 32, 32)) }
                $g.DrawString('GCLP_HICON', $font, [System.Drawing.Brushes]::White, 55, 170)
                $g.DrawString('GCLP_HICONSM', $font, [System.Drawing.Brushes]::White, 250, 170)
                $g.Dispose(); $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
                Note ("classicon {0}: big=0x{1:X} small=0x{2:X}" -f $path, $big.ToInt64(), $small.ToInt64())
            }
            'resize' {
                # x:y:w:h in screen px; SWP_NOZORDER | SWP_NOACTIVATE
                if (-not [Drive]::SetWindowPos($h, 0, [int]$parts[1], [int]$parts[2], [int]$parts[3], [int]$parts[4], 0x14)) { throw 'Owned window resize request failed' }
                Start-Sleep -Milliseconds 800
                $actual = New-Object Drive+RECT
                if (-not [Drive]::GetWindowRect($h, [ref]$actual)) { throw 'Owned resized window disappeared' }
                $size = @{ width = $actual.r - $actual.l; height = $actual.b - $actual.t; units = 'Windows outer physical pixels' }
                Note "resize requested $($parts[3])x$($parts[4]); observed $($size.width)x$($size.height)"
                if ([Math]::Abs($size.width - [int]$parts[3]) -gt 2 -or [Math]::Abs($size.height - [int]$parts[4]) -gt 2) { throw 'Owned window ignored or clamped the requested resize' }
                $size | ConvertTo-Json | Set-Content (Join-Path ([IO.Path]::GetDirectoryName($Log)) 'resize.json')
            }
            'simulated-dpi' {
                # Exercise GPUI's actual WM_DPICHANGED path without changing
                # the user's monitor settings. This is explicitly simulated;
                # GetDpiForWindow still reports the real monitor's DPI.
                $dpi = [int]$parts[1]
                if ($dpi -notin 96, 120, 144, 192) { throw 'Unsupported test DPI' }
                $rect = New-Object Drive+RECT
                $rect.l = 80; $rect.t = 40
                $rect.r = $rect.l + [int]$parts[2]
                $rect.b = $rect.t + [int]$parts[3]
                $result = [UIntPtr]::Zero
                $ok = [Drive]::SendMessageTimeoutW($h, 0x2E0, [IntPtr](($dpi -shl 16) -bor $dpi), [ref]$rect, 2, 5000, [ref]$result)
                if ($ok -eq [IntPtr]::Zero) { throw 'Simulated DPI event timed out' }
                Start-Sleep -Milliseconds 900
                $actual = New-Object Drive+RECT
                if (-not [Drive]::GetWindowRect($h, [ref]$actual)) { throw 'Owned window disappeared after the DPI event' }
                $size = @{ width = $actual.r - $actual.l; height = $actual.b - $actual.t; units = 'Windows outer physical pixels'; simulatedDpi = $dpi; actualMonitorDpi = [Drive]::GetDpiForWindow($h); defaultMaxTrackWidth = [Drive]::GetSystemMetrics(59); defaultMaxTrackHeight = [Drive]::GetSystemMetrics(60) }
                Note "simulated DPI=$dpi requested $($parts[2])x$($parts[3]); observed $($size.width)x$($size.height)"
                if ([Math]::Abs($size.width - [int]$parts[2]) -gt 2 -or [Math]::Abs($size.height - [int]$parts[3]) -gt 2) { throw 'Owned window ignored or clamped the simulated DPI rectangle' }
                $size | ConvertTo-Json | Set-Content (Join-Path ([IO.Path]::GetDirectoryName($Log)) "dpi-$dpi.json")
                Note "simulated WM_DPICHANGED=$dpi; actual monitor DPI=$([Drive]::GetDpiForWindow($h))"
            }
            'maximize' { [Drive]::ShowWindow($h, 3) | Out-Null; Start-Sleep -Milliseconds 1000 }
            'minimize' { [Drive]::ShowWindow($h, 6) | Out-Null; Start-Sleep -Milliseconds 1000 }
            'restore' { [Drive]::ShowWindow($h, 9) | Out-Null; Start-Sleep -Milliseconds 1000 }
            'geometry' {
                $r = New-Object Drive+RECT; $d = New-Object Drive+RECT
                [Drive]::GetWindowRect($h,[ref]$r) | Out-Null; $result=[Drive]::DwmGetWindowAttribute($h,9,[ref]$d,16)
                Note ("geometry: raw={0},{1},{2},{3} dwm={4},{5},{6},{7} dpi={8} result={9}" -f $r.l,$r.t,$r.r,$r.b,$d.l,$d.t,$d.r,$d.b,[Drive]::GetDpiForWindow($h),$result)
                Note ("desktop={0}x{1}; default max-track={2}x{3}" -f [Drive]::GetSystemMetrics(0),[Drive]::GetSystemMetrics(1),[Drive]::GetSystemMetrics(59),[Drive]::GetSystemMetrics(60))
            }
            'rect' {
                $rect = New-Object Drive+RECT
                [Drive]::GetWindowRect($h, [ref]$rect) | Out-Null
                Note ("rect: x={0} y={1} w={2} h={3}" -f $rect.l, $rect.t, ($rect.r - $rect.l), ($rect.b - $rect.t))
            }
            'close' { [Drive]::PostMessageW($h, 0x10, 0, 0) | Out-Null }
            default { throw "unknown action $($parts[0])" }
        }
        if ($parts[0] -in @('text64', 'input64', 'field64', 'leftfield64', 'rightfield64')) { Note "done $($parts[0]) [payload withheld]" }
        else { Note "done $action" }
    }
}
finally {
    if ($recorder) {
        Stop-Job $recorder
        Remove-Job $recorder -Force
    }
    [Drive]::Detach()
}

