"""Exact category-open deltas against the owned anonymous Settings backend."""
import json
import sys


COUNTERS = (
    "appLaunches", "tabsOpened", "prsViewed", "prsReviewed", "prsMergedWatched",
    "reposBrowsed", "repoDetailViews", "jobsCreated", "runsTriggered", "runsCompleted",
    "runsFailed", "schedulesCreated", "bookmarksCreated", "settingsChanged",
    "searchesPerformed", "copilotPrReviews", "totalUptimeMs",
)


def require_delta(before, after, expected):
    changes = {key: after.get(key, 0) - before.get(key, 0) for key in COUNTERS}
    wanted = {key: expected if key == "tabsOpened" else 0 for key in COUNTERS}
    if changes != wanted or before.get("lastSessionStart") != after.get("lastSessionStart"):
        raise RuntimeError(f"Settings category activity differs: {changes}, expected {wanted}")


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
        light["ui"]["theme"] = "light"
        config.write_text(json.dumps(light), encoding="utf-8")
        restored = driver.drive("category-restored-light", [
            ("resize", 900, 650), ("capture", "restored-accounts-light"),
        ])
        verify(restored / "restored-accounts-light.png", "Settings", "Add account")
        restarted = backend.query("buddyStats:get")
        require_delta(after, restarted, 0)
        (output / "category-activity.json").write_text(json.dumps(dict(
            before=before, after=after, restarted=restarted, explicitOpens=3,
            restartOpens=0,
        ), indent=2), encoding="utf-8")
    finally:
        primary = sys.exc_info()[1]
        try:
            config.write_bytes(original)
        except Exception as cleanup:
            if primary is None:
                raise
            primary.add_note(f"Fixture configuration restore failed: {type(cleanup).__name__}")
