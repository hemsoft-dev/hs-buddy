use serde_json::{Value, json};

use super::*;

fn project() -> JiraProjectId {
    JiraProjectId::parse(9007199254740993).unwrap()
}
fn link(account: u64, default: bool) -> Value {
    json!({"id": account, "account":{"self":format!("https://api.tempo.io/4/accounts/{account}")},"default":default,"scope":{"id":project().get(),"type":"PROJECT"}})
}
fn body(rows: Vec<Value>) -> Vec<u8> {
    serde_json::to_vec(&json!({"metadata":{"count":rows.len()},"results":rows})).unwrap()
}

#[test]
fn account_links_accept_schema_self_references_and_matching_optional_ids() {
    let mut expanded = link(2, false);
    expanded["account"]["id"] = json!(2);
    let links = ProjectLinks::parse(&body(vec![link(1, true), expanded]), project()).unwrap();
    assert_eq!(links.0.len(), 2);
    assert_eq!(links.0[0].0.get(), 1);
    assert!(links.0[0].1 && !links.0[1].1);
}

#[test]
fn account_references_reject_changed_origin_route_or_identity() {
    for reference in [
        "http://api.tempo.io/4/accounts/1",
        "https://evil.invalid/4/accounts/1",
        "https://user@api.tempo.io/4/accounts/1",
        "https://api.tempo.io:444/4/accounts/1",
        "https://api.tempo.io/4/accounts/1?token=value",
        "https://api.tempo.io/4/accounts/1#fragment",
        "https://api.tempo.io/4/worklogs/1",
        "https://api.tempo.io/4/accounts/01",
        "https://api.tempo.io/4/accounts/1/extra",
        "https://api.tempo.io/4/accounts/2147483648",
        "https://api.tempo.io/4/accounts/0",
        "/4/accounts/1",
    ] {
        let mut value = link(1, true);
        value["account"]["self"] = json!(reference);
        assert!(
            ProjectLinks::parse(&body(vec![value]), project()).is_err(),
            "{reference}"
        );
    }
    let mut mismatch = link(1, true);
    mismatch["account"]["id"] = json!(2);
    assert!(ProjectLinks::parse(&body(vec![mismatch]), project()).is_err());
}

#[test]
fn project_links_reject_wrong_scope_duplicates_and_ambiguous_defaults() {
    for (field, value) in [("id", json!(7)), ("type", json!("GLOBAL"))] {
        let mut invalid = link(1, false);
        invalid["scope"][field] = value;
        assert!(matches!(
            ProjectLinks::parse(&body(vec![invalid]), project()),
            Err(Error::WrongScope)
        ));
    }
    for rows in [
        vec![link(1, false), link(1, false)],
        vec![link(1, true), link(2, true)],
        {
            let mut duplicate_link = link(2, false);
            duplicate_link["id"] = json!(1);
            vec![link(1, false), duplicate_link]
        },
    ] {
        assert!(ProjectLinks::parse(&body(rows), project()).is_err());
    }
    let mut invalid = link(1, false);
    invalid["id"] = json!(0);
    assert!(ProjectLinks::parse(&body(vec![invalid]), project()).is_err());
}

#[test]
fn links_resolve_against_the_catalog_and_preserve_closed_account_status() {
    let page = json!({"metadata":{"count":1,"offset":0,"limit":50},"results":[{"id":1,"key":"SYNTHETIC","name":"Synthetic closed account","global":false,"status":"CLOSED"}]});
    let (catalog, complete) = super::super::accounts::AccountPages::new()
        .push(&serde_json::to_vec(&page).unwrap())
        .unwrap();
    assert!(complete);
    let choices = ProjectLinks::parse(&body(vec![link(1, true)]), project())
        .unwrap()
        .resolve(catalog.finish())
        .unwrap();
    assert_eq!(choices[0].account.key(), "SYNTHETIC");
    assert!(
        choices[0].is_default
            && choices[0].account.status() == crate::tempo::BillingAccountStatus::Closed
    );
    assert!(matches!(
        ProjectLinks::parse(&body(vec![link(2, false)]), project())
            .unwrap()
            .resolve(Vec::new()),
        Err(Error::ChangedWhileLoading)
    ));
}

#[test]
fn link_envelopes_enforce_count_size_and_required_fields() {
    assert!(ProjectLinks::parse(b"null", project()).is_err());
    assert!(ProjectLinks::parse(br#"{"metadata":{"count":1},"results":[]}"#, project()).is_err());
    assert!(ProjectLinks::parse(&body(vec![json!({"id":1})]), project()).is_err());
    assert!(matches!(
        ProjectLinks::parse(&vec![b' '; MAX_RESPONSE_BYTES + 1], project()),
        Err(Error::TooLarge)
    ));
    assert!(
        ProjectLinks::parse(&body(vec![]), project())
            .unwrap()
            .resolve(vec![])
            .unwrap()
            .is_empty()
    );
}
