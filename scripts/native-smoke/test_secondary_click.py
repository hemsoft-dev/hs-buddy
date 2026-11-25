"""A tab menu needs the secondary button on every hosted driver, never a left click.

Real driver code with fake OS bindings only: these prove which events each
driver constructs and posts, not that a hosted desktop delivers them.
"""
import base64
import importlib.util
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import unittest
from unittest.mock import MagicMock, Mock, call, patch

HERE = Path(__file__).resolve().parent


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), HERE / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


desktop, inputs = load("settings-desktop"), load("macos-input")
TAB = "Needs Review"
ENCODED = base64.b64encode(TAB.encode()).decode()
# A tab title's OCR box in a 1200x820 window.
LABEL = dict(left=386, top=44, width=81, height=10)


def unix_driver(session):
    driver = desktop.Driver.__new__(desktop.Driver)
    driver.session = session
    return driver


def capture_directory(scale=1):
    directory = MagicMock()
    image = directory.__truediv__.return_value
    image.read_bytes.return_value = bytes(16) + struct.pack(">II", 1200 * scale, 820 * scale)
    image.with_suffix.return_value.read_text.return_value = json.dumps(dict(
        kCGWindowBounds=dict(X=100, Y=50), capturePixelsPerWindowPoint=scale))
    return directory


class X11SecondaryClick(unittest.TestCase):
    def press(self, kind):
        driver, window = unix_driver("x11"), Mock()
        with patch.object(desktop.ocr, "locate_image", return_value=dict(LABEL)) as locate, \
                patch.object(desktop, "run") as run, patch.object(desktop.time, "sleep"):
            driver.interact(window, capture_directory(), kind, [TAB])
        # The owned window is activated, then receives one move and one press.
        self.assertEqual(run.call_args_list[0], call("xdotool", "windowactivate", "--sync", window.window))
        moves = [args.args for args in run.call_args_list if args.args[:2] == ("xdotool", "mousemove")]
        self.assertEqual(len(moves), 1)
        self.assertEqual(moves[0][:4], ("xdotool", "mousemove", "--window", window.window))
        return moves[0][4:], (locate.call_args.args[1], locate.call_args.kwargs)

    def test_a_tab_menu_presses_button_three_where_a_tab_click_presses_button_one(self):
        click, click_locator = self.press("workspace")
        menu, menu_locator = self.press("workspace_menu")
        # The same point, the middle of the tab title; only the button differs.
        self.assertEqual(click, (426, 49, "click", 1))
        self.assertEqual(menu, (426, 49, "click", 3))
        self.assertEqual(menu_locator, click_locator)
        self.assertEqual(menu_locator[0], TAB)
        self.assertTrue(menu_locator[1]["workspace"])

    def test_a_tab_menu_names_exactly_one_tab_before_any_input(self):
        driver = unix_driver("x11")
        with patch.object(desktop.ocr, "locate_image") as locate, patch.object(desktop, "run") as run:
            with self.assertRaisesRegex(RuntimeError, "one visible tab title"):
                driver.interact(Mock(), capture_directory(), "workspace_menu", [TAB, "Dashboard"])
        locate.assert_not_called()
        run.assert_not_called()


class MacSecondaryClick(unittest.TestCase):
    def helper(self):
        value = object.__new__(inputs.OwnedMacInput)
        value.process, value.graphics, value.core = Mock(), Mock(), Mock()
        value.process.poll.return_value = None
        value.graphics.CGPreflightPostEventAccess.return_value = True
        value.release_modifiers = Mock()
        # A fake event is (type, button): the two values Quartz would keep.
        value.graphics.CGEventCreateMouseEvent.side_effect = lambda _source, kind, _point, button: (kind, button)
        value.graphics.CGEventGetType.side_effect = lambda event: event[0]
        value.graphics.CGEventGetLocation.side_effect = lambda event: inputs.Point(704.5, 290.25)
        value.graphics.CGEventGetFlags.return_value = 0
        # Field 1 is the click state, field 3 the button number.
        value.graphics.CGEventGetIntegerValueField.side_effect = lambda event, field: 1 if field == 1 else event[1]
        value.graphics.CGEventSourceButtonState.side_effect = [False, True, False]
        return value

    def press(self, value, method="secondary_click"):
        with patch.object(inputs.time, "sleep"):
            getattr(value, method)(704.5, 290.25)
        return [posted.args[1] for posted in value.graphics.CGEventPost.call_args_list]

    def test_secondary_click_posts_right_down_and_up_after_a_plain_move(self):
        value = self.helper()
        # kCGEventMouseMoved, kCGEventRightMouseDown and kCGEventRightMouseUp
        # with kCGMouseButtonRight; the move carries no pressed button.
        self.assertEqual(self.press(value), [(5, 0), (3, 1), (4, 1)])
        self.assertEqual([posted.args[0] for posted in value.graphics.CGEventPost.call_args_list], [0, 0, 0])
        for created in value.graphics.CGEventCreateMouseEvent.call_args_list:
            self.assertEqual((created.args[2].x, created.args[2].y), (704.5, 290.25))
        value.release_modifiers.assert_called_once_with()
        # Both presses are single clicks, and every event is released.
        self.assertEqual([marked.args for marked in value.graphics.CGEventSetIntegerValueField.call_args_list],
            [((3, 1), 1, 1), ((4, 1), 1, 1)])
        self.assertEqual([cleared.args for cleared in value.graphics.CGEventSetFlags.call_args_list],
            [((5, 0), 0), ((3, 1), 0), ((4, 1), 0)])
        self.assertEqual([released.args[0] for released in value.core.CFRelease.call_args_list],
            [(5, 0), (3, 1), (4, 1)])

    def test_the_trace_names_the_right_button_and_never_a_left_press(self):
        value = self.helper()
        self.press(value)
        trace = value.last_click_trace
        self.assertEqual([phase["eventType"] for phase in trace], [5, 3, 4])
        self.assertEqual([phase["rightButtonDown"] for phase in trace], [False, True, False])
        self.assertTrue(all("leftButtonDown" not in phase for phase in trace))
        # The pressed-state telemetry reads the right button of the session.
        self.assertEqual([state.args for state in value.graphics.CGEventSourceButtonState.call_args_list],
            [(inputs.COMBINED_SESSION, 1)] * 3)
        readings = [phase["constructedEvent"] for phase in trace]
        self.assertEqual([reading["actualType"] for reading in readings], ["move", "down", "up"])
        for reading in readings[1:]:
            self.assertEqual(set(reading), {"status", "actualType", "typeMatches", "locationMatches",
                "rightButtonMatches", "clickStateMatches", "flagsClear"})
            self.assertTrue(all(reading[key] is True for key in
                ("typeMatches", "locationMatches", "rightButtonMatches", "clickStateMatches", "flagsClear")))
        for phase in trace:
            self.assertLessEqual({"postCall", "producerTimestamp", "postReturnTimestamp"}, set(phase))

    def test_a_left_event_is_not_receipted_as_the_requested_right_press(self):
        value = self.helper()
        # Quartz hands back left-button events whatever was requested.
        value.graphics.CGEventCreateMouseEvent.side_effect = lambda _s, kind, _p, _b: ({3: 1, 4: 2}.get(kind, kind), 0)
        self.press(value)
        for reading in [phase["constructedEvent"] for phase in value.last_click_trace][1:]:
            self.assertIs(reading["typeMatches"], False)
            self.assertIs(reading["rightButtonMatches"], False)
            self.assertEqual(reading["actualType"], "other")

    def test_the_left_click_still_posts_only_left_button_events(self):
        value = self.helper()
        self.assertEqual(self.press(value, "click"), [(5, 0), (1, 0), (2, 0)])
        self.assertEqual([state.args for state in value.graphics.CGEventSourceButtonState.call_args_list],
            [(inputs.COMBINED_SESSION, 0)] * 3)
        for phase in value.last_click_trace:
            self.assertIn("leftButtonDown", phase)
            self.assertIn("leftButtonMatches", phase["constructedEvent"])
            self.assertNotIn("rightButtonMatches", phase["constructedEvent"])

    def test_denied_post_access_injects_no_right_button_event(self):
        value = self.helper()
        value.graphics.CGPreflightPostEventAccess.return_value = False
        with self.assertRaisesRegex(RuntimeError, "cannot post input"):
            self.press(value)
        value.graphics.CGEventPost.assert_not_called()
        value.core.CFRelease.assert_called_once_with((5, 0))

    def test_stuck_modifiers_fail_before_any_right_button_event(self):
        value = self.helper()
        value.release_modifiers.side_effect = RuntimeError("modifier state did not clear")
        with self.assertRaisesRegex(RuntimeError, "modifier"):
            self.press(value)
        value.graphics.CGEventCreateMouseEvent.assert_not_called()

    def interact(self, kind):
        driver, window, helper = unix_driver("macos"), Mock(), Mock()
        helper.focus_state.return_value = dict(flags="owned fixture window")
        directory = capture_directory(scale=2)
        label = {key: value * 2 for key, value in LABEL.items()}
        with patch.object(desktop.ocr, "locate_image", return_value=label) as locate, \
                patch.object(desktop, "OwnedMacInput", return_value=helper), \
                patch.object(desktop, "run"), patch.object(desktop.time, "sleep"):
            driver.interact(window, directory, kind, [TAB])
        receipts = directory.__truediv__.return_value.open.return_value.__enter__.return_value
        written = [json.loads(line.args[0]) for line in receipts.write.call_args_list]
        return helper, (locate.call_args.args[1], locate.call_args.kwargs), written

    def test_the_driver_sends_a_tab_menu_through_the_secondary_click_only(self):
        helper, locate, written = self.interact("workspace_menu")
        # Window origin (100, 50) plus the title's middle, in window points.
        # The press is on a whole point: a half-point one leaves the pointer
        # outside the menu GPUI opens there, and the tab's tooltip covers it.
        helper.secondary_click.assert_called_once_with(526, 99)
        helper.click.assert_not_called()
        helper.replace_text.assert_not_called()
        self.assertTrue(locate[1]["workspace"])
        self.assertEqual({(row["kind"], row["label"]) for row in written}, {("workspace_menu", TAB)})
        click, click_locate, _ = self.interact("workspace")
        click.click.assert_called_once_with(526.5, 99.0)
        click.secondary_click.assert_not_called()
        self.assertEqual(click_locate, locate)


def driver_text():
    return (HERE / "windows-actions.ps1").read_text(encoding="utf-8").splitlines()


def block(start):
    lines = driver_text()
    index = next(i for i, line in enumerate(lines) if line.strip().startswith(start))
    depth, selected = 0, []
    for line in lines[index:]:
        selected.append(line)
        depth += line.count("{") - line.count("}")
        if depth == 0:
            return "\n".join(selected)
    raise AssertionError(f"Unterminated driver block {start}")


def case_body(start):
    """The action body of one `switch` case, without its condition."""
    lines = block(start).split("\n")
    body = "\n".join([lines[0][lines[0].rindex("{") + 1:]] + lines[1:])
    return body[:body.rindex("}")]


def powershell(script):
    with tempfile.TemporaryDirectory(prefix="buddy-secondary-click-") as root:
        path = Path(root) / "probe.ps1"
        path.write_text(script, encoding="utf-8")
        result = subprocess.run(["pwsh", "-NoProfile", "-File", str(path)], capture_output=True,
            text=True, encoding="utf-8", timeout=60, check=True,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
    return json.loads(result.stdout)


MESSAGE_SINK = """
$ErrorActionPreference = 'Stop'
Add-Type @'
public static class Drive {
 public static int Rejected;
 public static System.Collections.Generic.List<string> Messages = new System.Collections.Generic.List<string>();
 public static System.IntPtr MakeLParam(int x,int y) { return (System.IntPtr)((y<<16)|(x&65535)); }
 public static bool PostMessageW(System.IntPtr h,int message,int wparam,System.IntPtr point) {
  if (h!=(System.IntPtr)1 || point!=MakeLParam(88,302)) throw new System.Exception("Wrong owned destination");
  Messages.Add(message + ":" + wparam);return message!=Rejected;
 }
}
'@
$h = [IntPtr]1
function Note($text) {}
function Start-Sleep { param($Milliseconds) }
"""
# WM_MOUSEMOVE, then WM_RBUTTONDOWN with MK_RBUTTON, then WM_RBUTTONUP.
RIGHT_MESSAGES = ["512:0", "516:2", "517:0"]


@unittest.skipUnless(shutil.which("pwsh"), "PowerShell is required to execute the Windows driver")
class WindowsSecondaryClick(unittest.TestCase):
    def post(self, rejected=0):
        return powershell(MESSAGE_SINK + block("function SecondaryClick-OwnedPoint(") + f"""
[Drive]::Rejected={rejected}
$errorMessage=$null
try {{ SecondaryClick-OwnedPoint 88 302 }} catch {{ $errorMessage=$_.Exception.Message }}
@{{ error=$errorMessage; messages=@([Drive]::Messages.ToArray()) }} | ConvertTo-Json -Compress
""")

    def test_secondary_click_posts_right_button_messages_to_the_owned_window(self):
        result = self.post()
        self.assertEqual(result["messages"], RIGHT_MESSAGES)
        self.assertIsNone(result["error"])
        # Posted messages only: the function never reaches for global input.
        self.assertNotIn("SendInput", block("function SecondaryClick-OwnedPoint("))

    def test_a_rejected_right_button_message_is_not_a_successful_click(self):
        for rejected, expected in ((512, RIGHT_MESSAGES[:1]), (516, RIGHT_MESSAGES[:2]), (517, RIGHT_MESSAGES)):
            with self.subTest(rejected=rejected):
                result = self.post(rejected)
                self.assertEqual(result["messages"], expected)
                self.assertIn("was not posted", result["error"])

    def run_case(self, start, parts):
        stubs = (
            "$journal = [Collections.Generic.List[string]]::new()\n"
            "function Find-OwnedLabelPoint([string] $Encoded, [string] $Mode, [int] $Offset = 20) {\n"
            "  $journal.Add('find:' + [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($Encoded)) + ':' + $Mode)\n"
            "  return @{ X=88; Y=302 }\n"
            "}\n"
            "function Find-OwnedCellPoint([string] $EncodedRow, [string] $EncodedDate) {\n"
            "  $journal.Add('cell:' + [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($EncodedRow)) + ':'"
            " + [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($EncodedDate)))\n"
            "  return @{ X=88; Y=302 }\n"
            "}\n"
            "function Click-OwnedPoint([int] $X, [int] $Y) { $journal.Add(\"left:$X,$Y\") }\n"
            "function SecondaryClick-OwnedPoint([int] $X, [int] $Y) { $journal.Add(\"right:$X,$Y\") }\n"
            "$parts = @(" + ",".join(f"'{part}'" for part in parts) + ")\n"
            "try {\n" + case_body(start) + "\n} catch { $journal.Add('error:' + $_.Exception.Message) }\n"
            "ConvertTo-Json -InputObject @($journal.ToArray()) -Compress\n")
        return powershell(stubs)

    def test_the_tab_menu_case_locates_a_tab_like_a_click_and_presses_the_right_button(self):
        menu = self.run_case("'workspace_menu64' {", ["workspace_menu64", ENCODED])
        self.assertEqual(menu, [f"find:{TAB}:workspace", "right:88,302"])
        click = self.run_case("{ $_ -in 'nav64',", ["workspace64", ENCODED])
        self.assertEqual(click, [f"find:{TAB}:workspace", "left:88,302"])

    def test_the_tab_menu_case_takes_exactly_one_tab_title(self):
        for parts in (["workspace_menu64"], ["workspace_menu64", ENCODED, ENCODED]):
            with self.subTest(parts=len(parts)):
                self.assertEqual(self.run_case("'workspace_menu64' {", parts),
                    ["error:A tab menu needs one visible tab title"])


    def test_the_cell_menu_case_locates_a_grid_cell_like_a_click_and_presses_the_right_button(self):
        row, date = (base64.b64encode(text.encode()).decode() for text in ("FIX-17", "14 Oct"))
        menu = self.run_case("'cell_menu64' {", ["cell_menu64", row, date])
        self.assertEqual(menu, ["cell:FIX-17:14 Oct", "right:88,302"])
        click = self.run_case("'cell64' {", ["cell64", row, date])
        self.assertEqual(click, ["cell:FIX-17:14 Oct", "left:88,302"])
        for parts in (["cell_menu64", row], ["cell_menu64", row, date, date]):
            with self.subTest(parts=len(parts)):
                self.assertEqual(self.run_case("'cell_menu64' {", parts),
                    ["error:A grid cell needs one row and one date"])


class WindowsCommandEncoding(unittest.TestCase):
    def test_a_tab_menu_is_its_own_windows_command_beside_the_unchanged_tab_click(self):
        with tempfile.TemporaryDirectory(prefix="buddy-tab-menu-") as root:
            directory = Path(root)
            driver = object.__new__(desktop.Driver)
            driver.profile, driver.session, driver.large_size = directory, "windows", (1200, 820)
            driver.executable, driver.config, driver.checkout = directory / "owned.exe", directory / "config.json", directory
            driver.backend = Mock(url="http://127.0.0.1:9")
            with patch.object(desktop, "run") as run:
                driver.drive("menu", [("workspace", TAB), ("workspace_menu", TAB), ("key", "Escape")])
            commands = run.call_args.args[run.call_args.args.index("-Actions") + 1].split(",")
        self.assertEqual(commands[1:4], ["workspace64:" + ENCODED, "workspace_menu64:" + ENCODED, "key:27"])
        # Every encoded command has a case in the real Windows driver.
        self.assertTrue(any(line.strip().startswith("'workspace_menu64' {") for line in driver_text()))


if __name__ == "__main__":
    unittest.main()
