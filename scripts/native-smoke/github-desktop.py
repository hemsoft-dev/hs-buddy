"""Exercise an installed release with two synthetic accounts on an owned desktop."""
import argparse
from contextlib import contextmanager
import hashlib
import json
import sys
import os
from pathlib import Path
import urllib.request
import wave

from desktop import fixture, require_closed_sessions, require_version
import importlib.util

HERE = Path(__file__).resolve().parent


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), HERE / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


settings = load("settings-desktop")
github_backend = load("github-backend")
GitHubBackend = github_backend.GitHubBackend
sidebar_layout = load("sidebar-layout")


@contextmanager
def fixture_environment(endpoint):
    changes = dict(BUDDY_GITHUB_FIXTURE="1", BUDDY_GITHUB_FIXTURE_URL=endpoint)
    saved = {key: os.environ.get(key) for key in changes}
    try:
        os.environ.update(changes)
        yield
    finally:
        for key, value in saved.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value


def mutate(backend, path, arguments):
    payload = json.dumps(dict(path=path, args=arguments, format="json")).encode()
    request = urllib.request.Request(backend.url + "/api/mutation", data=payload, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(request, timeout=10) as response:
        result = json.load(response)
    if result.get("status") != "success":
        raise RuntimeError("The disposable Convex fixture mutation failed")
    return result["value"]


def reset_section(config):
    value = json.loads(config.read_text(encoding="utf-8-sig"))
    value["native"]["activeSection"] = "github"
    config.write_text(json.dumps(value), encoding="utf-8")


def require_activity(backend, before, expected, launches=None):
    """Exact counter deltas; with `launches`, also that many closed sessions."""
    after = backend.query("buddyStats:get")
    observed = {field: after.get(field, 0) - before.get(field, 0) for field in expected}
    if observed != expected:
        raise RuntimeError(f"Native PR-list activity differs from explicit opens: {observed}, expected {expected}")
    if launches is not None:
        require_closed_sessions(before, after, launches)
    return after


def installed_binary(executable, version):
    output = settings.run(executable, "--version")
    require_version(output, version)
    return dict(versionOutput=output, binarySha256=hashlib.sha256(executable.read_bytes()).hexdigest())


def analysis_images(path):
    yield from settings.ocr.analysis_images(path, settings.run)


def browser_analysis(path):
    # Keep already-qualified profiles first. Some Windows captures read zero
    # counts as Q-prefixed labels in every default layout. Only that missing
    # footer permits the existing Linux contrast profile; ambiguity and tool
    # failures still stop immediately. Use the selected profile for the body.
    smooth = sys.platform == "linux"
    try:
        image = settings.ocr.status_content_image(path, settings.run, suffix=".browser-ocr.png", smooth_inverted=smooth)
    except settings.ocr.MissingStatusFooter:
        if sys.platform != "win32":
            raise
        smooth = True
        image = settings.ocr.status_content_image(path, settings.run, suffix=".browser-ocr.png", smooth_inverted=smooth)
    yield from settings.ocr.analysis_images(image, settings.run, smooth_inverted=smooth)


def check(path, *markers, absent=(), browser=False):
    missing = None
    for image in (browser_analysis(path) if browser else analysis_images(path)):
        for mode in (11, 3, 6):
            complete = True
            try:
                settings.verify_text(image, *markers, mode=mode)
            except settings.MissingVisibleText as failure:
                missing, complete = failure, False
            text = image.with_suffix(".ocr.txt").read_text(encoding="utf-8")
            for marker in absent:
                if marker.casefold() in text.casefold():
                    raise RuntimeError("Another account's fixture data remained visible")
            if complete:
                if image != path:
                    path.with_suffix(".ocr.txt").write_text(text, encoding="utf-8")
                return
    raise RuntimeError("GitHub screenshot does not show all required text in one OCR layout") from missing


# Normalized OCR words of the background PR sync item: "Syncing PR lists…",
# "Next sync 4m 12s", "Auto-refresh active" or "Auto-refresh off".
SYNC_WORDS = ("syncing", "sync", "autorefresh")


def verify_status_bar(words):
    """Require the assistant indicator, the brand and the PR sync item on the
    one native status footer row, not merely anywhere in the window."""
    ocr = settings.ocr
    footer = ocr.status_footer(words)
    if footer is None:
        raise RuntimeError("No native status footer is visible")
    row = {ocr.normalize(word["text"]) for word in words
        if abs(word["top"] - footer["top"]) <= footer["height"] / 2}
    missing = [name for name, present in (
        ("Copilot", "copilot" in row), ("Buddy", "buddy" in row),
        ("PR sync", any(word in row for word in SYNC_WORDS))) if not present]
    if missing:
        raise RuntimeError("Status footer is missing " + ", ".join(missing))


def check_status_bar(path):
    failure = None
    for image in analysis_images(path):
        for mode in (11, 3, 6):
            words = settings.ocr.recognize(image, mode)
            image.with_suffix(f".status-words-{mode}.json").write_text(json.dumps(words), encoding="utf-8")
            try:
                verify_status_bar(words)
            except settings.ocr.AmbiguousLabel:
                raise
            except RuntimeError as error:
                failure = error
                continue
            return
    raise RuntimeError("Screenshot does not show the status bar items in one OCR layout") from failure


def check_column(path, anchor, *markers):
    missing = None
    wanted = [" ".join(marker.split()).casefold() for marker in markers]
    for image in analysis_images(path):
        for mode in (11, 3, 6):
            # Each attempt uses one engine/layout. Engine and IO errors remain
            # failures, and a competing visible anchor cannot be retried away.
            words = settings.ocr.recognize(image, mode)
            image.with_suffix(f".column-words-{mode}.json").write_text(json.dumps(words), encoding="utf-8")
            try:
                text = settings.ocr.column_text(words, anchor)
            except settings.ocr.AmbiguousLabel:
                raise
            except RuntimeError as failure:
                missing = failure
                continue
            image.with_suffix(f".column-ocr-{mode}.txt").write_text(text, encoding="utf-8")
            normalized = " ".join(text.split()).casefold()
            if all(marker in normalized for marker in wanted):
                path.with_suffix(".column-ocr.txt").write_text(text, encoding="utf-8")
                return
            missing = settings.MissingVisibleText("The visible detail column is missing required text")
    raise RuntimeError("The screenshot does not show all required text in one OCR column layout") from missing


def verify_detail_requests(requests):
    reads = [request for request in requests if request.get("account") == "fixture-one"]
    for operation in ("NativePullComments", "NativePullReviews", "NativePullCommits",
            "NativePullThreads", "NativeThreadComments"):
        if not any(operation in request.get("query", "") for request in reads):
            raise RuntimeError(f"Installed PR navigation did not load {operation}")
    for path in ("/pulls/7/files", "/check-runs", "/statuses", "/issues/7/timeline"):
        if not any(request.get("method") == "GET" and path in request.get("path", "") for request in reads):
            raise RuntimeError(f"Installed PR navigation did not load {path}")


def preserve_failed_requests(output, requests, failure):
    try:
        if output.is_dir():
            (output / "requests.json").write_text(json.dumps(requests, indent=2), encoding="utf-8")
    except Exception as diagnostic_error:
        failure.add_note(f"Request diagnostic failed: {type(diagnostic_error).__name__}")


def qualify_review_sound(driver, config, output, api):
    """Capture completion with isolated settings, never a production account.

    Headless native tests prove sound-request counts. This capture proves the
    installed review transition, not physical speaker output on a hosted VM.
    """
    sound = output / "review-complete.wav"
    with wave.open(str(sound), "wb") as audio:
        audio.setnchannels(1)
        audio.setsampwidth(2)
        audio.setframerate(8000)
        audio.writeframes(b"\0\0" * 800)
    saved = json.loads(config.read_text(encoding="utf-8-sig"))
    initial_notifications = saved.get("notifications", {})
    saved["notifications"] = dict(initial_notifications,
        playSoundOnReviewComplete=True, reviewCompleteSoundPath=str(sound.resolve()))
    saved["native"]["activeSection"] = "github"
    config.write_text(json.dumps(saved), encoding="utf-8")
    start = len(api.requests)
    try:
        actions = [
            ("sidebar", "My PRs"), ("input", "Filter loaded items", "First private"),
            ("content", "#7 First private navigation review"),
            ("content", "Request CodeRabbit…"), ("content", "Confirm action"),
            # Native monitoring polls every 20 seconds. This bounded wait is
            # inside the disposable installed fixture, not an agent CI loop.
            ("sleep", 22), ("wait_content", "a new or updated review is available"),
            ("capture", "review-completed-dark"),
        ]
        if driver.session == "x11":
            # The preceding activity pass retained this tab. Restore it
            # above Dashboard's tooltip, which can cover the sidebar label.
            actions[0] = ("workspace", "My PRs")
        elif driver.session == "macos":
            # The hosted Mac capture also retains My PRs, but Dashboard's
            # tooltip covers its sidebar label. Require the restored PR row
            # before filtering; this completion fixture needs no list reopen.
            actions[0] = ("wait_content", "#7 First private navigation review")
        capture = driver.drive("review-completion-dark", actions, record=True)
        requests = [request for request in api.requests[start:]
            if request["method"] == "POST" and request["path"].endswith("/issues/7/comments")]
        if len(requests) != 1 or requests[0]["account"] != "fixture-one" or requests[0]["body"] != dict(body="@coderabbitai full review"):
            raise RuntimeError("Review completion must follow exactly one scoped synthetic review request")
        check(capture / "review-completed-dark.png", "CodeRabbit", "review is available")
    finally:
        current = json.loads(config.read_text(encoding="utf-8-sig"))
        current["notifications"] = initial_notifications
        config.write_text(json.dumps(current), encoding="utf-8")


def remove_personal_account(config, backend, identity, primary):
    """Undo qualify_workbench's fixture changes: the local configuration
    first, then the Convex account if it was created. Every step runs. A
    cleanup failure is raised when the scene passed, and is otherwise a note
    on the scene's own error, which still propagates."""
    failures = []
    try:
        value = json.loads(config.read_text(encoding="utf-8-sig"))
        value["github"]["accounts"] = [account for account in value["github"]["accounts"]
            if account.get("org") != github_backend.PERSON]
        config.write_text(json.dumps(value), encoding="utf-8")
    except Exception as error:
        failures.append(("Fixture configuration restore", error))
    if identity is not None:
        try:
            mutate(backend, "githubAccounts:remove", dict(id=identity))
        except Exception as error:
            failures.append(("Fixture account removal", error))
    if not failures:
        return
    if primary is None:
        _, error = failures[0]
        for label, other in failures[1:]:
            error.add_note(f"{label} also failed: {type(other).__name__}")
        raise error
    for label, error in failures:
        primary.add_note(f"{label} failed: {type(error).__name__}")


def qualify_workbench(driver, config, backend):
    """ActiveRepositoriesSection.tsx: a personal namespace's overview ranks
    its recent issues and PRs. The organization fixture never shows it, so a
    third account opens the synthetic User namespace, then is removed again."""
    personal = dict(username="fixture-one", org=github_backend.PERSON)
    identity = None
    try:
        value = json.loads(config.read_text(encoding="utf-8-sig"))
        value["github"]["accounts"].append(personal)
        value["native"]["activeSection"] = "github"
        config.write_text(json.dumps(value), encoding="utf-8")
        # Native mirrors Convex's accounts at launch, so the account is created there too.
        identity = mutate(backend, "githubAccounts:create", personal)
        workbench = driver.drive("workbench-dark", [
            ("content", github_backend.PERSON + " / fixture-one"), ("content", "Organization"),
            ("wait_content", "Current workbench"),
            # The section sits above the repository list; its cards may be below the fold.
            ("wait_scroll_text", github_backend.WORKBENCH_ISSUE, "Current workbench"),
            ("capture", "workbench-dark"),
            # Later scenes start from the organization namespace.
            ("content", "fixture-org / fixture-one"), ("sleep", 1),
        ])
        # Checked before cleanup, so a failed cleanup is a note on this error
        # rather than hiding it.
        check(workbench / "workbench-dark.png", "Current workbench", "workbench-demo", github_backend.WORKBENCH_ISSUE)
    finally:
        remove_personal_account(config, backend, identity, sys.exc_info()[1])


def qualify(executable, output, session, version, backend, api):
    # GitHub's route does not display a version label. Bind these captures to
    # the actual installed executable, also exercised by the base UI fixture.
    binary = installed_binary(executable, version)
    config = fixture(output, "dark")
    initial = json.loads(config.read_text())
    initial["github"]["accounts"] = [dict(username=user, org="fixture-org") for user in ("fixture-one", "fixture-two")]
    initial["native"]["activeSection"] = "github"
    initial["ui"]["dashboardCards"]["workspace-pulse"] = True
    config.write_text(json.dumps(initial))
    (output / "github-fixture.allow").write_text("hs-buddy native GitHub qualification v1")
    for user in ("fixture-one", "fixture-two"):
        mutate(backend, "githubAccounts:create", dict(username=user, org="fixture-org"))
    driver = settings.Driver(executable, output, session, backend, HERE.parent.parent)
    activity_before = backend.query("buddyStats:get")
    review = driver.drive("review-dark", [
        ("capture", "list-dark"), ("input", "Filter loaded items", "First private"),
        ("content", "List"), ("capture", "filter-list-dark"),
        ("sidebar", "Needs Review"), ("sidebar", "Recently Merged"), ("sidebar", "Needs a nudge"),
        ("sidebar", "My PRs"), ("sidebar", "My PRs"), ("input", "Filter loaded items", "First private"),
        ("content", "#7 First private navigation review"), ("tab", "Reviews"),
        ("tab", "Commits"), ("tab", "Checks"), ("tab", "Files"), ("wheel", -360), ("capture", "diff-dark"),
        ("tab", "History"), ("tab", "Review threads"), ("content", "src/navigation.rs"),
        ("content", "Back"), ("tab", "Conversation"), ("wheel", -480),
        ("input", "PR comment", "Installed native fixture comment"), ("content", "Review comment"),
        ("capture", "confirmation-dark"), ("content", "Confirm action"), ("sleep", 1),
        ("capture", "write-ack-dark"),
        # The acknowledgement also says Refresh. Identify the toolbar row.
        ("between", "Refresh", "Filter loaded items", "Conversation"),
    ], record=True)
    check(review / "filter-list-dark.png", "First private", "List")
    check(review / "diff-dark.png", "navigation.rs", "native navigation", "Patch unavailable")
    check(review / "confirmation-dark.png", "Confirm as", "fixture-one", "Installed native fixture comment")
    check(review / "write-ack-dark.png", "Comment posted", "Refresh")
    verify_detail_requests(api.requests)
    writes = [request for request in api.requests if request["method"] == "POST" and request["path"].endswith("/issues/7/comments")]
    if len(writes) != 1 or writes[0]["account"] != "fixture-one" or writes[0]["body"] != dict(body="Installed native fixture comment"):
        raise RuntimeError("The observed confirmed write did not reach exactly the reviewed fixture account and PR")
    saved = json.loads(config.read_text(encoding="utf-8-sig"))
    if saved["native"]["github"]["viewModes"]["pr-list-my-prs"] != "list":
        raise RuntimeError("The native list preference was not persisted")
    if backend.query("settings:get")["viewModes"]["pr-list-my-prs"] != "list":
        raise RuntimeError("The native list preference did not reach Convex")

    # The Dashboard Electron opens at launch, five PR-list opens, then
    # Electron's openTab for the PR and seven section choices: Reviews,
    # Commits, Checks, Files, History, Review threads and Conversation.
    # Electron shows a review thread inside its PR page, and Back restores a
    # view, so neither counts.
    require_activity(backend, activity_before,
        dict(tabsOpened=14, prsViewed=2, prsReviewed=1, prsMergedWatched=1), launches=1)
    # A fresh installed process must retain the backend counters without
    # treating initial restoration, refresh or selecting a retained tab as opens.
    activity = driver.drive("activity-restored-dark", [
        ("sidebar", "Needs Review"), ("sidebar", "Needs Review"),
        ("workspace", "Dashboard"), ("workspace", "Needs Review"),
        ("content", "Refresh"),
        ("capture", "restored-sidebar-list-dark"), ("workspace", "My PRs"),
        ("capture", "restored-pr-list-dark"), ("workspace", "Dashboard"),
        ("wait_content", "PRs Viewed"), ("capture", "workspace-activity-dark"),
    ], record=True)
    # Two Needs Review opens and the second launch's Dashboard.
    activity_after = require_activity(backend, activity_before,
        dict(tabsOpened=17, prsViewed=2, prsReviewed=3, prsMergedWatched=1), launches=2)
    # The fixture's reviewed-by source marks #7 approved. Needs Review keeps
    # the unapproved #8 from its paginated assignee source instead.
    check(activity / "restored-sidebar-list-dark.png", "Needs Review", "#8 Paginated fixture follow-up")
    check(activity / "restored-pr-list-dark.png", "My PRs", "First private")
    # The drive's wait_content step located "PRs Viewed" on screen just before
    # this capture. Whole-frame Tesseract misses that small grey label on
    # Windows in every layout (runs 45074068390 and 45361405781) although the
    # card shows it, so the capture check proves the card and the counters are
    # proven by require_activity above.
    check(activity / "workspace-activity-dark.png", "Workspace Pulse")
    (output / "activity.json").write_text(json.dumps(dict(before=activity_before, after=activity_after,
        checks=["four explicit modes", "repeated current mode", "repeated loaded mode retained across tabs",
            "no refresh write", "no retained-tab write",
            "restart persistence", "one closed session per launch"]), indent=2))

    qualify_review_sound(driver, config, output, api)
    reset_section(config)
    navigation_before = backend.query("buddyStats:get")
    navigation = driver.drive("navigation-dark", [
        ("content", "Repositories"), ("content", "fixture-org/native-demo"), ("tab", "Commits"),
        ("content", "Improve native navigation"), ("content", "Back"), ("tab", "Open issues"),
        ("content", "#11 Fixture keyboard follow-up"), ("content", "Back"), ("tab", "Closed issues"),
        ("tab", "Open PRs"), ("tab", "Closed PRs"), ("tab", "Overview"), ("capture", "repository-dark"),
        ("content", "Bookmark repository"), ("sleep", 1),
        # Electron's sidebar filter: only the repository just bookmarked stays listed.
        ("content", "Repositories"), ("content", "Bookmarked only"),
        ("wait_content", "1 of 3 loaded repositories"), ("capture", "bookmarked-only-dark"),
        ("content", "People"), ("content", "fixture-author"), ("capture", "profile-dark"),
        ("content", "Favorite user"), ("tab", "Authored PRs"), ("tab", "Reviewed PRs"),
        ("tab", "Activity"), ("tab", "Team memberships"), ("content", "Native Team"),
        # RateLimitGauge.tsx: the overview header shows the remaining quota.
        ("content", "Organization"), ("wait_content", "resets"), ("capture", "organization-dark"),
    ])
    check(navigation / "repository-dark.png", "native-demo", "Languages", "Rust")
    check(navigation / "organization-dark.png", "4,321", "resets")
    check(navigation / "profile-dark.png", "Demo Contributor", "Contribution calendar")
    check(navigation / "bookmarked-only-dark.png", "1 of 3 loaded repositories", "native-demo",
        absent=("fixture-org/limited", "fixture-org/denied"))
    if json.loads(config.read_text(encoding="utf-8-sig"))["ui"].get("showBookmarkedOnly") is not True:
        raise RuntimeError("The bookmarked-only repository filter was not persisted")
    # The launch's Dashboard, then Electron's openTab for the repository, its
    # six tab choices, a commit, an issue, a user and the organization. People, a user's own tabs and a
    # team are Electron sidebar groups or page sections. Both repository-list
    # opens are organization expansions, which count reposBrowsed only.
    require_activity(backend, navigation_before, dict(tabsOpened=12, reposBrowsed=2,
        prsViewed=0, prsReviewed=0, prsMergedWatched=0), launches=1)
    if not backend.query("repoBookmarks:list"):
        raise RuntimeError("The installed bookmark action did not reach Convex")
    if "fixture-author" not in json.loads(config.read_text(encoding="utf-8-sig"))["ui"]["favoriteUsers"]:
        raise RuntimeError("The installed favorite action was not persisted")
    qualify_workbench(driver, config, backend)

    # StatusBar.tsx: the assistant indicator and brand in the centre, and the
    # background sync item for the configured accounts' PR lists.
    value = json.loads(config.read_text(encoding="utf-8-sig"))
    value["ui"]["assistantOpen"] = True
    value["native"]["activeSection"] = "github"
    config.write_text(json.dumps(value), encoding="utf-8")
    status = driver.drive("status-bar-dark", [("sleep", 3), ("capture", "status-bar-dark")])
    check_status_bar(status / "status-bar-dark.png")
    value = json.loads(config.read_text(encoding="utf-8-sig"))
    value["ui"]["assistantOpen"] = False
    config.write_text(json.dumps(value), encoding="utf-8")

    # The sidebar split from ui.paneSizes, a divider drag saved to it, and
    # the root zoom from ui.zoomLevel, measured from the captures' pixels.
    layout = sidebar_layout.qualify(driver, config, check, settings.run)
    (output / "sidebar-layout.json").write_text(json.dumps(layout, indent=2), encoding="utf-8")

    light_template = output / "light-template"
    light_config = fixture(light_template, "light")
    value = json.loads(config.read_text(encoding="utf-8-sig"))
    value["ui"].update(json.loads(light_config.read_text())["ui"])
    # Later scenes open repositories that are not bookmarked.
    value["ui"]["showBookmarkedOnly"] = False
    value["native"]["activeSection"] = "github"
    config.write_text(json.dumps(value))
    light = driver.drive("accounts-light", [
        ("resize", 900, 650), ("input", "Filter loaded items", "First private"),
        ("key", "Tab"), ("key", "Return"), ("sleep", 1), ("capture", "compact-keyboard-light"),
        ("resize", 1200, 820), ("content", "fixture-org / fixture-two"), ("sleep", 1), ("capture", "second-account-light"),
        ("content", "Repositories"), ("content", "fixture-org/limited"), ("capture", "rate-limit-light"),
        ("content", "Repositories"), ("content", "fixture-org/denied"), ("capture", "denied-light"),
    ])
    check(light / "second-account-light.png", "Second private", absent=("First private", "Installed native fixture comment"))
    check(light / "rate-limit-light.png", "GitHub is limiting requests")
    check(light / "denied-light.png", "cannot access")
    check(light / "compact-keyboard-light.png", "My PRs", "First private")
    if json.loads(config.read_text(encoding="utf-8-sig"))["native"]["github"]["viewModes"]["pr-list-my-prs"] != "card":
        raise RuntimeError("Tab and Enter did not activate the native Cards view control")

    api.scenario = "auth"
    reset_section(config)
    missing = driver.drive("missing-auth", [("content", "fixture-org / fixture-two"), ("sleep", 1), ("capture", "missing-auth-light")])
    check(missing / "missing-auth-light.png", "Sign in to GitHub CLI", absent=("First private", "Second private"))
    api.scenario = "empty"
    reset_section(config)
    empty = driver.drive("empty", [("capture", "empty-light")])
    check(empty / "empty-light.png", "No items for this account and view")
    (output / "requests.json").write_text(json.dumps(api.requests, indent=2))
    (output / "qualification.json").write_text(json.dumps(dict(result="passed", version=version, session=session,
        **binary, largeWindow=driver.large_size,
        dataSource="Synthetic loopback GitHub API with fake credentials and an isolated anonymous Convex deployment",
        checks=["four PR modes", "PR-list activity and restart persistence", "filter and card/list persistence", "all PR detail tabs", "confirmed scoped fixture comment", "monitored synthetic review completion with sound enabled; no speaker-output claim",
            "repository/commit/issue navigation", "profile/calendar/teams", "organization rate-limit gauge", "personal-namespace current workbench", "status bar assistant, brand and PR sync items",
            "sidebar width from ui.paneSizes", "divider drag saved to ui.paneSizes", "ui.zoomLevel 125 root zoom", "Convex bookmark and favorite",
            "GitHub view and repository-list activity", "bookmarked-only repository filter",
            "two-account data separation", "missing auth", "rate limit", "denial", "empty", "light/dark", "keyboard Tab", "compact window"]), indent=2))


def main():
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("BUDDY_SETTINGS_FIXTURE") != "1":
        raise RuntimeError("Installed GitHub qualification requires a disposable GitHub Actions runner")
    parser = argparse.ArgumentParser()
    parser.add_argument("--executable", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--session", choices=("windows", "macos", "x11"), required=True)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    if output.exists() or not output.is_relative_to(Path(os.environ["RUNNER_TEMP"]).resolve()):
        raise RuntimeError("A fresh owned runner output directory is required")
    with settings.SettingsBackend(HERE.parent.parent) as backend, GitHubBackend() as api, fixture_environment(api.url):
        try:
            qualify(args.executable.resolve(strict=True), output, args.session, args.version, backend, api)
        except Exception as failure:
            preserve_failed_requests(output, api.requests, failure)
            raise


if __name__ == "__main__":
    main()
