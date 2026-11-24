use super::*;
use serde_json::json;

fn saved() -> Value {
    json!({"_id":"synthetic-result","prompt":"Prompt 界🦀","status":"completed","result":"Answer 界",
        "model":"fixture","category":"pr-review","metadata":{"future":{"$integer":"AQAAAAAAAAA="}},
        "createdAt":1792152000123.0,"duration":1250.5,"completedAt":1792152001373.5,"futureField":"retained on server"})
}

#[test]
fn captured_review_number_survives_the_actual_convex_float_conversion() {
    use std::collections::BTreeMap;
    let metadata = Value::from(convex::Value::Object(BTreeMap::from([
        ("owner".into(), convex::Value::String("fixture-org".into())),
        ("repo".into(), convex::Value::String("native-demo".into())),
        ("prNumber".into(), convex::Value::Float64(7.0)),
        (
            "ghAccount".into(),
            convex::Value::String("synthetic-account".into()),
        ),
        (
            "reviewedHeadSha".into(),
            convex::Value::String("a".repeat(40)),
        ),
    ])));
    assert_eq!(metadata["prNumber"].as_u64(), None);
    let mut source = saved();
    source["metadata"] = metadata.clone();
    let result = parse_detail(&serde_json::to_vec(&source).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(result.review_pull_request_number(), Some(7));
    assert_eq!(result.metadata, Some(metadata));
}

#[test]
fn review_numbers_accept_exact_numeric_forms_without_coercing_invalid_metadata() {
    for number in [json!(7), json!(7.0), json!(9_007_199_254_740_991_u64)] {
        let mut source = saved();
        source["metadata"] = json!({"prNumber":number});
        let result = parse_detail(&serde_json::to_vec(&source).unwrap())
            .unwrap()
            .unwrap();
        assert!(result.review_pull_request_number().is_some());
    }
    for number in [
        Value::Null,
        json!(0),
        json!(-1),
        json!(7.5),
        json!("7"),
        json!(true),
        json!({"$integer":"BwAAAAAAAAA="}),
        json!(9_007_199_254_740_992_u64),
        json!(u64::MAX),
    ] {
        let mut source = saved();
        source["metadata"] = json!({"prNumber":number});
        let result = parse_detail(&serde_json::to_vec(&source).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(result.review_pull_request_number(), None);
    }
}

#[test]
fn reads_existing_records_without_erasing_future_metadata_or_unicode() {
    let source = saved();
    let result = parse_detail(&serde_json::to_vec(&source).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(result.id, "synthetic-result");
    assert_eq!(result.result.as_deref(), Some("Answer 界"));
    assert_eq!(result.content_state(), ContentState::Captured);
    assert_eq!(result.metadata, Some(source["metadata"].clone()));
    assert_eq!(result.duration, Some(1250.5));
}

#[test]
fn existing_large_prompts_remain_readable_even_when_new_executions_have_a_smaller_cap() {
    let mut source = saved();
    let prompt = "x".repeat(300 * 1024);
    source["prompt"] = json!(prompt);
    source["error"] = json!("e".repeat(40 * 1024));
    let result = parse_detail(&serde_json::to_vec(&source).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(result.prompt, prompt);
    assert_eq!(result.prompt_preview().len(), 200);
    assert_eq!(result.error.as_ref().unwrap().len(), 40 * 1024);
}

#[test]
fn missing_output_and_unknown_status_are_distinct_from_captured_success() {
    let mut source = saved();
    source.as_object_mut().unwrap().remove("result");
    let result = parse_detail(&serde_json::to_vec(&source).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(result.content_state(), ContentState::Missing);
    source["status"] = json!("future-status");
    let result = parse_detail(&serde_json::to_vec(&source).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(result.status, Status::Unknown);
    assert_eq!(result.content_state(), ContentState::Unknown);
    for (status, expected) in [
        ("pending", ContentState::Pending),
        ("running", ContentState::Running),
        ("failed", ContentState::Failed),
    ] {
        source["status"] = json!(status);
        assert_eq!(
            parse_detail(&serde_json::to_vec(&source).unwrap())
                .unwrap()
                .unwrap()
                .content_state(),
            expected
        );
    }
}

#[test]
fn malformed_rows_report_partial_history_and_keep_valid_rows_in_server_order() {
    let mut second = saved();
    second["_id"] = json!("second");
    let page = parse_history(
        &serde_json::to_vec(&json!([saved(), {"private":"not a result"}, second])).unwrap(),
    )
    .unwrap();
    assert_eq!(page.skipped, 1);
    assert_eq!(
        page.results
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["synthetic-result", "second"]
    );
}

#[test]
fn row_response_and_field_limits_return_static_errors_without_private_content() {
    assert!(matches!(
        parse_history(&vec![b'x'; MAX_RESPONSE + 1]),
        Err(DataError::TooLarge)
    ));
    assert!(matches!(
        parse_history(&serde_json::to_vec(&vec![saved(); MAX_ROWS + 1]).unwrap()),
        Err(DataError::TooManyResults)
    ));
    let mut source = saved();
    source["result"] = json!("x".repeat(4 * 1024 * 1024 + 1));
    assert!(matches!(
        parse_detail(&serde_json::to_vec(&source).unwrap()),
        Err(DataError::InvalidRecord)
    ));
    source = saved();
    source["duration"] = json!(-1);
    assert!(matches!(
        parse_detail(&serde_json::to_vec(&source).unwrap()),
        Err(DataError::InvalidRecord)
    ));
    let error = parse_history(br#"{"privatePrompt":"PRIVATE-SYNTHETIC-CONTENT"}"#)
        .err()
        .unwrap();
    assert!(!error.to_string().contains("PRIVATE-SYNTHETIC-CONTENT"));
}

#[test]
fn missing_result_is_not_a_malformed_result_and_preview_counts_unicode_characters() {
    assert!(parse_detail(b"null").unwrap().is_none());
    assert!(matches!(parse_detail(b"[]"), Err(DataError::InvalidRecord)));
    assert!(matches!(
        parse_detail(b"not JSON"),
        Err(DataError::InvalidResponse)
    ));
    let mut source = saved();
    let prompt = "界🦀".repeat(150);
    source["prompt"] = json!(prompt);
    let result = parse_detail(&serde_json::to_vec(&source).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        result.prompt_preview(),
        prompt.chars().take(200).collect::<String>()
    );
}

#[test]
fn only_a_pr_review_result_offers_its_stored_web_link() {
    let detail = |category: Value, url: Value| {
        let mut source = saved();
        source["category"] = category;
        source["metadata"] = json!({"prUrl": url, "owner": "owner"});
        parse_detail(&serde_json::to_vec(&source).unwrap())
            .unwrap()
            .unwrap()
    };
    let link = "https://github.com/owner/repo/pull/7";
    assert_eq!(
        detail(json!("pr-review"), json!(link))
            .pull_request_url()
            .as_deref(),
        Some(link)
    );
    for (category, url) in [
        (json!("general"), json!(link)),
        (json!(null), json!(link)),
        (json!("pr-review"), json!(7)),
        (json!("pr-review"), json!("javascript:alert(1)")),
        (json!("pr-review"), json!("file:///private")),
        (
            json!("pr-review"),
            json!("https://user:secret@github.com/o/r/pull/1"),
        ),
        (json!("pr-review"), json!("https://github.com/o/r/pull/1\n")),
        (
            json!("pr-review"),
            json!(format!("https://github.com/{}", "a".repeat(2048))),
        ),
        (json!("pr-review"), json!("github.com/owner/repo/pull/7")),
    ] {
        assert!(detail(category, url).pull_request_url().is_none());
    }
    let mut source = saved();
    source["category"] = json!("pr-review");
    source["metadata"] = Value::Null;
    let result = parse_detail(&serde_json::to_vec(&source).unwrap())
        .unwrap()
        .unwrap();
    assert!(result.pull_request_url().is_none());
}

#[test]
fn categories_store_electrons_values_and_status_emoji_match_its_recent_list() {
    let codes: Vec<_> = Category::ALL.iter().map(|c| c.code()).collect();
    assert_eq!(
        codes,
        ["general", "pr-review", "code-analysis", "documentation"]
    );
    let labels: Vec<_> = Category::ALL.iter().map(|c| c.label()).collect();
    assert_eq!(
        labels,
        ["General", "PR Review", "Code Analysis", "Documentation"]
    );
    assert_eq!(status_emoji(Status::Pending), "⏳");
    assert_eq!(status_emoji(Status::Running), "🔄");
    assert_eq!(status_emoji(Status::Completed), "✅");
    assert_eq!(status_emoji(Status::Failed), "❌");
    assert_eq!(status_emoji(Status::Unknown), "•");
}

#[test]
fn a_re_run_uses_the_saved_metadata_account_when_there_is_one() {
    let mut value = saved();
    value["metadata"] = json!({"ghAccount": " saved-account "});
    let result = parse_detail(&serde_json::to_vec(&value).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(result.saved_account(), Some("saved-account"));
    for metadata in [
        json!({"ghAccount": ""}),
        json!({"ghAccount": 7}),
        json!(null),
    ] {
        value["metadata"] = metadata;
        let result = parse_detail(&serde_json::to_vec(&value).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(result.saved_account(), None);
    }
}
