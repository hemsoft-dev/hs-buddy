"""Exercise the real Windows driver's capture receipt and message failure boundary."""
import json
import base64
import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


HERE = Path(__file__).resolve().parent


def function(name):
    lines = (HERE / "windows-actions.ps1").read_text(encoding="utf-8").splitlines()
    start = next(i for i, line in enumerate(lines) if line.startswith(f"function {name}("))
    depth, selected = 0, []
    for line in lines[start:]:
        selected.append(line)
        depth += line.count("{") - line.count("}")
        if depth == 0:
            return "\n".join(selected)
    raise AssertionError(f"Unterminated driver function {name}")


def powershell(script, directory):
    path = directory / "probe.ps1"
    path.write_text(script, encoding="utf-8")
    result = subprocess.run(["pwsh", "-NoProfile", "-File", str(path)], capture_output=True,
        text=True, encoding="utf-8", timeout=60, check=True,
        creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
    return json.loads(result.stdout)


@unittest.skipUnless(shutil.which("pwsh"), "PowerShell is required for the Windows driver seam")
class WindowsInputReceipts(unittest.TestCase):
    def test_crew_selection_lands_inside_the_card_with_the_hosted_icon_contaminated_box(self):
        spec = importlib.util.spec_from_file_location("crew_receipt_scene", HERE / "crew-desktop.py")
        crew = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(crew)
        mode, label = crew.project_action("alpha")
        encoded = base64.b64encode(label.encode()).decode()
        fixture = json.loads((HERE / "fixtures" / "crew-windows-alpha-target.json").read_text(encoding="utf-8"))
        with tempfile.TemporaryDirectory(prefix="buddy-crew-target-") as root:
            directory = Path(root)
            (directory / "fixture.json").write_text(json.dumps(fixture), encoding="utf-8")
            script = """
$ErrorActionPreference = 'Stop'
Add-Type @'
public static class Drive {
 public struct RECT { public int l,t,r,b; }
 public struct POINT { public int x,y; }
 public static RECT Rect; public static POINT Origin; public static uint Dpi;
 public static bool GetWindowRect(System.IntPtr h, ref RECT r) { r=Rect;return true; }
 public static bool ClientToScreen(System.IntPtr h, ref POINT p) { p=Origin;return true; }
 public static uint GetDpiForWindow(System.IntPtr h) { return Dpi; }
}
'@
$fixture = Get-Content (Join-Path $PSScriptRoot 'fixture.json') -Raw | ConvertFrom-Json
$rect = [Drive+RECT]::new()
$rect.l=$fixture.window.left; $rect.t=$fixture.window.top; $rect.r=$fixture.window.right; $rect.b=$fixture.window.bottom
[Drive]::Rect=$rect
$origin = [Drive+POINT]::new(); $origin.x=$fixture.clientOrigin.x; $origin.y=$fixture.clientOrigin.y
[Drive]::Origin=$origin; [Drive]::Dpi=$fixture.dpi
$h = [IntPtr]1; $Log = Join-Path $PSScriptRoot 'actions.log'; $targetSequence=0
function Save-OwnedImage($path) { [IO.File]::WriteAllText($path, 'hosted pre-click fixture') }
function python {
 $labelIndex=[Array]::IndexOf([object[]]$args,'--label')
 $box=$fixture.labels.PSObject.Properties[$args[$labelIndex+1]].Value
 if (-not $box) { throw 'Unrecorded visible fixture label' }
 $global:LASTEXITCODE=0; $box | ConvertTo-Json -Compress
}
""" + function("Get-OwnedLabelTarget") + "\n" + function("Find-OwnedLabelPoint") + f"""
Find-OwnedLabelPoint '{encoded}' '{mode}' | ConvertTo-Json -Compress
"""
            point = powershell(script, directory)
        card = fixture["projectCardClientBounds"]
        self.assertGreaterEqual(point["X"], card["left"])
        self.assertLess(point["X"], card["right"])
        self.assertGreaterEqual(point["Y"], card["top"])
        self.assertLess(point["Y"], card["bottom"])

    def test_receipt_retains_the_actual_preclick_frame_and_100_percent_transform(self):
        # Clean recorded 100% frames have alpha at this box. The failing
        # pre-click receipt has wider bounds, covered by the regression above.
        # The outer frame includes the 8px side border; mouse messages take
        # client coordinates, so the actual rounded target is (88, 302).
        with tempfile.TemporaryDirectory(prefix="buddy-input-receipt-") as root:
            directory = Path(root)
            script = """
$ErrorActionPreference = 'Stop'
Add-Type @'
public static class Drive {
 public struct RECT { public int l,t,r,b; }
 public struct POINT { public int x,y; }
 public static bool GetWindowRect(System.IntPtr h, ref RECT r) { r.l=80;r.t=40;r.r=1124;r.b=828;return true; }
 public static bool ClientToScreen(System.IntPtr h, ref POINT p) { p.x=88;p.y=40;return true; }
 public static uint GetDpiForWindow(System.IntPtr h) { return 96; }
}
'@
$h = [IntPtr]1
$Log = Join-Path $PSScriptRoot 'actions.log'
$targetSequence = 0
function Save-OwnedImage($path) { [IO.File]::WriteAllText($path, 'pre-click pixels') }
function python { $global:LASTEXITCODE=0; '{"left":81,"top":295,"width":31,"height":13}' }
""" + function("Get-OwnedLabelTarget") + "\n" + function("Find-OwnedLabelPoint") + """
$point = Find-OwnedLabelPoint ([Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes('alpha'))) 'content'
[IO.File]::WriteAllText((Join-Path $PSScriptRoot 'settings-locator.png'), 'later failed wait pixels')
$receipt = Get-Content (Join-Path $PSScriptRoot 'label-targets.jsonl') -Raw | ConvertFrom-Json
@{ point=$point; receipt=$receipt; pixels=[IO.File]::ReadAllText((Join-Path $PSScriptRoot $receipt.image)) } |
 ConvertTo-Json -Depth 10 -Compress
"""
            result = powershell(script, directory)
            self.assertEqual(result["point"], dict(X=88, Y=302))
            self.assertEqual(result["pixels"], "pre-click pixels")
            receipt = result["receipt"]
            self.assertEqual((receipt["label"], receipt["mode"], receipt["dpi"]), ("alpha", "content", 96))
            self.assertEqual(receipt["clientPoint"], result["point"])
            self.assertEqual(receipt["window"], dict(left=80, top=40, right=1124, bottom=828))
            self.assertEqual(receipt["clientOrigin"], dict(x=88, y=40))

    def test_a_rejected_mouse_message_cannot_be_reported_as_a_successful_click(self):
        for rejected, expected in ((0, [512, 513, 514]), (512, [512]), (513, [512, 513]),
                (514, [512, 513, 514])):
            with self.subTest(rejected=rejected), tempfile.TemporaryDirectory(prefix="buddy-posted-click-") as root:
                script = """
$ErrorActionPreference = 'Stop'
Add-Type @'
public static class Drive {
 public static int Rejected;
 public static System.Collections.Generic.List<int> Messages = new System.Collections.Generic.List<int>();
 public static System.IntPtr MakeLParam(int x,int y) { return (System.IntPtr)((y<<16)|(x&65535)); }
 public static bool PostMessageW(System.IntPtr h,int message,int wparam,System.IntPtr point) {
  if (h!=(System.IntPtr)1 || point!=MakeLParam(88,302)) throw new System.Exception("Wrong owned destination");
  Messages.Add(message);return message!=Rejected;
 }
}
'@
$h = [IntPtr]1
function Note($text) {}
function Start-Sleep { param($Milliseconds) }
""" + function("Click-OwnedPoint") + f"\n[Drive]::Rejected={rejected}\n" + """
$errorMessage=$null
try { Click-OwnedPoint 88 302 } catch { $errorMessage=$_.Exception.Message }
@{ error=$errorMessage; messages=@([Drive]::Messages.ToArray()) } | ConvertTo-Json -Compress
"""
                result = powershell(script, Path(root))
                self.assertEqual(result["messages"], expected)
                if rejected:
                    self.assertIn("was not posted", result["error"])
                else:
                    self.assertIsNone(result["error"])


if __name__ == "__main__":
    unittest.main()
