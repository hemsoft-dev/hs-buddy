use super::*;

fn task(id: &str) -> Task {
    Task {
        id: Id::parse(id).unwrap(),
        content: "Synthetic task".into(),
        description: String::new(),
        project_id: Id::parse("project").unwrap(),
        parent_id: None,
        priority: Priority::from_api(1).unwrap(),
        due: None,
        labels: Vec::new(),
        completed: false,
        deleted: false,
        created_at: None,
        order: 0,
    }
}
fn page(ids: &[&str], next: Option<&str>) -> Page<Task> {
    Page {
        items: ids.iter().map(|id| task(id)).collect(),
        next_cursor: next.map(str::to_owned),
    }
}

#[test]
fn zero_pages_cannot_qualify_as_an_empty_read_but_an_observed_empty_end_can() {
    assert!(matches!(task_pages().finish(), Err(Error::IncompleteRead)));
    let mut read = task_pages();
    assert_eq!(read.request_cursor().unwrap(), None);
    read.push(None, page(&[], None)).unwrap();
    assert!(read.is_complete());
    assert_eq!(read.received_pages(), 1);
    assert!(read.finish().unwrap().is_empty());
}

#[test]
fn a_completed_read_returns_every_page_in_server_order() {
    let mut read = task_pages();
    read.push(None, page(&["first", "second"], Some("cursor-one")))
        .unwrap();
    assert_eq!(read.received_items(), 2);
    assert!(!read.is_complete());
    let cursor = read.request_cursor().unwrap();
    assert_eq!(cursor.as_deref(), Some("cursor-one"));
    read.push(cursor.as_deref(), page(&["third"], None))
        .unwrap();
    assert!(matches!(read.request_cursor(), Err(Error::IncompleteRead)));
    assert_eq!(read.received_pages(), 2);
    let rows = read.finish().unwrap();
    assert_eq!(
        rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        ["first", "second", "third"]
    );
}

#[test]
fn an_unfinished_read_never_returns_its_partial_records() {
    let mut read = task_pages();
    read.push(None, page(&["known-task"], Some("next-page")))
        .unwrap();
    assert!(matches!(read.finish(), Err(Error::IncompleteRead)));
}

#[test]
fn cursor_cycles_and_out_of_order_responses_poison_the_read_without_replacing_the_error() {
    let mut read = task_pages();
    read.push(None, page(&["first"], Some("one"))).unwrap();
    read.push(Some("one"), page(&["second"], Some("two")))
        .unwrap();
    assert_eq!(
        read.push(Some("two"), page(&["third"], Some("one"))),
        Err(Error::InvalidResponse)
    );
    assert_eq!(read.request_cursor(), Err(Error::InvalidResponse));
    assert_eq!(
        read.push(Some("two"), page(&[], None)),
        Err(Error::InvalidResponse)
    );
    assert!(matches!(read.finish(), Err(Error::InvalidResponse)));
    let mut read = task_pages();
    read.push(None, page(&["first"], Some("expected"))).unwrap();
    let mut copied = read.request_cursor().unwrap().unwrap();
    copied.push_str("-changed");
    assert_eq!(
        read.push(Some(&copied), page(&[], None)),
        Err(Error::InvalidResponse)
    );
    assert!(matches!(read.finish(), Err(Error::InvalidResponse)));
}

#[test]
fn duplicates_on_later_pages_and_malformed_constructed_cursors_are_rejected() {
    let mut read = task_pages();
    read.push(None, page(&["same-task"], Some("next"))).unwrap();
    assert_eq!(
        read.push(Some("next"), page(&["same-task"], None)),
        Err(Error::InvalidResponse)
    );
    assert!(matches!(read.finish(), Err(Error::InvalidResponse)));
    for cursor in ["", "\n"] {
        let mut read = task_pages();
        assert_eq!(
            read.push(None, page(&["first"], Some(cursor))),
            Err(Error::InvalidResponse)
        );
        assert_eq!(read.received_items(), 0);
    }
}

#[test]
fn empty_intermediate_pages_are_allowed_when_the_cursor_advances() {
    let mut read = task_pages();
    read.push(None, page(&[], Some("one"))).unwrap();
    read.push(Some("one"), page(&[], Some("two"))).unwrap();
    read.push(Some("two"), page(&["last"], None)).unwrap();
    assert_eq!(read.received_pages(), 3);
    assert_eq!(read.finish().unwrap()[0].id.as_str(), "last");
}

#[test]
fn page_budget_requires_a_terminal_page_at_the_boundary() {
    let mut read = task_pages();
    for index in 0..MAX_READ_PAGES - 1 {
        let cursor = read.request_cursor().unwrap();
        let next = format!("cursor-{index}");
        read.push(cursor.as_deref(), page(&[], Some(&next)))
            .unwrap();
    }
    let cursor = read.request_cursor().unwrap();
    assert_eq!(
        read.push(cursor.as_deref(), page(&[], Some("one-too-many"))),
        Err(Error::ReadLimit)
    );
    assert!(matches!(read.finish(), Err(Error::ReadLimit)));
    let mut read = task_pages();
    for index in 0..MAX_READ_PAGES {
        let cursor = read.request_cursor().unwrap();
        let next = (index + 1 < MAX_READ_PAGES).then(|| format!("cursor-{index}"));
        let ids = if index == 0 {
            &["first-task"][..]
        } else {
            &[][..]
        };
        read.push(cursor.as_deref(), page(ids, next.as_deref()))
            .unwrap();
    }
    assert_eq!(read.received_pages(), MAX_READ_PAGES);
    assert_eq!(read.finish().unwrap().len(), 1);
}

fn fill_item_budget() -> ReadPages<Task> {
    let mut read = task_pages();
    for start in (0..MAX_READ_ITEMS).step_by(MAX_PAGE_ITEMS) {
        let cursor = read.request_cursor().unwrap();
        let items = (start..start + MAX_PAGE_ITEMS)
            .map(|index| task(&format!("task-{index}")))
            .collect();
        read.push(
            cursor.as_deref(),
            Page {
                items,
                next_cursor: Some(format!("after-{start}")),
            },
        )
        .unwrap();
    }
    read
}

#[test]
fn item_budget_cannot_truncate_a_nonterminal_read_into_success() {
    let mut read = fill_item_budget();
    assert_eq!(read.received_items(), MAX_READ_ITEMS);
    let cursor = read.request_cursor().unwrap();
    assert_eq!(
        read.push(cursor.as_deref(), page(&["extra-task"], None)),
        Err(Error::ReadLimit)
    );
    assert!(matches!(read.finish(), Err(Error::ReadLimit)));
    let mut read = fill_item_budget();
    let cursor = read.request_cursor().unwrap();
    read.push(cursor.as_deref(), page(&[], None)).unwrap();
    assert_eq!(read.finish().unwrap().len(), MAX_READ_ITEMS);
}

#[test]
fn project_reads_use_their_own_identity_set() {
    let project = Project {
        id: Id::parse("same-id").unwrap(),
        name: "Synthetic project".into(),
        color: "berry_red".into(),
        parent_id: None,
        order: 1,
        archived: false,
        deleted: false,
        frozen: false,
        role: super::ProjectRole::Full,
    };
    let mut projects = project_pages();
    projects
        .push(
            None,
            Page {
                items: vec![project],
                next_cursor: None,
            },
        )
        .unwrap();
    let mut tasks = task_pages();
    tasks.push(None, page(&["same-id"], None)).unwrap();
    assert_eq!(projects.finish().unwrap()[0].name, "Synthetic project");
    assert_eq!(tasks.finish().unwrap()[0].id.as_str(), "same-id");
}
