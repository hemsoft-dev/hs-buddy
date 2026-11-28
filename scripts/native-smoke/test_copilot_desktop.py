import importlib.util
import json
import os
from pathlib import Path
import unittest
from tempfile import TemporaryDirectory
from unittest.mock import patch
from urllib.request import Request, urlopen

spec = importlib.util.spec_from_file_location('copilot_desktop', Path(__file__).with_name('copilot-desktop.py'))
copilot = importlib.util.module_from_spec(spec)
spec.loader.exec_module(copilot)

class ReviewPublication(unittest.TestCase):
    def test_publication_scenes_record_confirmation_outcome_and_refresh(self):
        class RecordingDriver:
            def __init__(self):
                self.calls = []

            def drive(self, name, actions, *, record=False):
                self.calls.append((name, actions, record))
                return Path('synthetic-recording')

        driver = RecordingDriver()
        with patch.object(copilot.github, 'check'), patch.object(copilot.github, 'check_column'):
            copilot.review_pull_request(driver)
            copilot.review_publication_unconfirmed(driver)
        for name, actions, record in driver.calls:
            with self.subTest(scene=name):
                self.assertTrue(record, 'Publication captures must include an interaction recording')
                captures = [action[1] for action in actions if action[0] == 'capture']
                self.assertTrue(any('confirmation' in capture for capture in captures))
                self.assertTrue(any('retained' in capture for capture in captures))
                self.assertIn(('content', 'Confirm publication'), actions)
                self.assertIn(('content', 'Refresh this result'), actions)

    def test_retained_unconfirmed_capture_waits_for_the_whole_status(self):
        class RecordingDriver:
            def __init__(self):
                self.actions = []

            def drive(self, name, actions, *, record=False):
                self.actions = actions
                return Path('synthetic-recording')

        driver = RecordingDriver()
        with patch.object(copilot.github, 'check') as check:
            copilot.review_publication_unconfirmed(driver)
        capture = driver.actions.index(('capture', 'publish-unconfirmed-retained-light'))
        # The check needs the status's last words, so the wait before the
        # capture must scroll until its last line is visible, not just its first.
        required = check.call_args.args[1:]
        kind, waited = driver.actions[capture - 1][:2]
        self.assertEqual(kind, 'wait_scroll_text')
        # A wait matches inside one OCR line and the status wraps differently
        # per platform font: run 44786129019 broke it after "will" on both
        # Macs. Only a single word survives every wrap.
        self.assertEqual(len(waited.split()), 1)
        status = 'Publication unconfirmed. Check the PR on GitHub before any further action. This session will not resend it.'
        source = Path(__file__).resolve().parents[2] / 'rust/crates/buddy-app/src/copilot_view/results_publish.rs'
        self.assertIn(status, source.read_text(encoding='utf-8'))
        self.assertIn(required[-1], status)
        self.assertEqual(status.split().count(waited), 1)
        self.assertIn(waited, status.split()[-2:])
        self.assertIn(waited, required[-1].split())

    def test_every_publication_wait_wheels_inside_the_opened_result(self):
        # Beside the Copilot sidebar the default wheel point is on the short
        # results list (Ubuntu run 46084940944: 'Confirm publication' never
        # came into view). Each downward wait after the result opens names a
        # label inside the result pane to wheel at.
        class RecordingDriver:
            def __init__(self):
                self.calls = []

            def drive(self, name, actions, *, record=False):
                self.calls.append((name, actions))
                return Path('synthetic-recording')

        driver = RecordingDriver()
        with patch.object(copilot.github, 'check'), patch.object(copilot.github, 'check_column'):
            copilot.review_pull_request(driver)
            copilot.review_publication_unconfirmed(driver)
        for name, actions in driver.calls:
            opened = actions.index(('content', 'Inspect saved result'))
            for action in actions[opened:]:
                if action[0] == 'wait_scroll_text':
                    with self.subTest(scene=name, wait=action[1]):
                        self.assertEqual(len(action), 3)
                        self.assertIn(action[2], (copilot.RESULT_PANE, copilot.RESULT_TOP))

    def test_the_compact_pass_reaches_publish_with_a_coarse_step_at_the_pane(self):
        class RecordingDriver:
            def drive(self, name, actions, *, record=False):
                self.actions = actions
                return Path('synthetic-recording')

        driver = RecordingDriver()
        with patch.object(copilot.github, 'check'):
            copilot.review_publication_unconfirmed(driver)
        resized = driver.actions.index(('resize', 900, 650))
        publish = driver.actions.index(('content', 'Publish to PR'))
        self.assertGreater(publish, resized)
        self.assertEqual(driver.actions[publish - 1], ('wait_scroll_text', 'Publish to PR', copilot.RESULT_PANE))
        self.assertNotIn('reveal_text', [action[0] for action in driver.actions[resized:publish]])

    def test_the_pane_anchor_takes_whichever_of_its_labels_is_in_view(self):
        def word(text, left, top):
            return dict(text=text, left=left, top=top, width=8 * len(text), height=14, line=str(top))
        ocr = copilot.settings.ocr
        # Scrolled to the publication block: the header button has left the
        # view, the action row and the publish button remain (wait-status.png
        # of review-dark in run 46084940944).
        scrolled = [word('Copy', 820, 596), word('answer', 860, 596), word('Publish', 853, 640),
            word('to', 910, 640), word('PR...', 930, 640)]
        self.assertEqual(ocr.locate_label(scrolled, copilot.RESULT_PANE)['top'], 640)
        # After publishing only the action row may remain.
        actions_only = [word('Copy', 820, 300), word('answer', 860, 300)]
        self.assertEqual(ocr.locate_label(actions_only, copilot.RESULT_PANE)['top'], 300)
        with self.assertRaises(RuntimeError):
            ocr.locate_label([word('Completed:', 363, 276)], copilot.RESULT_PANE)

    def test_the_mac_wrapped_status_is_found_by_its_word_and_not_by_its_phrase(self):
        # Vision lines from the failing macOS capture's layout: the status
        # ends "This session will" / "not resend it."
        def line(number, top, text):
            return [dict(text=word, left=340 + 60 * index, top=top, width=52, height=14, line=str(number))
                for index, word in enumerate(text.split())]
        words = line(1, 394, 'Publication unconfirmed. Check the PR on GitHub before any further action. This session will') \
            + line(2, 414, 'not resend it.')
        ocr = copilot.settings.ocr
        with self.assertRaises(RuntimeError):
            ocr.locate_label(words, copilot.UNCONFIRMED_ENDING)
        self.assertEqual(ocr.locate_label(words, copilot.UNCONFIRMED_LAST_LINE_WORD)['top'], 414)

    def test_committed_comment_with_lost_ack_and_strict_attempt_receipt(self):
        with copilot.CopilotGitHubBackend() as backend:
            body = {'body': '## 🤖 AI Review\n\nSynthetic assistant answer\n\n---\n*Published from HS Buddy — fixture review*'}
            for acknowledged in (True, False):
                backend.publication_ack = acknowledged
                request = Request(backend.url + '/repos/fixture-org/native-demo/issues/7/comments',
                    data=json.dumps(body).encode(), headers={'Authorization': 'Bearer native-fixture-synthetic-account', 'Content-Type': 'application/json'}, method='POST')
                with urlopen(request) as response:
                    receipt = json.load(response)
                    self.assertEqual('id' in receipt, acknowledged)
            self.assertEqual(len(backend.comments['synthetic-account']), 2)
            results = [dict(category='pr-review', status='completed', result='Synthetic assistant answer', model='fixture')] * 2
            self.assertEqual(copilot.require_review_publications(backend.requests, results)['refreshReplay'], 0)
            for field, value in (('account', 'other-account'), ('path', '/repos/other/repo/issues/7/comments'), ('body', {'body': 'wrong answer'})):
                changed = json.loads(json.dumps(backend.requests))
                changed[0][field] = value
                with self.subTest(field=field), self.assertRaises(RuntimeError):
                    copilot.require_review_publications(changed, results)
            with self.assertRaises(RuntimeError):
                copilot.require_review_publications(backend.requests + backend.requests[:1], results)


class SessionErrorScope(unittest.TestCase):
    def test_captured_errors_are_unique_and_footer_alone_cannot_satisfy_the_wait(self):
        ocr = copilot.settings.ocr
        for platform in ('ubuntu', 'windows', 'macos'):
            with self.subTest(platform=platform):
                evidence = json.loads(Path(__file__).with_name('fixtures').joinpath(
                    f'status-session-error-{platform}.json').read_text(encoding='utf-8'))
                words = evidence['words']
                with self.assertRaises(ocr.AmbiguousLabel):
                    ocr.find_label(words, 'unavailable')
                found = ocr.find_label(words, copilot.SESSION_SOURCE_ERROR, content=True)
                self.assertLess(found['top'], 620)
                self.assertEqual(ocr.find_label(words, 'unavailable', content=True)['top'], found['top'])
                error_line = next(w['line'] for w in words if w['left'] == found['left']
                    and found['top'] <= w['top'] <= found['top'] + found['height'])
                without_error = [w for w in words if w['line'] != error_line]
                with self.assertRaises(RuntimeError):
                    ocr.find_label(without_error, copilot.SESSION_SOURCE_ERROR, content=True)
                with self.assertRaises(RuntimeError):
                    ocr.find_label(without_error, 'unavailable', content=True)
                duplicate = [dict(w, top=w['top']-25, line='duplicate-error')
                    for w in words if w['line'] == error_line]
                with self.assertRaises(ocr.AmbiguousLabel):
                    ocr.find_label(words + duplicate, copilot.SESSION_SOURCE_ERROR, content=True)


    def test_footer_scope_scales_with_captured_glyph_geometry(self):
        ocr = copilot.settings.ocr
        for platform in ('ubuntu', 'windows', 'macos'):
            words = json.loads(Path(__file__).with_name('fixtures').joinpath(
                f'status-session-error-{platform}.json').read_text(encoding='utf-8'))['words']
            expected = ocr.find_label(words, 'unavailable', content=True)
            for scale in (0.75, 1.5, 2, 3):
                with self.subTest(platform=platform, scale=scale):
                    scaled = [dict(w, **{key: w[key]*scale for key in ('top', 'left', 'width', 'height')})
                        for w in words]
                    found = ocr.find_label(scaled, 'unavailable', content=True)
                    self.assertEqual(found, {key: value*scale for key, value in expected.items()})


class StartupDiagnostics(unittest.TestCase):
    def logs(self, output):
        for scene in copilot.STARTUP_SCENES:
            (output / scene).mkdir()
            (output / scene / 'app.log').write_text('', encoding='utf-8')

    def test_clean_complete_logs_pass(self):
        with TemporaryDirectory() as temporary:
            output = Path(temporary)
            self.logs(output)
            self.assertEqual(copilot.assert_startup_logs_clean(output)['result'], 'passed')

    def test_either_warning_in_any_scene_fails_and_preserves_counts(self):
        for scene in copilot.STARTUP_SCENES:
            for marker in ('RefCell already borrowed', 'unable to get cursor position'):
                with self.subTest(scene=scene, marker=marker), TemporaryDirectory() as temporary:
                    output = Path(temporary)
                    self.logs(output)
                    log = output / scene / 'app.log'
                    original = ('[ERROR] ' + marker + '\n') * 2
                    log.write_text(original, encoding='utf-8')
                    with self.assertRaisesRegex(RuntimeError, 'startup diagnostics failed'):
                        copilot.assert_startup_logs_clean(output)
                    receipt = json.loads((output / 'startup-diagnostics.json').read_text())
                    self.assertEqual(receipt['scenes'][scene][marker], 2)
                    self.assertEqual(log.read_text(), original)

    def test_missing_scene_cannot_qualify(self):
        with TemporaryDirectory() as temporary:
            with self.assertRaises(FileNotFoundError):
                copilot.assert_startup_logs_clean(Path(temporary))


class SessionSourceIntegrity(unittest.TestCase):
    def test_session_fixture_contains_two_real_request_pages_with_known_metrics(self):
        with TemporaryDirectory() as temporary:
            _, source, _ = copilot.session_fixture(Path(temporary))
            lines = [json.loads(line) for line in source.read_text(encoding='utf-8').splitlines()]
            prompts = lines[0]['v']['requests']
            self.assertEqual(len(prompts), 21)
            self.assertEqual(prompts[19]['message']['text'], 'Inspect synthetic request 20')
            self.assertEqual(prompts[20]['message']['text'], 'Final synthetic request twenty one')
            self.assertEqual([line['k'] for line in lines[1:]], [['requests', i, 'result'] for i in range(21)])
            self.assertEqual(sum(line['v']['metadata']['promptTokens'] for line in lines[1:]), 100)
            self.assertEqual(sum(line['v']['metadata']['outputTokens'] for line in lines[1:]), 25)

    def test_unchanged_source_is_verified_after_success_and_navigation_failure(self):
        for fail in (False, True):
            with self.subTest(fail=fail), TemporaryDirectory() as temporary:
                output = Path(temporary)
                _, source, expected = copilot.session_fixture(output)
                original = source.read_bytes()
                primary = RuntimeError('synthetic navigation failure')
                try:
                    with copilot.session_source_guard(source, expected, output):
                        if fail:
                            raise primary
                except RuntimeError as error:
                    self.assertTrue(fail)
                    self.assertIs(error, primary)
                receipt = json.loads((output / 'session-source-integrity.json').read_text())
                self.assertEqual(receipt['result'], 'passed')
                self.assertEqual(receipt['expectedSha256'], expected)
                self.assertEqual(receipt['actualSha256'], expected)
                self.assertEqual(source.read_bytes(), original)

    def test_source_mutation_fails_without_restoring_bytes_or_replacing_primary_error(self):
        for fail in (False, True):
            with self.subTest(fail=fail), TemporaryDirectory() as temporary:
                output = Path(temporary)
                _, source, expected = copilot.session_fixture(output)
                primary = RuntimeError('synthetic navigation failure')
                with self.assertRaises(RuntimeError) as raised:
                    with copilot.session_source_guard(source, expected, output):
                        source.write_bytes(b'changed source')
                        if fail:
                            raise primary
                if fail:
                    self.assertIs(raised.exception, primary)
                    self.assertIn('Session source verification failed: RuntimeError', primary.__notes__)
                else:
                    self.assertIn('changed its source', str(raised.exception))
                self.assertEqual(source.read_bytes(), b'changed source')
                receipt = json.loads((output / 'session-source-integrity.json').read_text())
                self.assertEqual(receipt['result'], 'failed')
                self.assertNotEqual(receipt['expectedSha256'], receipt['actualSha256'])

    def test_failed_integrity_receipt_does_not_replace_primary_failure(self):
        with TemporaryDirectory() as temporary:
            output = Path(temporary)
            _, source, expected = copilot.session_fixture(output)
            primary = RuntimeError('synthetic navigation failure')
            with self.assertRaises(RuntimeError) as raised:
                with copilot.session_source_guard(source, expected, output):
                    (output / 'session-source-integrity.json').mkdir()
                    raise primary
            self.assertIs(raised.exception, primary)
            self.assertTrue(any('Session source verification failed:' in note for note in primary.__notes__))


class CopilotQualificationTests(unittest.TestCase):
    # The recorded OCR fixture predates the footer links, so the harness adds them.
    def verify_usage_capture(self, history_text, copilot_link=' Open Copilot settings on GitHub',
                             codex_link=' Usage settings'):
        class CapturedUsageDriver:
            def __init__(self, output):
                self.output = output

            def drive(self, _name, _actions):
                return self.output

        with TemporaryDirectory() as temporary:
            output = Path(temporary)
            original = output / 'usage-history-light.png'
            original.write_bytes(b'unchanged owned capture')

            def recognize(*arguments, **_kwargs):
                if arguments[0] == 'ffmpeg':
                    return ''
                if Path(arguments[1]).name.startswith('usage-history-light'):
                    return history_text + copilot_link
                return 'Enterprise users fixture-member Weekly allowance 5-hour allowance remaining' + codex_link

            with patch.object(copilot.github.settings, 'run', side_effect=recognize):
                copilot.usage_pages(CapturedUsageDriver(output))
            self.assertEqual(original.read_bytes(), b'unchanged owned capture')

    def test_actual_billing_capture_requires_the_complete_numeric_credits_line(self):
        evidence = json.loads(Path(__file__).with_name('fixtures').joinpath('usage-history-credits-ocr.json').read_text(encoding='utf-8'))
        self.assertNotIn('ai credits', evidence['text'].casefold())
        self.verify_usage_capture(evidence['text'])

    def test_billing_capture_rejects_wrong_credit_values_or_missing_projection_and_history(self):
        evidence = json.loads(Path(__file__).with_name('fixtures').joinpath('usage-history-credits-ocr.json').read_text(encoding='utf-8'))
        text = evidence['text']
        for original, replacement in [('1200 used', '1201 used'), ('3900 included', '3901 included'),
                                      ('projected period spend', 'missing projection'),
                                      ('copilot billing history', 'missing history')]:
            with self.subTest(original=original):
                self.assertIn(original, text)
                with self.assertRaisesRegex(RuntimeError, 'one OCR layout'):
                    self.verify_usage_capture(text.replace(original, replacement))
        for missing in ('copilot_link', 'codex_link'):
            with self.subTest(missing=missing):
                with self.assertRaisesRegex(RuntimeError, 'one OCR layout'):
                    self.verify_usage_capture(text, **{missing: ''})

    def test_loaded_enterprise_user_must_be_revealed_below_the_captured_viewport(self):
        class ClippedUsageDriver:
            def __init__(self):
                self.loaded = False
                self.row_visible = False
                self.captured = False

            def drive(self, name, actions):
                for action in actions:
                    if action == ('content', 'Load metrics file'):
                        # Actual Windows capture: the search field occupies
                        # the bottom edge; summary and user follow below it.
                        self.loaded = True
                        self.row_visible = False
                    elif action == ('wait_scroll_text', 'fixture-member'):
                        if not self.loaded:
                            raise AssertionError('No metrics file has been loaded')
                        self.row_visible = True
                    elif action == ('wait_text', 'fixture-member') and not self.row_visible:
                        raise AssertionError('Loaded enterprise row remains below the viewport')
                    elif action == ('capture', 'enterprise-users-light'):
                        if not self.row_visible:
                            raise AssertionError('The enterprise capture does not show its user')
                        self.captured = True
                return Path('unused-owned-usage-output')

        driver = ClippedUsageDriver()
        with patch.object(copilot.github, 'check') as checks:
            copilot.usage_pages(driver)
        self.assertTrue(driver.captured)
        self.assertEqual(checks.call_count, 3)

    def test_streaming_capture_handshake_is_scoped_and_restored_after_failure(self):
        with TemporaryDirectory() as temporary, patch.dict(os.environ, {'BUDDY_COPILOT_STREAM_CAPTURE': 'previous'}):
            output = Path(temporary)
            with self.assertRaisesRegex(RuntimeError, 'owned driver failed'):
                with copilot.streaming_capture(output) as capture:
                    self.assertEqual(capture, output / 'chat-dark' / 'streaming-dark.png')
                    self.assertEqual(os.environ['BUDDY_COPILOT_STREAM_CAPTURE'], str(capture))
                    raise RuntimeError('owned driver failed')
            self.assertEqual(os.environ['BUDDY_COPILOT_STREAM_CAPTURE'], 'previous')

    def test_existing_streaming_capture_is_preserved_and_cannot_release_a_new_stream(self):
        with TemporaryDirectory() as temporary:
            output = Path(temporary)
            capture = output / 'chat-dark' / 'streaming-dark.png'
            capture.parent.mkdir()
            capture.write_bytes(b'previous evidence')
            with self.assertRaisesRegex(RuntimeError, 'Preserve the existing streaming capture'):
                with copilot.streaming_capture(output):
                    self.fail('An old image cannot acknowledge a new stream')
            self.assertEqual(capture.read_bytes(), b'previous evidence')

    def test_saved_status_does_not_prove_its_offscreen_controls_are_visible(self):
        class ClippedConversationDriver:
            def __init__(self):
                self.visible = set()
                self.clicked = []

            def drive(self, name, actions, record=False):
                for action in actions:
                    if action == ('wait_scroll_text', 'Saved to results'):
                        # The captured Linux viewport shows this status at the
                        # bottom while both following buttons remain clipped.
                        self.visible = {'Saved to results'}
                    elif action[0] == 'reveal_text' and action[1] in ('Copy answer', 'Inspect saved result'):
                        if action[2] not in self.visible:
                            raise AssertionError('Scroll anchor is not in the visible conversation')
                        self.visible.add(action[1])
                    elif action[0] == 'content' and action[1] in ('Copy answer', 'Inspect saved result'):
                        if action[1] not in self.visible:
                            raise AssertionError('Saved status did not reveal the requested control: ' + action[1])
                        self.clicked.append(action[1])
                return Path('unused-synthetic-conversation-output')

        for flow, expected in ((copilot.chat_turns, ['Copy answer', 'Inspect saved result']),
                (copilot.review_pull_request, ['Inspect saved result'])):
            with self.subTest(flow=flow.__name__):
                driver = ClippedConversationDriver()
                with patch.object(copilot.github, 'check'), patch.object(copilot.github, 'check_column'):
                    flow(driver)
                self.assertEqual(driver.clicked, expected)

    def test_code_copy_captures_the_visible_control_before_activating_it(self):
        class CodeDriver:
            def __init__(self):
                self.runs = []

            def drive(self, name, actions, record=False):
                self.runs.append((name, actions, record))
                return Path('unused-' + name)

        driver = CodeDriver()
        with patch.object(copilot.github, 'check') as screenshots:
            copilot.code_copy(driver, 'dark')
            copilot.code_copy(driver, 'light')
        self.assertEqual([(name, record) for name, _, record in driver.runs],
            [('code-copy-dark', True), ('code-copy-light', False)])
        for name, actions, _ in driver.runs:
            self.assertIn(('input', 'Message', 'BUDDYSTREAM61527 ' + copilot.CODE_MARKER), actions)
            self.assertIn(name, copilot.STARTUP_SCENES)
            saved = actions.index(('wait_scroll_text', 'Saved to results'))
            click = actions.index(('content', 'Copy code'))
            self.assertEqual(click, len(actions) - 1)
            account = actions.index(('input', 'GitHub account', 'synthetic-account'))
            badge = ('capture', 'copilot-usage-badge-light')
            if name.endswith('light'):
                # The badge is captured once its quota has arrived.
                self.assertEqual(actions[account + 1:account + 3],
                    [('wait_text', copilot.USAGE_BADGE), badge])
            else:
                self.assertNotIn(badge, actions)
            if name.endswith('light'):
                # The compact chat pane is too short, so both captures use the
                # saved result and reveal the code line before each capture.
                inspect = actions.index(('content', 'Inspect saved result'))
                large, resize = actions.index(('capture', name)), actions.index(('resize', 900, 650))
                compact = actions.index(('capture', 'code-copy-compact-light'))
                reveals = [index for index, action in enumerate(actions)
                    if action == ('reveal_text', copilot.CODE_LINE, 'Copy prompt')]
                self.assertEqual(len(reveals), 2)
                self.assertLess(saved, inspect)
                self.assertLess(inspect, reveals[0])
                self.assertLess(reveals[0], large)
                self.assertLess(large, resize)
                self.assertLess(resize, reveals[1])
                self.assertLess(reveals[1], compact)
            else:
                self.assertNotIn(('resize', 900, 650), actions)
                reveal = actions.index(('wait_scroll_up', 'Copy code', 'Saved to results'))
                self.assertLess(saved, reveal)
                self.assertLess(reveal, actions.index(('capture', name)))
        self.assertEqual([(call.args[0].name, *call.args[1:]) for call in screenshots.call_args_list], [
            ('code-copy-dark.png', 'Copy code', copilot.CODE_LINE),
            ('copilot-usage-badge-light.png', 'GitHub account', '174/300'),
            ('code-copy-light.png', 'Copy code', copilot.CODE_LINE),
            ('code-copy-compact-light.png', 'Copy code', copilot.CODE_LINE),
        ])

    def test_recent_results_waits_for_the_badge_and_the_row_before_each_click(self):
        class Driver:
            def __init__(self):
                self.runs = []

            def drive(self, name, actions, record=False):
                self.runs.append((name, actions, record))
                return Path('unused-' + name)

        driver = Driver()
        with patch.object(copilot.github, 'check') as screenshots:
            copilot.recent_results(driver)
        [(name, actions, record)] = driver.runs
        self.assertEqual((name, record), ('recent-results-dark', True))
        self.assertIn(name, copilot.STARTUP_SCENES)
        self.assertEqual(copilot.RECENT_REVIEW_LABEL, 'PR Review')
        order = [
            ('input', 'GitHub account', 'synthetic-account'),
            ('wait_text', '174/300'),
            ('capture', 'copilot-usage-badge-dark'),
            # Chat's category button would win over the row; Sessions has none.
            ('content', 'Sessions'),
            ('content', 'Recent Results'),
            ('wait_text', copilot.RECENT_REVIEW_LABEL),
            ('capture', 'copilot-recent-results-dark'),
            ('content', copilot.RECENT_REVIEW_LABEL),
            ('wait_text', 'Captured review'),
            ('capture', 'copilot-recent-result-opened-dark'),
        ]
        positions = [actions.index(action) for action in order]
        self.assertEqual(positions, sorted(positions))
        # The row is clicked only after the list has settled.
        self.assertIn(('sleep', 1), actions[positions[5]:positions[7]])
        # Nothing returns to Chat before the row is clicked.
        self.assertNotIn(('content', 'Chat'), actions)
        self.assertEqual([(call.args[0].name, *call.args[1:], call.kwargs) for call in screenshots.call_args_list], [
            ('copilot-usage-badge-dark.png', 'GitHub account', '174/300', {}),
            ('copilot-recent-results-dark.png', 'Recent Results', 'PR Review: Se|PR Review. Se',
                dict(absent=('Untitled',))),
            ('copilot-recent-result-opened-dark.png', 'PR Review: Se|PR Review. Se', 'Captured review',
                dict(absent=('Untitled',))),
        ])

    # Tesseract (mode 11) of the opened re-run result in Ubuntu run
    # 46007700214, 1200x820: the results list, the detail's first lines and
    # the failure notice above the clipped answer.
    UBUNTU_OPENED_RERUN = [
        dict(text='Failed:', left=364, top=270, width=43, height=11, line='26/1/1'),
        dict(text='BUDDYCANCEL73429', left=414, top=271, width=147, height=10, line='26/1/1'),
        dict(text='R...', left=567, top=271, width=18, height=10, line='26/1/1'),
        dict(text='Refresh', left=843, top=270, width=51, height=11, line='27/1/1'),
        dict(text='this', left=899, top=270, width=26, height=11, line='27/1/1'),
        dict(text='result', left=930, top=270, width=39, height=11, line='27/1/1'),
        dict(text='Completed:', left=363, top=306, width=80, height=14, line='28/1/1'),
        dict(text='BUDDYSTREAM61527', left=449, top=307, width=150, height=10, line='28/1/1'),
        dict(text='Answer', left=628, top=313, width=67, height=13, line='29/1/1'),
        dict(text='using', left=701, top=312, width=47, height=18, line='29/1/1'),
        dict(text='Completed:', left=363, top=378, width=80, height=14, line='33/1/1'),
        dict(text='BUDDYSTREAM61527', left=449, top=379, width=150, height=10, line='33/1/1'),
        dict(text='User:', left=629, top=400, width=44, height=13, line='35/1/1'),
        dict(text='BUDDYCANCEL73429', left=682, top=400, width=190, height=13, line='35/1/1'),
        dict(text='RECENTROW', left=879, top=400, width=116, height=13, line='35/1/1'),
        dict(text='This', left=628, top=697, width=26, height=11, line='46/1/1'),
        dict(text='saved', left=659, top=697, width=38, height=11, line='46/1/1'),
        dict(text='run', left=702, top=701, width=20, height=7, line='46/1/1'),
        dict(text='failed.', left=728, top=697, width=35, height=11, line='46/1/1'),
        dict(text='Any', left=772, top=699, width=24, height=12, line='46/1/1'),
        dict(text='captured', left=801, top=697, width=57, height=14, line='46/1/1'),
        dict(text='partial', left=864, top=697, width=40, height=14, line='46/1/1'),
        dict(text='answer', left=909, top=701, width=47, height=7, line='46/1/1'),
        dict(text='appears', left=961, top=701, width=51, height=10, line='46/1/1'),
        dict(text='below.', left=1017, top=697, width=40, height=11, line='46/1/1'),
    ]

    def test_the_opened_rerun_answer_is_scrolled_over_the_result_not_the_list(self):
        class Driver:
            def drive(self, name, actions, record=False):
                self.actions = actions
                return Path('unused-' + name)

        driver = Driver()
        with patch.object(copilot.github, 'check'):
            copilot.rerun_turns(driver)
        opened = driver.actions.index(('content', 'Open re-run result'))
        self.assertEqual(driver.actions[opened + 1],
            ('wait_scroll_text', 'Partial synthetic answer', copilot.RESULT_TOP))
        ocr = copilot.settings.ocr
        words = self.UBUNTU_OPENED_RERUN
        # The answer is below the window; only the notice's "partial answer" shows.
        with self.assertRaises(RuntimeError):
            ocr.locate_label(words, 'Partial synthetic answer')
        detail_left = min(word['left'] for word in words if word['line'] in ('29/1/1', '35/1/1', '46/1/1'))
        list_right = max(word['left'] + word['width'] for word in words if word['text'] in ('Completed:', 'BUDDYSTREAM61527'))
        # The anchorless wheel at x=600 is over the list column, left of the result.
        self.assertTrue(list_right <= 600 < detail_left)
        # The anchor is on this frame once, inside the result.
        top = ocr.locate_label(words, copilot.RESULT_TOP)
        self.assertGreater(top['left'] + top['width'] / 2, detail_left)

    # Windows OCR (win32 engine) of the failing frame in run 46007700214,
    # 1044x788: the expanded sidebar's first row, and Chat's category row.
    WINDOWS_SIDEBAR_ROW = [
        dict(text='PR', left=95, top=124, width=15, height=10, line='12'),
        dict(text='Review.', left=115, top=124, width=45, height=10, line='12'),
        dict(text='Se...', left=165, top=124, width=24, height=10, line='12'),
        dict(text='1', left=201, top=124, width=4, height=10, line='19'),
        dict(text='minute', left=212, top=124, width=43, height=10, line='19'),
        dict(text='ago', left=259, top=127, width=23, height=10, line='19'),
    ]
    # Windows OCR reads each category button as its own line.
    WINDOWS_CHAT_CATEGORIES = [
        dict(text='PR', left=467, top=585, width=15, height=10, line='40'),
        dict(text='Review', left=487, top=585, width=43, height=10, line='40'),
    ]

    def test_the_cut_windows_row_is_found_by_its_prefix_only_away_from_chat(self):
        ocr = copilot.settings.ocr
        sidebar, chat = self.WINDOWS_SIDEBAR_ROW, self.WINDOWS_CHAT_CATEGORIES
        # The row reads "PR Review: Se...", so the full title never appears.
        with self.assertRaises(RuntimeError):
            ocr.locate_label(sidebar + chat, 'PR Review: Second')
        # Beside Chat the prefix resolves to the complete category button,
        # which would select a category instead of opening the result.
        self.assertEqual(ocr.locate_label(sidebar + chat, copilot.RECENT_REVIEW_LABEL)['top'], 585)
        # In Sessions the row is the only "PR Review".
        row = ocr.locate_label(sidebar, copilot.RECENT_REVIEW_LABEL)
        self.assertEqual((row['left'], row['top']), (95, 124))

    def test_the_review_row_marker_reads_the_row_and_not_the_results_filter(self):
        has_marker = copilot.github.has_marker
        self.assertTrue(has_marker('PR Review: Se... 1 minute ago', copilot.RECENT_REVIEW_MARKER))
        self.assertTrue(has_marker('PR Review. Se...', copilot.RECENT_REVIEW_MARKER))
        self.assertTrue(has_marker('PR Review: Second private navigation review', copilot.RECENT_REVIEW_MARKER))
        self.assertFalse(has_marker('All Pending Running Completed Failed PR reviews', copilot.RECENT_REVIEW_MARKER))
        self.assertFalse(has_marker('Category General PR Review Code Analysis', copilot.RECENT_REVIEW_MARKER))

    def check_text(self, text, *markers):
        def recognize(*args, **kwargs):
            return '' if args[0] == 'ffmpeg' else text
        with TemporaryDirectory() as temporary, patch.object(copilot.github.settings, 'run', side_effect=recognize):
            copilot.github.check(Path(temporary) / 'copilot-recent-results-dark.png', *markers,
                absent=copilot.RECENT_REVIEW_ABSENT)

    def test_an_untitled_review_row_fails_the_recent_results_captures(self):
        # PR #1028 round 4: the row read as "PR Review: Untitled" when
        # prTitle was lost, and the bare prefix still passed.
        row = ' '.join(word['text'] for word in self.WINDOWS_SIDEBAR_ROW)
        self.assertEqual(row, 'PR Review. Se... 1 minute ago')
        self.check_text('Recent Results ' + row, 'Recent Results', copilot.RECENT_REVIEW_MARKER)
        self.check_text('Recent Results PR Review: Second private navigation review 1 minute ago',
            'Recent Results', copilot.RECENT_REVIEW_MARKER)
        self.check_text('PR Review: Second private Captured review: owner/repo #7',
            copilot.RECENT_REVIEW_MARKER, 'Captured review')
        for untitled in ('PR Review: Untitled 1 minute ago', 'PR Review. Untitled 1 minute ago'):
            with self.subTest(untitled), self.assertRaises(RuntimeError):
                self.check_text('Recent Results ' + untitled, 'Recent Results', copilot.RECENT_REVIEW_MARKER)
        # A titled row beside an untitled one still fails: the title is lost somewhere.
        with self.assertRaises(RuntimeError):
            self.check_text('Recent Results ' + row + ' PR Review: Untitled just now',
                'Recent Results', copilot.RECENT_REVIEW_MARKER)

    def test_a_failed_recent_results_check_stops_the_scene(self):
        class Driver:
            def drive(self, name, actions, record=False):
                return Path('unused-' + name)

        with patch.object(copilot.github, 'check', side_effect=RuntimeError('missing')):
            with self.assertRaises(RuntimeError):
                copilot.recent_results(Driver())

    def test_fresh_review_selects_the_offline_model_before_dispatch(self):
        # The actual failed Mac capture starts a fresh review app with this
        # configured model. Chat's earlier, view-local choice does not carry over.
        class FreshReviewDriver:
            def __init__(self):
                self.model = 'claude-sonnet-4.5'
                self.catalog = False
                self.context = False
                self.dispatched = False

            def drive(self, name, actions, *, record=False):
                self.assert_name = name
                self.recorded = record
                for action in actions:
                    if action == ('content', 'Review with Buddy assistant'):
                        self.context = True
                    elif action == ('content', 'Load available models'):
                        self.catalog = True
                    elif action == ('wait_models', 'Synthetic offline model'):
                        if not self.catalog:
                            raise AssertionError('Model discovery was not requested')
                    elif action == ('content', 'Synthetic offline model'):
                        if not self.catalog:
                            raise AssertionError('Cannot select a model before discovery')
                        self.model = 'gpt-4.1'
                    elif action == ('content', 'Run and save'):
                        if not self.context or self.model != 'gpt-4.1':
                            raise AssertionError('Fresh review dispatch reaches the offline identity guard with model=' + self.model)
                        self.dispatched = True
                return Path('unused-synthetic-review-output')

        driver = FreshReviewDriver()
        with patch.object(copilot.github, 'check') as screenshots, \
                patch.object(copilot.github, 'check_column') as review_notice:
            copilot.review_pull_request(driver)
        self.assertEqual(driver.assert_name, 'review-dark')
        self.assertTrue(driver.recorded)
        self.assertTrue(driver.dispatched)
        self.assertEqual(driver.model, 'gpt-4.1')
        screenshots.assert_any_call(Path('unused-synthetic-review-output') / 'review-context-dark.png', 'synthetic-account', 'Run and save')
        review_notice.assert_called_once_with(Path('unused-synthetic-review-output') / 'review-result-dark.png',
            'Captured review', 'Captured review', 'Freshness has not been checked')

    def test_privacy_gate_requires_app_logs_and_rejects_private_transport_content(self):
        with TemporaryDirectory() as temporary:
            output = Path(temporary)
            with self.assertRaisesRegex(RuntimeError, 'not captured'):
                copilot.assert_private_content_not_logged(output)
            case = output / 'synthetic-case'
            case.mkdir()
            log = case / 'app.log'
            log.write_text('static application warning')
            copilot.assert_private_content_not_logged(output)
            log.write_text('received payload BUDDYSTREAM61527')
            with self.assertRaisesRegex(RuntimeError, 'Private synthetic'):
                copilot.assert_private_content_not_logged(output)
    def test_non_ci_invocation_stops_before_argument_parsing_backend_or_ui(self):
        with patch.dict(os.environ, {'GITHUB_ACTIONS': 'false', 'BUDDY_SETTINGS_FIXTURE': '1'}), \
                patch.object(copilot.argparse.ArgumentParser, 'parse_args') as arguments, \
                patch.object(copilot.settings, 'SettingsBackend') as backend, \
                patch.object(copilot.settings, 'Driver') as driver:
            with self.assertRaisesRegex(RuntimeError, 'disposable GitHub Actions'):
                copilot.main()
            arguments.assert_not_called()
            backend.assert_not_called()
            driver.assert_not_called()

    def test_process_inspection_is_also_rejected_outside_ci(self):
        with patch.dict(os.environ, {'GITHUB_ACTIONS': 'false'}), patch.object(copilot.subprocess, 'check_output') as inspect:
            with self.assertRaisesRegex(RuntimeError, 'disposable CI'):
                copilot.assert_no_owned_children(Path('unused-owned-root'))
            inspect.assert_not_called()

    def request(self, api, path, body=None):
        request = Request(api.url.rstrip('/') + path, data=None if body is None else json.dumps(body).encode(),
            headers={'Authorization': 'Bearer native-fixture-synthetic-account', 'Content-Type': 'application/json'})
        with urlopen(request, timeout=2) as response:
            return json.load(response)

    def test_real_loopback_usage_budget_and_review_responses_stay_synthetic(self):
        with copilot.CopilotGitHubBackend() as api:
            usage = self.request(api, '/orgs/fixture-org/settings/billing/usage?year=2026&month=10')
            self.assertEqual(usage['usageItems'][0]['date'], '2026-10-02')
            self.assertEqual(usage['usageItems'][0]['quantity'], 1200)
            self.assertEqual(self.request(api, '/orgs/fixture-org/copilot/billing')['seat_breakdown']['total'], 1)
            premium = self.request(api, '/copilot_internal/user')['quota_snapshots']['premium_interactions']
            self.assertEqual((premium['remaining'], premium['entitlement']), (174, 300))
            self.assertEqual(self.request(api, '/organizations/fixture-org/settings/billing/budgets?page=1')['budgets'], [])
            first = self.request(api, '/enterprises/fixture-enterprise/settings/billing/budgets?page=1')
            second = self.request(api, '/enterprises/fixture-enterprise/settings/billing/budgets?page=2')
            self.assertTrue(first['has_next_page'])
            self.assertFalse(second['has_next_page'])
            self.assertEqual(second['budgets'][0]['budget_amount'], 200)
            pull = self.request(api, '/graphql', {'query': 'query NativePull($number:Int!){repository{pullRequest{body}}}', 'variables': {'number': 7}})
            self.assertIn('BUDDYSTREAM61527', pull['data']['repository']['pullRequest']['body'])
            self.assertTrue(all(request['account'] == 'synthetic-account' for request in api.requests))
            self.assertNotIn('Authorization', json.dumps(api.requests))

if __name__ == '__main__':
    unittest.main()
