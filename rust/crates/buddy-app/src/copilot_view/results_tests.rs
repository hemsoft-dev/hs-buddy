use super::*;

fn setup(cx: &mut gpui_kit::TestAppContext) -> Entity<ResultsView> {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        cx.set_global(Runtime::for_test(cx.background_executor().clone()));
        let (_, view) = gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| ResultsView::new(window, cx))
        })
        .unwrap();
        view
    })
}
fn dropped() -> futures::channel::oneshot::Receiver<()> {
    let (sender, receiver) = futures::channel::oneshot::channel();
    drop(sender);
    receiver
}

#[gpui_kit::test]
fn a_panicked_worker_channel_unlocks_each_operation_and_preserves_unconfirmed_deletion(
    cx: &mut gpui_kit::TestAppContext,
) {
    let view = setup(cx);
    for kind in 0..3 {
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.list_loading = kind == 0;
                view.detail_loading = kind == 1;
                view.write_loading = kind == 2;
                view.list_error = None;
                view.detail_error = None;
                view.selected_id = Some("owned-result".into());
                let loading = match kind {
                    0 => Loading::List(view.list_ticket),
                    1 => Loading::Detail(view.detail_ticket),
                    _ => Loading::Delete,
                };
                view.receive(cx, loading, dropped(), |_, _, _| {
                    panic!("A dropped worker cannot complete successfully")
                });
            })
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let view = view.read(cx);
            assert!(!view.busy());
            assert!(view.selected_id.as_deref() == Some("owned-result"));
            if kind == 0 {
                assert!(view.list_error.is_some());
            } else {
                assert!(view.detail_error.is_some());
            }
            if kind == 2 {
                assert!(view.detail_error.as_ref().unwrap().contains("unconfirmed"));
                assert!(!view.delete_confirm);
            }
        });
    }
}

#[gpui_kit::test]
fn an_old_dropped_worker_cannot_unlock_a_new_request(cx: &mut gpui_kit::TestAppContext) {
    let view = setup(cx);
    for detail in [false, true] {
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.list_loading = !detail;
                view.detail_loading = detail;
                view.list_error = None;
                view.detail_error = None;
                let old = uuid::Uuid::new_v4();
                let loading = if detail {
                    Loading::Detail(old)
                } else {
                    Loading::List(old)
                };
                view.receive(cx, loading, dropped(), |_, _, _| {
                    panic!("A dropped worker cannot complete successfully")
                });
            })
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let view = view.read(cx);
            assert!(view.busy() && view.list_error.is_none() && view.detail_error.is_none());
        });
    }
}
