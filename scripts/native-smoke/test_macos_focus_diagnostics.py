"""A stalled diagnostic must not replace app qualification or its focus guard."""
import importlib.util
from pathlib import Path
import subprocess
import unittest
from unittest.mock import Mock, patch

SPEC=importlib.util.spec_from_file_location('owned_macos_focus_diagnostics',Path(__file__).with_name('macos-input.py'))
macos=importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(macos)

class FocusDiagnostics(unittest.TestCase):
    def helper(self):
        helper=object.__new__(macos.OwnedMacInput)
        helper.require_focus=Mock()
        helper.process=Mock(pid=12345)
        helper.graphics=Mock()
        helper.core=Mock()
        helper.graphics.CGEventCreate.return_value=123
        helper.graphics.CGEventGetLocation.return_value=macos.Point(10,20)
        helper.graphics.CGEventSourceFlagsState.return_value=0
        helper.graphics.CGPreflightPostEventAccess.return_value=True
        helper.graphics.CGEventSourceButtonState.return_value=False
        return helper

    def test_already_clear_modifiers_do_not_post_unpaired_key_releases(self):
        helper = self.helper()
        helper.post = Mock()
        # Non-coalesced pointer state is not a keyboard modifier.
        helper.graphics.CGEventSourceFlagsState.return_value = 1 << 29
        with patch.object(macos.time, 'sleep') as pause:
            state = helper.release_modifiers()
        self.assertEqual(state, dict(before=1 << 29, after=1 << 29))
        helper.require_focus.assert_called_once_with()
        helper.graphics.CGEventCreateKeyboardEvent.assert_not_called()
        helper.post.assert_not_called()
        helper.graphics.CGEventPost.assert_not_called()
        pause.assert_not_called()

    def test_pressed_modifiers_are_released_and_verified_before_input(self):
        helper = self.helper()
        helper.post = Mock()
        helper.graphics.CGEventSourceFlagsState.side_effect = [1 << 20, 1 << 29]
        with patch.object(macos.time, 'sleep'):
            state = helper.release_modifiers()
        self.assertEqual(state, dict(before=1 << 20, after=1 << 29))
        self.assertEqual([call.args for call in helper.graphics.CGEventCreateKeyboardEvent.call_args_list],
                         [(None, code, False) for code in (54, 55, 56, 60, 58, 61, 59, 62)])
        self.assertEqual(helper.post.call_count, 8)

    def test_modifiers_that_do_not_clear_fail_without_emitting_a_click(self):
        helper = self.helper()
        helper.post = Mock()
        helper.graphics.CGEventSourceFlagsState.return_value = 1 << 20
        with patch.object(macos.time, 'sleep'):
            with self.assertRaisesRegex(RuntimeError, 'modifier state did not clear'):
                helper.click(100, 200)
        helper.graphics.CGEventCreateMouseEvent.assert_not_called()
        self.assertEqual(helper.post.call_count, 8)

    def test_timed_out_element_inspection_remains_unavailable_telemetry_without_posting_input(self):
        helper=self.helper()
        with patch.object(macos.subprocess,'run',side_effect=[
                subprocess.CompletedProcess([],0,'true, true',''),
                subprocess.TimeoutExpired(['osascript'],10)]):
            state=helper.focus_state()
        helper.require_focus.assert_called_once_with()
        self.assertEqual(state['flags'],'true, true')
        self.assertEqual(state['pointer'],[10,20])
        self.assertIsNone(state['focusedElement']['returncode'])
        self.assertEqual(state['focusedElement']['geometry'],'')
        self.assertEqual(state['focusedElement']['error'],'Owned Mac focus diagnostics timed out.')
        helper.graphics.CGEventPost.assert_not_called()
        helper.core.CFRelease.assert_called_once_with(123)

    def test_timed_out_window_flags_are_reported_without_inventing_focus(self):
        helper=self.helper()
        with patch.object(macos.subprocess,'run',side_effect=[
                subprocess.TimeoutExpired(['osascript'],10),
                subprocess.CompletedProcess([],0,'AXWindow, true, 80, 80, 1200, 820','')]):
            state=helper.focus_state()
        self.assertIsNone(state['returncode'])
        self.assertEqual(state['flags'],'')
        self.assertEqual(state['error'],'Owned Mac focus diagnostics timed out.')
        self.assertEqual(state['focusedElement']['geometry'],'AXWindow, true, 80, 80, 1200, 820')
        helper.graphics.CGEventPost.assert_not_called()

    def test_missing_owned_window_focus_still_fails_before_diagnostics(self):
        helper=self.helper()
        primary=RuntimeError('Owned app is not frontmost')
        helper.require_focus.side_effect=primary
        with patch.object(macos.subprocess,'run') as query:
            with self.assertRaises(RuntimeError) as failure:helper.focus_state()
        self.assertIs(failure.exception,primary)
        query.assert_not_called()
        helper.graphics.CGEventCreate.assert_not_called()

if __name__=='__main__':unittest.main()
