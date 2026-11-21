"""Installed workspace-tab context menus on disposable CI desktops.

Three workspace tabs are open in each pass: the Dashboard and My PRs tabs every
launch restores, and the repository list opened from the GitHub view. The
GitHub data is the synthetic loopback API the GitHub qualification uses, and
activity goes to the same disposable anonymous Convex deployment. A right-click
on a tab is the driver's `workspace_menu` action: the tab is found by its title
like a left click, and only the mouse button differs.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import sys

from desktop import fixture

HERE = Path(__file__).resolve().parent
CHECKOUT = HERE.parent.parent
ACCOUNTS = ("fixture-one", "fixture-two")
DASHBOARD, MY_PRS, REPOSITORIES = "Dashboard", "My PRs", "Repositories"
# Left to right once the repository list is open (rust/crates/buddy-app/src/app.rs).
TABS = (DASHBOARD, MY_PRS, REPOSITORIES)
# shell/tab_bar.rs lists Close first; every other command names it uniquely.
CLOSE_OTHERS, CLOSE_RIGHT = "Close Others", "Close to the Right"
COMMANDS = (CLOSE_OTHERS, CLOSE_RIGHT, "Close All", "Move Left", "Move Right")
# Commands no tab count disables. A wait reads one of them in one OCR line.
MENU_OPEN = "Close All|Move Left|Move Right"
ALWAYS_ENABLED = ("Close All", "Move Left", "Move Right")
# Only the Dashboard draws this header button (dashboard/header.rs), at the
# window's right edge, and only the GitHub view lists this sidebar mode, at its
# left edge. A menu opened on a tab covers neither.
DASHBOARD_ONLY, GITHUB_ONLY = "Customize", "Needs a nudge"


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), HERE / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


terminal = load("terminal-desktop")
github = terminal.github
settings = terminal.settings
COUNTERS = load("settings-activity").COUNTERS


def require_runner():
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("BUDDY_SETTINGS_FIXTURE") != "1":
        raise RuntimeError("Tab menu desktop qualification requires a disposable GitHub Actions runner")


def open_tabs():
    """Add the third tab, and wait for the list that gives it its title.

    The repository list is a read: unlike a sidebar PR mode it records no
    activity, so a pass that only opens tabs and menus must leave the backend
    exactly as it found it.
    """
    return [("content", REPOSITORIES), ("wait_content", "fixture-org/native-demo")]


def dismissed_actions(theme, compact):
    """Right-click an inactive tab, capture its menu, then dismiss it with Escape.

    The Dashboard is active, so both GitHub tabs are inactive. The large pass
    targets the last tab, whose menu shows Close to the Right disabled; the
    compact pass targets the middle tab, where every command is enabled.
    """
    actions = open_tabs() + [("workspace", DASHBOARD)]
    # The compact pass settles after its resize instead of reading a card.
    actions += [("resize", 900, 650), ("sleep", 1)] if compact else [("wait_content", "PRs Viewed")]
    return actions + [("capture", f"tabs-{theme}"),
        ("workspace_menu", MY_PRS if compact else REPOSITORIES),
        ("wait_text", MENU_OPEN), ("capture", f"menu-{theme}"),
        ("key", "Escape"), ("sleep", 1), ("capture", f"dismissed-{theme}")]


def command_actions():
    """Enabled bulk commands close tabs; once unavailable they are inert.

    A disabled command neither runs nor closes its menu, so the menu is still
    open after it is chosen. The repository list is active when the first menu
    opens on the inactive My PRs tab.
    """
    return open_tabs() + [
        ("capture", "three-tabs-dark"),
        ("workspace_menu", MY_PRS), ("wait_text", MENU_OPEN), ("capture", "right-enabled-dark"),
        # Closing the active repository tab shows My PRs, which reloads its
        # list and, with it, its tab title. No text is unique to that list.
        ("content", CLOSE_RIGHT), ("sleep", 2), ("capture", "closed-right-dark"),
        # My PRs is now the last of two tabs: nothing is to its right.
        ("workspace_menu", MY_PRS), ("wait_text", MENU_OPEN),
        ("content", CLOSE_RIGHT), ("wait_text", MENU_OPEN), ("capture", "right-disabled-dark"),
        ("content", CLOSE_OTHERS), ("sleep", 1), ("capture", "closed-others-dark"),
        # My PRs is now the only tab: there are no others.
        ("workspace_menu", MY_PRS), ("wait_text", MENU_OPEN),
        ("content", CLOSE_OTHERS), ("wait_text", MENU_OPEN), ("capture", "others-disabled-dark"),
        # An enabled command closes the menu. Moving the only tab changes nothing.
        ("content", "Move Left"), ("sleep", 1), ("capture", "single-tab-dark"),
    ]


def tab_row_words(capture):
    """The words in the workspace tab row, read with the tab locator's own parts.

    The same OCR engine per platform and the same row boundaries as a click on
    a tab. When OCR cannot read the menu above the row, the row is the painted
    tab band, as it is for the locator.
    """
    ocr = settings.ocr
    words = ocr.recognize_windows(capture) if sys.platform == "win32" else ocr.recognize(capture, 11)
    capture.with_suffix(".tab-row-ocr.json").write_text(json.dumps(words), encoding="utf-8")
    try:
        return ocr.workspace_words(words)
    except ocr.AmbiguousLabel:
        raise
    except RuntimeError:
        top, bottom = ocr.workspace_geometry(capture)
        return [word for word in words if top <= word["top"] + word["height"] / 2 < bottom]


def require_tab_row(capture, present, absent=()):
    """Require open and closed workspace tabs by the titles in the tab row.

    A title counts as closed only on a frame whose remaining titles the same
    read found, so an unreadable row cannot pass for a closed tab. The GitHub
    view repeats these names below the row, where they never count.
    """
    if not present:
        raise RuntimeError("A closed tab needs another title read from the same frame")
    row = tab_row_words(capture)
    for title in present:
        settings.ocr.locate_label(row, title)
    for title in absent:
        try:
            settings.ocr.locate_label(row, title)
        except settings.ocr.AmbiguousLabel:
            raise
        except RuntimeError:
            continue
        raise RuntimeError(f"The {title!r} workspace tab is still open in {capture.name}")


def require_unchanged(before, after, name):
    """Opening tabs and using their menus recorded no activity at all."""
    changes = {key: after.get(key, 0) - before.get(key, 0) for key in COUNTERS}
    if any(changes.values()) or before.get("lastSessionStart") != after.get("lastSessionStart"):
        raise RuntimeError(f"Tab menus changed the backend in {name}: {changes}")
    return changes


def require_reads_only(requests):
    """No tab menu or bulk close may write to GitHub."""
    for request in requests:
        graphql = request.get("path") == "/graphql"
        if request.get("method") != ("POST" if graphql else "GET") or (
                graphql and request.get("query", "").lstrip().startswith("mutation")):
            raise RuntimeError("A workspace tab menu pass wrote to the synthetic GitHub API")


def use_light_theme(config, output):
    preset = json.loads(fixture(output / "light-template", "light").read_text())["ui"]
    terminal.write_config(config, lambda value: value["ui"].update(preset))


def measured(driver, backend, baseline, name, actions, journal):
    """Drive one recorded pass, then compare the backend with the scene's start."""
    flow = driver.drive(name, actions, record=True)
    after = backend.query("buddyStats:get")
    journal[name] = dict(after=after, changes=require_unchanged(baseline, after, name))
    return flow


def check_dismissed(flow, theme, compact):
    """The menu opened over the Dashboard, and Escape left the tabs as they were."""
    github.check(flow / f"tabs-{theme}.png", DASHBOARD_ONLY, absent=ALWAYS_ENABLED)
    # The right-clicked GitHub tab is inactive: the Dashboard stays behind its
    # menu. A disabled command is dimmed, so only enabled ones are required.
    enabled = COMMANDS if compact else tuple(command for command in COMMANDS if command != CLOSE_RIGHT)
    github.check(flow / f"menu-{theme}.png", *enabled, DASHBOARD_ONLY, absent=(GITHUB_ONLY,))
    github.check(flow / f"dismissed-{theme}.png", DASHBOARD_ONLY, absent=(*ALWAYS_ENABLED, GITHUB_ONLY))
    require_tab_row(flow / f"dismissed-{theme}.png", TABS)


def check_commands(flow):
    """Enabled commands closed exactly their tabs; disabled ones left the menu open."""
    github.check(flow / "three-tabs-dark.png", MY_PRS, absent=ALWAYS_ENABLED)
    require_tab_row(flow / "three-tabs-dark.png", TABS)
    github.check(flow / "right-enabled-dark.png", *COMMANDS)
    github.check(flow / "closed-right-dark.png", MY_PRS, absent=ALWAYS_ENABLED)
    require_tab_row(flow / "closed-right-dark.png", (DASHBOARD, MY_PRS), absent=(REPOSITORIES,))
    github.check(flow / "right-disabled-dark.png", *ALWAYS_ENABLED)
    github.check(flow / "closed-others-dark.png", MY_PRS, absent=ALWAYS_ENABLED)
    require_tab_row(flow / "closed-others-dark.png", (MY_PRS,), absent=(DASHBOARD, REPOSITORIES))
    github.check(flow / "others-disabled-dark.png", *ALWAYS_ENABLED)
    github.check(flow / "single-tab-dark.png", MY_PRS, absent=ALWAYS_ENABLED)
    require_tab_row(flow / "single-tab-dark.png", (MY_PRS,), absent=(DASHBOARD, REPOSITORIES))


def qualify(executable, output, session, version, backend, api):
    binary = github.installed_binary(executable, version)
    config = fixture(output, "dark")

    def configure(value):
        value["github"]["accounts"] = [dict(username=user, org="fixture-org") for user in ACCOUNTS]
        value["native"]["activeSection"] = "github"
        # The large Dashboard pass waits for this card's "PRs Viewed" label.
        value["ui"]["dashboardCards"]["workspace-pulse"] = True
    terminal.write_config(config, configure)
    (output / "github-fixture.allow").write_text("hs-buddy native GitHub qualification v1")
    for user in ACCOUNTS:
        github.mutate(backend, "githubAccounts:create", dict(username=user, org="fixture-org"))
    driver = settings.Driver(executable, output, session, backend, CHECKOUT)
    baseline, journal = backend.query("buddyStats:get"), {}

    def drive(name, actions):
        # A pass that quits on the Dashboard saves it as the next launch's
        # section; every pass must start in the GitHub view, on My PRs. The
        # Dashboard is "no section", and the app then omits the whole
        # `native` object from the file (Ubuntu, run 44854538756).
        terminal.write_config(config, lambda value: value.setdefault("native", {}).update(activeSection="github"))
        return measured(driver, backend, baseline, name, actions, journal)
    check_dismissed(drive("menu-dark", dismissed_actions("dark", False)), "dark", False)
    check_commands(drive("commands-dark", command_actions()))
    use_light_theme(config, output)
    check_dismissed(drive("menu-light", dismissed_actions("light", True)), "light", True)
    require_reads_only(api.requests)
    (output / "requests.json").write_text(json.dumps(api.requests, indent=2), encoding="utf-8")
    (output / "backend-activity.json").write_text(json.dumps(dict(counters=COUNTERS, before=baseline, passes=journal),
        indent=2), encoding="utf-8")
    (output / "qualification.json").write_text(json.dumps(dict(result="passed", version=version, session=session,
        **binary, largeWindow=driver.large_size, desktopUi=True,
        dataSource="Synthetic loopback GitHub API with fake credentials and an isolated anonymous Convex deployment",
        checks=["three workspace tabs", "right-click menu on an inactive tab", "active tab unchanged by the menu",
            "Escape dismissal leaves every tab", "Close to the Right closes later tabs and is inert on the last tab",
            "Close Others closes the other tabs and is inert on the only tab", "interaction recordings",
            "no backend activity in any pass", "no GitHub write",
            f"dark {driver.large_size[0]}x{driver.large_size[1]}", "light 900x650"]),
        indent=2), encoding="utf-8")


def main():
    # Check the host before argparse, filesystem access, or any input.
    require_runner()
    parser = argparse.ArgumentParser()
    parser.add_argument("--executable", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--session", choices=("windows", "macos", "x11"), required=True)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    output = terminal.owned_output(args.output)
    executable = args.executable.resolve(strict=True)
    with settings.SettingsBackend(CHECKOUT) as backend, github.GitHubBackend() as api, \
            github.fixture_environment(api.url):
        try:
            qualify(executable, output, args.session, args.version, backend, api)
        except Exception as failure:
            github.preserve_failed_requests(output, api.requests, failure)
            raise


if __name__ == "__main__":
    main()
