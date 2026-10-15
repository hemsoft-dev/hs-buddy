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


def check(path, version, *markers, absent=()):
    settings.verify_text(path, *markers)
    text = path.with_suffix(".ocr.txt").read_text()
    require_version(text, version)
    for marker in absent:
        if marker.casefold() in text.casefold():
            raise RuntimeError("Another account's fixture data remained visible")


def qualify(executable, output, session, version, backend, api):
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
        ("capture", "list-dark"), ("field", "Filter loaded items", "First private"),
        ("click", "List"), ("capture", "filter-list-dark"),
        ("click", "Needs Review"), ("click", "Recently Merged"), ("click", "Needs a nudge"),
        ("click", "My PRs"), ("field", "Filter loaded items", "First private"),
        ("click", "#7 First private navigation review"), ("click", "Reviews"),
        ("click", "Commits"), ("click", "Checks"), ("click", "Files"), ("capture", "diff-dark"),
        ("click", "History"), ("click", "Review threads"), ("click", "src/navigation.rs"),
        ("click", "Back"), ("click", "Conversation"), ("wheel", -480),
        ("field", "PR comment", "Installed native fixture comment"), ("click", "Review comment"),
        ("capture", "confirmation-dark"), ("click", "Confirm action"), ("sleep", 1),
        ("capture", "write-ack-dark"), ("click", "Refresh"),
    ], record=True)
    check(review / "filter-list-dark.png", version, "First private", "List")
    check(review / "diff-dark.png", version, "navigation.rs", "native navigation", "Patch unavailable")
    check(review / "confirmation-dark.png", version, "Confirm as", "fixture-one", "Installed native fixture comment")
    check(review / "write-ack-dark.png", version, "Comment posted", "Refresh")
    writes = [request for request in api.requests if request["method"] == "POST" and request["path"].endswith("/issues/7/comments")]
    if len(writes) != 1 or writes[0]["account"] != "fixture-one" or writes[0]["body"] != dict(body="Installed native fixture comment"):
        raise RuntimeError("The observed confirmed write did not reach exactly the reviewed fixture account and PR")
    saved = json.loads(config.read_text(encoding="utf-8-sig"))
    if saved["native"]["github"]["viewModes"]["pr-list-my-prs"] != "list":
        raise RuntimeError("The native list preference was not persisted")
    if backend.query("settings:get")["viewModes"]["pr-list-my-prs"] != "list":
        raise RuntimeError("The native list preference did not reach Convex")

    navigation = driver.drive("navigation-dark", [
        ("click", "Repositories"), ("click", "fixture-org/native-demo"), ("click", "Commits"),
        ("click", "Improve native navigation"), ("click", "Back"), ("click", "Open issues"),
        ("click", "#11 Fixture keyboard follow-up"), ("click", "Back"), ("click", "Closed issues"),
        ("click", "Open PRs"), ("click", "Closed PRs"), ("click", "Overview"), ("capture", "repository-dark"),
        ("click", "Bookmark repository"), ("sleep", 1),
        ("click", "People"), ("click", "fixture-author"), ("capture", "profile-dark"),
        ("click", "Favorite user"), ("click", "Authored PRs"), ("click", "Reviewed PRs"),
        ("click", "Activity"), ("click", "Team memberships"), ("click", "Native Team"),
        ("click", "Organization"),
    ])
    check(navigation / "repository-dark.png", version, "native-demo", "Languages", "Rust")
    check(navigation / "profile-dark.png", version, "Demo Contributor", "Contribution calendar")
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
        ("resize", 900, 650), ("key", "Tab"), ("capture", "compact-keyboard-light"),
        ("resize", 1200, 820), ("click", "fixture-org / fixture-two"), ("sleep", 1), ("capture", "second-account-light"),
        ("click", "Repositories"), ("click", "fixture-org/limited"), ("capture", "rate-limit-light"),
        ("click", "Repositories"), ("click", "fixture-org/denied"), ("capture", "denied-light"),
    ])
    check(light / "second-account-light.png", version, "Second private", absent=("First private", "Installed native fixture comment"))
    check(light / "rate-limit-light.png", version, "GitHub is limiting requests")
    check(light / "denied-light.png", version, "cannot access")
    check(light / "compact-keyboard-light.png", version, "My PRs", "First private")

    api.scenario = "auth"
    reset_section(config)
    missing = driver.drive("missing-auth", [("click", "fixture-org / fixture-two"), ("sleep", 1), ("capture", "missing-auth-light")])
    check(missing / "missing-auth-light.png", version, "Sign in to GitHub CLI", absent=("First private", "Second private"))
    api.scenario = "empty"
    reset_section(config)
    empty = driver.drive("empty", [("capture", "empty-light")])
    check(empty / "empty-light.png", version, "No items for this account and view")
    (output / "requests.json").write_text(json.dumps(api.requests, indent=2))
    (output / "qualification.json").write_text(json.dumps(dict(result="passed", version=version, session=session,
        binarySha256=hashlib.sha256(executable.read_bytes()).hexdigest(), largeWindow=driver.large_size,
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
        qualify(args.executable.resolve(strict=True), output, args.session, args.version, backend, api)


if __name__ == "__main__":
    main()
