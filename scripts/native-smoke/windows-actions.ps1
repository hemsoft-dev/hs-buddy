#requires -Version 7
# Runs ON the hidden desktop. Drives one window with posted messages and a
# thread-attached key state (never SendInput), and captures it with PrintWindow.
# Actions (comma separated): ctrlkey:<vk>  wheel:<delta>  ctrlwheel:<delta>
#   click:<x>:<y> (client px)  capture:<file.png>  sleep:<ms>  close
param([Parameter(Mandatory)] [long] $Hwnd, [Parameter(Mandatory)] [string] $Actions, [string] $Log)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
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
    [DllImport("user32.dll", EntryPoint = "GetClassLongPtrW")] public static extern IntPtr GetClassLongPtr(IntPtr h, int index);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);

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
[Drive]::Attach($h)
try {
    foreach ($action in $Actions.Split(',')) {
        $parts = $action.Split(':')
        switch ($parts[0]) {
            'sleep' { Start-Sleep -Milliseconds ([int]$parts[1]) }
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
                [Drive]::SetWindowPos($h, 0, [int]$parts[1], [int]$parts[2], [int]$parts[3], [int]$parts[4], 0x14) | Out-Null
                Start-Sleep -Milliseconds 800
            }
            'maximize' { [Drive]::ShowWindow($h, 3) | Out-Null; Start-Sleep -Milliseconds 1000 }
            'minimize' { [Drive]::ShowWindow($h, 6) | Out-Null; Start-Sleep -Milliseconds 1000 }
            'restore' { [Drive]::ShowWindow($h, 9) | Out-Null; Start-Sleep -Milliseconds 1000 }
            'geometry' {
                $r = New-Object Drive+RECT; $d = New-Object Drive+RECT
                [Drive]::GetWindowRect($h,[ref]$r) | Out-Null; $result=[Drive]::DwmGetWindowAttribute($h,9,[ref]$d,16)
                Note ("geometry: raw={0},{1},{2},{3} dwm={4},{5},{6},{7} dpi={8} result={9}" -f $r.l,$r.t,$r.r,$r.b,$d.l,$d.t,$d.r,$d.b,[Drive]::GetDpiForWindow($h),$result)
            }
            'rect' {
                $rect = New-Object Drive+RECT
                [Drive]::GetWindowRect($h, [ref]$rect) | Out-Null
                Note ("rect: x={0} y={1} w={2} h={3}" -f $rect.l, $rect.t, ($rect.r - $rect.l), ($rect.b - $rect.t))
            }
            'close' { [Drive]::PostMessageW($h, 0x10, 0, 0) | Out-Null }
            default { throw "unknown action $action" }
        }
        Note "done $action"
    }
}
finally { [Drive]::Detach() }

