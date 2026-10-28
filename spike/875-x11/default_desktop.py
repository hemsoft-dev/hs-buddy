"""Spike-only driver tweak: run the app on the runner's existing Default desktop
instead of a fresh CreateDesktopW desktop, to test whether WebView2's
"Invalid window handle" (0x80070578) comes from the hidden desktop.
usage: default_desktop.py <checkout>"""
import sys
from pathlib import Path

path = Path(sys.argv[1]) / "scripts" / "native-smoke" / "windows-window.ps1"
text = path.read_text(encoding="utf-8")
edits = [
    ('    public static extern IntPtr CreateDesktopW(string name, IntPtr device, IntPtr devmode, int flags, uint access, IntPtr sa);\n',
     '    public static extern IntPtr CreateDesktopW(string name, IntPtr device, IntPtr devmode, int flags, uint access, IntPtr sa);\n'
     '    [DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Unicode)]\n'
     '    public static extern IntPtr OpenDesktopW(string name, int flags, bool inherit, uint access);\n'),
    ('$desktopName = "buddy-package-smoke-$PID-$([Guid]::NewGuid().ToString(\'N\'))"\n'
     '$desk = [HiddenDesktop]::CreateDesktopW($desktopName, 0, 0, 0, 0x10000000, 0)\n',
     '$desktopName = "WinSta0\\Default"\n'
     '$desk = [HiddenDesktop]::OpenDesktopW("Default", 0, $false, 0x10000000)\n'),
]
for old, new in edits:
    assert text.count(old) == 1, old
    text = text.replace(old, new)
path.write_text(text, encoding="utf-8")
print("app runs on WinSta0\\Default")
