use super::*;

#[test]
fn convex_float_thread_counts_are_exact_and_fractional_counts_are_rejected() {
    let value: serde_json::Value =
        convex::Value::try_from(serde_json::json!({"total":3.0,"unresolved":2.0,"outdated":1.0}))
            .unwrap()
            .into();
    let stats: ThreadStats = serde_json::from_value(value).unwrap();
    assert_eq!((stats.total, stats.unresolved, stats.outdated), (3, 2, 1));
    for count in [-1.0, 1.5, u32::MAX as f64 + 1.0] {
        assert!(
            serde_json::from_value::<ThreadStats>(
                serde_json::json!({"total":count,"unresolved":0,"outdated":0})
            )
            .is_err()
        );
    }
}
use serde_json::json;

fn row() -> Value {
    json!({"_id":"result-1", "promptPreview":"synthetic 界", "status":"completed", "createdAt":1,
        "completedAt":2, "duration":1, "hasOutput":false})
}

fn wire(rows: Vec<Value>) -> Value {
    json!({"page":rows, "isDone":true, "continueCursor":""})
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

#[test]
fn completed_without_output_and_future_status_are_preserved() {
    let mut future = row();
    future["_id"] = json!("result-2");
    future["status"] = json!("future-status");
    let page = parse_results(&bytes(&wire(vec![row(), future]))).unwrap();
    assert_eq!(page.rows.len(), 2);
    assert!(!page.rows[0].has_output);
    assert_eq!(page.rows[1].status, Status::Unknown);
    assert!(page.is_done && page.can_append());
}

#[test]
fn malformed_rows_do_not_hide_healthy_rows_or_claim_empty_history() {
    let mut invalid = row();
    invalid["createdAt"] = json!(-1);
    let page = parse_results(&bytes(&wire(vec![invalid, row(), json!(null)]))).unwrap();
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.skipped, 2);
}

#[test]
fn unicode_preview_limit_counts_characters_and_rejects_controls() {
    let mut valid = row();
    valid["promptPreview"] = json!("😀".repeat(200) + "…");
    let mut long = valid.clone();
    long["promptPreview"] = json!("😀".repeat(202));
    let mut control = row();
    control["promptPreview"] = json!("unsafe\nline");
    let page = parse_results(&bytes(&wire(vec![valid, long, control]))).unwrap();
    assert_eq!(page.rows[0].prompt_preview.chars().count(), 201);
    assert_eq!(page.skipped, 2);
}

#[test]
fn response_and_row_limits_fail_before_unbounded_history_allocation() {
    assert!(matches!(
        parse_results(&vec![b' '; MAX_BYTES + 1]),
        Err(DataError::TooLarge)
    ));
    assert!(matches!(
        parse_results(&bytes(&wire(vec![row(); 26]))),
        Err(DataError::TooManyResults)
    ));
    for invalid in [json!(null), json!([]), json!({"page":[]})] {
        assert!(matches!(
            parse_results(&bytes(&invalid)),
            Err(DataError::InvalidResponse)
        ));
    }
}

#[test]
fn empty_intermediate_pages_keep_the_opaque_continuation() {
    let page = parse_results(&bytes(
        &json!({"page":[],"isDone":false,"continueCursor":"opaque-next"}),
    ))
    .unwrap();
    assert!(!page.is_done);
    assert!(page.can_append());
    assert_eq!(page.continue_cursor, "opaque-next");
    assert!(matches!(
        parse_results(&bytes(
            &json!({"page":[],"isDone":false,"continueCursor":""})
        )),
        Err(DataError::InvalidResponse)
    ));
}

#[test]
fn required_and_unknown_splits_cannot_be_appended_as_complete_ranges() {
    for (status, expected) in [
        ("SplitRequired", false),
        ("SplitRecommended", true),
        ("future-split", false),
    ] {
        let page = parse_results(&bytes(&json!({"page":[row()],"isDone":false,"continueCursor":"end", "splitCursor":"middle", "pageStatus":status}))).unwrap();
        assert_eq!(page.can_append(), expected);
        assert_eq!(page.split_cursor.as_deref(), Some("middle"));
    }
    assert!(matches!(
        parse_results(&bytes(
            &json!({"page":[],"isDone":false,"continueCursor":"end", "pageStatus":"SplitRequired"})
        )),
        Err(DataError::InvalidResponse)
    ));
    assert!(matches!(
        parse_results(&bytes(
            &json!({"page":[],"isDone":false,"continueCursor":"x".repeat(MAX_CURSOR+1)})
        )),
        Err(DataError::InvalidResponse)
    ));
}

fn review() -> Value {
    json!({"_id":"review-1","resultId":"result-1","titlePreview":"Synthetic PR","status":"pending",
        "reviewedHeadSha":"a".repeat(40), "reviewedThreadStats":{"total":2,"unresolved":1,"outdated":1},
        "createdAt":1,"updatedAt":2})
}

#[test]
fn legacy_reviews_without_captured_context_remain_unknown() {
    let mut legacy = review();
    legacy.as_object_mut().unwrap().remove("reviewedHeadSha");
    legacy
        .as_object_mut()
        .unwrap()
        .remove("reviewedThreadStats");
    let page = parse_reviews(&bytes(&wire(vec![legacy, review()]))).unwrap();
    assert_eq!(page.rows.len(), 2);
    assert!(page.rows[0].reviewed_head_sha.is_none());
    assert!(page.rows[0].reviewed_thread_stats.is_none());
    assert_eq!(
        page.rows[1].reviewed_head_sha.as_ref().unwrap(),
        &"a".repeat(40)
    );
}

#[test]
fn invalid_review_snapshots_are_counted_instead_of_becoming_current() {
    let mut invalid_head = review();
    invalid_head["reviewedHeadSha"] = json!("not-a-head");
    let mut invalid_stats = review();
    invalid_stats["reviewedThreadStats"]["unresolved"] = json!(3);
    let mut invalid_count = review();
    invalid_count["reviewedThreadStats"]["total"] = json!(-1);
    let page = parse_reviews(&bytes(&wire(vec![
        invalid_head,
        invalid_stats,
        invalid_count,
        review(),
    ])))
    .unwrap();
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.skipped, 3);
}
