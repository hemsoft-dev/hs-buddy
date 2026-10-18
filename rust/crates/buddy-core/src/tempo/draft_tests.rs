use super::*;

fn row() -> Worklog {
    Worklog {
        id: WorklogId::parse(1).unwrap(),
        issue: Some(IssueId::parse(2).unwrap()),
        author: AccountId::parse("123456:synthetic-user").unwrap(),
        seconds: 90061,
        billable_seconds: Some(90000),
        date: "2026-10-16".parse().unwrap(),
        start_time: Some("20:06:00".parse().unwrap()),
        start_instant: Some("2026-10-17T00:06:00Z".parse().unwrap()),
        description: Some("Synthetic Unicode 界\nQuoted \"description\"".into()),
        attributes: Some(vec![
            Attribute {
                key: "_Account_".into(),
                value: Some("billing-one".into()),
            },
            Attribute {
                key: "_Custom_".into(),
                value: None,
            },
        ]),
        created_at: "2026-10-16T00:00:00Z".parse().unwrap(),
        updated_at: "2026-10-16T01:00:00Z".parse().unwrap(),
    }
}
fn payload(draft: &Draft) -> Value {
    serde_json::from_slice(&draft.encode().unwrap()).unwrap()
}

#[test]
fn copying_an_expanded_worklog_creates_new_identity_and_preserves_all_other_recorded_work() {
    let original = row();
    let draft = Draft::copy_to_date(
        &original,
        "2026-11-01".parse().unwrap(),
        "08:15:01".parse().unwrap(),
    )
    .unwrap();
    assert_eq!(draft.kind(), DraftKind::Create);
    assert!(
        draft.worklog_id().is_none()
            && draft.observed_update().is_none()
            && draft.baseline().is_none()
    );
    let body = payload(&draft);
    assert_eq!(body["authorAccountId"], "123456:synthetic-user");
    assert_eq!(body["issueId"], 2);
    assert_eq!(body["startDate"], "2026-11-01");
    assert_eq!(body["startTime"], "08:15:01");
    assert_eq!(body["timeSpentSeconds"], 90061);
    assert_eq!(body["billableSeconds"], 90000);
    assert_eq!(
        body["description"],
        "Synthetic Unicode 界\nQuoted \"description\""
    );
    assert_eq!(
        body["attributes"],
        json!([{"key":"_Account_","value":"billing-one"},{"key":"_Custom_"}])
    );
    assert_eq!(original.date.to_string(), "2026-10-16");
    assert_eq!(original.start_time.unwrap().to_string(), "20:06:00");
}

#[test]
fn copying_cannot_silently_drop_unloaded_attributes_or_a_missing_issue() {
    let mut source = row();
    source.attributes = None;
    assert!(matches!(
        Draft::copy_to_date(&source, source.date, "08:00:00".parse().unwrap()),
        Err(Error::AttributesUnavailable)
    ));
    source = row();
    source.issue = None;
    assert!(matches!(
        Draft::copy_to_date(&source, source.date, "08:00:00".parse().unwrap()),
        Err(Error::InvalidDraft)
    ));
    assert!(matches!(
        Draft::copy_to_date(&row(), row().date, "08:00:00.001".parse().unwrap()),
        Err(Error::InvalidDraft)
    ));
}

#[test]
fn deletion_retains_the_exact_baseline_and_has_no_override_or_provider_body() {
    for attributes in [Some(vec![]), None, row().attributes] {
        let mut original = row();
        original.attributes = attributes;
        let draft = Draft::delete(&original).unwrap();
        assert_eq!(draft.kind(), DraftKind::Delete);
        assert!(draft.author() == &original.author);
        assert!(draft.worklog_id() == Some(original.id));
        assert!(draft.baseline() == Some(&original));
        assert!(draft.encode().unwrap().is_empty());
        assert_eq!(draft.check_update_baseline(&original), Ok(()));
        for change in [
            |row: &mut Worklog| row.seconds += 1,
            |row: &mut Worklog| row.id = WorklogId::parse(42).unwrap(),
            |row: &mut Worklog| row.author = AccountId::parse("other-author").unwrap(),
            |row: &mut Worklog| row.description = Some("Changed remotely".into()),
        ] {
            let mut current = original.clone();
            change(&mut current);
            assert_eq!(current.updated_at, original.updated_at);
            assert_eq!(
                draft.check_update_baseline(&current),
                Err(Error::ChangedWhileLoading)
            );
        }
    }
}

#[test]
fn a_draft_keeps_its_complete_baseline_even_after_local_edits() {
    let original = row();
    let mut draft = Draft::edit(&original).unwrap();
    draft
        .set_description(Some("Changed locally".into()))
        .unwrap();
    draft.set_duration(600).unwrap();
    draft
        .set_billing_account(Some("new-local-account"))
        .unwrap();
    assert_eq!(draft.check_update_baseline(&original), Ok(()));
    assert_eq!(draft.observed_update(), Some(original.updated_at));
    assert_eq!(
        Draft::create(
            original.author.clone(),
            original.issue.unwrap(),
            original.date,
            60
        )
        .unwrap()
        .check_update_baseline(&original),
        Err(Error::InvalidDraft)
    );
}

#[test]
fn unchanged_update_timestamps_do_not_hide_changed_worklog_fields() {
    let original = row();
    let draft = Draft::edit(&original).unwrap();
    let changes: [fn(&mut Worklog); 12] = [
        |v| v.id = WorklogId::parse(3).unwrap(),
        |v| v.issue = Some(IssueId::parse(4).unwrap()),
        |v| v.author = AccountId::parse("other-user").unwrap(),
        |v| v.seconds += 1,
        |v| v.billable_seconds = None,
        |v| v.date = "2026-10-17".parse().unwrap(),
        |v| v.start_time = None,
        |v| v.start_instant = None,
        |v| v.description = None,
        |v| v.attributes.as_mut().unwrap()[0].value = Some("changed-remotely".into()),
        |v| v.created_at = "2026-10-15T00:00:00Z".parse().unwrap(),
        |v| v.attributes = None,
    ];
    for change in changes {
        let mut current = original.clone();
        change(&mut current);
        assert_eq!(current.updated_at, original.updated_at);
        assert_eq!(
            draft.check_update_baseline(&current),
            Err(Error::ChangedWhileLoading)
        );
    }
    let mut current = original.clone();
    current.updated_at = "2026-10-17T00:00:00Z".parse().unwrap();
    assert_eq!(
        draft.check_update_baseline(&current),
        Err(Error::ChangedWhileLoading)
    );
}

#[test]
fn billing_edits_preserve_a_multiline_custom_attribute_without_rewriting_it() {
    let mut original = row();
    original.attributes.as_mut().unwrap()[1].value = Some("first line\nsecond line".into());
    let mut draft = Draft::edit(&original).unwrap();
    draft
        .set_billing_account(Some("different-billing"))
        .unwrap();
    assert!(payload(&draft)["attributes"][1]["value"] == "first line\nsecond line");
    assert!(
        original.attributes.as_ref().unwrap()[1].value.as_deref()
            == Some("first line\nsecond line")
    );
}

#[test]
fn create_contains_the_required_scope_without_hidden_override_or_estimate_fields() {
    let row = row();
    let draft = Draft::create(
        row.author.clone(),
        row.issue.unwrap(),
        row.date,
        row.seconds,
    )
    .unwrap();
    assert_eq!(draft.kind(), DraftKind::Create);
    assert!(draft.worklog_id().is_none() && draft.observed_update().is_none());
    let value = payload(&draft);
    assert!(value["authorAccountId"] == row.author.as_str() && value["issueId"] == 2);
    assert!(value["timeSpentSeconds"] == 90061 && value["startDate"] == "2026-10-16");
    assert_eq!(value.as_object().unwrap().len(), 5);
    assert!(
        value.get("bypassPeriodClosuresAndApprovals").is_none()
            && value.get("remainingEstimateSeconds").is_none()
    );
}

#[test]
fn edit_captures_author_identity_freshness_and_every_loaded_optional_field() {
    let row = row();
    let draft = Draft::edit(&row).unwrap();
    assert_eq!(draft.kind(), DraftKind::Update);
    assert!(
        draft.author() == &row.author
            && draft.worklog_id() == Some(row.id)
            && draft.observed_update() == Some(row.updated_at)
    );
    let value = payload(&draft);
    assert!(value.get("issueId").is_none() && value.get("tempoWorklogId").is_none());
    assert!(value["description"].as_str() == row.description.as_deref());
    assert!(value["billableSeconds"] == 90000 && value["startTime"] == "20:06:00");
    assert!(value["startDate"] == "2026-10-16" && value.get("startDateTimeUtc").is_none());
    assert!(
        value["attributes"][1]["key"] == "_Custom_"
            && value["attributes"][1].get("value").is_none()
    );
}

#[test]
fn invalid_edits_leave_the_original_draft_intact_and_create_is_bounded() {
    let row = row();
    let mut draft = Draft::edit(&row).unwrap();
    let before = draft.encode().unwrap();
    assert_eq!(draft.set_duration(0), Err(Error::InvalidDraft));
    assert_eq!(
        draft.set_duration(i64::MAX as u64 + 1),
        Err(Error::InvalidDraft)
    );
    assert_eq!(
        draft.set_date(NaiveDate::from_ymd_opt(10000, 1, 1).unwrap()),
        Err(Error::InvalidDraft)
    );
    assert_eq!(
        draft.set_start_time(Some(NaiveTime::from_hms_nano_opt(12, 0, 0, 1).unwrap())),
        Err(Error::InvalidDraft)
    );
    assert_eq!(
        draft.set_description(Some("invalid\0description".into())),
        Err(Error::InvalidDraft)
    );
    assert_eq!(
        draft.set_description(Some("界".repeat(128 * 1024))),
        Err(Error::InvalidDraft)
    );
    assert_eq!(
        draft.set_billing_account(Some("\n")),
        Err(Error::InvalidDraft)
    );
    assert!(draft.encode().unwrap() == before);
    for seconds in [0, i64::MAX as u64 + 1] {
        assert!(matches!(
            Draft::create(row.author.clone(), row.issue.unwrap(), row.date, seconds),
            Err(Error::InvalidDraft)
        ));
    }
}

#[test]
fn unexpanded_attributes_and_invalid_imported_rows_cannot_form_replacement_updates() {
    let mut row = row();
    row.attributes = None;
    assert!(matches!(
        Draft::edit(&row),
        Err(Error::AttributesUnavailable)
    ));
    row.attributes = Some(vec![
        Attribute {
            key: "_Custom_".into(),
            value: None,
        },
        Attribute {
            key: "_Custom_".into(),
            value: Some("duplicate".into()),
        },
    ]);
    assert!(matches!(Draft::edit(&row), Err(Error::InvalidDraft)));
    row.attributes = Some(vec![Attribute {
        key: "".into(),
        value: None,
    }]);
    assert!(matches!(Draft::edit(&row), Err(Error::InvalidDraft)));
}

#[test]
fn billing_changes_preserve_unrelated_attributes_and_never_mutate_the_loaded_row() {
    let row = row();
    let mut draft = Draft::edit(&row).unwrap();
    draft.set_billing_account(Some("billing-two")).unwrap();
    assert!(payload(&draft)["attributes"][0]["value"] == "billing-two");
    assert!(row.billing_account().unwrap() == Some("billing-one"));
    draft.set_billing_account(None).unwrap();
    let value = payload(&draft);
    assert_eq!(value["attributes"].as_array().unwrap().len(), 1);
    assert!(
        value["attributes"][0]["key"] == "_Custom_"
            && value["attributes"][0].get("value").is_none()
    );
    draft.set_billing_account(Some("billing-three")).unwrap();
    assert_eq!(payload(&draft)["attributes"].as_array().unwrap().len(), 2);
}

#[test]
fn explicit_optional_changes_keep_calendar_dates_and_integer_seconds_exact() {
    let mut draft = Draft::edit(&row()).unwrap();
    draft.set_date("2024-02-29".parse().unwrap()).unwrap();
    draft.set_duration(i64::MAX as u64).unwrap();
    draft.set_start_time(None).unwrap();
    draft.set_description(Some(String::new())).unwrap();
    draft.set_billable_seconds(None);
    let value = payload(&draft);
    assert!(
        value["startDate"] == "2024-02-29"
            && value["timeSpentSeconds"].as_u64() == Some(i64::MAX as u64)
    );
    assert!(
        value["description"] == ""
            && value.get("startTime").is_none()
            && value.get("billableSeconds").is_none()
    );
}

#[test]
fn individually_valid_attributes_cannot_create_an_unbounded_outgoing_body() {
    let mut row = row();
    row.attributes = Some(
        (0..256)
            .map(|index| Attribute {
                key: format!("key-{index}"),
                value: Some("x".repeat(16 * 1024)),
            })
            .collect(),
    );
    let draft = Draft::edit(&row).unwrap();
    assert!(matches!(draft.encode(), Err(Error::DraftTooLarge)));
}
