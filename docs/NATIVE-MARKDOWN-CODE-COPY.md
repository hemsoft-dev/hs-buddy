# Native Markdown code copying

Native assistant, chat and saved-result Markdown offers Copy code for each code
block. Click the button or reach it with Tab and activate it with Enter or Space.
Copying a block does not run it. Inline code keeps ordinary text selection.
Empty blocks cannot replace the clipboard.

The copied text excludes Markdown fences, language metadata and surrounding
prose. It preserves Unicode, indentation and internal blank lines, then adds
the same final newline as Electron's Markdown renderer. A streamed or replaced
answer supplies the current block text. Separate responses keep their own
controls. Language labels remain alongside the buttons, with reserved space
above the first code line.

This uses the existing gpui-kit 0.8.1 code-action hook. Parsing, highlighting,
selection, validated web links, and the suppression of raw HTML and remote media
stay with the existing Markdown helper. It changes no saved answer, publication
body, assistant execution, account, backend or configuration.

## Validation

From `rust/`:

```sh
cargo test --locked -p buddy-app copilot_view::markdown::
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
```

The native tests use real GPUI controls with an in-memory test clipboard and no
Runtime, service connection or host UI. They cover distinct blocks and responses,
pointer and keyboard activation, Unicode and whitespace, empty/inline code and
updated streaming content. The original source fails the missing-control case.
The same synthetic document passed through the installed Electron Markdown
pipeline establishes the code-only text and trailing newline contract.

Local tests are source evidence. The hosted native qualification also runs the
installed app. Its `code-copy-dark` scene at 1200x820 and its compact
`code-copy-light` scene at 900x650 send an offline prompt whose synthetic answer
ends with one fenced Rust block. Each scene captures the language label, the
Copy code button and the first code line together, then activates the button.
The dark scene is recorded. Electron stays primary; broader parity, Windows 11,
profile migration and cutover remain open under
[issue 823](https://github.com/hemsoft-dev/hs-buddy/issues/823).
