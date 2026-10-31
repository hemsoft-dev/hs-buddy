"""Exercise an installed release with two synthetic accounts on an owned desktop."""
import argparse
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import urllib.request

from desktop import fixture, require_version
import importlib.util

HERE = Path(__file__).resolve().parent


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), HERE / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


settings = load("settings-desktop")
GitHubBackend = load("github-backend").GitHubBackend


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


def installed_binary(executable, version):
    output = settings.run(executable, "--version")
    require_version(output, version)
    return dict(versionOutput=output, binarySha256=hashlib.sha256(executable.read_bytes()).hexdigest())


def analysis_images(path):
    yield path
    # Analysis copy only; the original window capture is never edited.
    image = path.with_suffix(".ocr-scale3.png")
    settings.run("ffmpeg", "-nostdin", "-y", "-loglevel", "error", "-i", path,
        "-vf", "scale=iw*3:ih*3:flags=neighbor", "-frames:v", "1", image)
    yield image
    # Mixed dark shell chrome and a light hosted page can segment separately.
    # Keep one-layout matching, but offer the same contrast analysis as locators.
    image = path.with_suffix(".ocr-scale3-inverted.png")
    settings.run("ffmpeg", "-nostdin", "-y", "-loglevel", "error", "-i", path,
        "-vf", "scale=iw*3:ih*3:flags=neighbor,format=gray,negate", "-frames:v", "1", image)
    yield image


def check(path, *markers, absent=()):
    missing = None
    for image in analysis_images(path):
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


def qualify(executable, output, session, version, backend, api):
    # GitHub's route does not display a version label. Bind these captures to
    # the actual installed executable, also exercised by the base UI fixture.
    binary = installed_binary(executable, version)
    config = fixture(output, "dark")
    initial = json.loads(config.read_text())
    initial["github"]["accounts"] = [dict(username=user, org="fixture-org") for user in ("fixture-one", "fixture-two")]
    initial["native"]["activeSection"] = "github"
    config.write_text(json.dumps(initial))
    (output / "github-fixture.allow").write_text("hs-buddy native GitHub qualification v1")
    for user in ("fixture-one", "fixture-two"):
        mutate(backend, "githubAccounts:create", dict(username=user, org="fixture-org"))
    driver = settings.Driver(executable, output, session, backend, HERE.parent.parent)
    review = driver.drive("review-dark", [
        ("capture", "list-dark"), ("input", "Filter loaded items", "First private"),
        ("content", "List"), ("capture", "filter-list-dark"),
        ("content", "Needs Review"), ("content", "Recently Merged"), ("content", "Needs a nudge"),
        ("content", "My PRs"), ("input", "Filter loaded items", "First private"),
        ("content", "#7 First private navigation review"), ("tab", "Reviews"),
        ("tab", "Commits"), ("tab", "Checks"), ("tab", "Files"), ("wheel", -360), ("capture", "diff-dark"),
        ("tab", "History"), ("tab", "Review threads"), ("content", "src/navigation.rs"),
        ("content", "Back"), ("tab", "Conversation"), ("wheel", -480),
        ("input", "PR comment", "Installed native fixture comment"), ("content", "Review comment"),
        ("capture", "confirmation-dark"), ("content", "Confirm action"), ("sleep", 1),
        ("capture", "write-ack-dark"), ("content", "Refresh"),
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

    navigation = driver.drive("navigation-dark", [
        ("content", "Repositories"), ("content", "fixture-org/native-demo"), ("tab", "Commits"),
        ("content", "Improve native navigation"), ("content", "Back"), ("tab", "Open issues"),
        ("content", "#11 Fixture keyboard follow-up"), ("content", "Back"), ("tab", "Closed issues"),
        ("tab", "Open PRs"), ("tab", "Closed PRs"), ("tab", "Overview"), ("capture", "repository-dark"),
        ("content", "Bookmark repository"), ("sleep", 1),
        ("content", "People"), ("content", "fixture-author"), ("capture", "profile-dark"),
        ("content", "Favorite user"), ("tab", "Authored PRs"), ("tab", "Reviewed PRs"),
        ("tab", "Activity"), ("tab", "Team memberships"), ("content", "Native Team"),
        ("content", "Organization"),
    ])
    check(navigation / "repository-dark.png", "native-demo", "Languages", "Rust")
    check(navigation / "profile-dark.png", "Demo Contributor", "Contribution calendar")
    if not backend.query("repoBookmarks:list"):
        raise RuntimeError("The installed bookmark action did not reach Convex")
    if "fixture-author" not in json.loads(config.read_text(encoding="utf-8-sig"))["ui"]["favoriteUsers"]:
        raise RuntimeError("The installed favorite action was not persisted")

    light_template = output / "light-template"
    light_config = fixture(light_template, "light")
    value = json.loads(config.read_text(encoding="utf-8-sig"))
    value["ui"].update(json.loads(light_config.read_text())["ui"])
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
        checks=["four PR modes", "filter and card/list persistence", "all PR detail tabs", "confirmed scoped fixture comment",
            "repository/commit/issue navigation", "profile/calendar/teams", "Convex bookmark and favorite",
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
