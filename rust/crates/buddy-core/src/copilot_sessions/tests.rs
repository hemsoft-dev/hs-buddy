use std::io::Cursor;

use serde_json::json;

use super::*;

fn init() -> Value {
    json!({"kind":0,"v":{"sessionId":"session-界-1234","creationDate":1792152000123_i64,
        "customTitle":"Initial title","requests":[{"message":{"text":"First 界"}},{"message":{"text":"Second 🦀"}}],
        "inputState":{"selectedModel":{"metadata":{"id":"model-id","name":"Model name","multiplierNumeric":3,
            "maxInputTokens":128000,"maxOutputTokens":16000}}}}})
}

fn update(index: Value, input: u64, output: u64) -> Value {
    json!({"kind":1,"k":["requests",index,"result"],"v":{"metadata":{"promptTokens":input,"outputTokens":output},
        "timings":{"firstProgress":1.5,"totalElapsed":12.25}}})
}

fn input(lines: &[Value]) -> Vec<u8> {
    lines
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes()
}

fn detail(lines: &[Value]) -> SessionDetail {
    parse(&mut Cursor::new(input(lines)), Limits::default()).unwrap()
}

#[test]
fn orders_requests_and_replaces_duplicate_results_without_double_counting() {
    let session = detail(&[
        init(),
        update(json!(1), 10, 2),
        update(json!("0"), 20, 3),
        update(json!(0), 30, 4),
    ]);
    assert_eq!(session.results.len(), 2);
    assert_eq!(session.results[0].prompt, "First 界");
    assert_eq!(session.results[1].prompt, "Second 🦀");
    assert_eq!(session.totals().total_prompt_tokens, 40);
    assert_eq!(session.totals().total_output_tokens, 6);
    assert_eq!(session.totals().total_duration_ms, 24.5);
    assert_eq!(session.start_time, 1792152000123);
    assert_eq!(session.model.as_ref().unwrap().multiplier_numeric, 3.0);
    assert_eq!(session.model.as_ref().unwrap().max_input_tokens, 128000);
    assert_eq!(session.malformed_lines, 0);
}

#[test]
fn snapshots_fill_missing_prompts_and_keep_the_initial_prompt_for_existing_indices() {
    let mut first = init();
    first["v"]["requests"] = json!([{"message":{"text":"Original"}}]);
    let snapshot = json!({"kind":2,"k":["requests"],"v":[{"message":{"text":"Replacement"}},{"message":{"text":"Added"}}]});
    let session = detail(&[
        first,
        snapshot,
        update(json!(0), 1, 1),
        update(json!(1), 2, 2),
    ]);
    assert_eq!(session.results[0].prompt, "Original");
    assert_eq!(session.results[1].prompt, "Added");
}

#[test]
fn title_updates_use_real_json_unescaping_and_ignore_empty_titles() {
    let title = "Escaped \"name\" \\ path 界\nline";
    let session = detail(&[
        init(),
        json!({"kind":1,"k":["customTitle"],"v":title}),
        json!({"kind":1,"k":["customTitle"],"v":""}),
    ]);
    assert_eq!(session.title, title);
}

#[test]
fn unrelated_updates_cannot_be_mistaken_for_request_results() {
    let session = detail(&[
        init(),
        json!({"kind":1,"k":["future","0","result"],"v":{"metadata":{"promptTokens":999}}}),
        json!({"kind":1,"k":["requests","bad-index","result"],"v":{}}),
        update(json!(0), 1, 2),
    ]);
    assert_eq!(session.results.len(), 1);
    assert_eq!(session.totals().total_prompt_tokens, 1);
    assert_eq!(session.malformed_lines, 1);
}

#[test]
fn truncated_or_unknown_lines_produce_explicit_partial_metrics() {
    let mut bytes = input(&[init(), update(json!(0), 1, 2)]);
    bytes.extend_from_slice(b"\n{\"kind\":1,\"k\":[\n{\"kind\":42,\"v\":{}}\n");
    let session = parse(&mut Cursor::new(bytes), Limits::default()).unwrap();
    assert_eq!(session.malformed_lines, 2);
    assert_eq!(session.results.len(), 1);
    assert!(session.digest("", "", 0).partial);
}

#[test]
fn null_metadata_uses_timing_tokens_but_explicit_zero_wins() {
    let session = detail(&[
        init(),
        json!({"kind":1,"k":["requests",0,"result"],"v":{
        "metadata":{"promptTokens":0,"outputTokens":null},"timings":{"promptTokens":99,"outputTokens":5,"totalElapsed":0.5}}}),
    ]);
    assert_eq!(session.results[0].prompt_tokens, 0);
    assert_eq!(session.results[0].output_tokens, 5);
    assert_eq!(session.results[0].total_elapsed_ms, 0.5);
    assert_eq!(session.malformed_lines, 0);
}

#[test]
fn invalid_metrics_are_bounded_and_mark_the_session_partial() {
    let session = detail(&[
        init(),
        json!({"kind":1,"k":["requests",0,"result"],"v":{
        "metadata":{"promptTokens":-1,"outputTokens":"private-fixture-text"},"timings":{"totalElapsed":-2}}}),
    ]);
    assert_eq!(session.totals().total_prompt_tokens, 0);
    assert_eq!(session.totals().total_output_tokens, 0);
    assert_eq!(session.totals().total_duration_ms, 0.0);
    assert!(session.digest("", "", 0).partial);
}

#[test]
fn counts_tool_calls_but_keeps_unique_tool_names_in_first_seen_order() {
    let session = detail(&[
        init(),
        json!({"kind":1,"k":["requests",0,"result"],"v":{"metadata":{
        "toolCallRounds":[{"toolCalls":[{"name":"z_tool"},{"name":"grep_search"},{"name":"z_tool"}]}]}}}),
    ]);
    assert_eq!(session.results[0].tool_call_count, 3);
    assert_eq!(session.results[0].tool_names, ["z_tool", "grep_search"]);
    assert_eq!(session.digest("", "", 0).search_churn, 1);
}

#[test]
fn rejects_mixed_session_identities_instead_of_combining_private_prompts() {
    let mut other = init();
    other["v"]["sessionId"] = json!("other-session");
    assert_eq!(
        parse(&mut Cursor::new(input(&[init(), other])), Limits::default()),
        Err(ParseError::MixedSessionIds)
    );
}

#[test]
fn malformed_or_missing_identity_does_not_yield_a_successful_empty_session() {
    for bytes in [b"".as_slice(), b"{}", b"{\"kind\":0,\"v\":{}}", b"false"] {
        assert_eq!(
            parse(&mut Cursor::new(bytes), Limits::default()),
            Err(ParseError::MissingSession)
        );
    }
}

#[test]
fn rejects_invalid_utf8_even_after_valid_records() {
    let mut bytes = input(&[init()]);
    bytes.extend_from_slice(b"\n\xff");
    assert_eq!(
        parse(&mut Cursor::new(bytes), Limits::default()),
        Err(ParseError::InvalidUtf8)
    );
}

#[test]
fn file_limit_consumes_only_one_additional_byte_and_exact_boundary_is_valid() {
    let bytes = input(&[init()]);
    let limits = Limits {
        max_bytes: bytes.len(),
        ..Limits::default()
    };
    assert!(parse(&mut Cursor::new(bytes.clone()), limits).is_ok());
    let mut excessive = bytes.clone();
    excessive.extend_from_slice(b"\nmore bytes that must remain unread");
    let mut reader = Cursor::new(excessive);
    assert_eq!(parse(&mut reader, limits), Err(ParseError::TooLarge));
    assert_eq!(reader.position(), bytes.len() as u64 + 1);
}

#[test]
fn line_limit_stops_an_unterminated_oversized_record() {
    let mut reader = Cursor::new(vec![b' '; 10_000]);
    let limits = Limits {
        max_line_bytes: 64,
        ..Limits::default()
    };
    assert_eq!(parse(&mut reader, limits), Err(ParseError::LineTooLarge));
    assert_eq!(reader.position(), 65);
}

#[test]
fn bounds_request_count_even_when_all_results_arrive_as_updates() {
    let mut first = init();
    first["v"]["requests"] = json!([]);
    let limits = Limits {
        max_requests: 1,
        ..Limits::default()
    };
    let bytes = input(&[
        first.clone(),
        update(json!(0), 1, 1),
        update(json!(0), 2, 2),
    ]);
    assert_eq!(
        parse(&mut Cursor::new(bytes), limits).unwrap().results[0].prompt_tokens,
        2
    );
    let bytes = input(&[first, update(json!(0), 1, 1), update(json!(1), 2, 2)]);
    assert_eq!(
        parse(&mut Cursor::new(bytes), limits),
        Err(ParseError::TooManyRequests)
    );
    assert_eq!(
        parse(&mut Cursor::new(input(&[init()])), limits),
        Err(ParseError::TooManyRequests)
    );
}

#[test]
fn bounds_text_tools_and_record_count_without_suppressing_the_error() {
    let limits = Limits {
        max_lines: 1,
        ..Limits::default()
    };
    assert_eq!(
        parse(
            &mut Cursor::new(input(&[init(), update(json!(0), 1, 1)])),
            limits
        ),
        Err(ParseError::TooManyLines)
    );
    let limits = Limits {
        max_text_bytes: 8,
        ..Limits::default()
    };
    assert_eq!(
        parse(&mut Cursor::new(input(&[init()])), limits),
        Err(ParseError::TextTooLarge)
    );
    let tool = json!({"kind":1,"k":["requests",0,"result"],"v":{"metadata":{"toolCallRounds":[{"toolCalls":[{"name":"a"},{"name":"b"}]}]}}});
    let limits = Limits {
        max_tools_per_request: 1,
        ..Limits::default()
    };
    assert_eq!(
        parse(&mut Cursor::new(input(&[init(), tool])), limits),
        Err(ParseError::TooManyTools)
    );
}

#[test]
fn read_failures_and_invalid_limits_return_static_errors_without_file_contents() {
    struct Broken;
    impl std::io::Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("private-session-body"))
        }
    }
    impl BufRead for Broken {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            Err(std::io::Error::other("private-session-body"))
        }
        fn consume(&mut self, _: usize) {}
    }
    let error = parse(&mut Broken, Limits::default()).unwrap_err();
    assert_eq!(error, ParseError::ReadFailed);
    assert!(!error.to_string().contains("private-session-body"));
    assert_eq!(
        parse(
            &mut Cursor::new([]),
            Limits {
                max_bytes: 0,
                ..Limits::default()
            }
        ),
        Err(ParseError::InvalidLimits)
    );
}

#[test]
fn whitespace_crlf_unknown_fields_and_default_model_multiplier_are_supported() {
    let mut first = init();
    first["v"]["customTitle"] = json!("");
    first["v"]["inputState"]["selectedModel"]["metadata"]["multiplierNumeric"] = Value::Null;
    first["v"]["future"] = json!({"retained":"not an error"});
    let bytes = format!(" \r\n{}\r\n{}\r\n", first, update(json!(0), 1, 2));
    let session = parse(&mut Cursor::new(bytes), Limits::default()).unwrap();
    assert_eq!(session.title, "Session session-");
    assert_eq!(session.model.unwrap().multiplier_numeric, 1.0);
    assert_eq!(session.malformed_lines, 0);
}
