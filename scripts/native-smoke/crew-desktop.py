"""Installed Crew on disposable CI desktops: projects, conversations, changed files, restart, errors.

Replies come from the offline Copilot fixture that Chat's qualification uses
(BUDDY_COPILOT_FIXTURE): the real SDK runtime runs, but every provider request is
answered locally and nothing reaches a model. The two projects are local clones whose
origin names GitHub; no network is used. Files are changed from this script, as an
editor or agent would, and kept or undone through the installed app.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import socket
import types
import uuid

from desktop import fixture, run

HERE = Path(__file__).resolve().parent
CHECKOUT = HERE.parent.parent
UNICODE = "資料界é"
ACCOUNT = "synthetic-account"
MODEL = "gpt-4.1"
STREAM = "BUDDYSTREAM61527"
CANCEL = "BUDDYCANCEL73429"


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
        raise RuntimeError("Crew desktop qualification requires a disposable GitHub Actions runner")


def unreachable_url():
    """Replies are not saved, so no backend is needed; the fixture requires a loopback URL."""
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return f"http://127.0.0.1:{probe.getsockname()[1]}"


def git(repository, *arguments):
    return run("git", "-C", repository, "-c", "core.autocrlf=false", *arguments)


def repository(path, name):
    """A clone of fixture-org/<name> with two committed files."""
    path.mkdir()
    git(path, "init", "-q", "-b", "main")
    git(path, "config", "core.autocrlf", "false")
    git(path, "remote", "add", "origin", f"https://github.com/fixture-org/{name}.git")
    (path / "README.md").write_text(f"# {name}\n", encoding="utf-8")
    (path / "src").mkdir()
    (path / "src" / "lib.rs").write_text("pub fn answer() -> u32 { 42 }\n", encoding="utf-8")
    git(path, "add", "-A")
    git(path, "-c", "user.name=Buddy Qualification", "-c", "user.email=qualification@example.invalid",
        "-c", "commit.gpgsign=false", "commit", "-q", "-m", "Initial")
    return path


def state(output):
    return json.loads((output / "native-crew" / "state.json").read_text(encoding="utf-8"))


def conversation(value, slug):
    project = next(item for item in value["projects"] if item["githubSlug"] == slug)
    sessions = [item for item in value["sessions"] if item["projectId"] == project["id"]]
    if len(sessions) > 1:
        raise RuntimeError(f"{slug} has more than one conversation")
    return sessions[0] if sessions else None


def roles(session):
    return [(message["role"], message.get("error", False)) for message in session["conversationHistory"]]


def verify_conversations(value, expected):
    """Each project's messages, in order, as (role, failed) pairs; no other project's."""
    if sorted(item["githubSlug"] for item in value["projects"]) != sorted(expected):
        raise RuntimeError("Unexpected Crew projects")
    for slug, wanted in expected.items():
        session = conversation(value, slug)
        found = roles(session) if session else None
        if found != wanted:
            raise RuntimeError(f"{slug} conversation is {found}, expected {wanted}")
        if session and (session["account"] != ACCOUNT or session["model"] != MODEL):
            raise RuntimeError(f"{slug} conversation changed account or model")
    for session in value["sessions"]:
        if session.get("pending"):
            raise RuntimeError("A reply is still marked as in flight")


def provider_report(output):
    reports = sorted(output.glob("copilot-provider-*.json"))
    totals = dict(requests=0, rejected=0, cancelled=0)
    for path in reports:
        value = json.loads(path.read_text(encoding="utf-8"))
        if value.get("offline") is not True or value.get("upstreamForwarder") is not False:
            raise RuntimeError("The provider fixture was not offline")
        for key in totals:
            totals[key] += value.get(key, 0)
    return totals


def qualify(executable, output, session, version):
    binary = github.installed_binary(executable, version)
    config = fixture(output, "dark")

    def configure(value):
        value["native"].update(activeSection="crew")
        value["github"]["accounts"] = [dict(username=ACCOUNT, org="fixture-org")]
        value.setdefault("copilot", {})["model"] = MODEL

    terminal.write_config(config, configure)
    (output / "copilot-fixture.allow").write_text(
        "hs-buddy native Copilot qualification v1\n" + str(uuid.uuid4()), encoding="utf-8")
    projects = output / "projects"
    projects.mkdir()
    alpha = repository(projects / f"alpha repo {UNICODE}", "alpha")
    beta = repository(projects / "beta repo", "beta")
    # The user's own edit before any conversation; Undo must keep it.
    users = "pub fn answer() -> u32 { 41 + 1 } // the user's edit\n"
    (alpha / "src" / "lib.rs").write_text(users, encoding="utf-8")
    plain = output / "plain folder"
    plain.mkdir()
    if any((folder / ".git").exists() for folder in (plain, *plain.parents)):
        raise RuntimeError("The evidence folder must be outside any Git repository")

    backend = types.SimpleNamespace(url=unreachable_url())
    previous = {key: os.environ.get(key) for key in ("BUDDY_COPILOT_FIXTURE",)}
    os.environ["BUDDY_COPILOT_FIXTURE"] = "1"
    driver = settings.Driver(executable, output, session, backend, CHECKOUT)
    sentinel = terminal.start_sentinel()
    primary = None
    try:
        chats = driver.drive("conversations-dark", [
            ("wait_text", "The Crew"), ("capture", "empty-dark"),
            ("input", "Project folder", str(plain)), ("click", "Add project"),
            ("wait_text", "This folder is not inside a Git repository."), ("capture", "not-a-repository-dark"),
            ("input", "Project folder", str(alpha)), ("click", "Add project"),
            ("wait_text", "Start a conversation"), ("capture", "start-dark"),
            ("click", "Start conversation"), ("wait_text", "No changes since this conversation started."),
            ("input", "Message", f"{STREAM} What does alpha contain?"), ("click", "Send message"),
            ("wait_text", "Offline stream"), ("sleep", 3), ("capture", "alpha-reply-dark"),
            ("input", "Project folder", str(beta)), ("click", "Add project"),
            ("wait_text", "Start a conversation"), ("click", "Start conversation"),
            ("wait_text", "No changes since this conversation started."),
            # Enter sends from the keyboard. Captures wait for the reply to finish streaming.
            # OCR misreads short paths ("src/lib.rs") and red button text, so checks use the
            # words it reads reliably; disabled buttons are too faint to require.
            ("input", "Message", f"{STREAM} And beta?"), ("key", "Return"),
            ("wait_text", "Offline stream"), ("sleep", 3), ("capture", "beta-reply-dark"),
        ], record=True)
        # Add project is disabled (faint) until a folder is entered; Browse is always available.
        github.check(chats / "empty-dark.png", "The Crew", "Project folder", "Browse", "No projects yet")
        github.check(chats / "not-a-repository-dark.png", "This folder is not inside a Git repository.")
        github.check(chats / "start-dark.png", "Start a conversation", ACCOUNT, "Model", "Start conversation")
        github.check(chats / "alpha-reply-dark.png", "alpha", "fixture-org/alpha", "Synthetic assistant",
            "Offline stream", "Changed files", "Message", "Clear conversation")
        github.check(chats / "beta-reply-dark.png", "beta", "fixture-org/beta", "Synthetic assistant", "2 messages")
        verify_conversations(state(output), {
            "fixture-org/alpha": [("user", False), ("assistant", False)],
            "fixture-org/beta": [("user", False), ("assistant", False)],
        })

        # Files change outside Buddy between launches, as an editor or agent would change them.
        # Opening the project reads its changed files.
        readme = alpha / "README.md"
        library = alpha / "src" / "lib.rs"
        agent_readme = "# alpha\n\nEdited by an agent.\n"
        readme.write_text(agent_readme, encoding="utf-8")
        kept = driver.drive("keep-dark", [
            ("click", "alpha"), ("wait_text", "README.md"), ("capture", "one-change-dark"),
            ("click", "Keep"), ("wait_text", "Kept the changes to README.md."), ("capture", "kept-dark"),
        ], record=True)
        github.check(kept / "one-change-dark.png", "Changed files", "README.md", "Modified", "+2", "Keep", "Undo change")
        github.check(kept / "kept-dark.png", "Kept the changes to README.md.",
            "No changes since this conversation started.")
        if readme.read_text(encoding="utf-8") != agent_readme:
            raise RuntimeError("Keep changed the file")

        library.write_text("pub fn answer() -> u32 { 0 } // an agent's edit\n", encoding="utf-8")
        undone = driver.drive("undo-dark", [
            ("click", "alpha"), ("wait_text", "Modified"),
            ("click", "Undo change"), ("wait_text", "Confirm undo"), ("capture", "undo-confirm-dark"),
            ("click", "Confirm undo"), ("wait_text", "Undid the changes to"), ("capture", "undone-dark"),
        ], record=True)
        github.check(undone / "undo-confirm-dark.png", "lib.rs", "Modified", "Keep")
        github.check(undone / "undone-dark.png", "Undid the changes to",
            "No changes since this conversation started.")
        if library.read_text(encoding="utf-8") != users:
            raise RuntimeError("Undo did not restore the user's earlier edit")

        # A link that leads out of the project is listed but cannot be undone through.
        outside = output / "outside the project"
        outside.mkdir()
        (outside / "keep.txt").write_text("outside\n", encoding="utf-8")
        os.symlink(outside, alpha / "outside-link", target_is_directory=True)
        readme.write_text("# alpha\n\nA second agent edit.\n", encoding="utf-8")
        linked = driver.drive("outside-dark", [
            ("click", "alpha"), ("wait_text", "Undo is unavailable"), ("capture", "outside-dark"),
        ], record=True)
        github.check(linked / "outside-dark.png", "README.md", "outside-link", "Added",
            "Undo is unavailable for links, folders and submodules.")
        if (outside / "keep.txt").read_text(encoding="utf-8") != "outside\n":
            raise RuntimeError("A file outside the project changed")
        # A directory link is removed as a directory entry on Windows.
        if os.name == "nt":
            os.rmdir(alpha / "outside-link")
        else:
            (alpha / "outside-link").unlink()

        light = json.loads(fixture(config.parent / "light-template", "light").read_text())
        terminal.write_config(config, lambda value: value["ui"].update(light["ui"]))
        errors = driver.drive("errors-light", [
            ("click", "alpha"), ("wait_text", "Clear conversation"),
            ("click", "Clear conversation"), ("wait_text", "Confirm clear"), ("capture", "clear-confirm-light"),
            ("click", "Confirm clear"), ("wait_text", "Conversation cleared."), ("capture", "cleared-light"),
            ("click", "Start conversation"), ("wait_text", "No changes since this conversation started."),
            # The offline provider refuses an unmarked prompt: a service error.
            ("input", "Message", "A question the provider refuses"), ("click", "Send message"),
            ("wait_text", "Last reply failed"), ("capture", "service-error-light"),
            ("click", "beta"), ("wait_text", "And beta?"),
            ("input", "Message", f"{CANCEL} Keep answering"), ("click", "Send message"),
            ("wait_text", "Partial synthetic answer"), ("capture", "replying-light"),
            # The drive ends by quitting Buddy while this reply is still streaming.
        ], record=True)
        github.check(errors / "clear-confirm-light.png", "Confirm clear")
        github.check(errors / "cleared-light.png", "Conversation cleared.", "Start a conversation")
        github.check(errors / "service-error-light.png", "Copilot, failed", "Last reply failed")
        github.check(errors / "replying-light.png", "Copilot is replying", "Partial synthetic answer", "Stop reply")

        reopened = driver.drive("restart-light", [
            ("wait_text", "The Crew"), ("click", "beta"),
            ("wait_text", "Buddy closed before this reply finished."), ("capture", "interrupted-light"),
            ("click", "alpha"), ("wait_text", "A question the provider refuses"), ("capture", "restored-light"),
            ("resize", 900, 650), ("sleep", 1), ("capture", "compact-light"),
        ], record=True)
        github.check(reopened / "interrupted-light.png", "Buddy closed before this reply finished.",
            "Synthetic assistant", "Last reply failed")
        github.check(reopened / "restored-light.png", "A question the provider refuses", "Copilot, failed")
        github.check(reopened / "compact-light.png", "Changed files", "Message", "Clear conversation")

        verify_conversations(state(output), {
            "fixture-org/alpha": [("user", False), ("assistant", True)],
            "fixture-org/beta": [("user", False), ("assistant", False), ("user", False), ("assistant", True)],
        })
        totals = provider_report(output)
        # Three answered prompts; the refused one is retried by the SDK, so it counts at least once.
        if totals["requests"] != 3 or totals["rejected"] < 1:
            raise RuntimeError(f"Unexpected provider traffic: {totals}")
        if git(beta, "status", "--porcelain"):
            raise RuntimeError("The Crew changed a repository it was only asked about")
        terminal.verify_sentinel(sentinel, "the full Crew exercise")
    except BaseException as failure:
        primary = failure
        raise
    finally:
        for key, value in previous.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        terminal.stop_sentinel(sentinel, primary)
    (output / "qualification.json").write_text(json.dumps(dict(result="passed", version=version, session=session,
        **binary, largeWindow=driver.large_size, desktopUi=True,
        dataSource="Installed app with local GitHub-origin clones and the offline Copilot provider fixture",
        checks=["folder validation", "two projects with separate conversations", "Enter sends",
            "keep a change", "undo restores the session's start, keeping an earlier user edit",
            "a link out of the project cannot be undone through", "clear conversation",
            "service error persisted", "reply interrupted by quitting is marked after restart",
            "state survives restart", "compact window", "light/dark"]), indent=2), encoding="utf-8")


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
    qualify(executable, output, args.session, args.version)


if __name__ == "__main__":
    main()
