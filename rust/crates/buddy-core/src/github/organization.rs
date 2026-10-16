use serde_json::{Value, json};

use super::details::web_url;
use super::pulls::{PULL_LIST_FRAGMENT, text};
use super::{
    ApiRequest, GitHubClient, GitHubError, Page, PageCursor, PullSummary, RepositoryRef,
    TeamSummary, UserSummary,
};

impl GitHubClient {
    pub async fn repository_pull_requests(
        &self,
        repository: &RepositoryRef,
        closed: bool,
        cursor: Option<&PageCursor>,
        force: bool,
    ) -> Result<Page<PullSummary>, GitHubError> {
        let query = format!(
            "query NativeRepositoryPulls($owner:String!,$repo:String!,$states:[PullRequestState!]!,$reviewer:String!,$after:String) {{repository(owner:$owner,name:$repo) {{pullRequests(states:$states,first:20,after:$after,orderBy:{{field:UPDATED_AT,direction:DESC}}) {{pageInfo {{hasNextPage endCursor}} nodes {{...NativePullList}}}}}}}}\n{PULL_LIST_FRAGMENT}"
        );
        let states = if closed {
            json!(["CLOSED", "MERGED"])
        } else {
            json!(["OPEN"])
        };
        self.connection(&query,json!({"owner":repository.owner(),"repo":repository.name(),"states":states,"reviewer":self.account.username()}),"/data/repository/pullRequests",cursor,force).await?.map(|value|{
            let pull=PullSummary::from_graphql(&value)?;
            if pull.target.repository!=*repository {return Err(GitHubError::InvalidData);}
            Ok(pull)
        })
    }

    pub async fn organization_teams(
        &self,
        cursor: Option<&PageCursor>,
        force: bool,
    ) -> Result<Page<TeamSummary>, GitHubError> {
        if self.namespace_is_personal(force).await? {
            return Ok(Page {
                items: Vec::new(),
                next: None,
                truncated: false,
            });
        }
        self.rest_page(
            &format!("/orgs/{}/teams", self.account.organization()),
            cursor,
            None,
            force,
        )
        .await?
        .map(|value| TeamSummary::parse(&value))
    }

    pub async fn team(&self, slug: &str, force: bool) -> Result<TeamSummary, GitHubError> {
        valid_team(slug)?;
        let response = self
            .request(
                ApiRequest::get(format!(
                    "/orgs/{}/teams/{slug}",
                    self.account.organization()
                )),
                force,
            )
            .await?
            .value;
        let team = TeamSummary::parse(&response.body)?;
        if !team.slug.eq_ignore_ascii_case(slug) {
            return Err(GitHubError::InvalidData);
        }
        Ok(team)
    }

    pub async fn team_members(
        &self,
        slug: &str,
        cursor: Option<&PageCursor>,
        force: bool,
    ) -> Result<Page<UserSummary>, GitHubError> {
        valid_team(slug)?;
        self.rest_page(
            &format!(
                "/orgs/{}/teams/{slug}/members?role=all",
                self.account.organization()
            ),
            cursor,
            None,
            force,
        )
        .await?
        .map(|value| UserSummary::parse(&value))
    }
}

fn valid_team(slug: &str) -> Result<(), GitHubError> {
    if slug.is_empty()
        || slug.len() > 100
        || !slug.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
    {
        Err(GitHubError::InvalidResource)
    } else {
        Ok(())
    }
}

impl TeamSummary {
    fn parse(value: &Value) -> Result<Self, GitHubError> {
        let slug = text(value, "slug")?;
        valid_team(slug)?;
        Ok(Self {
            slug: slug.into(),
            name: text(value, "name")?.into(),
            description: value["description"].as_str().map(Into::into),
            members: value["members_count"].as_u64(),
            repositories: value["repos_count"].as_u64(),
            url: web_url(&value["html_url"]),
        })
    }
}
