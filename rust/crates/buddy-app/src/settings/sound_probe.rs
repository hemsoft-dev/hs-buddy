//! Read-only fixture probe: delegate the SAME child layout; add no hitbox.
use gpui_kit::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, MouseDownEvent, MouseUpEvent, Pixels, Window,
};
use std::io::Write;
use std::rc::Rc;
type Sink = Rc<dyn Fn(serde_json::Value)>;

pub(crate) fn observe(label: &'static str, child: impl IntoElement) -> AnyElement {
    let child = child.into_any_element();
    if super::sound_diagnostic::enabled() {
        Probe {
            label,
            child,
            sink: Rc::new(emit),
        }
        .into_any_element()
    } else {
        child
    }
}

struct Probe {
    label: &'static str,
    child: AnyElement,
    sink: Sink,
}
impl IntoElement for Probe {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for Probe {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let label = self.label;
        (self.sink)(serde_json::json!({"kind":"paint", "label":label, "bounds":rectangle(bounds)}));
        // Raw frame listeners deliberately do NOT use hitbox.is_hovered.
        // Register before painting children, so child capture cancellation
        // cannot erase this ancestor's initial observation.
        let sink = self.sink.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, _| {
            emit_event(
                &sink,
                label,
                "down",
                phase.capture(),
                bounds,
                event.position,
                window.mouse_position(),
            );
        });
        let sink = self.sink.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, _| {
            emit_event(
                &sink,
                label,
                "up",
                phase.capture(),
                bounds,
                event.position,
                window.mouse_position(),
            );
        });
        self.child.paint(window, cx);
    }
}
fn rectangle(bounds: Bounds<Pixels>) -> [f32; 4] {
    [
        bounds.origin.x.into(),
        bounds.origin.y.into(),
        bounds.size.width.into(),
        bounds.size.height.into(),
    ]
}
fn emit_event(
    sink: &Sink,
    label: &'static str,
    event: &'static str,
    capture: bool,
    bounds: Bounds<Pixels>,
    position: gpui_kit::Point<Pixels>,
    cursor: gpui_kit::Point<Pixels>,
) {
    sink(
        serde_json::json!({"kind":"event", "label":label, "event":event, "capture":capture,
        "bounds":rectangle(bounds), "eventPosition":[f32::from(position.x),f32::from(position.y)],
        "cursorPosition":[f32::from(cursor.x),f32::from(cursor.y)],
        "eventInside":bounds.contains(&position), "cursorInside":bounds.contains(&cursor)}),
    );
}
fn emit(mut value: serde_json::Value) {
    value["millis"] = serde_json::json!(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );
    let _ = writeln!(std::io::stderr().lock(), "buddy-sound-dispatch {value}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::component::button::Button;
    use gpui_kit::{
        AppContext as _, Global, InteractiveElement, Modifiers, ParentElement, Styled,
        VisualTestContext, div, point, px,
    };
    use std::cell::RefCell;
    #[derive(Default)]
    struct Counts {
        surface_down: usize,
        clicks: usize,
    }
    impl Global for Counts {}

    // Real GPUI paint/dispatch/hitbox seam, not a source-token oracle.
    // Executed by hosted native tests; no workstation input is posted.
    #[gpui_kit::test]
    fn raw_probe_sees_outside_click_and_preserves_button_callback(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let records = Rc::new(RefCell::new(Vec::new()));
        let sink: Sink = {
            let records = records.clone();
            Rc::new(move |v| records.borrow_mut().push(v))
        };
        let (window, _root) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
            cx.set_global(Counts::default());
            gpui_kit::open_window(Default::default(), cx, |_, cx| {
                cx.new(|_| TestView { sink: sink.clone() })
            })
            .unwrap()
        });
        let mut visual = VisualTestContext::from_window(window, cx);
        visual.run_until_parked();
        let before = visual.debug_bounds("probe-surface").unwrap();
        assert_eq!(before.size.width, px(100.));
        assert_eq!(before.size.height, px(60.));
        visual.simulate_click(
            before.origin + point(px(200.), px(150.)),
            Modifiers::default(),
        );
        visual.update(|_, cx| {
            assert_eq!(cx.global::<Counts>().surface_down, 0);
            assert_eq!(cx.global::<Counts>().clicks, 0);
        });
        assert!(records.borrow().iter().any(|v| v["label"] == "surface"
            && v["event"] == "down"
            && v["capture"] == true
            && v["eventInside"] == false));
        records.borrow_mut().clear();
        visual.simulate_click(
            before.origin + point(px(50.), px(20.)),
            Modifiers::default(),
        );
        visual.update(|_, cx| {
            assert_eq!(cx.global::<Counts>().surface_down, 1);
            assert_eq!(cx.global::<Counts>().clicks, 1);
        });
        assert!(records.borrow().iter().any(|v| v["label"] == "button"
            && v["event"] == "down"
            && v["capture"] == true
            && v["eventInside"] == true));
        assert_eq!(visual.debug_bounds("probe-surface").unwrap(), before);
    }
    struct TestView {
        sink: Sink,
    }
    impl gpui_kit::Render for TestView {
        fn render(&mut self, _: &mut Window, _: &mut gpui_kit::Context<Self>) -> impl IntoElement {
            let wrap = |label, child: AnyElement| Probe {
                label,
                child,
                sink: self.sink.clone(),
            };
            wrap(
                "root",
                div()
                    .size_full()
                    .child(wrap(
                        "surface",
                        div()
                            .id("probe-surface")
                            .debug_selector(|| "probe-surface".into())
                            .w(gpui_kit::px(100.))
                            .h(gpui_kit::px(60.))
                            .capture_any_mouse_down(|_, _, cx| {
                                cx.global_mut::<Counts>().surface_down += 1
                            })
                            .child(wrap(
                                "button",
                                Button::new("probe-button")
                                    .label("Synthetic sound")
                                    .w(gpui_kit::px(100.))
                                    .h(gpui_kit::px(40.))
                                    .on_click(|_, _, cx| cx.global_mut::<Counts>().clicks += 1)
                                    .into_any_element(),
                            ))
                            .into_any_element(),
                    ))
                    .into_any_element(),
            )
        }
    }
}
