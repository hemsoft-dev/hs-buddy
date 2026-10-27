# Native Crew

The Crew in Buddy Native keeps a list of project folders, one Copilot conversation per
project, and the files that changed in a project since its conversation began.

## Projects

Enter a folder under Project folder and choose Add project, or choose Browse… to pick
it. The folder must be inside a Git work tree whose `origin` is a GitHub repository:
`https://github.com/owner/repo`, `git@github.com:owner/repo.git`, or an SSH alias
that `ssh -G` resolves to `github.com`, as in Electron Buddy. A folder outside Git, a
repository without `origin`, or another host is refused with the reason. A project
records the repository root (not the folder you chose), its `owner/repo`, and the
default branch from `refs/remotes/origin/HEAD`, else the checked-out branch.
Adding a folder whose root is already listed opens that project. Paths with spaces
and non-ASCII characters work on every platform.

Remove asks once more (Confirm remove, or Cancel), then forgets the project, its conversation
and its snapshot. The folder and its files are never touched. Remove is unavailable
while a reply is in progress in any Buddy window: it holds the conversation's lock while
it works, so no reply can start meanwhile.

## Conversations

A project starts without a conversation. Start conversation asks for the GitHub
account and the model it uses. The account configured in Settings for the
repository's owner is chosen first, else `gh`'s active account when Settings lists
it, else the first configured account; until you pick one, the choice follows a later
change of `gh`'s active account. The model starts as the Copilot model in Settings,
also after a conversation is cleared. A conversation keeps its account and model; to change either, clear it and
start again.

Replies come from the same assistant as Copilot Chat: the bundled Copilot runtime,
signed in as the conversation's account through `gh`, with no tools. It cannot edit
files, run commands or post to GitHub. Each message sends Electron's context line,
`Project: owner/repo at <path>`, followed by the conversation as stored when the reply
starts (including turns another window added), so one project's conversation never
reaches another's. Enter sends; Shift+Enter adds a
line. Stop reply ends the reply and keeps what arrived as a partial answer. An unsent
message stays with its project when you switch to another one.

The question is saved before the reply starts, and the reply is saved only to the
conversation that asked for it, once. While a reply is in progress, Send is
unavailable for that project, in every Buddy window: the replying process holds a
lock on the conversation, and the other windows show "Replying in another window" with
Send, Clear and Remove unavailable. Another project can be asked at the same time. A failed
reply is saved with its reason and the project is listed as "Last reply failed". If an
answer cannot be saved (another window keeps the state busy), it stays on screen and
Buddy saves it again every two seconds; Send stays unavailable until it is saved.
If Buddy closes or crashes before a reply finishes, the next start records
"Buddy closed before this reply finished." in that conversation.

Clear conversation asks once more (Confirm clear, or Cancel), then deletes the conversation and
its snapshot. It is unavailable while a reply is in progress, and holds the
conversation's lock while it works. If a snapshot folder cannot be deleted at once
(Windows can hold a file briefly), Buddy says so and deletes it at its next start.

## Changed files

Starting a conversation takes a snapshot of the project: the `HEAD` commit, and a
copy of every file Git reports as changed or untracked at that moment. So that an amend,
rebase or `git gc` cannot remove that commit, Buddy points a ref at it in the project's
repository, `refs/buddy/crew/<conversation>`. It is the only thing the Crew writes to a
repository on its own, and Clear and Remove delete it. Changed
files then lists each file whose content, executable bit (on macOS and Linux) or link
target differs from the snapshot, whoever changed
it: an editor, a terminal, an agent loop. Edits made before the conversation started
are the starting point, not changes. Commits made during the conversation still
count, because the comparison is with the snapshot's commit, read with that commit's
own attributes, so a later `.gitattributes` edit does not change it (Git 2.40 or later;
older Git uses the working tree's attributes). Ignored files and
submodules are not followed. Text files show added and removed lines (a very large
rewrite, or a file of more than 100,000 lines, shows none); binary files show no counts.
Git's output for a snapshot file is read up to 8 MiB; a filter (such as Git LFS) that
expands it further is compared by digest.

The list is read when a project is opened, after each reply, after Keep or Undo, and
on Refresh.

- **Keep** accepts the file as it is now. It leaves the list until it changes again.
  Keeps from two windows in the same conversation are both recorded.
- **Undo change** asks once more (Confirm undo, or Cancel), then writes the file back to the snapshot:
  the commit's version for a file that was clean, the copied version for a file that
  was already changed (so your earlier edits survive), or no file for one that was
  added. A file deleted since the snapshot is recreated with its executable bit.

Both act only on the file as the list showed it (its content, and on macOS and Linux its
executable bit). If the file changed after the list
was read, nothing is written and the row reports it; Refresh and look again. Undo
prepares the restored file first and checks the file once more just before replacing
or deleting it, which leaves only the moment of the replacement itself for an editor to
write in. Undo
writes through a handle on the file's own folder, opened one folder at a time without
following links, so it cannot leave the project or reach `.git`: paths with `..`,
absolute paths, any `.git` part, and any link on the way (even one that stays inside the
project) are refused. Links,
folders and submodules are listed without Undo. Files larger than 8 MiB, or beyond
64 MiB of copies per conversation, are compared by digest and cannot be undone.
A conversation can start with at most 5,000 changed or untracked files.

## Storage

State lives in `native-crew` next to the settings file:

- `state.json`: projects and conversations, in Electron's field names
  (`displayName`, `githubSlug`, `conversationHistory`, …). Every change is read,
  applied and written back while holding a lock beside the file (the same kind of lock
  the settings file uses), then replaced atomically, so two Buddy windows never lose
  each other's writes. A damaged file is reported, never overwritten.
- `snapshots/<conversation>`: the snapshot's commit and copies.
- `turns/<conversation>.lock`: held by the process whose reply is in flight.

On macOS and Linux these folders are readable only by you. Electron Buddy keeps its
own Crew data; the two apps do not share projects or conversations.

## Qualification

`scripts/native-smoke/crew-desktop.py` drives the installed app on Windows, macOS
(Apple silicon and Intel) and Ubuntu runners. Replies come from Copilot Chat's
offline provider fixture, so the real runtime runs and nothing reaches a model. It
adds a plain folder (refused) and two GitHub-origin clones, one with a Unicode
path; holds a conversation in each (one sent with Enter); keeps one change; undoes
another back to an edit made before the conversation; shows that a link out of the
project cannot be undone; clears a conversation; records a service error; quits
Buddy mid-reply; and checks after restart that state persisted and the reply was
marked interrupted. It checks `state.json` for each project's messages, account and
model, the provider fixture's request counts, and that the other repository was not
changed. Captures cover the dark and light themes at 1200 × 820 (1044 × 788 on the
Windows runner, its largest window) and 900 × 650.
