use serde_json::{Value, json};

use super::*;

fn row(id: u64) -> Value {
    json!({"id":id,"key":format!("SYNTHETIC-{id}"),"name":"Synthetic billing 雪", "status":"OPEN", "global":true})
}
fn page(offset: usize, rows: Vec<Value>, next: Option<&str>) -> Value {
    json!({"metadata":{"count":rows.len(),"limit":50,"offset":offset,"next":next},"results":rows})
}
fn push(pages: AccountPages, value: &Value) -> Result<(AccountPages, bool), Error> {
    pages.push(&serde_json::to_vec(value).unwrap())
}

#[test]
fn account_identity_uses_positive_int32_without_javascript_number_coercion() {
    assert_eq!(BillingAccountId::parse(1).unwrap().get(), 1);
    assert_eq!(
        BillingAccountId::parse(i32::MAX as u64).unwrap().get(),
        i32::MAX as u32
    );
    for id in [0, i32::MAX as u64 + 1, u64::MAX] {
        assert!(BillingAccountId::parse(id).is_err());
    }
    for id in [json!("7"), json!(-1), json!(7.1), Value::Null] {
        let mut invalid = row(7);
        invalid["id"] = id;
        assert!(push(AccountPages::new(), &page(0, vec![invalid], None)).is_err());
    }
}

#[test]
fn catalog_preserves_labels_global_flag_and_all_documented_statuses() {
    let mut rows = Vec::new();
    for (index, status) in ["OPEN", "CLOSED", "ARCHIVED"].iter().enumerate() {
        let mut value = row(index as u64 + 1);
        value["status"] = json!(status);
        value["global"] = json!(index == 0);
        rows.push(value);
    }
    let (pages, done) = push(AccountPages::new(), &page(0, rows, None)).unwrap();
    assert!(done);
    let rows = pages.finish();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].name(), "Synthetic billing 雪");
    assert_eq!(rows[0].key(), "SYNTHETIC-1");
    assert_eq!(rows[0].id().get(), 1);
    assert!(rows[0].is_global() && !rows[1].is_global());
    assert!(rows[0].status() == BillingAccountStatus::Open);
    assert!(rows[1].status() == BillingAccountStatus::Closed);
    assert!(rows[2].status() == BillingAccountStatus::Archived);
}

#[test]
fn catalog_rejects_unsupported_status_and_invalid_private_labels() {
    for (field, value) in [
        ("status", json!("UNKNOWN")),
        ("status", json!("open")),
        ("global", json!("true")),
        ("key", json!(" ")),
        ("key", json!("x\ny")),
        ("key", json!("x".repeat(257))),
        ("name", json!("")),
        ("name", json!("x\u{0}y")),
        ("name", json!("x".repeat(1025))),
    ] {
        let mut invalid = row(1);
        invalid[field] = value;
        assert!(push(AccountPages::new(), &page(0, vec![invalid], None)).is_err());
    }
}

#[test]
fn catalog_validates_page_metadata_and_body_limits() {
    for (field, value) in [
        ("count", json!(0)),
        ("offset", json!(1)),
        ("limit", json!(100)),
    ] {
        let mut invalid = page(0, vec![row(1)], None);
        invalid["metadata"][field] = value;
        assert!(push(AccountPages::new(), &invalid).is_err());
    }
    assert!(
        push(
            AccountPages::new(),
            &page(0, (1..=51).map(row).collect(), None)
        )
        .is_err()
    );
    assert!(matches!(
        AccountPages::new().push(&vec![b' '; MAX_RESPONSE_BYTES + 1]),
        Err(Error::TooLarge)
    ));
    assert!(AccountPages::new().push(b"null").is_err());
    assert!(
        AccountPages::new()
            .push(b"{\"metadata\":{},\"results\":[]}")
            .is_err()
    );
}

#[test]
fn duplicate_account_ids_and_keys_are_rejected_within_and_across_pages() {
    let mut duplicate_key = row(2);
    duplicate_key["key"] = row(1)["key"].clone();
    for duplicate in [row(1), duplicate_key] {
        assert!(push(AccountPages::new(), &page(0, vec![row(1), duplicate], None)).is_err());
    }
    for duplicate in [row(1), {
        let mut value = row(51);
        value["key"] = row(1)["key"].clone();
        value
    }] {
        let (pages, done) = push(
            AccountPages::new(),
            &page(
                0,
                (1..=50).map(row).collect(),
                Some("https://api.tempo.io/4/accounts?offset=50&limit=50"),
            ),
        )
        .unwrap();
        assert!(!done);
        assert!(push(pages, &page(50, vec![duplicate], None)).is_err());
    }
}

#[test]
fn continuations_reject_changed_scope_or_ambiguous_query_without_following_urls() {
    for next in [
        "http://api.tempo.io/4/accounts?offset=50",
        "https://evil.invalid/4/accounts?offset=50",
        "https://api.tempo.io:444/4/accounts?offset=50",
        "https://user@api.tempo.io/4/accounts?offset=50",
        "https://api.tempo.io/4/accounts?offset=50#fragment",
        "https://api.tempo.io/4/worklogs?offset=50",
        "https://api.tempo.io/4/accounts?offset=50&offset=100",
        "https://api.tempo.io/4/accounts?offset=50&limit=100",
        "https://api.tempo.io/4/accounts?offset=50&from=2026-10-17",
        "https://api.tempo.io/4/accounts?offset=0",
        "https://api.tempo.io/4/accounts?offset=100",
        "https://api.tempo.io/4/accounts?offset=2147483648",
        "/4/accounts?offset=50",
        "https://api.tempo.io/4/accounts",
    ] {
        assert!(
            push(
                AccountPages::new(),
                &page(0, (1..=50).map(row).collect(), Some(next))
            )
            .is_err(),
            "{next}"
        );
    }
    assert!(
        push(
            AccountPages::new(),
            &page(
                0,
                vec![row(1)],
                Some("https://api.tempo.io/4/accounts?offset=50")
            )
        )
        .is_err()
    );
}

#[test]
fn catalog_finishes_an_empty_response_and_a_complete_bounded_multi_page_read() {
    let (pages, done) = push(AccountPages::new(), &page(0, vec![], None)).unwrap();
    assert!(done && pages.finish().is_empty());
    let (pages, done) = push(
        AccountPages::new(),
        &page(
            0,
            (1..=50).map(row).collect(),
            Some("https://api.tempo.io/4/accounts?offset=50&limit=50"),
        ),
    )
    .unwrap();
    assert!(!done && pages.offset() == 50);
    let (pages, done) = push(pages, &page(50, vec![row(51)], None)).unwrap();
    assert!(done && pages.finish().len() == 51);
}

#[test]
fn page_budget_rejects_an_unbounded_catalog() {
    let mut pages = AccountPages::new();
    for index in 0..99 {
        let offset = index * 50;
        let next = format!("https://api.tempo.io/4/accounts?offset={}", offset + 50);
        let (loaded, done) = push(
            pages,
            &page(
                offset,
                (offset as u64 + 1..=offset as u64 + 50).map(row).collect(),
                Some(&next),
            ),
        )
        .unwrap();
        assert!(!done);
        pages = loaded;
    }
    assert!(matches!(
        push(
            pages,
            &page(
                4950,
                (4951..=5000).map(row).collect(),
                Some("https://api.tempo.io/4/accounts?offset=5000")
            )
        ),
        Err(Error::ReadLimit)
    ));
}
