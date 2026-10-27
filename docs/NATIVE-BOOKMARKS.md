# Native Bookmarks

Bookmarks in Buddy Native shows the web bookmarks Electron Buddy keeps in Convex, in a
category tree and a searchable list, and adds, edits, reorders and deletes them. Both
apps read and write the same `bookmarks` table through the same Convex functions, so a
change in one shows up in the other. Embedded browsing (tabs, navigation, sessions) is
a separate part of issue #822 and is not ported yet.

## Data

The section uses only the existing functions in `convex/bookmarks.ts`: `list`,
`listCategories`, `create`, `update`, `recordVisit`, `remove` and `reorder`. The
client is `buddy_core::bookmarks::client`. Every call has a 12 second timeout, and
every result is checked before it is used: a record with a missing or mistyped field,
an empty or repeated id, or a mutation that answers with the wrong id makes the whole
result unusable instead of being half-shown. Convex error text is never shown, since
it can echo the URL; the section says what kind of failure it was (unreachable,
duplicate URL in the category, bookmark gone, rejected, result unknown).

The deployment is the one the rest of Buddy Native uses: `BUDDY_CONVEX_URL`, then
`VITE_CONVEX_URL`, then the checkout's `.env.local` or `.env`, then the local
backend. Bookmarks are read when the section opens, after every change, on Refresh,
and every 30 seconds while the section is shown (not while the editor or a delete
confirmation is open).

If the first read fails, or has not answered after 10 seconds, the section shows
"Unable to load bookmarks" with the reason and Retry, as Electron does. A read that
fails after bookmarks were shown keeps them on screen and says the refresh failed.

## List and tree

The tree on the left lists every category with the number of bookmarks in it and its
subcategories. Categories nest with `/`, as `Development/Frontend`. Choosing a
category expands it and shows its bookmarks, including those in its subcategories;
the chevron expands or collapses it without choosing it, and All bookmarks shows
everything. An expanded category lists its subcategories and then its own bookmarks.

The list on the right has Search, which matches title, URL, description and tags and
ignores case, and a row of tag chips that limit the list to one tag. Clear filters
resets category, tag and search. Bookmarks are sorted by their saved order, then by
age. Each card shows the title, URL, description, the category's last part and the
tags, with Open externally, Edit and Delete. An empty list says "No bookmarks yet";
a list emptied by filters says "No bookmarks match your filters".

## Opening

Clicking a card or a bookmark in the tree, Open, and Open externally all hand the URL
to the system browser and record the visit (`recordVisit`). Before that, the URL must
pass `buddy_core::bookmarks::url::validate_url`, the native copy of Electron's
`validateUrl`: `http` or `https` only, and no internal host (`localhost`, loopback,
`10.`, `172.16.` to `172.31.`, `192.168.`, `169.254.`, `.local`, `.internal`). A host
written as an IP address is also checked by range, which refuses `0.0.0.0`, private
IPv6 addresses and IPv4 addresses mapped into IPv6; Electron checks only the text.
A refused bookmark is not opened and the section says why. The embedded browser is
meant to reuse the same check.

Favicons are not loaded: showing them would fetch a remote image for every bookmark.
Each bookmark shows a globe icon instead.

## Adding and editing

Add opens the editor: URL, Title, Category, Tags and Description. Edit opens it with
the bookmark's values. Save checks the fields in Electron's order and with its
messages: URL, title and category are required, the URL must pass the check above,
and at most 50 tags are allowed. A category may not have an empty part (`Dev/` or
`Dev//UI`), which Electron accepts. Existing categories are listed under the field;
choosing one fills it in, and typing `/Name` after it makes a new subcategory. Enter
in a field saves. A duplicate URL in the same category, or any other refusal, keeps
the editor open with the reason.

Clearing the description or all tags while editing clears them. Electron leaves the
old values in that case, because it omits empty fields from the update.

## Pasting a URL

GPUI delivers only files when something is dropped on a window, never a URL dragged
from a browser, so Electron's drop target cannot be ported. Instead, copy a link and
press Ctrl+V (Cmd+V on macOS) on the list, choose Edit → Paste, or choose Paste link.
Inputs keep their own paste. The clipboard's first `http` or `https` URL (a
`text/uri-list` comment line is skipped) opens the editor with that URL, and then,
as Electron does for a drop:

1. The page title is fetched, unless the URL fails the check. The fetch resolves the
   host itself and refuses it if any address is private or loopback, then connects
   only to those checked addresses through no proxy, so a name cannot pass the check
   and then reach an internal address. Each redirect (at most five) is checked the
   same way. Each request times out after 5 seconds, only HTML is read, and at most
   64 KiB of it. The title comes from `<title>`, else `og:title`, with HTML entities
   decoded.
2. Copilot is asked for a description and three to five tags, with Electron's prompt.
   The request runs as the Copilot account in Settings, else `gh`'s active account if
   Settings lists it, else the first configured account, with the Copilot model from
   Settings, through the same tool-less assistant as Copilot Chat. Without a
   configured account or model nothing is asked. A failed, unreadable or late reply
   (after 60 seconds the run is stopped) changes nothing.

A field you typed in is never replaced by the fetched title or the suggestion, even
if you cleared it again. If you change the URL before they arrive, neither is applied.

## Reordering

Drag a bookmark in the tree onto another bookmark of the same category to put it in
that bookmark's place. The section renumbers that category from 0 and sends the new
order with one `reorder` call, as Electron's sidebar does; the list moves at once and
is read again afterwards. Dropping on a bookmark of another category does nothing.
Right-click a bookmark in the tree for Open, Open externally, Edit, Move up, Move
down and Delete; Move up and Move down are the keyboard-friendly way to reorder.

## Deleting

Delete asks once more, under the bookmark's card: "Delete "title"? This bookmark will
be permanently removed." with Confirm delete and Cancel, like the app's other
destructive actions. A delete started from the tree shows its card even when filters
hide it. If the delete fails, the reason replaces the description and the question
stays open.

## Qualification

`rust/crates/buddy-app/src/bookmarks_view/tests.rs` drives the view headlessly with an
in-memory service: filters and search, the tree's counts and expansion, the `reorder`
updates a drop or move sends, the editor's checks, create and update, the delete
confirmation and its failure, a failed and a timed-out first load with Retry, the URL
check before opening, Ctrl+V with the fetched title and the suggestion (a typed
description wins), a skipped suggestion without an account, and the layout of the
compact window. `buddy_core::bookmarks` has unit tests for the URL rules, result
checking, error classification, title extraction and the guarded fetch against a
loopback server, the AI reply parsing and the tree and ordering rules.

`scripts/native-smoke/bookmarks-desktop.py` drives the installed app on Windows,
macOS (Apple silicon and Intel) and Ubuntu runners, against a disposable anonymous
Convex deployment seeded through `bookmarks:create`. In each theme it shows the
categorized list, chooses a category, adds a bookmark through the editor, finds one by
search, deletes it through the confirmation, and shows the list and editor in the
compact window. It then points the app at a closed loopback port for the failure state
and Retry. It checks the deployment afterwards for exactly the expected bookmarks and
the added bookmark's fields. Captures cover the dark and light themes at 1200 × 820
(1044 × 788 on the Windows runner, its largest window) and 900 × 650, with a recording
of each pass. Opening a browser, pasting and the AI suggestion are covered by the
headless tests only.
