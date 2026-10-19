use serde_json::json;

use super::*;
use crate::tempo::parse_worklogs;

#[test]
fn review_regression_worklog_offsets_cannot_skip_or_overlap_advertised_pages() {
    for offset in [25, 49, 51, 100] {
        let next = format!(
            "https://api.tempo.io/4/worklogs/user/123456:synthetic-user?offset={offset}&limit=50"
        );
        let mut pages = read();
        assert_eq!(
            pages.push(0, page(Some(1), 0, Some(&next))),
            Err(Error::InvalidResponse)
        );
        assert_eq!(pages.request_offset(), Err(Error::InvalidResponse));
        assert!(matches!(pages.finish(), Err(Error::InvalidResponse)));
    }
    let mut pages = read();
    pages.push(0, page(Some(1), 0, Some(NEXT))).unwrap();
    let gap = "https://api.tempo.io/4/worklogs/user/123456:synthetic-user?offset=150&limit=50";
    assert_eq!(
        pages.push(50, page(Some(2), 50, Some(gap))),
        Err(Error::InvalidResponse)
    );
    assert!(matches!(pages.finish(), Err(Error::InvalidResponse)));
}

fn read() -> ReadPages {
    ReadPages::new(
        AccountId::parse("123456:synthetic-user").unwrap(),
        DateRange::new("2026-10-16".parse().unwrap(), "2026-10-17".parse().unwrap()).unwrap(),
    )
    .unwrap()
}
fn page(id: Option<u64>, offset: usize, next: Option<&str>) -> Page {
    let rows=id.map(|id|json!({"tempoWorklogId":id,"issue":{"self":"https://api.tempo.io/issue"},"author":{"accountId":"123456:synthetic-user"},"timeSpentSeconds":1,"startDate":"2026-10-16","createdAt":"2026-10-16T00:00:00Z","updatedAt":"2026-10-16T00:00:00Z","attributes":{"self":"https://api.tempo.io/attributes","values":[]},"self":"https://api.tempo.io/worklog"})).into_iter().collect::<Vec<_>>();
    parse_worklogs(&serde_json::to_vec(&json!({"self":"https://api.tempo.io/worklogs","metadata":{"count":rows.len(),"limit":50,"offset":offset,"next":next},"results":rows})).unwrap()).unwrap()
}
const NEXT: &str = "https://api.tempo.io/4/worklogs/user/123456:synthetic-user?offset=50&limit=50&from=2026-10-16&to=2026-10-17";

#[test]
fn no_pages_are_not_an_empty_success_and_partial_rows_stay_private() {
    assert!(matches!(read().finish(), Err(Error::IncompleteRead)));
    let mut partial = read();
    partial.push(0, page(Some(1), 0, Some(NEXT))).unwrap();
    assert_eq!(partial.request_offset(), Ok(50));
    assert!(matches!(partial.finish(), Err(Error::IncompleteRead)));
    let mut empty = read();
    empty.push(0, page(None, 0, None)).unwrap();
    assert!(empty.finish().unwrap().is_empty());
}
#[test]
fn completed_reads_keep_all_pages_and_allow_empty_intermediate_pages() {
    let mut read = read();
    read.push(0, page(Some(1), 0, Some(NEXT))).unwrap();
    read.push(
        50,
        page(
            None,
            50,
            Some("https://api.tempo.io/4/worklogs/user/123456:synthetic-user?offset=100"),
        ),
    )
    .unwrap();
    assert_eq!(read.request_offset(), Ok(100));
    assert!(read.author().as_str() == "123456:synthetic-user");
    assert_eq!(read.range().end.to_string(), "2026-10-17");
    read.push(100, page(Some(2), 100, None)).unwrap();
    assert!(read.is_complete());
    assert_eq!(read.request_offset(), Err(Error::IncompleteRead));
    assert_eq!(
        read.finish()
            .unwrap()
            .iter()
            .map(|row| row.id.get())
            .collect::<Vec<_>>(),
        [1, 2]
    );
}
#[test]
fn next_links_cannot_change_origin_user_range_or_filters() {
    for next in [
        NEXT.replace("https:", "http:"),
        NEXT.replace("api.tempo.io", "other.invalid"),
        NEXT.replace("synthetic-user", "other-user"),
        NEXT.replace("2026-10-16", "2026-10-15"),
        format!("{NEXT}&updatedFrom=2027-01-01"),
        format!("{NEXT}#fragment"),
        NEXT.replace("api.tempo.io", "user:password@api.tempo.io"),
    ] {
        let mut read = read();
        assert_eq!(
            read.push(0, page(Some(1), 0, Some(&next))),
            Err(Error::WrongScope)
        );
        assert!(matches!(read.finish(), Err(Error::WrongScope)));
    }
}
#[test]
fn duplicate_or_nonadvancing_offsets_and_out_of_order_pages_poison_a_read() {
    for next in [
        NEXT.replace("offset=50", "offset=0"),
        format!("{NEXT}&offset=100"),
        NEXT.replace("offset=50", "offset=-1"),
    ] {
        let mut read = read();
        assert_eq!(
            read.push(0, page(Some(1), 0, Some(&next))),
            Err(Error::InvalidResponse)
        );
    }
    let mut read = read();
    read.push(0, page(Some(1), 0, Some(NEXT))).unwrap();
    assert_eq!(
        read.push(0, page(Some(2), 50, None)),
        Err(Error::InvalidResponse)
    );
    assert_eq!(read.request_offset(), Err(Error::InvalidResponse));
}
#[test]
fn duplicate_records_and_another_authors_rows_never_become_partial_totals() {
    let mut duplicate = read();
    duplicate.push(0, page(Some(1), 0, Some(NEXT))).unwrap();
    assert_eq!(
        duplicate.push(50, page(Some(1), 50, None)),
        Err(Error::InvalidResponse)
    );
    let mut wrong = page(Some(1), 0, None);
    wrong.rows[0].author = AccountId::parse("other-user").unwrap();
    assert_eq!(read().push(0, wrong), Err(Error::WrongScope));
}
#[test]
fn page_budget_cannot_turn_an_unfinished_read_into_a_truncated_success() {
    let mut read = read();
    for index in 0..99 {
        let offset = index * 50;
        let next = format!(
            "https://api.tempo.io/4/worklogs/user/123456:synthetic-user?offset={}",
            offset + 50
        );
        read.push(offset, page(None, offset, Some(&next))).unwrap();
    }
    assert_eq!(
        read.push(
            4950,
            page(
                None,
                4950,
                Some("https://api.tempo.io/4/worklogs/user/123456:synthetic-user?offset=5000")
            )
        ),
        Err(Error::ReadLimit)
    );
    assert!(matches!(read.finish(), Err(Error::ReadLimit)));
}
