//! `tabsOpened` for shell view opens that have no view of their own to count
//! them: Electron's `openTab` adds one for every call (`useAppSessionStats.ts`
//! `trackViewOpen`), including the Dashboard tab it opens at launch. Here that
//! is the Dashboard at window start and from the activity bar, the Bookmarks
//! activity-bar entry, the Insights sidebar's Productivity and Activity, and
//! a folder view opened from a terminal. Tab switches, Back and Forward, and
//! restored tabs are not opens.
//!
//! Without the global nothing is sent, so view tests never reach Convex.
use std::rc::Rc;

use gpui_kit::{App, Global};

use crate::runtime::Runtime;

pub(crate) struct ViewOpens(pub Rc<dyn Fn(&App)>);

impl Global for ViewOpens {}

/// The app's recorder: one content-free increment per open, never replayed.
pub(crate) fn install(cx: &mut App) {
    cx.set_global(ViewOpens(Rc::new(|cx| {
        let url = buddy_core::convex_data::convex_url();
        Runtime::global(cx).spawn_detached(async move {
            if buddy_core::stats::tab_activity::record(&url).await.is_err() {
                log::debug!("A view open was not confirmed; it will not be replayed");
            }
        });
    })));
}

pub(crate) fn record(cx: &App) {
    if let Some(ViewOpens(record)) = cx.try_global::<ViewOpens>() {
        record(cx);
    }
}
