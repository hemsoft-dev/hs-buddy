//! Qualification-only contract probe. Never runs against the production API.
use buddy_core::github::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("BUDDY_GITHUB_FIXTURE").as_deref() != Ok("1")
        || std::env::var_os("BUDDY_GITHUB_FIXTURE_URL").is_none()
    {
        return Err("An explicit owned GitHub fixture is required".into());
    }
    let clients = GitHubClients::configured();
    let client = clients.for_account(AccountScope::new("fixture-one", "fixture-org")?);
    let repo = RepositoryRef::parse("fixture-org/native-demo")?;
    let pull = PullRef::new(repo.clone(), 7)?;
    for mode in PrMode::ALL {
        let result = client.pull_requests(mode, 7, true).await?;
        assert!(!result.items.is_empty());
    }
    assert!(
        client
            .pull_requests(PrMode::MyPrs, 7, true)
            .await?
            .items
            .iter()
            .any(|pull| pull.target.number == 8)
    );
    client.repositories(None, true).await?;
    client.organization_overview(true).await?;
    client.organization_members(None, true).await?;
    client.organization_teams(None, true).await?;
    client.team("native-team", true).await?;
    client.team_members("native-team", None, true).await?;
    client.repository_detail(&repo, true).await?;
    client
        .repository_pull_requests(&repo, false, None, true)
        .await?;
    client.repository_issues(&repo, false, None, true).await?;
    client.issue(&repo, 11, true).await?;
    client.issue_comments(&repo, 11, None, true).await?;
    let summary = client.pull(&pull, true).await?;
    client.pull_comments(&pull, None, true).await?;
    client.pull_reviews(&pull, None, true).await?;
    client.pull_commits(&pull, None, true).await?;
    client.pull_checks(&pull, &summary.head_sha, true).await?;
    client.pull_history(&pull, None, true).await?;
    client.pull_files(&pull, None, true).await?;
    let threads = client.pull_threads(&pull, None, true).await?;
    client
        .thread_comments(&threads.items[0].reference, None, true)
        .await?;
    let commits = client.repository_commits(&repo, None, true).await?;
    client.commit(&repo, &commits.items[0].sha, true).await?;
    client
        .commit_files(&repo, &commits.items[0].sha, None, true)
        .await?;
    client.user_profile("fixture-author", true).await?;
    client.user_contributions("fixture-author", true).await?;
    client.user_org_role("fixture-author", true).await?;
    client.user_teams("fixture-author", None, true).await?;
    client
        .user_pull_requests("fixture-author", false, true)
        .await?;
    client.user_events("fixture-author", None, true).await?;
    let mut selection = SelectionState::default();
    selection.select(Some(client.account.clone()), pull.key());
    let watch = client
        .review_checkpoint(&pull, ReviewKind::CodeRabbit)
        .await?;
    let pending = PendingWrite::new(
        &selection,
        WriteAction::RequestCodeRabbit { pull: pull.clone() },
    )?;
    client.execute(pending.confirm(&selection)?).await?;
    assert!(matches!(
        watch.poll(&client).await?,
        ReviewProgress::Completed { .. }
    ));
    let second = clients.for_account(AccountScope::new("fixture-two", "fixture-org")?);
    assert!(
        second
            .pull(&pull, true)
            .await?
            .title
            .starts_with("Second private")
    );
    assert!(
        client
            .pull(&pull, false)
            .await?
            .title
            .starts_with("First private")
    );
    assert!(matches!(
        client
            .repository(&RepositoryRef::parse("fixture-org/denied")?, true)
            .await,
        Err(GitHubError::Denied)
    ));
    assert!(matches!(
        client
            .repository(&RepositoryRef::parse("fixture-org/limited")?, true)
            .await,
        Err(GitHubError::RateLimited { .. })
    ));
    println!(
        "All synthetic GitHub provider contracts, account isolation, pagination, review monitoring, and one confirmed fixture write passed."
    );
    Ok(())
}
