"""Source wiring controls, NOT proof of hosted GPUI event delivery."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]

class SoundObserverWiring(unittest.TestCase):
    def test_actual_notification_callback_records_before_single_unchanged_edit(self):
        source = (ROOT / 'rust/crates/buddy-app/src/settings_view.rs').read_text()
        button = source.split('Button::new("review-sound-enabled")', 1)[1].split('.child(self.field(', 1)[0]
        self.assertEqual(button.count('this.local(LocalEdit::NotificationEnabled(!enabled), cx)'), 1)
        self.assertLess(button.index('"callback"'), button.index('this.local('))
        local = source.split('fn local(', 1)[1].split('fn load_backend(', 1)[0]
        self.assertLess(local.index('"local-enter"'), local.index('Settings::try_update'))
        self.assertLess(local.index('Settings::try_update'), local.index('"local-result"'))
        self.assertIn('Some(result.is_ok())', local)

    def test_capture_is_guarded_and_config_replacements_are_observed(self):
        view = (ROOT / 'rust/crates/buddy-app/src/settings_view.rs').read_text()
        panel = view.split('fn notifications(', 1)[1].split('Button::new("review-sound-enabled")', 1)[0]
        self.assertIn('.when(crate::settings::sound_diagnostic::enabled()', panel)
        self.assertIn('.capture_any_mouse_down(', panel)
        self.assertIn('.capture_any_mouse_up(', panel)
        self.assertNotIn('stop_propagation', panel)
        settings = (ROOT / 'rust/crates/buddy-app/src/settings.rs').read_text()
        self.assertIn('"install"', settings)
        self.assertIn('"replace"', settings)
