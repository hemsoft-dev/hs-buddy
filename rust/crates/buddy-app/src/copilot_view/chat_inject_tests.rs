use super::super::*;
use super::AssistantPrompt;
use buddy_core::copilot_runs::Failure;
use std::cell::RefCell;
use std::rc::Rc;

const PROMPT: &str = "Address the unresolved review comments on PR #7 in team/private.";

/// The sidebar assistant, never drawn, as a closed assistant is.
fn assistant(cx: &mut gpui_kit::TestAppContext) -> (gpui_kit::AnyWindowHandle, Entity<ChatView>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let mut config = buddy_core::config::AppConfig::default();
        config.copilot.gh_account = "synthetic-account".into();
        config.copilot.model = "gpt-4.1".into();
        crate::theme::install(&config, cx);
        cx.set_global(Settings::new(config));
        cx.set_global(Runtime::for_test(cx.background_executor().clone()));
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let mut view = ChatView::new(window, cx);
                view.set_sidebar_context(
                    crate::assistant_context::AssistantContext::section(None),
                    cx,
                );
                view
            })
        })
        .unwrap()
    })
}

fn injected(model: Option<&str>) -> AssistantPrompt {
    AssistantPrompt {
        prompt: PROMPT.into(),
        model: model.map(Into::into),
        context: Some("THREADCONTEXT src/lib.rs: please rename".into()),
        context_label: Some("1 unresolved review thread".into()),
    }
}

/// A real handle whose driver never runs, so the turn stays busy. Nothing
/// is started or written.
fn idle_run(
    temp: &tempfile::TempDir,
    options: &RunOptions,
) -> Result<(RunHandle, std::future::Ready<()>), Failure> {
    copilot_assistant::start(RunOptions {
        context: options.context.clone(),
        prompt: options.prompt.clone(),
        saved: false,
        review: None,
        backend: None,
        executable: temp.path().join("never-start.exe"),
        owned_directory: temp.path().join("must-remain-absent"),
    })
    .map(|(run, _driver)| (run, std::future::ready(())))
}

type Seen = Rc<RefCell<Vec<RunOptions>>>;

fn inject(
    cx: &mut gpui_kit::TestAppContext,
    handle: gpui_kit::AnyWindowHandle,
    view: &Entity<ChatView>,
    prompt: AssistantPrompt,
    temp: &tempfile::TempDir,
) -> Seen {
    let seen: Seen = Rc::default();
    let record = seen.clone();
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |view, cx| {
            view.inject_with(prompt, window, cx, |options, _| {
                let started = idle_run(temp, &options);
                record.borrow_mut().push(options);
                started
            })
        })
    })
    .unwrap();
    cx.run_until_parked();
    seen
}

#[gpui_kit::test]
fn an_injected_prompt_starts_a_turn_with_its_model_and_context(cx: &mut gpui_kit::TestAppContext) {
    let (handle, view) = assistant(cx);
    let temp = tempfile::tempdir().unwrap();
    let seen = inject(cx, handle, &view, injected(Some("claude-opus-4.6")), &temp);
    let seen = seen.borrow();
    assert_eq!(seen.len(), 1, "one turn starts at once, as in Electron");
    let options = &seen[0];
    assert!(!options.saved, "Electron's assistant turn is not saved");
    assert_eq!(options.context.model, "claude-opus-4.6");
    // The closed assistant starts from Settings' account, as when opened.
    assert_eq!(options.context.account, "synthetic-account");
    assert!(
        options.prompt.ends_with(&format!(
            "User: {PROMPT}\n\nTHREADCONTEXT src/lib.rs: please rename"
        )),
        "{}",
        options.prompt
    );
    cx.update(|cx| {
        let view = view.read(cx);
        assert!(view.run.is_some() && view.injected.is_none());
        assert_eq!(view.messages.len(), 1);
        let message = &view.messages[0];
        assert!(message.user);
        assert_eq!(message.text, PROMPT, "the context is not shown");
        assert_eq!(
            message.note.as_deref(),
            Some("Model: claude-opus-4.6 · with 1 unresolved review thread as context")
        );
        // The model applies to this turn only, as Electron's per-message model.
        assert_eq!(view.model.read(cx).value(), "gpt-4.1");
        assert_eq!(view.prompt.read(cx).value(), "");
        // A later turn still reads the threads the first one carried.
        assert!(
            view.conversation_prompt("And the next one?")
                .contains("THREADCONTEXT src/lib.rs")
        );
    });
}

#[gpui_kit::test]
fn a_busy_assistant_keeps_the_injected_prompt_ready_for_its_next_turn(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (handle, view) = assistant(cx);
    let temp = tempfile::tempdir().unwrap();
    let first = inject(cx, handle, &view, injected(None), &temp);
    assert_eq!(first.borrow().len(), 1);
    // The first turn is still responding: Electron drops a prompt then;
    // native keeps it in the box with its model and context.
    let second = inject(cx, handle, &view, injected(Some("claude-opus-4.6")), &temp);
    assert!(second.borrow().is_empty(), "no second turn while one runs");
    cx.update(|cx| {
        let view = view.read(cx);
        assert_eq!(view.prompt.read(cx).value(), PROMPT);
        assert_eq!(view.messages.len(), 1);
        assert!(view.notice.as_deref().unwrap().contains("still responding"));
        assert_eq!(
            view.injected.as_ref().unwrap().note(),
            "Model: claude-opus-4.6 · with 1 unresolved review thread as context"
        );
    });
    // Clear forgets it, with the conversation.
    cx.update(|cx| view.update(cx, |view, cx| view.new_conversation(cx)));
    cx.update(|cx| assert!(view.read(cx).injected.is_none()));
}

#[gpui_kit::test]
fn without_a_model_or_one_the_account_is_not_offered_the_model_field_is_used(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (handle, view) = assistant(cx);
    let temp = tempfile::tempdir().unwrap();
    let seen = inject(cx, handle, &view, injected(Some("  ")), &temp);
    assert_eq!(seen.borrow()[0].context.model, "gpt-4.1");
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.new_conversation(cx);
            view.run = None;
            view.models = vec![buddy_core::copilot_models::Model {
                id: "gpt-4.1".into(),
                name: "GPT-4.1".into(),
            }];
            view.models_account = "synthetic-account".into();
        })
    });
    let seen = inject(cx, handle, &view, injected(Some("claude-opus-4.6")), &temp);
    assert_eq!(seen.borrow()[0].context.model, "gpt-4.1");
    cx.update(|cx| {
        let view = view.read(cx);
        assert_eq!(
            view.notice.as_deref(),
            Some(
                "claude-opus-4.6 is not offered to synthetic-account, so this prompt uses the Model field's model."
            )
        );
        assert_eq!(
            view.messages[0].note.as_deref(),
            Some("Model: the Model field's model · with 1 unresolved review thread as context")
        );
    });
}

#[gpui_kit::test]
fn a_message_being_written_stays_in_the_box_after_the_injected_turn_starts(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (handle, view) = assistant(cx);
    let prompt = cx.update(|cx| view.read(cx).prompt.clone());
    cx.update_window(handle, |_, window, cx| {
        prompt.update(cx, |state, cx| state.set_value("Half-written", window, cx))
    })
    .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let seen = inject(cx, handle, &view, injected(None), &temp);
    assert!(seen.borrow()[0].prompt.ends_with(&format!(
        "User: {PROMPT}\n\nTHREADCONTEXT src/lib.rs: please rename"
    )));
    cx.update(|cx| assert_eq!(prompt.read(cx).value(), "Half-written"));
}

#[gpui_kit::test]
fn a_turn_that_cannot_start_leaves_the_prompt_and_its_model_ready(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (handle, view) = assistant(cx);
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |view, cx| {
            view.inject_with(
                injected(Some("claude-opus-4.6")),
                window,
                cx,
                |_, _| -> Result<(RunHandle, std::future::Ready<()>), Failure> {
                    Err(Failure::InvalidContext)
                },
            )
        })
    })
    .unwrap();
    cx.update(|cx| {
        let view = view.read(cx);
        assert_eq!(
            view.error.as_deref(),
            Some(Failure::InvalidContext.to_string().as_str())
        );
        assert_eq!(view.prompt.read(cx).value(), PROMPT);
        assert!(view.messages.is_empty());
        assert_eq!(
            view.injected
                .as_ref()
                .and_then(|pending| pending.model.as_deref()),
            Some("claude-opus-4.6")
        );
        assert!(view.notice.is_none());
    });
}
