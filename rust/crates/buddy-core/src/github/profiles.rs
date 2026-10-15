use chrono::Days;
use serde_json::json;

use super::browsing::{number, valid_login};
use super::details::web_url;
use super::pulls::text;
use super::{
    ApiRequest, GitHubClient, GitHubError, Page, PageCursor, PullSearchResult, RepositoryRef,
    TeamSummary, UserSummary,
};

#[derive(Debug, Clone)]
pub struct UserProfile {
    pub user: UserSummary,
    pub bio: Option<String>,
    pub company: Option<String>,
    pub location: Option<String>,
    pub email: Option<String>,
    pub created_at: String,
    pub public_repositories: u64,
    pub followers: u64,
    pub following: u64,
}

#[derive(Debug, Clone)]
pub struct ContributionDay {
    pub date: String,
    pub count: u64,
    pub color: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UserContributions {
    pub total: u64,
    pub days: Vec<ContributionDay>,
    pub status_message: Option<String>,
    pub status_emoji: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UserEvent {
    pub id: String,
    pub kind: String,
    pub action: Option<String>,
    pub repository: RepositoryRef,
    pub created_at: String,
    pub title: String,
    pub url: Option<String>,
}

impl GitHubClient {
    pub async fn user_profile(&self, login: &str, force: bool) -> Result<UserProfile, GitHubError> {
        valid_login(login)?;
        let response = self
            .request(
                ApiRequest::get(format!("/users/{}", crate::http::encode_component(login))),
                force,
            )
            .await?
            .value;
        let value = &response.body;
        let user = UserSummary::parse(value)?;
        if !user.login.eq_ignore_ascii_case(login) {
            return Err(GitHubError::InvalidData);
        }
        Ok(UserProfile {
            user,
            bio: value["bio"].as_str().map(Into::into),
            company: value["company"].as_str().map(Into::into),
            location: value["location"].as_str().map(Into::into),
            email: value["email"].as_str().map(Into::into),
            created_at: text(value, "created_at")?.into(),
            public_repositories: number(value, "public_repos")?,
            followers: number(value, "followers")?,
            following: number(value, "following")?,
        })
    }

    pub async fn user_contributions(
        &self,
        login: &str,
        force: bool,
    ) -> Result<UserContributions, GitHubError> {
        valid_login(login)?;
        let query = "query NativeUserContributions($login:String!) {user(login:$login) {status {message emoji} contributionsCollection {contributionCalendar {totalContributions weeks {contributionDays {date contributionCount color}}}}}}";
        let response = self
            .request(
                ApiRequest::graphql(query, json!({"login":login}), false),
                force,
            )
            .await?
            .value;
        let user = &response.body["data"]["user"];
        if user.is_null() {
            return Err(GitHubError::NotFound);
        }
        let calendar = &user["contributionsCollection"]["contributionCalendar"];
        let weeks = calendar["weeks"]
            .as_array()
            .ok_or(GitHubError::InvalidData)?;
        let mut days = Vec::new();
        for week in weeks {
            for day in week["contributionDays"]
                .as_array()
                .ok_or(GitHubError::InvalidData)?
            {
                let date = text(day, "date")?;
                chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
                    .map_err(|_| GitHubError::InvalidData)?;
                days.push(ContributionDay {
                    date: date.into(),
                    count: number(day, "contributionCount")?,
                    color: day["color"].as_str().map(Into::into),
                });
            }
        }
        Ok(UserContributions {
            total: number(calendar, "totalContributions")?,
            days,
            status_message: user["status"]["message"].as_str().map(Into::into),
            status_emoji: user["status"]["emoji"].as_str().map(Into::into),
        })
    }

    pub async fn user_teams(
        &self,
        login: &str,
        cursor: Option<&PageCursor>,
        force: bool,
    ) -> Result<Page<TeamSummary>, GitHubError> {
        valid_login(login)?;
        if self.namespace_is_personal(force).await? {
            return Ok(Page {
                items: Vec::new(),
                next: None,
                truncated: false,
            });
        }
        let query = "query NativeUserTeams($org:String!,$login:String!,$after:String) {organization(login:$org) {teams(first:100,userLogins:[$login],after:$after) {pageInfo {hasNextPage endCursor} nodes {slug name description url}}}}";
        self.connection(
            query,
            json!({"org":self.account.organization(),"login":login}),
            "/data/organization/teams",
            cursor,
            force,
        )
        .await?
        .map(|value| {
            Ok(TeamSummary {
                slug: text(&value, "slug")?.into(),
                name: text(&value, "name")?.into(),
                description: value["description"].as_str().map(Into::into),
                members: None,
                repositories: None,
                url: web_url(&value["url"]),
            })
        })
    }

    pub async fn user_org_role(
        &self,
        login: &str,
        force: bool,
    ) -> Result<Option<String>, GitHubError> {
        valid_login(login)?;
        if self.namespace_is_personal(force).await? {
            return Ok(None);
        }
        let response = self
            .request(
                ApiRequest::get(format!(
                    "/orgs/{}/memberships/{}",
                    self.account.organization(),
                    crate::http::encode_component(login)
                )),
                force,
            )
            .await?
            .value;
        Ok(Some(text(&response.body, "role")?.into()))
    }

    pub async fn user_pull_requests(
        &self,
        login: &str,
        reviewed: bool,
        force: bool,
    ) -> Result<PullSearchResult, GitHubError> {
        valid_login(login)?;
        let qualifier = if reviewed { "reviewed-by" } else { "author" };
        let namespace = if self.namespace_is_personal(force).await? {
            "user"
        } else {
            "org"
        };
        let date = chrono::Utc::now()
            .date_naive()
            .checked_sub_days(Days::new(90))
            .ok_or(GitHubError::InvalidData)?
            .format("%Y-%m-%d");
        let query = format!(
            "is:pr {qualifier}:{login} {namespace}:{} updated:>={date} sort:updated-desc",
            self.account.organization()
        );
        let (items, truncated) = self.search_pulls(&query, force).await?;
        Ok(PullSearchResult {
            items,
            truncated,
            warnings: Vec::new(),
        })
    }

    pub async fn user_events(
        &self,
        login: &str,
        cursor: Option<&PageCursor>,
        force: bool,
    ) -> Result<Page<UserEvent>, GitHubError> {
        valid_login(login)?;
        let page = self
            .rest_page(
                &format!("/users/{}/events", crate::http::encode_component(login)),
                cursor,
                None,
                force,
            )
            .await?;
        let mut items = Vec::new();
        for value in page.items {
            let repository = RepositoryRef::parse(text(&value["repo"], "name")?)?;
            if !repository
                .owner()
                .eq_ignore_ascii_case(self.account.organization())
            {
                continue;
            }
            let payload = &value["payload"];
            let url = web_url(&payload["pull_request"]["html_url"])
                .or_else(|| web_url(&payload["issue"]["html_url"]))
                .or_else(|| Some(repository.web_url()));
            let title = payload["pull_request"]["title"]
                .as_str()
                .or_else(|| payload["issue"]["title"].as_str())
                .or_else(|| payload["ref"].as_str())
                .unwrap_or("")
                .to_string();
            items.push(UserEvent {
                id: text(&value, "id")?.into(),
                kind: text(&value, "type")?.into(),
                action: payload["action"].as_str().map(Into::into),
                repository,
                created_at: text(&value, "created_at")?.into(),
                title,
                url,
            });
        }
        Ok(Page {
            items,
            next: page.next,
            truncated: page.truncated,
        })
    }
}
