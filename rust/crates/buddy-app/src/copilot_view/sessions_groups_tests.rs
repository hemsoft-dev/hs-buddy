use super::*;
use buddy_core::copilot_sessions::Limits;
use buddy_core::copilot_sessions::storage::{ScanLimits, SessionRoot};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext as _, VisualTestContext};
use std::sync::Arc;

/// A fixed viewing time: 15:00 on 2026-06-17 in New York.
fn now() -> jiff::Zoned {
    jiff::civil::date(2026, 6, 17)
        .at(15, 0, 0, 0)
        .in_tz("America/New_York")
        .unwrap()
}

fn fixture(
    rows: &[(&str, &str, u64)],
) -> (
    tempfile::TempDir,
    Arc<SessionRoot>,
    buddy_core::copilot_sessions::storage::ScanResult,
) {
    let temp = tempfile::tempdir().unwrap();
    for (workspace, id, _) in rows {
        let directory = temp
            .path()
            .join("workspaceStorage")
            .join(workspace)
            .join("chatSessions");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join(format!("{id}.jsonl")), serde_json::json!({"kind":0,"v":{"sessionId":id,"customTitle":id,"creationDate":123,"requests":[]}}).to_string()).unwrap();
    }
    let root = Arc::new(SessionRoot::open(temp.path()).unwrap());
    let mut scan = root.scan(ScanLimits::default()).unwrap();
    for session in &mut scan.sessions {
        session.modified_at = rows
            .iter()
            .find(|(_, id, _)| *id == session.session_id)
            .unwrap()
            .2;
    }
    scan.sessions
        .sort_by_key(|session| std::cmp::Reverse(session.modified_at));
    (temp, root, scan)
}

#[test]
fn workspace_identity_groups_interleaved_sessions_without_retargeting_their_files() {
    let (_temp, root, mut scan) =
        fixture(&[("a", "a-new", 30), ("b", "b-new", 20), ("a", "a-old", 10)]);
    for session in &mut scan.sessions {
        session.workspace_name = "Same project".into();
    }
    assert_eq!(
        scan.sessions
            .iter()
            .map(|session| session.session_id.as_str())
            .collect::<Vec<_>>(),
        ["a-new", "b-new", "a-old"]
    );
    let mut groups = Groups::default();
    groups.reconcile(&scan.sessions, false, &now());
    let ids: Vec<_> = groups
        .page(0, 20)
        .into_iter()
        .filter_map(|row| match row {
            Row::Session(i) => Some(
                root.detail(&scan.sessions[i].file, Limits::default())
                    .unwrap()
                    .session_id,
            ),
            _ => None,
        })
        .collect();
    assert_eq!(ids, ["a-new", "a-old", "b-new"]);
    assert_eq!(groups.groups.len(), 2);
    assert_ne!(groups.label(0), groups.label(1));
    groups.toggle("a");
    assert_eq!(
        groups.page(0, 20),
        [
            Row::Group(0),
            Row::Group(1),
            Row::Bucket(DateBucket::Older),
            Row::Session(1)
        ]
    );
    assert_eq!(
        root.detail(&scan.sessions[2].file, Limits::default())
            .unwrap()
            .session_id,
        "a-old"
    );
}

#[test]
fn continuation_pages_keep_the_group_control_and_every_original_row_exactly_once() {
    let rows: Vec<_> = (0..45)
        .map(|i| ("a", format!("session-{i}"), 100 - i))
        .collect();
    let borrowed: Vec<_> = rows
        .iter()
        .map(|(key, id, time)| (*key, id.as_str(), *time))
        .collect();
    let (_temp, _root, scan) = fixture(&borrowed);
    let mut groups = Groups::default();
    groups.reconcile(&scan.sessions, false, &now());
    let mut seen = Vec::new();
    for page in 0..3 {
        let rows = groups.page(page, 20);
        assert_eq!(rows[0], Row::Group(0));
        // A continuation page repeats its group control and date heading.
        assert_eq!(rows[1], Row::Bucket(DateBucket::Older));
        assert!(rows.len() <= 22);
        seen.extend(rows.into_iter().filter_map(|row| {
            if let Row::Session(i) = row {
                Some(i)
            } else {
                None
            }
        }));
    }
    assert_eq!(seen, (0..45).collect::<Vec<_>>());
    groups.toggle("a");
    assert_eq!(groups.page(0, 20), [Row::Group(0)]);
    assert!(groups.page(1, 20).is_empty());
}

#[test]
fn rescans_reconcile_only_current_source_identities_and_the_three_newest_defaults() {
    let (_temp, _root, mut scan) = fixture(&[
        ("a", "one", 40),
        ("b", "two", 30),
        ("c", "three", 20),
        ("d", "four", 10),
    ]);
    let mut groups = Groups::default();
    groups.reconcile(&scan.sessions, false, &now());
    assert!(!groups.is_expanded(3));
    groups.toggle("d");
    groups.reconcile(&scan.sessions, true, &now());
    assert!(groups.is_expanded(3));
    groups.reconcile(&scan.sessions, false, &now());
    assert!(!groups.is_expanded(3));
    groups.toggle("d");
    scan.sessions.retain(|session| session.workspace != "d");
    groups.reconcile(&scan.sessions, true, &now());
    assert!(!groups.expanded.contains("d"));
}

#[gpui_kit::test]
fn grouping_controls_are_reachable_by_pointer_and_keyboard_without_changing_selected_detail(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (temp, root, scan) = fixture(&[("a", "one", 30), ("b", "two", 20), ("a", "three", 10)]);
    let detail = root
        .detail(&scan.sessions[2].file, Limits::default())
        .unwrap();
    let (window, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let mut view = super::super::SessionsView::new(window, cx);
                view.groups.reconcile(&scan.sessions, false, &now());
                view.scan = Arc::new(scan);
                view.root = Some(root.clone());
                view.loaded_source = Some(temp.path().to_path_buf());
                view.selected = Some(2);
                view.detail = Some(detail);
                view
            })
        })
        .unwrap()
    });
    let mut visual = VisualTestContext::from_window(window, cx);
    visual.update(|window, cx| window.click("copilot-session-workspace-a", cx));
    visual.update(|window, cx| {
        assert!(!view.read(cx).groups.is_expanded(0));
        let path = view.read(cx).path.clone();
        path.update(cx, |state, cx| state.focus(window, cx));
        window.draw(cx).clear(cx);
        for _ in 0..24 {
            if window.find("copilot-session-workspace-a").focused() == Some(true) {
                break;
            }
            window.press("tab", cx);
        }
        assert_eq!(
            window.find("copilot-session-workspace-a").focused(),
            Some(true)
        );
        window.press("enter", cx);
        let state = view.read(cx);
        assert!(state.groups.is_expanded(0));
        assert_eq!(state.selected, Some(2));
        assert_eq!(state.detail.as_ref().unwrap().session_id, "three");
        assert_eq!(state.loaded_source.as_deref(), Some(temp.path()));
    });
    visual.update(|window, cx| {
        let destination = view.read(cx).destination(view.clone());
        view.update(cx, |view, cx| view.toggle_workspace("a", cx));
        assert!(!view.read(cx).groups.is_expanded(0));
        (destination.restore)(window, cx);
        assert!(view.read(cx).groups.is_expanded(0));
        view.update(cx, |view, _| {
            view.groups = Groups::default();
            view.scan = Arc::default();
            view.root = None;
            view.loaded_source = None;
            view.selected = None;
            view.detail = None;
            view.page = 8;
        });
        (destination.restore)(window, cx);
        let state = view.read(cx);
        assert_eq!(state.page, 0);
        assert_eq!(state.groups.groups.len(), 2);
        assert_eq!(state.selected, Some(2));
        assert_eq!(
            state
                .root
                .as_ref()
                .unwrap()
                .detail(&state.scan.sessions[2].file, Limits::default())
                .unwrap()
                .session_id,
            "three"
        );
    });
}

#[test]
fn each_workspace_lists_its_sessions_under_electrons_date_headings() {
    let midnight = now().start_of_day().unwrap().timestamp().as_millisecond() as u64;
    let hour = 60 * 60 * 1000;
    let (_temp, _root, scan) = fixture(&[
        ("a", "a-today", midnight + hour),
        ("b", "b-today", midnight + 2 * hour),
        ("a", "a-yesterday", midnight - hour),
        ("a", "a-week", midnight - 3 * 24 * hour),
        ("a", "a-older-1", midnight - 30 * 24 * hour),
        ("a", "a-older-2", midnight - 31 * 24 * hour),
        ("b", "b-older", 0),
    ]);
    let mut groups = Groups::default();
    groups.reconcile(&scan.sessions, false, &now());
    let id = |row: &Row| match row {
        Row::Group(index) => format!("group {}", groups.groups[*index].key),
        Row::Bucket(bucket) => bucket.label().to_string(),
        Row::Session(index) => scan.sessions[*index].session_id.clone(),
    };
    let rows: Vec<_> = groups.page(0, 20).iter().map(id).collect();
    assert_eq!(
        rows,
        [
            "group b",
            "Today",
            "b-today",
            "Older",
            "b-older",
            "group a",
            "Today",
            "a-today",
            "Yesterday",
            "a-yesterday",
            "This Week",
            "a-week",
            "Older",
            "a-older-1",
            "a-older-2",
        ]
    );
    // Headings do not consume paged rows, so paging still covers each
    // session once and a continuation page restates its first heading.
    let first: Vec<_> = groups.page(0, 6).iter().map(id).collect();
    assert_eq!(
        first,
        [
            "group b",
            "Today",
            "b-today",
            "Older",
            "b-older",
            "group a",
            "Today",
            "a-today",
            "Yesterday",
            "a-yesterday",
        ]
    );
    let second: Vec<_> = groups.page(1, 6).iter().map(id).collect();
    assert_eq!(
        second,
        [
            "group a",
            "This Week",
            "a-week",
            "Older",
            "a-older-1",
            "a-older-2",
        ]
    );
    assert_eq!(groups.row_count(), 9);
}

#[test]
fn buckets_are_fixed_at_scan_time_like_electrons_memoized_groups() {
    let midnight = now().start_of_day().unwrap().timestamp().as_millisecond() as u64;
    let (_temp, _root, scan) = fixture(&[("a", "one", midnight + 1)]);
    let mut groups = Groups::default();
    groups.reconcile(&scan.sessions, false, &now());
    assert_eq!(groups.page(0, 20)[1], Row::Bucket(DateBucket::Today));
    let tomorrow = now().checked_add(jiff::Span::new().days(1)).unwrap();
    assert_eq!(groups.page(0, 20)[1], Row::Bucket(DateBucket::Today));
    groups.reconcile(&scan.sessions, true, &tomorrow);
    assert_eq!(groups.page(0, 20)[1], Row::Bucket(DateBucket::Yesterday));
}
