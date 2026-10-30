# Copilot Assistant parity slice (#823)

The native title-bar Assistant button and Ctrl+Shift+A open a conversation
beside the current workspace. The shared `ui.assistantOpen` setting restores
visibility after restart. Drag the divider to resize the panel; its width uses
the shared third `ui.paneSizes` entry and is restored after restart. The conversation is separate from Copilot's prompt
workspace and saved results; sending from the side panel does not save a result
to Convex.

The panel uses `copilot_view::chat::ChatView` and the existing assistant runtime
for account/model selection, streamed Markdown, cancellation and errors. Clear
cancels the current turn and removes local conversation history. Hiding the
panel retains its conversation for the current app session. No assistant runs
until the user sends a message or selects a suggested prompt.

`assistant_context` supplies the same welcome, PR and repository suggestions as
Electron. GitHub routes supply repository, PR number, selected detail tab,
issue or commit context. Other sections supply their section name. The currently loaded GitHub page text is included, bounded to 32,000
characters. The existing native runtime tool permissions are retained. Context is
read again when sending, so a just-completed navigation cannot send the previous
route's context. The panel does not fetch hidden GitHub data to build context.

Validation from `rust/`:

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo build --release -p buddy-app`

Windows manual comparison: use isolated `BUDDY_CONFIG_PATH` and backend settings;
open the assistant from Dashboard and a GitHub PR, send a harmless question,
check Markdown output, stop a response, clear, switch sections, hide/reopen and
restart. Check Enter sends and Shift+Enter inserts a newline. Compare Electron's
Assistant panel using the same route and window size. Attach current-head
screenshots to the PR. This slice does not qualify the native app as the default.
