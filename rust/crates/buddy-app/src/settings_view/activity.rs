//! Explicit Settings category opens. Retained destinations never use this path.
use std::rc::Rc;

use gpui_kit::App;

use crate::runtime::Runtime;

pub(super) type OnOpen = Rc<dyn Fn(&App)>;

pub(super) fn record(cx: &App) {
    if !cx.has_global::<Runtime>() {
        return;
    }
    let url = buddy_core::convex_data::convex_url();
    Runtime::global(cx).spawn_detached(async move {
        if buddy_core::settings_activity::record(&url).await.is_err() {
            log::debug!("Settings category activity was not confirmed; it will not be replayed");
        }
    });
}
