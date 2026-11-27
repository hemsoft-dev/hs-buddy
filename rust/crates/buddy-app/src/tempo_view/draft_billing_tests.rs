//! The draft billing state model (`draft_billing`) under random event
//! sequences, plus the round-4 and round-5 review scenarios by name. Every
//! step drives the real editor (and, for some lookups, the view's
//! begin/settle), and an independent oracle checks I1-I5 after it.
//!
//! Failure messages name the seed, sequence, step, event kinds and the
//! invariant, never an account key, a list or the billing state: account
//! keys are private provider data, and test output reaches CI logs.
use super::behavior_tests::{account_with, both_linked, catalog, details, project_lookup, worklog};
use super::draft_billing::{CarrySource, Choice, Resolution};
use super::*;
use buddy_core::tempo::BillingAccountStatus::{self, Archived, Closed, Open};
use buddy_core::tempo::{JiraProjectId, ProjectBillingAccount};
use serde_json::Value;

/// The accounts sequences draw from. A SYNTH-19 create infers GEN-DEV.
const KEYS: [&str; 4] = ["SYNTH-A", "SYNTH-B", "GEN-DEV", "SYNTH-C"];
const EXISTING: &str = "SYNTH-EXISTING";
const SEQUENCES: usize = 4000;
const MAX_STEPS: usize = 30;
const SEED: u64 = 0x1026_0004_5eed_cafe;

/// xorshift64*: deterministic, no dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, in_ten: usize) -> bool {
        self.below(10) < in_ten
    }
    fn status(&mut self) -> Option<BillingAccountStatus> {
        [Some(Open), Some(Open), Some(Closed), Some(Archived), None][self.below(5)]
    }
    /// One of KEYS, or `None` for "No billing account".
    fn key(&mut self) -> Option<&'static str> {
        let index = self.below(KEYS.len() + 1);
        KEYS.get(index).copied()
    }
}

fn account_id(key: &str) -> u64 {
    100 + KEYS.iter().position(|k| *k == key).unwrap() as u64
}

/// The oracle's view of a list: (key, status, is_default), in order.
type List = Vec<(String, BillingAccountStatus, bool)>;

fn open_in(list: &List, key: &str) -> bool {
    list.iter()
        .any(|(k, status, _)| k == key && *status == Open)
}

/// A random lookup response for SYNTH-19 in project 11 or 12, and the list
/// the oracle expects it to show.
fn lookup_response(rng: &mut Rng) -> (EditorIssue, List) {
    let mut info = details();
    let project = if rng.chance(7) { 11 } else { 12 };
    info.billing.project = JiraProjectId::parse(project).unwrap();
    let linked = rng.chance(6);
    let mut list = List::new();
    for key in KEYS {
        let catalog_status = rng.status();
        if let Some(status) = catalog_status {
            info.billing
                .catalog
                .push(account_with(account_id(key), key, status));
        }
        if linked && let Some(status) = rng.status() {
            info.billing.choices.push(ProjectBillingAccount {
                account: account_with(account_id(key), key, status),
                is_default: false,
            });
        }
    }
    if !info.billing.choices.is_empty() && rng.chance(7) {
        let index = rng.below(info.billing.choices.len());
        info.billing.choices[index].is_default = true;
    }
    if info.billing.choices.is_empty() {
        // No links: every account, with no default.
        for account in &info.billing.catalog {
            list.push((account.key().to_owned(), account.status(), false));
        }
    } else {
        for choice in &info.billing.choices {
            list.push((
                choice.account.key().to_owned(),
                choice.account.status(),
                choice.is_default,
            ));
        }
    }
    (info, list)
}
fn catalog_response(rng: &mut Rng) -> (buddy_core::tempo::BillingCatalog, List) {
    let mut catalog = catalog();
    catalog.accounts.clear();
    let mut list = List::new();
    for key in KEYS {
        if let Some(status) = rng.status() {
            catalog
                .accounts
                .push(account_with(account_id(key), key, status));
            list.push((key.to_owned(), status, false));
        }
    }
    (catalog, list)
}

/// What saving the draft must do now.
enum Expected {
    /// I1: no resolved lookup, so nothing can be saved.
    NeedsLookup,
    /// I5: the person's pick is not open in the current list.
    RefusedPick,
    /// I3: the request carries this account.
    Sends(Option<String>),
}

#[derive(Clone, PartialEq)]
enum OracleResolution {
    NotLookedUp(Option<Option<List>>),
    Pending(u64),
    Resolved(u64, JiraProjectId, List),
    Failed(u64),
}

/// What the model must do, computed without it.
struct Oracle {
    generation: u64,
    resolution: OracleResolution,
    choice: Choice,
    /// An edit's existing account.
    existing: Option<Option<String>>,
}
impl Oracle {
    fn shown(&self) -> Option<&List> {
        match &self.resolution {
            OracleResolution::NotLookedUp(Some(Some(list))) => Some(list),
            OracleResolution::Resolved(_, _, list) => Some(list),
            _ => None,
        }
    }
    fn shown_project(&self) -> Option<JiraProjectId> {
        match &self.resolution {
            OracleResolution::Resolved(_, project, _) => Some(*project),
            _ => None,
        }
    }
    /// Whether a list admits `key`: no account, the edit's own, or open.
    fn admits(&self, list: &List, key: Option<&str>) -> bool {
        key.is_none_or(|key| {
            self.existing
                .as_ref()
                .is_some_and(|existing| existing.as_deref() == Some(key))
                || open_in(list, key)
        })
    }
    /// What a fresh draft sends from `list`: an edit its existing account; a
    /// create the open default, else GEN-DEV (inferred) where it is open.
    fn fresh(&self, list: &List) -> Option<String> {
        if let Some(existing) = &self.existing {
            return existing.clone();
        }
        list.iter()
            .find(|(_, status, default)| *default && *status == Open)
            .map(|(key, ..)| key.clone())
            .or_else(|| open_in(list, "GEN-DEV").then(|| "GEN-DEV".to_owned()))
    }
    /// I1, I3 and I5: whether the draft saves, and what the request sends,
    /// from the current list and the current choice only.
    fn request(&self) -> Expected {
        let OracleResolution::Resolved(_, _, list) = &self.resolution else {
            return Expected::NeedsLookup;
        };
        match &self.choice {
            Choice::Untouched => Expected::Sends(self.fresh(list)),
            Choice::User { key, .. } => {
                if self.admits(list, key.as_deref()) {
                    Expected::Sends(key.clone())
                } else {
                    Expected::RefusedPick
                }
            }
            Choice::Carried { key, .. } => Expected::Sends(if self.admits(list, key.as_deref()) {
                key.clone()
            } else {
                self.fresh(list)
            }),
        }
    }
    fn resolve(&mut self, generation: u64, project: JiraProjectId, list: List) {
        if self.resolution != OracleResolution::Pending(generation) {
            return;
        }
        // I2's one reset: a choice made for another project starts over.
        match &mut self.choice {
            Choice::User { project: made, .. }
            | Choice::Carried {
                project: made,
                source: CarrySource::Template,
                ..
            } => match *made {
                Some(made_for) if made_for != project => self.choice = Choice::Untouched,
                _ => *made = Some(project),
            },
            _ => {}
        }
        self.resolution = OracleResolution::Resolved(generation, project, list);
    }
    fn fail(&mut self, generation: u64) {
        if self.resolution == OracleResolution::Pending(generation) {
            self.resolution = OracleResolution::Failed(generation);
        }
    }
}

fn author() -> AccountId {
    AccountId::parse("synthetic-user").unwrap()
}
/// The `_Account_` value of the draft's request.
fn sent(editor: &Editor, cx: &Context<TempoView>) -> Result<Option<String>, String> {
    let draft = editor.draft(&author(), cx)?;
    let body: Value = serde_json::from_slice(&draft.encode().unwrap()).unwrap();
    Ok(body["attributes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|attribute| attribute["key"] == "_Account_")
        .and_then(|attribute| attribute["value"].as_str().map(str::to_owned)))
}
/// Whether the request carries `expected`. The caller's message names the
/// case only: neither value is formatted.
fn sends(editor: &Editor, cx: &Context<TempoView>, expected: Option<&str>) -> bool {
    sent(editor, cx).is_ok_and(|account| account.as_deref() == expected)
}

fn keys_of(selection: &buddy_core::tempo::BillingSelection) -> Vec<String> {
    selection
        .choices()
        .iter()
        .map(|choice| choice.account.key().to_owned())
        .collect()
}

/// I1-I5 and the request property, checked after every step. `trail` names
/// the seed, sequence, step and event kinds only; no message here formats
/// an account key, a list or the billing state.
fn check(editor: &Editor, oracle: &Oracle, cx: &Context<TempoView>, trail: &str) {
    let state = editor.billing_state();
    assert_eq!(state.generation(), oracle.generation, "generation. {trail}");
    let kind = match (state.resolution(), &oracle.resolution) {
        (Resolution::NotLookedUp { catalog }, OracleResolution::NotLookedUp(expected)) => {
            assert!(
                catalog.as_ref().map(|c| c.is_ok()) == expected.as_ref().map(Option::is_some),
                "I4: the catalog state differs. {trail}"
            );
            "not looked up"
        }
        (Resolution::Pending { generation }, OracleResolution::Pending(expected))
        | (Resolution::Failed { generation, .. }, OracleResolution::Failed(expected)) => {
            assert_eq!(generation, expected, "I1: generation. {trail}");
            "pending or failed"
        }
        (Resolution::Resolved { generation, .. }, OracleResolution::Resolved(expected, ..)) => {
            assert_eq!(generation, expected, "I1: generation. {trail}");
            "resolved"
        }
        _ => panic!("I1: the resolution differs from the oracle's. {trail}"),
    };
    // I2: the choice is exactly the oracle's.
    assert!(
        state.choice() == &oracle.choice,
        "I2: the choice differs from the oracle's ({kind}). {trail}"
    );
    // I4: the catalog is shown only before a lookup; the shown list is the
    // oracle's in every state.
    let shown = editor.billing().map(keys_of);
    let expected: Option<Vec<String>> = oracle
        .shown()
        .map(|list| list.iter().map(|(key, ..)| key.clone()).collect());
    assert!(
        shown == expected,
        "I4: the shown list differs from the oracle's ({kind}). {trail}"
    );
    // I1: a resolved lookup of the issue as typed exactly in Resolved.
    let resolved = matches!(oracle.resolution, OracleResolution::Resolved(..));
    assert_eq!(
        editor.has_current_issue(cx),
        resolved,
        "I1 ({kind}). {trail}"
    );
    let expected = oracle.request();
    // I5: a refused pick is reported exactly when the oracle expects one.
    assert_eq!(
        state.refused_pick().is_some(),
        matches!(expected, Expected::RefusedPick),
        "I5: refused pick ({kind}). {trail}"
    );
    // Save and Enter are blocked exactly when saving fails.
    let blocker = editor.save_blocker(cx);
    match (sent(editor, cx), expected) {
        (Err(error), Expected::NeedsLookup) => {
            assert!(
                error == LOOKUP_BEFORE_SAVING,
                "I1: the save error differs. {trail}"
            );
            assert!(
                blocker.as_deref() == Some(LOOKUP_BEFORE_SAVING),
                "I1: Save. {trail}"
            );
        }
        (Err(error), Expected::RefusedPick) => {
            let key = state.choice().key().unwrap_or_default();
            assert!(
                error != LOOKUP_BEFORE_SAVING && !key.is_empty() && error.starts_with(key),
                "I5: the save error does not name the refused pick. {trail}"
            );
            assert!(
                blocker.as_deref() == Some(error.as_str()),
                "I5: Save. {trail}"
            );
        }
        (Ok(account), Expected::Sends(expected)) => {
            assert!(blocker.is_none(), "I1/I5: Save is blocked. {trail}");
            // I3: the request from the current list and choice only.
            assert!(
                account == expected,
                "I3: the request differs from the oracle's. {trail}"
            );
            // Never a key the current list does not hold open, except an
            // edit's existing account.
            if let (Some(sent), OracleResolution::Resolved(_, _, list)) =
                (&account, &oracle.resolution)
            {
                assert!(
                    open_in(list, sent)
                        || oracle
                            .existing
                            .as_ref()
                            .is_some_and(|existing| existing.as_deref() == Some(sent.as_str())),
                    "I3: the request carries a key the list does not hold open. {trail}"
                );
            }
        }
        (sent, expected) => panic!(
            "saving {} while the oracle expects {}. {trail}",
            if sent.is_ok() { "succeeded" } else { "failed" },
            match expected {
                Expected::NeedsLookup => "I1 to block it",
                Expected::RefusedPick => "I5 to block it",
                Expected::Sends(_) => "a request",
            }
        ),
    }
}

/// Random event sequences through the real editor state, with I1-I4 and
/// the request property checked after every step.
#[gpui_kit::test]
fn random_billing_event_sequences_keep_every_invariant(cx: &mut gpui_kit::TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let mut view = TempoView::new(window, cx);
                let mut rng = Rng(SEED);
                let date = "2026-10-16".parse().unwrap();
                let mut steps = 0;
                for sequence in 0..SEQUENCES {
                    let mut trail = format!("seed {SEED:#x} sequence {sequence}:");
                    let mut step = 0;
                    // Start: a blank create, a template, or an edit.
                    let start = rng.below(3);
                    let (mut editor, mut oracle) = match start {
                        0 => (
                            Editor::new(None, date, next_start_time(0), window, cx),
                            Oracle {
                                generation: 0,
                                resolution: OracleResolution::NotLookedUp(None),
                                choice: Choice::Untouched,
                                existing: None,
                            },
                        ),
                        1 => {
                            let key = rng.key();
                            let mut row = worklog();
                            match key {
                                Some(key) => {
                                    row.attributes.as_mut().unwrap()[0].value = Some(key.into())
                                }
                                None => row
                                    .attributes
                                    .as_mut()
                                    .unwrap()
                                    .retain(|attribute| attribute.key != "_Account_"),
                            }
                            trail += " template";
                            (
                                Editor::from_template(&row, date, next_start_time(0), window, cx)
                                    .unwrap(),
                                Oracle {
                                    generation: 0,
                                    resolution: OracleResolution::NotLookedUp(None),
                                    choice: Choice::Carried {
                                        key: key.map(str::to_owned),
                                        source: CarrySource::Template,
                                        project: None,
                                    },
                                    existing: None,
                                },
                            )
                        }
                        _ => {
                            let existing = [Some(EXISTING), Some("SYNTH-A"), None][rng.below(3)];
                            let mut row = worklog();
                            match existing {
                                Some(key) => {
                                    row.attributes.as_mut().unwrap()[0].value = Some(key.into())
                                }
                                None => row
                                    .attributes
                                    .as_mut()
                                    .unwrap()
                                    .retain(|attribute| attribute.key != "_Account_"),
                            }
                            trail += " edit";
                            let editor =
                                Editor::new(Some(row), date, next_start_time(0), window, cx);
                            let existing = existing.map(str::to_owned);
                            (
                                editor,
                                Oracle {
                                    generation: 0,
                                    resolution: OracleResolution::NotLookedUp(None),
                                    choice: Choice::Carried {
                                        key: existing.clone(),
                                        source: CarrySource::Existing,
                                        project: None,
                                    },
                                    existing: Some(existing),
                                },
                            )
                        }
                    };
                    editor
                        .issue
                        .update(cx, |field, cx| field.set_value("19", window, cx));
                    // The fixture's 25 hours is not a duration a save accepts.
                    editor
                        .hours
                        .update(cx, |field, cx| field.set_value("1", window, cx));
                    if start == 0 {
                        trail += " create";
                    }
                    let edit = oracle.existing.is_some();
                    // Before its first lookup any create asks for every
                    // account; a template's lookup arrives with it at once.
                    assert_eq!(editor.wants_catalog(), start != 2, "{trail}");
                    check(&editor, &oracle, cx, &trail);
                    // A template or an edit arrives with its first lookup,
                    // through the same path as any other.
                    if start != 0 {
                        let (info, list) = lookup_response(&mut rng);
                        let project = info.billing.project;
                        editor.accept_lookup("19".into(), info, cx).unwrap();
                        oracle.generation += 1;
                        oracle.resolution = OracleResolution::Pending(oracle.generation);
                        oracle.resolve(oracle.generation, project, list);
                        trail += " first-lookup";
                        check(&editor, &oracle, cx, &trail);
                    }
                    let mut older_ids = Vec::new();
                    for _ in 0..rng.below(MAX_STEPS) + 1 {
                        steps += 1;
                        step += 1;
                        trail += &format!(" [{step}]");
                        // A response may answer the current lookup, an older
                        // one, or one never sent.
                        let pick_generation = |rng: &mut Rng, oracle: &Oracle| match rng.below(4) {
                            0 => rng.below(oracle.generation as usize + 1) as u64,
                            1 => oracle.generation + 1,
                            _ => oracle.generation,
                        };
                        match rng.below(11) {
                            0 => {
                                let (catalog, list) = catalog_response(&mut rng);
                                trail += " catalog-ok";
                                let accepted = editor.accept_catalog(editor.id, Ok(catalog));
                                let lands = !edit
                                    && matches!(
                                        oracle.resolution,
                                        OracleResolution::NotLookedUp(None | Some(None))
                                    );
                                if lands {
                                    oracle.resolution =
                                        OracleResolution::NotLookedUp(Some(Some(list)));
                                }
                                assert_eq!(accepted, lands, "{trail}");
                            }
                            1 => {
                                trail += " catalog-err";
                                assert!(!editor.accept_catalog(editor.id, Err("limited".into())));
                                if !edit
                                    && matches!(
                                        oracle.resolution,
                                        OracleResolution::NotLookedUp(None | Some(None))
                                    )
                                {
                                    oracle.resolution = OracleResolution::NotLookedUp(Some(None));
                                }
                            }
                            2 => {
                                trail += " stale-catalog";
                                if let Some(old) = older_ids.last() {
                                    assert!(!editor.accept_catalog(*old, Ok(catalog())), "{trail}");
                                }
                            }
                            3 => {
                                trail += " begin";
                                let generation = editor.begin_lookup();
                                oracle.generation += 1;
                                assert_eq!(generation, oracle.generation, "{trail}");
                                oracle.resolution = OracleResolution::Pending(generation);
                            }
                            4 => {
                                let generation = pick_generation(&mut rng, &oracle);
                                let (info, list) = lookup_response(&mut rng);
                                trail += &format!(" lookup-ok(g{generation})");
                                let project = info.billing.project;
                                editor
                                    .settle_lookup(generation, "19".into(), Ok(info), cx)
                                    .unwrap();
                                oracle.resolve(generation, project, list);
                            }
                            5 => {
                                let generation = pick_generation(&mut rng, &oracle);
                                trail += &format!(" lookup-err(g{generation})");
                                let _ = editor.settle_lookup(
                                    generation,
                                    "19".into(),
                                    Err("The issue lookup failed.".into()),
                                    cx,
                                );
                                oracle.fail(generation);
                            }
                            6 => {
                                // The view's own lookup path: begin, then
                                // settle with a response or a failure.
                                let fails = rng.chance(4);
                                trail += if fails {
                                    " view-lookup-err"
                                } else {
                                    " view-lookup-ok"
                                };
                                let id = editor.id;
                                view.editor = Some(editor);
                                view.begin(&Operation::Lookup(id, "19".into()), None);
                                oracle.generation += 1;
                                oracle.resolution = OracleResolution::Pending(oracle.generation);
                                if fails {
                                    view.settle(Err("The issue lookup failed.".into()), cx);
                                    oracle.fail(oracle.generation);
                                } else {
                                    let (info, list) = lookup_response(&mut rng);
                                    let project = info.billing.project;
                                    view.settle(
                                        Ok(actions::Update::Lookup(Box::new((
                                            id,
                                            "19".into(),
                                            info,
                                        )))),
                                        cx,
                                    );
                                    oracle.resolve(oracle.generation, project, list);
                                }
                                editor = view.editor.take().unwrap();
                            }
                            7 | 8 => {
                                let key = if edit && rng.chance(2) {
                                    oracle.existing.clone().flatten()
                                } else {
                                    rng.key().map(str::to_owned)
                                };
                                trail += if key.is_some() { " pick" } else { " pick-none" };
                                let result = editor.choose(key.as_deref());
                                let allowed = oracle
                                    .shown()
                                    .is_some_and(|list| oracle.admits(list, key.as_deref()));
                                assert_eq!(result.is_ok(), allowed, "pick admission. {trail}");
                                if allowed {
                                    oracle.choice = Choice::User {
                                        key,
                                        project: oracle.shown_project(),
                                    };
                                }
                            }
                            9 => {
                                if edit {
                                    continue;
                                }
                                let key = rng.key().map(str::to_owned);
                                trail += " carry";
                                editor
                                    .billing_state_mut()
                                    .carry(key.clone(), CarrySource::Template)
                                    .unwrap();
                                if !matches!(oracle.choice, Choice::User { .. }) {
                                    oracle.choice = Choice::Carried {
                                        key,
                                        source: CarrySource::Template,
                                        project: None,
                                    };
                                }
                            }
                            _ => {
                                trail += " recheck";
                                older_ids.push(editor.id);
                                editor.invalidate_connection();
                                oracle.resolution = OracleResolution::NotLookedUp(None);
                                assert_eq!(
                                    editor.wants_catalog(),
                                    !edit && oracle.generation == 0,
                                    "{trail}"
                                );
                            }
                        }
                        check(&editor, &oracle, cx, &trail);
                    }
                }
                assert!(steps > SEQUENCES * 10, "{steps} steps");
                view
            })
        })
        .unwrap();
    });
}

/// Round 4, finding 1. A project with no linked accounts lists every
/// account, and the person picks one. The project then links accounts that
/// exclude it, and the repeat lookup fails. Create must be disabled, and no
/// request may carry the pick; the next successful lookup decides. Since
/// round 5 (I5) that lookup, which refuses the pick, keeps Create disabled
/// until the person chooses again.
#[gpui_kit::test]
fn a_failed_repeat_lookup_cannot_save_a_pick_from_the_earlier_fallback_list(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let mut view = TempoView::new(window, cx);
                let date = "2026-10-16".parse().unwrap();
                let editor = Editor::new(None, date, next_start_time(0), window, cx);
                editor
                    .issue
                    .update(cx, |field, cx| field.set_value("19", window, cx));
                let id = editor.id;
                view.editor = Some(editor);
                // The first lookup: no links, so every account is listed.
                view.begin(&Operation::Lookup(id, "19".into()), None);
                view.settle(
                    Ok(actions::Update::Lookup(Box::new((
                        id,
                        "19".into(),
                        project_lookup(false),
                    )))),
                    cx,
                );
                let editor = view.editor.as_mut().unwrap();
                assert!(editor.billing().unwrap().lists_every_account());
                editor.choose(Some("SYNTH-UNLINKED")).unwrap();
                assert!(sends(editor, cx, Some("SYNTH-UNLINKED")));

                // The project now links only SYNTH-LINKED, but the repeat
                // lookup fails before it can say so.
                view.begin(&Operation::Lookup(id, "19".into()), None);
                view.settle(Err("Tempo is limiting requests.".into()), cx);
                assert_eq!(view.error.as_deref(), Some("Tempo is limiting requests."));
                let editor = view.editor.as_ref().unwrap();
                assert!(!editor.has_current_issue(cx), "Create must be disabled");
                assert!(editor.billing().is_none(), "the earlier list is gone");
                assert!(matches!(
                    editor.billing_state().resolution(),
                    Resolution::Failed { .. }
                ));
                assert!(sent(editor, cx).err().as_deref() == Some(LOOKUP_BEFORE_SAVING));
                // Enter refuses too.
                view.submit_editor_on_enter(view.editor.as_ref().unwrap().hours.entity_id(), cx);
                assert_eq!(view.error.as_deref(), Some(LOOKUP_BEFORE_SAVING));

                // A late answer to the failed lookup is ignored.
                view.settle(
                    Ok(actions::Update::Lookup(Box::new((
                        id,
                        "19".into(),
                        project_lookup(false),
                    )))),
                    cx,
                );
                assert!(!view.editor.as_ref().unwrap().has_current_issue(cx));

                // The next lookup succeeds and links only SYNTH-LINKED: the
                // pick stays the person's, and since that list refuses it,
                // Create stays disabled with the reason (I5).
                view.begin(&Operation::Lookup(id, "19".into()), None);
                view.settle(
                    Ok(actions::Update::Lookup(Box::new((
                        id,
                        "19".into(),
                        project_lookup(true),
                    )))),
                    cx,
                );
                let editor = view.editor.as_mut().unwrap();
                assert!(editor.has_current_issue(cx));
                assert!(editor.billing_state().choice().key() == Some("SYNTH-UNLINKED"));
                // Not `unwrap_err`: a panic must not print an account.
                let Err(blocked) = sent(editor, cx) else {
                    panic!("Create stays blocked")
                };
                assert!(
                    blocked
                        == "SYNTH-UNLINKED is not open for this issue (its project does not \
                            link it); choose another account or No billing account."
                );
                // Choosing again from this list saves it.
                editor.choose(Some("SYNTH-LINKED")).unwrap();
                assert!(sends(editor, cx, Some("SYNTH-LINKED")));
                view
            })
        })
        .unwrap();
    });
}

/// Round 4, finding 2. A template's first lookup arrives with it. After a
/// connection recheck, every account is listed (the round-4 code asked for
/// the catalog here), and the person picks another open account or clears
/// billing. The fresh lookup still lists the template's account open. The
/// request must carry the person's choice, not the template's.
#[gpui_kit::test]
fn a_template_recheck_never_overrides_the_persons_newer_choice(cx: &mut gpui_kit::TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let view = TempoView::new(window, cx);
                let date = "2026-11-05".parse().unwrap();
                // The template bills SYNTH-UNLINKED; the project links it and
                // SYNTH-LINKED (its default), both open.
                let template = "SYNTH-UNLINKED";
                for (case, (pick, expected)) in
                    [(Some("SYNTH-LINKED"), Some("SYNTH-LINKED")), (None, None)]
                        .into_iter()
                        .enumerate()
                {
                    for catalog_after_recheck in [true, false] {
                        let mut row = worklog();
                        row.attributes.as_mut().unwrap()[0].value = Some(template.into());
                        let mut editor =
                            Editor::from_template(&row, date, next_start_time(0), window, cx)
                                .unwrap();
                        // As render.rs applies the template's lookup.
                        editor
                            .issue
                            .update(cx, |field, cx| field.set_value("19", window, cx));
                        editor
                            .accept_lookup("19".into(), both_linked(Open), cx)
                            .unwrap();
                        assert!(sends(&editor, cx, Some(template)));

                        editor.invalidate_connection();
                        // The template was looked up, so no catalog is read.
                        assert!(!editor.wants_catalog());
                        let case = format!("case {case} catalog={catalog_after_recheck}");
                        if catalog_after_recheck {
                            // The round-4 path: the catalog lands anyway and
                            // the person picks from it.
                            assert!(editor.accept_catalog(editor.id, Ok(catalog())));
                            assert!(
                                editor.billing().unwrap().selected_key() == Some(template),
                                "{case}"
                            );
                            editor.choose(pick).unwrap();
                            editor
                                .accept_lookup("19".into(), both_linked(Open), cx)
                                .unwrap();
                        } else {
                            editor
                                .accept_lookup("19".into(), both_linked(Open), cx)
                                .unwrap();
                            editor.choose(pick).unwrap();
                            // A repeat lookup keeps it too.
                            editor
                                .accept_lookup("19".into(), both_linked(Open), cx)
                                .unwrap();
                        }
                        assert!(sends(&editor, cx, expected), "{case}");
                        // Carrying the template again changes nothing.
                        editor
                            .billing_state_mut()
                            .carry(Some(template.into()), CarrySource::Template)
                            .unwrap();
                        assert!(sends(&editor, cx, expected), "{case}");
                    }
                }
                view
            })
        })
        .unwrap();
    });
}

/// Round 5, finding 2. An edit bills SYNTH-A. The person filters the list to
/// SYNTH-B, which hides SYNTH-A, and chooses SYNTH-B. A repeat lookup then
/// finds SYNTH-B closed, or no longer linked. Update must be disabled with
/// a message naming SYNTH-B and why, never a silent save that keeps
/// SYNTH-A. Keeping SYNTH-A, or choosing another open account, saves again.
#[gpui_kit::test]
fn an_edit_whose_new_pick_closes_blocks_update_instead_of_keeping_the_original(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let mut view = TempoView::new(window, cx);
                let date = "2026-10-16".parse().unwrap();
                // SYNTH-A and SYNTH-B linked; `b` is SYNTH-B's status, `None`
                // for no longer linked. SYNTH-C stays open.
                let lookup = |b: Option<BillingAccountStatus>| {
                    let mut info = details();
                    for (key, status) in [("SYNTH-A", Some(Open)), ("SYNTH-B", b), ("SYNTH-C", Some(Open))] {
                        if let Some(status) = status {
                            info.billing.choices.push(ProjectBillingAccount {
                                account: account_with(account_id(key), key, status),
                                is_default: false,
                            });
                        }
                    }
                    info
                };
                for (case, (closed, why)) in [
                    (Some(Closed), "it is closed"),
                    (Some(Archived), "it is archived"),
                    (None, "its project does not link it"),
                ]
                .into_iter()
                .enumerate()
                {
                    let mut row = worklog();
                    row.attributes.as_mut().unwrap()[0].value = Some("SYNTH-A".into());
                    let mut editor = Editor::new(Some(row), date, next_start_time(0), window, cx);
                    editor
                        .hours
                        .update(cx, |field, cx| field.set_value("1", window, cx));
                    editor.accept_lookup("19".into(), lookup(Some(Open)), cx).unwrap();
                    let id = editor.id;
                    view.editor = Some(editor);
                    let editor = view.editor.as_mut().unwrap();
                    // The filter hides SYNTH-A; SYNTH-B is chosen.
                    editor
                        .billing_filter
                        .update(cx, |field, cx| field.set_value("SYNTH-B", window, cx));
                    editor.choose(Some("SYNTH-B")).unwrap();
                    assert!(sends(editor, cx, Some("SYNTH-B")), "case {case}");

                    // The repeat lookup, through the view's own path.
                    view.begin(&Operation::Lookup(id, "19".into()), None);
                    view.settle(
                        Ok(actions::Update::Lookup(Box::new((id, "19".into(), lookup(closed))))),
                        cx,
                    );
                    let message = format!(
                        "SYNTH-B is not open for this issue ({why}); choose another account or keep SYNTH-A."
                    );
                    let editor = view.editor.as_ref().unwrap();
                    assert!(editor.has_current_issue(cx), "case {case}: the lookup resolved");
                    assert!(editor.billing_state().choice().key() == Some("SYNTH-B"), "case {case}");
                    // Update is disabled with the reason, and a save refuses.
                    assert!(editor.save_blocker(cx) == Some(message.clone()), "case {case}");
                    assert!(sent(editor, cx).err() == Some(message.clone()), "case {case}");
                    // Enter says the same.
                    view.error = None;
                    view.submit_editor_on_enter(view.editor.as_ref().unwrap().hours.entity_id(), cx);
                    assert!(view.error.as_deref() == Some(message.as_str()), "case {case}");
                    assert!(!view.busy, "case {case}: nothing was sent");

                    // Keeping the original account (the editor's Keep button)
                    // or choosing another open account saves again.
                    let editor = view.editor.as_mut().unwrap();
                    editor.choose(Some("SYNTH-A")).unwrap();
                    assert!(editor.save_blocker(cx).is_none(), "case {case}");
                    assert!(sends(editor, cx, Some("SYNTH-A")), "case {case}");
                    editor.choose(Some("SYNTH-C")).unwrap();
                    assert!(sends(editor, cx, Some("SYNTH-C")), "case {case}");
                    view.editor = None;
                }

                // An untouched edit keeps its account, which the list closed
                // or unlinked: a carried value still falls back.
                let mut row = worklog();
                row.attributes.as_mut().unwrap()[0].value = Some("SYNTH-B".into());
                let mut editor = Editor::new(Some(row), date, next_start_time(0), window, cx);
                editor
                    .hours
                    .update(cx, |field, cx| field.set_value("1", window, cx));
                editor.accept_lookup("19".into(), lookup(Some(Closed)), cx).unwrap();
                assert!(editor.save_blocker(cx).is_none());
                assert!(sends(&editor, cx, Some("SYNTH-B")));
                view
            })
        })
        .unwrap();
    });
}

/// Round 5, finding 2, for a create. A pick that a repeat lookup closes
/// blocks Create with the reason (round 4 fell back and only said so). A
/// carried template value that the list refuses still falls back.
#[gpui_kit::test]
fn a_create_whose_pick_closes_blocks_create_until_the_person_chooses_again(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let view = TempoView::new(window, cx);
                let date = "2026-10-16".parse().unwrap();
                let mut editor = Editor::new(None, date, next_start_time(0), window, cx);
                editor
                    .issue
                    .update(cx, |field, cx| field.set_value("19", window, cx));
                editor
                    .hours
                    .update(cx, |field, cx| field.set_value("1", window, cx));
                // SYNTH-LINKED is the open default; SYNTH-UNLINKED is open.
                editor
                    .accept_lookup("19".into(), both_linked(Open), cx)
                    .unwrap();
                editor.choose(Some("SYNTH-UNLINKED")).unwrap();
                assert!(sends(&editor, cx, Some("SYNTH-UNLINKED")));
                editor
                    .accept_lookup("19".into(), both_linked(Closed), cx)
                    .unwrap();
                let message = "SYNTH-UNLINKED is not open for this issue (it is closed); \
                    choose another account or No billing account.";
                assert!(editor.save_blocker(cx).as_deref() == Some(message));
                assert!(sent(&editor, cx).err().as_deref() == Some(message));
                // "No billing account" saves without an account.
                editor.choose(None).unwrap();
                assert!(sends(&editor, cx, None));

                // A template's account, refused the same way, is carried, not
                // chosen: it falls back to the open default.
                let mut row = worklog();
                row.attributes.as_mut().unwrap()[0].value = Some("SYNTH-UNLINKED".into());
                let mut editor =
                    Editor::from_template(&row, date, next_start_time(0), window, cx).unwrap();
                editor
                    .issue
                    .update(cx, |field, cx| field.set_value("19", window, cx));
                editor
                    .accept_lookup("19".into(), both_linked(Closed), cx)
                    .unwrap();
                assert!(editor.save_blocker(cx).is_none());
                assert!(sends(&editor, cx, Some("SYNTH-LINKED")));
                view
            })
        })
        .unwrap();
    });
}
