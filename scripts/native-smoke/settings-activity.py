"""Exact category-open deltas against the owned anonymous Settings backend."""
import json
import sys

from desktop import fixture, require_closed_sessions


COUNTERS = (
    "appLaunches", "tabsOpened", "prsViewed", "prsReviewed", "prsMergedWatched",
    "reposBrowsed", "repoDetailViews", "jobsCreated", "runsTriggered", "runsCompleted",
    "runsFailed", "schedulesCreated", "bookmarksCreated", "settingsChanged",
    "searchesPerformed", "copilotPrReviews", "totalUptimeMs",
)
THEME_FIELDS = (
    "theme", "accentColor", "fontColor", "bgPrimary", "bgSecondary",
    "statusBarBg", "statusBarFg",
)


# Written by every launch (Electron's useAppSessionStats); checked on their own.
SESSION_COUNTERS = ("appLaunches", "totalUptimeMs")


def require_delta(before, after, expected, launches=1):
    """`expected` category opens, plus the Dashboard each launch opens, as
    Electron's tab list does when its window starts."""
    changes = {key: after.get(key, 0) - before.get(key, 0) for key in COUNTERS if key not in SESSION_COUNTERS}
    wanted = {key: expected + launches if key == "tabsOpened" else 0 for key in changes}
    if changes != wanted:
        raise RuntimeError(f"Settings category activity differs: {changes}, expected {wanted}")
    require_closed_sessions(before, after, launches)


def qualify(driver, backend, config, output, verify):
    original = config.read_bytes()
    before = backend.query("buddyStats:get")
    try:
        opened = driver.drive("category-activity-dark", [
            ("nav", "Accounts"), ("nav", "Accounts"), ("nav", "Appearance"),
            ("capture", "three-category-opens-dark"),
        ], record=True)
        verify(opened / "three-category-opens-dark.png", "Settings", "Accent color")
        after = backend.query("buddyStats:get")
        require_delta(before, after, 3)
        light = json.loads(original)
        # The theme name alone leaves the original explicit dark colors active.
        preset = json.loads(fixture(output / "category-light-preset", "light").read_text())["ui"]
        light["ui"].update({key: preset[key] for key in THEME_FIELDS})
        config.write_text(json.dumps(light), encoding="utf-8")
        restored = driver.drive("category-restored-light", [
            ("resize", 900, 650), ("capture", "restored-accounts-light"),
        ])
        verify(restored / "restored-accounts-light.png", "Settings", "Welcome to Buddy", "enterprise slug")
        restarted = backend.query("buddyStats:get")
        require_delta(after, restarted, 0)
        (output / "category-activity.json").write_text(json.dumps(dict(
            before=before, after=after, restarted=restarted, explicitOpens=3,
            restartOpens=0, launchDashboardOpens=1, sessionsPerLaunch=1,
        ), indent=2), encoding="utf-8")
    finally:
        primary = sys.exc_info()[1]
        try:
            config.write_bytes(original)
        except Exception as cleanup:
            if primary is None:
                raise
            primary.add_note(f"Fixture configuration restore failed: {type(cleanup).__name__}")
