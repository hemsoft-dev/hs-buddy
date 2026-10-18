use super::{Operation, actions::Update};
use buddy_core::tempo::{
    Client, JiraClient, Session,
    credentials::{self, Credentials},
    fixture,
};
use chrono::{DateTime, Utc};
use std::path::PathBuf;
use std::sync::Arc;

/// Retaining a draft may replace credentials, never its account or calendar.
/// Account/site/profile values deliberately have no Debug or serialization.
#[derive(Clone)]
pub(super) struct Binding {
    tempo_origin: buddy_core::tempo::ApiOrigin,
    account: buddy_core::tempo::AccountId,
    jira_origin: String,
    profile: String,
    time_zone: Option<String>,
    range: buddy_core::tempo::DateRange,
}
impl Binding {
    fn for_session(session: &Session) -> Self {
        let data = session.snapshot();
        Self {
            tempo_origin: session.tempo_origin(),
            account: data.account.clone(),
            jira_origin: session.jira_origin().to_owned(),
            profile: data.configuration_profile().to_owned(),
            time_zone: data.time_zone.clone(),
            range: data.range,
        }
    }

    fn retained_range(
        previous: Option<&Self>,
        candidate: &Self,
        retaining_draft: bool,
    ) -> Result<Option<buddy_core::tempo::DateRange>, String> {
        let same = previous.is_some_and(|old| {
            old.tempo_origin == candidate.tempo_origin
                && old.account == candidate.account
                && old.jira_origin == candidate.jira_origin
                && old.profile == candidate.profile
                && old.time_zone == candidate.time_zone
        });
        if retaining_draft && !same {
            return Err("The open draft belongs to its original Jira account, site, profile and timezone. Reconnect that account or explicitly cancel the draft before changing them.".into());
        }
        Ok(previous.filter(|_| same).map(|old| old.range))
    }
}

/// The same captured credentials create both clients and authorize the save.
/// No input, secret or provider response is written to tracing.
pub(super) struct Connection {
    pub session: Session,
    pub metadata: Arc<super::metadata::Metadata>,
    pub templates: Option<Arc<buddy_core::tempo::Templates>>,
    fixed_now: Option<DateTime<Utc>>,
}
impl Connection {
    pub fn binding(&self) -> Binding {
        Binding::for_session(&self.session)
    }

    pub fn now(&self) -> DateTime<Utc> {
        self.fixed_now.unwrap_or_else(Utc::now)
    }
}
pub(super) async fn connect(
    config: PathBuf,
    entered: Option<Credentials>,
    previous: Option<Binding>,
    retaining_draft: bool,
) -> Result<Option<Connection>, String> {
    let save = entered.is_some();
    let value = match entered {
        Some(value) => value,
        None => match credentials::load_at(&config).map_err(|e| e.to_string())? {
            Some(value) => value,
            None => return Ok(None),
        },
    };
    let (client, jira, fixed_now) = if value.tempo_token() == fixture::TEMPO_TOKEN {
        let owned = fixture::connect(&value, &config).map_err(|e| e.to_string())?;
        (owned.tempo, owned.jira, Some(owned.now))
    } else {
        (
            Client::new_in_region(value.tempo_origin(), value.tempo_token())
                .map_err(|e| e.to_string())?,
            JiraClient::new(value.jira_origin(), value.jira_email(), value.jira_token())
                .map_err(|e| e.to_string())?,
            None,
        )
    };
    let mut session = Session::connect_current_month_at(
        &config,
        Arc::new(client),
        Arc::new(jira),
        fixed_now.unwrap_or_else(Utc::now),
    )
    .await
    .map_err(|e| e.to_string())?;
    // Refuse identity changes and finish the retained range read BEFORE any
    // protected-store replacement. The caller retains the old connection.
    if let Some(range) = Binding::retained_range(
        previous.as_ref(),
        &Binding::for_session(&session),
        retaining_draft,
    )? && (range.start != session.snapshot().range.start
        || range.end != session.snapshot().range.end)
    {
        session
            .refresh(range, fixed_now.unwrap_or_else(Utc::now))
            .await
            .map_err(|e| e.to_string())?;
    }
    Binding::retained_range(
        previous.as_ref(),
        &Binding::for_session(&session),
        retaining_draft,
    )
    .map(|_| ())?;
    let metadata = Arc::new(super::metadata::Metadata::load(&session).await);
    if save {
        let expected = session.credential_checkpoint().map_err(|e| e.to_string())?;
        credentials::save_preserving_recovery_at(&config, &value, expected)
            .map_err(|e| e.to_string())?;
    }
    Ok(Some(Connection {
        session,
        metadata,
        templates: None,
        fixed_now,
    }))
}

pub(super) async fn apply(
    connection: &mut Connection,
    operation: Operation,
) -> Result<Update, String> {
    let now = connection.now();
    let reload_metadata = matches!(
        &operation,
        Operation::Refresh | Operation::Today | Operation::Range(_) | Operation::Month(_)
    );
    let message=match operation {
        Operation::Refresh=>{let range=connection.session.snapshot().range;connection.session.refresh(range,now).await.map(|_|"Timesheet refreshed.")},
        Operation::Today=>connection.session.refresh_current_month_at(now).await.map(|_|"Loaded the current account month."),
        Operation::Range(range)=>connection.session.refresh(range,now).await.map(|_|"Date range loaded."),
        Operation::Month(delta)=>match month_navigation_range(connection.session.snapshot().range,delta) {
            Ok(range)=>connection.session.refresh(range,now).await.map(|_|"Month loaded."),Err(_)=>return Err("Use Today or load a whole month before using month navigation.".into()),
        },
        Operation::Cleanup=>connection.session.cleanup().map(|_|"Recovery cleanup finished. Refresh before preparing another change."),
        Operation::Discard=>connection.session.discard().map(|_|"Local recovery discarded. The remote change may already exist. Refresh before preparing another change."),
        Operation::Templates=>{
            let templates=Arc::new(connection.session.previous_month_templates(now).await.map_err(|e|e.to_string())?);
            let message=format!("Loaded {} issue templates from {} to {}. Select an empty cell to prepare a new worklog. No worklogs were saved.",templates.rows().len(),templates.range().start,templates.range().end);
            connection.metadata=Arc::new(super::metadata::Metadata::load_with_templates(&connection.session,Some(&templates)).await);
            connection.templates=Some(templates);
            return Ok(Update::Message(message));
        },
        Operation::Template(id,date)=>{
            let templates=connection.templates.as_ref().ok_or("Load last month's issues again before using a template.")?;
            let row=connection.session.expand_template(templates,id).await.map_err(|e|e.to_string())?;
            let issue=row.issue.ok_or("The template's issue is unavailable.")?;
            let info=connection.session.editor_issue(&issue.get().to_string()).await.map_err(|e|e.to_string())?;
            return Ok(Update::TemplateEditor(Box::new((row,info,date))));
        },
        Operation::Editor(id)=>{
            let row=connection.session.worklog_for_edit(id).await.map_err(|e|e.to_string())?;
            let issue=row.issue.ok_or("The worklog has no available issue. Its original data was retained.")?;
            let info=connection.session.editor_issue(&issue.get().to_string()).await.map_err(|e|e.to_string())?;
            return Ok(Update::Editor(Box::new((row,info))));
        },
        Operation::Lookup(id,query)=>{
            let info=connection.session.editor_issue(&query).await.map_err(|e|e.to_string())?;
            return Ok(Update::Lookup(Box::new((id,query,info))));
        },
        Operation::PrepareCopy(ids)=>{
            let plan=connection.session.prepare_copy(ids,now).await.map_err(|e|e.to_string())?;
            return Ok(Update::CopyReady(Box::new(plan)));
        },
        Operation::ExecuteCopy(plan)=>{
            let report=connection.session.execute_copy(*plan,now).await;
            let (message,problem)=super::copy::copy_message(&report);
            connection.templates=None;
            connection.metadata=Arc::new(super::metadata::Metadata::load(&connection.session).await);
            return Ok(Update::Wrote{confirmed:false,message,problem});
        },
        Operation::Submit(draft)=>{
            let kind=draft.kind();let date=draft.work_date();
            let report=connection.session.submit(*draft).await.map_err(|e|e.to_string())?;
            let confirmed=report.phase==buddy_core::tempo::WritePhase::Confirmed;
            let message=match report.phase {
                buddy_core::tempo::WritePhase::Confirmed=>"Tempo confirmed the worklog change.",
                buddy_core::tempo::WritePhase::Rejected=>"Tempo rejected the worklog change. The entered draft was retained.",
                _=>"The worklog result is unconfirmed. Keep this draft, inspect Tempo, and do not resubmit it.",
            }.to_owned();
            let mut problems=Vec::new();
            if let Some(error)=report.transport_problem {problems.push(error.to_string());}
            if let Some(error)=report.cleanup_problem {problems.push(format!("The service result requires local recovery cleanup. {error}"));}
            connection.templates=None;
            let current=connection.session.snapshot().range;
            let range=match range_after_write(current,confirmed,kind,date) {
                Ok(range)=>range,
                Err(error)=>{problems.push(format!("The service outcome is known, but the work date could not be displayed. {error}"));current},
            };
            match connection.session.refresh(range,connection.now()).await {
                Ok(())=>connection.metadata=Arc::new(super::metadata::Metadata::load(&connection.session).await),
                Err(error)=>problems.push(format!("The displayed range could not refresh. {error}")),
            }
            return Ok(Update::Wrote {confirmed,message,problem:(!problems.is_empty()).then(||problems.join(" "))});
        },
    }.map_err(|e|e.to_string())?;
    if reload_metadata {
        connection.templates = None;
        connection.metadata = Arc::new(super::metadata::Metadata::load(&connection.session).await);
    }
    Ok(Update::Message(message.to_owned()))
}

#[cfg(test)]
mod binding_tests {
    use super::*;

    fn original() -> Binding {
        Binding {
            tempo_origin: buddy_core::tempo::ApiOrigin::Global,
            account: buddy_core::tempo::AccountId::parse("synthetic-author").unwrap(),
            jira_origin: "https://synthetic.atlassian.net".into(),
            profile: "synthetic-profile-one".into(),
            time_zone: Some("America/New_York".into()),
            range: buddy_core::tempo::DateRange::new(
                "2026-09-05".parse().unwrap(),
                "2026-09-19".parse().unwrap(),
            )
            .unwrap(),
        }
    }

    #[test]
    fn draft_reauthentication_retains_its_custom_range_and_refuses_identity_or_calendar_changes() {
        let old = original();
        let mut current = original();
        current.range =
            buddy_core::tempo::DateRange::month_containing("2026-10-17".parse().unwrap()).unwrap();
        let range = Binding::retained_range(Some(&old), &current, true)
            .unwrap()
            .unwrap();
        assert_eq!(range.start.to_string(), "2026-09-05");
        assert_eq!(range.end.to_string(), "2026-09-19");

        let mut other = current.clone();
        other.tempo_origin = buddy_core::tempo::ApiOrigin::Europe;
        assert!(Binding::retained_range(Some(&old), &other, true).is_err());
        other = current.clone();
        other.account = buddy_core::tempo::AccountId::parse("another-author").unwrap();
        assert!(Binding::retained_range(Some(&old), &other, true).is_err());
        other = current.clone();
        other.jira_origin = "https://another.atlassian.net".into();
        assert!(Binding::retained_range(Some(&old), &other, true).is_err());
        other = current.clone();
        other.profile = "another-profile".into();
        assert!(Binding::retained_range(Some(&old), &other, true).is_err());
        other = current.clone();
        other.time_zone = Some("UTC".into());
        assert!(Binding::retained_range(Some(&old), &other, true).is_err());
        assert!(Binding::retained_range(None, &current, true).is_err());
        assert!(
            Binding::retained_range(Some(&old), &other, false)
                .unwrap()
                .is_none()
        );
        assert_eq!(old.range.start.to_string(), "2026-09-05");
        assert_eq!(old.range.end.to_string(), "2026-09-19");
    }
}

fn month_navigation_range(
    current: buddy_core::tempo::DateRange,
    delta: i32,
) -> Result<buddy_core::tempo::DateRange, buddy_core::tempo::Error> {
    current.shift_month(delta)
}

fn range_after_write(
    current: buddy_core::tempo::DateRange,
    confirmed: bool,
    kind: buddy_core::tempo::DraftKind,
    date: chrono::NaiveDate,
) -> Result<buddy_core::tempo::DateRange, buddy_core::tempo::Error> {
    if confirmed && kind != buddy_core::tempo::DraftKind::Delete && !current.contains(date) {
        buddy_core::tempo::DateRange::month_containing(date)
    } else {
        Ok(current)
    }
}

#[cfg(test)]
mod range_tests {
    use super::*;
    use buddy_core::tempo::{DateRange, DraftKind};
    #[test]
    fn review_regression_month_navigation_does_not_replace_a_custom_range() {
        let current =
            DateRange::new("2026-10-05".parse().unwrap(), "2026-10-19".parse().unwrap()).unwrap();
        for delta in [-1, 1] {
            assert!(matches!(
                month_navigation_range(current, delta),
                Err(buddy_core::tempo::Error::InvalidRange)
            ));
        }
        assert_eq!(current.start.to_string(), "2026-10-05");
        assert_eq!(current.end.to_string(), "2026-10-19");
        let month = DateRange::month_containing(current.start).unwrap();
        assert_eq!(
            month_navigation_range(month, 1).unwrap().start.to_string(),
            "2026-11-01"
        );
    }
    #[test]
    fn confirmed_create_and_moved_edit_show_the_work_date_without_losing_other_selected_ranges() {
        let current =
            DateRange::new("2026-10-05".parse().unwrap(), "2026-10-19".parse().unwrap()).unwrap();
        let outside = "2026-11-05".parse().unwrap();
        for kind in [DraftKind::Create, DraftKind::Update] {
            let range = range_after_write(current, true, kind, outside).unwrap();
            assert_eq!(range.start.to_string(), "2026-11-01");
            assert_eq!(range.end.to_string(), "2026-11-30");
        }
        for (confirmed, kind, date) in [
            (false, DraftKind::Update, outside),
            (true, DraftKind::Delete, outside),
            (true, DraftKind::Create, "2026-10-12".parse().unwrap()),
        ] {
            let range = range_after_write(current, confirmed, kind, date).unwrap();
            assert_eq!(range.start.to_string(), "2026-10-05");
            assert_eq!(range.end.to_string(), "2026-10-19");
        }
    }
}
