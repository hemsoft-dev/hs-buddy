"""Installed Bookmarks on disposable CI desktops: categorized list, editor, delete, failure.

Bookmarks live in the same disposable anonymous Convex deployment the Settings and
GitHub qualifications use, seeded through the incumbent `bookmarks:create` function.
Nothing is opened in a browser and no page title is fetched: the editor is driven by
typing, and Open is covered by the headless view tests. The failure state comes from
a loopback port with nothing listening.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import socket
import types

from desktop import fixture

HERE = Path(__file__).resolve().parent
CHECKOUT = HERE.parent.parent

# url, title, category, tags, description
SEED = (
    ("https://doc.rust-lang.org/book/", "Rust Book", "Development/Rust", ["rust", "docs"], "The Rust programming language book"),
    ("https://www.gpui.rs/", "GPUI guide", "Development/Rust", ["gpui"], None),
    ("https://developer.mozilla.org/", "MDN reference", "Development/Web", ["web"], None),
    ("https://news.ycombinator.com/", "Morning news", "News", [], None),
    ("https://example.com/dark", "Old dark link", "Archive", [], None),
    ("https://example.com/light", "Old light link", "Archive", [], None),
)
THEMES = ("dark", "light")


def created(theme):
    """The bookmark each theme's pass adds; Convex refuses a URL twice in one category."""
    return dict(url=f"https://example.org/native-guide-{theme}", title="Native guide", category="Reading/Guides",
        tags=["native", "guide"])


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), HERE / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


terminal = load("terminal-desktop")
github = terminal.github
settings = terminal.settings


def require_runner():
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("BUDDY_SETTINGS_FIXTURE") != "1":
        raise RuntimeError("Bookmarks desktop qualification requires a disposable GitHub Actions runner")


def unreachable_url():
    """A loopback address with nothing listening: the native client fails at once."""
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return f"http://127.0.0.1:{probe.getsockname()[1]}"


def seed_arguments():
    for url, title, category, tags, description in SEED:
        arguments = dict(url=url, title=title, category=category)
        if tags:
            arguments["tags"] = tags
        if description:
            arguments["description"] = description
        yield arguments


def seed(backend):
    if backend.query("bookmarks:list") != []:
        raise RuntimeError("The disposable bookmark list is not empty")
    for arguments in seed_arguments():
        github.mutate(backend, "bookmarks:create", arguments)


def verify_backend(bookmarks, deleted):
    """The seeded bookmarks minus the deleted ones, plus the one added in each theme."""
    seeded = sorted((url, title) for url, title, *_ in SEED if title not in deleted)
    added = sorted((value["url"], value["title"]) for value in map(created, THEMES))
    found = sorted((item["url"], item["title"]) for item in bookmarks)
    if found != sorted(seeded + added):
        raise RuntimeError(f"Unexpected bookmarks after the exercise: {found}")
    for theme in THEMES:
        expected = created(theme)
        item = next(item for item in bookmarks if item["url"] == expected["url"])
        saved = {key: item.get(key) for key in expected}
        if saved != expected:
            raise RuntimeError(f"The {theme} pass's bookmark was saved as {saved}")


def add_and_delete(driver, theme, victim):
    """List, category, editor and delete confirm at the large size; list and editor compact."""
    new = created(theme)
    flow = driver.drive(f"bookmarks-{theme}", [
        ("wait_text", "Rust Book"), ("capture", f"list-{theme}"),
        ("click", "Development"), ("wait_text", "GPUI guide"), ("capture", f"category-{theme}"),
        ("click", "All bookmarks"), ("wait_text", "Morning news"),
        ("click", "Add"), ("wait_text", "New bookmark"),
        ("input", "URL", new["url"]), ("input", "Title", new["title"]),
        ("input", "Category", new["category"]), ("input", "Tags", ", ".join(new["tags"])),
        ("capture", f"editor-{theme}"),
        ("click", "Add Bookmark"), ("wait_text", "Bookmark added."), ("capture", f"added-{theme}"),
        ("input", "Search", victim), ("wait_text", victim),
        ("click", "Delete"), ("wait_text", "Confirm delete"), ("capture", f"delete-confirm-{theme}"),
        ("click", "Confirm delete"), ("wait_text", "Deleted"), ("capture", f"deleted-{theme}"),
        ("click", "Clear filters"), ("wait_text", "Rust Book"),
        ("resize", 900, 650), ("sleep", 1), ("capture", f"compact-list-{theme}"),
        ("click", "Add"), ("wait_text", "New bookmark"), ("capture", f"compact-editor-{theme}"),
        ("click", "Cancel"), ("wait_text", "Rust Book"),
    ], record=True)
    github.check(flow / f"list-{theme}.png", "Bookmarks", "All bookmarks", "Development", "News",
        "Rust Book", "Morning news", "Open externally")
    github.check(flow / f"category-{theme}.png", "In Development", "Rust", "Web", "GPUI guide", "MDN reference",
        absent=("Morning news",))
    github.check(flow / f"editor-{theme}.png", "New bookmark", "URL", "Title", "Category", "Tags", "Add Bookmark")
    github.check(flow / f"added-{theme}.png", "Bookmark added.", "Native guide", "Reading")
    github.check(flow / f"delete-confirm-{theme}.png", "Confirm delete", "permanently removed")
    github.check(flow / f"deleted-{theme}.png", "Deleted", absent=("Confirm delete",))
    github.check(flow / f"compact-list-{theme}.png", "Bookmarks", "Rust Book", "Search")
    github.check(flow / f"compact-editor-{theme}.png", "New bookmark", "URL", "Category")


def failure(driver, theme, compact):
    actions = [("wait_text", "Unable to load bookmarks")]
    if compact:
        actions.append(("resize", 900, 650))
    name = f"failure-{theme}" + ("-compact" if compact else "")
    actions += [("capture", name), ("click", "Retry"), ("sleep", 1), ("wait_text", "Unable to load bookmarks")]
    flow = driver.drive(name, actions, record=True)
    github.check(flow / f"{name}.png", "Unable to load bookmarks", "Convex", "Retry")


def qualify(executable, output, session, version, backend):
    binary = github.installed_binary(executable, version)
    config = fixture(output, "dark")
    terminal.write_config(config, lambda value: value["native"].update(activeSection="bookmarks"))
    seed(backend)
    driver = settings.Driver(executable, output, session, backend, CHECKOUT)
    offline = settings.Driver(executable, output, session, types.SimpleNamespace(url=unreachable_url()), CHECKOUT)
    add_and_delete(driver, "dark", "Old dark link")
    failure(offline, "dark", compact=False)
    light = json.loads(fixture(config.parent / "light-template", "light").read_text())
    terminal.write_config(config, lambda value: value["ui"].update(light["ui"]))
    add_and_delete(driver, "light", "Old light link")
    failure(offline, "light", compact=True)
    verify_backend(backend.query("bookmarks:list"), deleted={"Old dark link", "Old light link"})
    (output / "qualification.json").write_text(json.dumps(dict(result="passed", version=version, session=session,
        **binary, largeWindow=driver.large_size, desktopUi=True,
        dataSource="Installed app with an isolated anonymous Convex deployment seeded through bookmarks:create",
        checks=["categorized list and tree", "category filter", "add through the editor", "search",
            "delete confirm", "unreachable backend failure and retry", "compact window", "light/dark"]),
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
    with settings.SettingsBackend(CHECKOUT) as backend:
        qualify(executable, output, args.session, args.version, backend)


if __name__ == "__main__":
    main()
