//! Typed navigation and account-scoped loading; no GPUI work on the I/O thread.
use buddy_core::github::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Pulls(PrMode),
    Repositories,
    Organization,
    Members,
    Teams,
    Team(String),
    Repository(RepositoryRef, RepoTab),
    Pull(PullRef, PullTab),
    Thread(ThreadRefKey),
    Issue(RepositoryRef, u64),
    Commit(RepositoryRef, String),
    User(String, UserTab),
}

// A thread route carries the bound handle, but equality compares its identity.
#[derive(Debug, Clone)]
pub struct ThreadRefKey(pub ThreadRef);
impl PartialEq for ThreadRefKey {
    fn eq(&self, other: &Self) -> bool {
        self.0.id() == other.0.id() && self.0.pull() == other.0.pull()
    }
}
impl Eq for ThreadRefKey {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoTab {
    Overview,
    Pulls,
    ClosedPulls,
    Issues,
    ClosedIssues,
    Commits,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullTab {
    Conversation,
    Reviews,
    Commits,
    Checks,
    Files,
    Threads,
    History,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserTab {
    Overview,
    Authored,
    Reviewed,
    Activity,
    Teams,
}

impl Route {
    pub fn key(&self) -> String {
        match self {
            Self::Pull(pull, tab) => format!("{}:{tab:?}", pull.key()),
            Self::Thread(thread) => format!("{}:thread:{}", thread.0.pull().key(), thread.0.id()),
            _ => format!("{self:?}"),
        }
    }
}

#[derive(Clone)]
pub struct Row {
    pub title: String,
    pub detail: String,
    pub body: Option<String>,
    pub route: Option<Route>,
    pub author: Option<String>,
    pub url: Option<String>,
    pub comment: Option<CommentRef>,
    pub thread: Option<ThreadRef>,
    pub resolved: bool,
}
impl Row {
    pub fn text(title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            detail: detail.into(),
            body: None,
            route: None,
            author: None,
            url: None,
            comment: None,
            thread: None,
            resolved: false,
        }
    }
}

#[derive(Default)]
pub struct PageData {
    pub title: String,
    pub description: Option<String>,
    pub rows: Vec<Row>,
    pub next: Option<PageCursor>,
    pub warnings: Vec<String>,
    pub pull: Option<PullSummary>,
    pub coderabbit: Option<bool>,
    pub contribution_days: Vec<ContributionDay>,
}
impl PageData {
    fn paged<T>(&mut self, page: Page<T>, map: impl Fn(T) -> Row) {
        self.next = page.next;
        self.rows.extend(page.items.into_iter().map(map));
        if page.truncated {
            self.warnings.push(
                "GitHub's page limit was reached. Open GitHub for the remaining items.".into(),
            );
        }
    }
}

fn pull_row(pull: PullSummary) -> Row {
    let timestamp = pull
        .merged_at
        .as_deref()
        .map(|date| format!("merged {date}"))
        .unwrap_or_else(|| format!("updated {}", pull.updated_at));
    let mut row = Row::text(
        format!("#{} {}", pull.target.number, pull.title),
        format!(
            "{} · {} · @{} · {} → {} · {}",
            pull.target.repository.full_name(),
            pull.state,
            pull.author,
            pull.head_branch,
            pull.base_branch,
            timestamp
        ),
    );
    row.author = Some(pull.author);
    row.url = Some(pull.target.web_url());
    row.route = Some(Route::Pull(pull.target, PullTab::Conversation));
    row
}
fn repository_row(repo: RepositorySummary) -> Row {
    let mut row = Row::text(
        repo.full_name,
        format!(
            "{} · {} · {} stars{}",
            repo.visibility,
            repo.language.as_deref().unwrap_or("No primary language"),
            repo.stars,
            if repo.archived { " · archived" } else { "" }
        ),
    );
    row.body = repo.description;
    row.url = Some(repo.target.web_url());
    row.route = Some(Route::Repository(repo.target, RepoTab::Overview));
    row
}
fn user_row(user: UserSummary) -> Row {
    let mut row = Row::text(user.login.clone(), user.name.unwrap_or(user.kind));
    row.url = user.url;
    row.route = Some(Route::User(user.login, UserTab::Overview));
    row
}
fn commit_row(commit: Commit) -> Row {
    let mut message = commit.message.lines();
    let mut row = Row::text(
        message.next().unwrap_or("").to_string(),
        format!("{} · {} · {}", commit.sha, commit.author, commit.date),
    );
    let body = message.collect::<Vec<_>>().join("\n");
    if !body.trim().is_empty() {
        row.body = Some(body.trim_matches('\n').to_string());
    }
    row.url = commit.url;
    row.route = Some(Route::Commit(commit.repository, commit.sha));
    row
}
fn comment_row(comment: Comment) -> Row {
    let mut row = Row::text(
        comment.author.clone(),
        format!("{} · updated {}", comment.created_at, comment.updated_at),
    );
    row.author = Some(comment.author);
    row.body = Some(match comment.diff_hunk {
        Some(hunk) => format!("{hunk}\n\n{}", comment.body),
        None => comment.body,
    });
    row.url = comment.url;
    row.detail.push_str(
        &comment
            .reactions
            .iter()
            .filter(|(_, count)| *count > 0)
            .map(|(name, count)| format!(" · {name} {count}"))
            .collect::<String>(),
    );
    row.comment = Some(comment.reference);
    row
}
fn file_row(file: FileDiff) -> Row {
    let mut row = Row::text(
        file.filename,
        format!(
            "{} · +{} −{}{}",
            file.status,
            file.additions,
            file.deletions,
            file.previous_filename
                .map(|name| format!(" · renamed from {name}"))
                .unwrap_or_default()
        ),
    );
    row.url = file.url;
    row.body = Some(file.patch.unwrap_or_else(|| {
        "Patch unavailable for this binary or large file. Open it on GitHub.".into()
    }));
    row
}

pub async fn load(
    client: GitHubClient,
    route: Route,
    cursor: Option<PageCursor>,
    force: bool,
    merged_days: u32,
) -> Result<PageData, GitHubError> {
    let mut data = PageData::default();
    let after = cursor.as_ref();
    match route {
        Route::Pulls(mode) => {
            data.title = mode.label().into();
            let result = client.pull_requests(mode, merged_days, force).await?;
            data.rows = result.items.into_iter().map(pull_row).collect();
            data.warnings = result
                .warnings
                .into_iter()
                .map(|error| error.to_string())
                .collect();
            if result.truncated {
                data.warnings.push("Search is limited to 1,000 results. Narrow the search on GitHub for older items.".into());
            }
        }
        Route::Repositories => {
            data.title = "Repositories".into();
            data.paged(client.repositories(after, force).await?, repository_row);
        }
        Route::Organization => {
            data.title = client.account.organization().into();
            let org = client.organization_overview(force).await?;
            data.description = Some(format!(
                "{} repositories · {} private or internal · {} archived · {} stars · {} forks · {} open issues and PRs · last push {}",
                org.repositories.len(),
                org.private_repositories,
                org.archived_repositories,
                org.stars,
                org.forks,
                org.open_items,
                org.last_push.as_deref().unwrap_or("unknown")
            ));
            data.rows = org.repositories.into_iter().map(repository_row).collect();
            if org.partial {
                data.warnings.push(
                    "Repository totals are partial. Open GitHub for the complete organization."
                        .into(),
                );
            }
        }
        Route::Members => {
            data.title = "Organization members".into();
            data.paged(client.organization_members(after, force).await?, user_row);
        }
        Route::Teams => {
            data.title = "Teams".into();
            data.paged(client.organization_teams(after, force).await?, |team| {
                let mut row = Row::text(team.name, team.description.unwrap_or_default());
                row.url = team.url;
                row.route = Some(Route::Team(team.slug));
                row
            });
        }
        Route::Team(slug) => {
            let team = client.team(&slug, force).await?;
            data.title = team.name;
            data.description = team.description;
            data.paged(client.team_members(&slug, after, force).await?, user_row);
        }
        Route::Repository(repo, tab) => {
            data.title = repo.full_name();
            match tab {
                RepoTab::Overview => {
                    let detail = client.repository_detail(&repo, force).await?;
                    let summary = detail.repository;
                    data.description = summary.description;
                    data.warnings = detail.warnings;
                    data.rows.push(Row::text("Repository",format!("{} · branch {} · {} stars · {} forks · {} watchers · {} open issues · {} open PRs",summary.visibility,summary.default_branch.as_deref().unwrap_or("none"),summary.stars,summary.forks,summary.watchers.map(|n|n.to_string()).unwrap_or("unavailable".into()),detail.open_issues.map(|n|n.to_string()).unwrap_or("unavailable".into()),detail.open_prs.map(|n|n.to_string()).unwrap_or("unavailable".into()))));
                    data.rows.push(Row::text(
                        "Languages",
                        detail
                            .languages
                            .into_iter()
                            .map(|(name, bytes)| format!("{name}: {bytes} bytes"))
                            .collect::<Vec<_>>()
                            .join(" · "),
                    ));
                    if let Some(workflow) = detail.latest_workflow {
                        let mut row = Row::text(
                            workflow.name,
                            format!(
                                "{} · {} · {}",
                                workflow.status,
                                workflow.conclusion.unwrap_or("pending".into()),
                                workflow.created_at
                            ),
                        );
                        row.url = workflow.url;
                        data.rows.push(row);
                    }
                    for contributor in detail.contributors {
                        let mut row = user_row(contributor.user);
                        row.detail = format!("{} contributions", contributor.contributions);
                        data.rows.push(row);
                    }
                    data.rows
                        .extend(detail.recent_commits.into_iter().map(commit_row));
                    if let Some(url) = summary.homepage {
                        let mut row = Row::text("Homepage", url.clone());
                        row.url = Some(url);
                        data.rows.push(row);
                    }
                }
                RepoTab::Pulls | RepoTab::ClosedPulls => data.paged(
                    client
                        .repository_pull_requests(&repo, tab == RepoTab::ClosedPulls, after, force)
                        .await?,
                    pull_row,
                ),
                RepoTab::Issues | RepoTab::ClosedIssues => data.paged(
                    client
                        .repository_issues(&repo, tab == RepoTab::ClosedIssues, after, force)
                        .await?,
                    |issue| {
                        let mut row = Row::text(
                            format!("#{} {}", issue.number, issue.title),
                            format!(
                                "{} · {} · {} comments · {}",
                                issue.state,
                                issue.author,
                                issue.comment_count,
                                issue.labels.join(", ")
                            ),
                        );
                        row.author = Some(issue.author);
                        row.route = Some(Route::Issue(repo.clone(), issue.number));
                        row.url = Some(format!("{}/issues/{}", repo.web_url(), issue.number));
                        row
                    },
                ),
                RepoTab::Commits => data.paged(
                    client.repository_commits(&repo, after, force).await?,
                    commit_row,
                ),
            }
        }
        Route::Pull(pull, tab) => {
            let summary = client.pull(&pull, force).await?;
            data.title = format!("#{} {}", pull.number, summary.title);
            data.description = summary.body.clone();
            data.coderabbit = client
                .repository_has_coderabbit(&pull.repository, force)
                .await
                .ok();
            match tab {
                PullTab::Conversation => data.paged(
                    client.pull_comments(&pull, after, force).await?,
                    comment_row,
                ),
                PullTab::Reviews => {
                    data.paged(client.pull_reviews(&pull, after, force).await?, |review| {
                        let mut row = Row::text(
                            review.author.clone(),
                            format!(
                                "{} · {}",
                                review.state,
                                review.submitted_at.unwrap_or_default()
                            ),
                        );
                        row.author = Some(review.author);
                        row.body = Some(review.body);
                        row.url = review.url;
                        row
                    })
                }
                PullTab::Commits => {
                    data.paged(client.pull_commits(&pull, after, force).await?, commit_row)
                }
                PullTab::Files => {
                    data.paged(client.pull_files(&pull, after, force).await?, file_row)
                }
                PullTab::Threads => {
                    data.paged(client.pull_threads(&pull, after, force).await?, |thread| {
                        let mut row = Row::text(
                            thread.path.clone().unwrap_or("Review thread".into()),
                            format!(
                                "{} · line {}{}",
                                if thread.resolved {
                                    "Resolved"
                                } else {
                                    "Unresolved"
                                },
                                thread
                                    .line
                                    .map(|n| n.to_string())
                                    .unwrap_or("unknown".into()),
                                if thread.outdated { " · outdated" } else { "" }
                            ),
                        );
                        row.body = Some(
                            thread
                                .comments
                                .items
                                .iter()
                                .map(|comment| format!("{}: {}", comment.author, comment.body))
                                .collect::<Vec<_>>()
                                .join("\n\n"),
                        );
                        if thread.comments.next.is_some() {
                            row.detail.push_str(
                                " · more comments available; open the thread to continue",
                            );
                        }
                        row.route = Some(Route::Thread(ThreadRefKey(thread.reference.clone())));
                        row.thread = Some(thread.reference);
                        row.resolved = thread.resolved;
                        row
                    })
                }
                PullTab::Checks => {
                    let checks = client.pull_checks(&pull, &summary.head_sha, force).await?;
                    data.rows.push(Row::text(
                        format!("Checks: {}", checks.state()),
                        format!("Head {}", checks.head_sha),
                    ));
                    if checks.partial {
                        data.warnings.push(
                            "Some check pages were unavailable. This is a partial result.".into(),
                        );
                    }
                    for check in checks.checks {
                        let mut row = Row::text(
                            check.name,
                            format!(
                                "{} · {} · {}",
                                check.app.unwrap_or("Unknown provider".into()),
                                check.state,
                                check.conclusion.unwrap_or("pending".into())
                            ),
                        );
                        row.url = check.url;
                        data.rows.push(row);
                    }
                }
                PullTab::History => {
                    data.paged(client.pull_history(&pull, after, force).await?, |event| {
                        let mut row =
                            Row::text(event.kind, format!("{} · {}", event.author, event.date));
                        row.body = Some(event.body);
                        row.url = event.url;
                        row
                    })
                }
            }
            data.pull = Some(summary);
        }
        Route::Thread(thread) => {
            data.title = "Review thread".into();
            data.paged(
                client.thread_comments(&thread.0, after, force).await?,
                comment_row,
            );
        }
        Route::Issue(repo, number) => {
            let detail = client.issue(&repo, number, force).await?;
            data.title = format!("#{} {}", number, detail.issue.title);
            data.description = Some(format!(
                "{} · {} · {}\n\n{}",
                detail.issue.state,
                detail.issue.author,
                detail.issue.labels.join(", "),
                detail.body
            ));
            data.paged(
                client.issue_comments(&repo, number, after, force).await?,
                |comment| {
                    let mut row = Row::text(
                        comment.author.clone(),
                        format!("{} · updated {}", comment.created_at, comment.updated_at),
                    );
                    row.author = Some(comment.author);
                    row.body = Some(comment.body);
                    row.url = comment.url;
                    row
                },
            );
        }
        Route::Commit(repo, sha) => {
            let detail = client.commit(&repo, &sha, force).await?;
            data.title = detail
                .commit
                .message
                .lines()
                .next()
                .unwrap_or("Commit")
                .into();
            data.description = Some(format!(
                "{}\n{} · {} · +{} −{}\n\n{}",
                sha,
                detail.commit.author,
                detail.commit.date,
                detail.additions,
                detail.deletions,
                detail.commit.message
            ));
            data.paged(
                client.commit_files(&repo, &sha, after, force).await?,
                file_row,
            );
        }
        Route::User(login, tab) => {
            let profile = client.user_profile(&login, force).await?;
            data.title = profile.user.name.clone().unwrap_or(login.clone());
            match tab {
                UserTab::Overview => {
                    data.description = Some(format!(
                        "@{} · {} followers · {} following · {} public repositories\n{}\n{} · {} · {}",
                        login,
                        profile.followers,
                        profile.following,
                        profile.public_repositories,
                        profile.bio.unwrap_or_default(),
                        profile.company.unwrap_or_default(),
                        profile.location.unwrap_or_default(),
                        profile.email.unwrap_or_default()
                    ));
                    match client.user_contributions(&login, force).await {
                        Ok(calendar) => {
                            data.rows.push(Row::text(
                                "Contributions",
                                format!(
                                    "{} in the last year · {} {}",
                                    calendar.total,
                                    calendar.status_emoji.unwrap_or_default(),
                                    calendar.status_message.unwrap_or_default()
                                ),
                            ));
                            data.contribution_days = calendar.days;
                        }
                        Err(error) => data.warnings.push(error.to_string()),
                    }
                    match client.user_org_role(&login, force).await {
                        Ok(Some(role)) => data.rows.push(Row::text("Organization role", role)),
                        Ok(None) => {}
                        Err(error) => data.warnings.push(error.to_string()),
                    }
                }
                UserTab::Authored | UserTab::Reviewed => {
                    let result = client
                        .user_pull_requests(&login, tab == UserTab::Reviewed, force)
                        .await?;
                    data.rows = result.items.into_iter().map(pull_row).collect();
                    data.warnings = result
                        .warnings
                        .into_iter()
                        .map(|error| error.to_string())
                        .collect();
                    if result.truncated {
                        data.warnings.push(
                            "Search results are partial. Open GitHub for older requests.".into(),
                        );
                    }
                }
                UserTab::Teams => {
                    data.paged(client.user_teams(&login, after, force).await?, |team| {
                        let mut row = Row::text(team.name, team.description.unwrap_or_default());
                        row.route = Some(Route::Team(team.slug));
                        row.url = team.url;
                        row
                    })
                }
                UserTab::Activity => {
                    data.paged(client.user_events(&login, after, force).await?, |event| {
                        let mut row = Row::text(
                            format!("{} {}", event.kind, event.action.unwrap_or_default()),
                            format!("{} · {}", event.repository.full_name(), event.created_at),
                        );
                        row.body = Some(event.title);
                        row.url = event.url;
                        row
                    })
                }
            }
        }
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_list_separates_the_title_from_its_remaining_message() {
        let repository = RepositoryRef::new("fixture-org", "native-demo").unwrap();
        for (message, expected_body) in [
            ("Improve native navigation", None),
            ("Improve native navigation\n\n", None),
            (
                "Improve native navigation\r\n\r\nKeep keyboard focus.\r\n\r\nPreserve navigation history.",
                Some("Keep keyboard focus.\n\nPreserve navigation history."),
            ),
        ] {
            let row = commit_row(Commit {
                repository: repository.clone(),
                sha: "a".repeat(40),
                message: message.into(),
                author: "fixture-author".into(),
                date: "2026-10-15T12:00:00Z".into(),
                url: Some("https://github.com/fixture-org/native-demo/commit/example".into()),
            });
            assert_eq!(row.title, "Improve native navigation");
            assert_eq!(row.body.as_deref(), expected_body);
            assert_eq!(
                row.route,
                Some(Route::Commit(repository.clone(), "a".repeat(40)))
            );
            assert_eq!(
                row.url.as_deref(),
                Some("https://github.com/fixture-org/native-demo/commit/example")
            );
        }
    }
}
