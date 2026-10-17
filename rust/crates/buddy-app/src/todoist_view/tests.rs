use super::*;

fn setup(cx: &mut gpui_kit::TestAppContext) -> Entity<TodoistView> {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        // No Runtime: these tests must never read the host vault or service.
        let (_, view) = gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let view = TodoistView::new(window, cx);
                view.token.update(cx, |token, cx| {
                    token.set_value("synthetic-unsaved-token", window, cx)
                });
                view
            })
        })
        .unwrap();
        view
    })
}

fn dropped<T>() -> futures::channel::oneshot::Receiver<T> {
    let (sender, receiver) = futures::channel::oneshot::channel();
    drop(sender);
    receiver
}

#[gpui_kit::test]
fn stopped_worker_requires_recovery_and_cannot_leave_mutations_enabled(
    cx: &mut gpui_kit::TestAppContext,
) {
    let view = setup(cx);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.busy = true;
            view.connection_form = false;
            view.receive_operation(view.generation, true, dropped(), cx);
        })
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let view = view.read(cx);
        assert!(!view.busy);
        assert!(!view.can_write());
        assert!(view.connection_form);
        assert!(view.data.is_none());
        assert!(view.error.as_ref().unwrap().contains("unconfirmed"));
    });
}

#[gpui_kit::test]
fn stale_connection_and_operation_channels_cannot_finish_a_new_request(
    cx: &mut gpui_kit::TestAppContext,
) {
    let view = setup(cx);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.busy = true;
            view.notice = Some("new request".into());
            let old = uuid::Uuid::new_v4();
            view.receive_connection(old, dropped(), cx);
            view.receive_operation(old, false, dropped(), cx);
        })
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let view = view.read(cx);
        assert!(view.busy);
        assert!(view.error.is_none());
        assert_eq!(view.notice.as_deref(), Some("new request"));
    });
}

#[gpui_kit::test]
fn failed_connection_preserves_editable_token_and_reports_a_stopped_check(
    cx: &mut gpui_kit::TestAppContext,
) {
    let view = setup(cx);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.busy = true;
            view.receive_connection(view.generation, dropped(), cx);
        })
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let view = view.read(cx);
        assert!(!view.busy && view.connection_form && !view.clear_token);
        assert!(view.error.as_ref().unwrap().contains("secure storage"));
        assert_eq!(view.token.read(cx).value(), "synthetic-unsaved-token");
        assert!(!view.can_write());
    });
}
