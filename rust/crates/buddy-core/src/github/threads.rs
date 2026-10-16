use serde_json::{Value, json};

use super::details::valid_node_id;
use super::pulls::text;
use super::{
    ApiRequest, Comment, GitHubClient, GitHubError, Page, PageCursor, PullRef, RepositoryRef,
};

#[derive(Debug, Clone)]
pub struct ThreadRef {
    pub(crate) pull: PullRef,
    pub(crate) id: String,
}

impl ThreadRef {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn pull(&self) -> &PullRef {
        &self.pull
    }
}

#[derive(Debug, Clone)]
pub struct ReviewThread {
    pub reference: ThreadRef,
    pub resolved: bool,
    pub outdated: bool,
    pub path: Option<String>,
    pub line: Option<u64>,
    pub side: Option<String>,
    pub comments: Page<Comment>,
}

const COMMENT_FIELDS: &str = "id body createdAt updatedAt url diffHunk author {login} reactionGroups {content users {totalCount}}";

impl GitHubClient {
    pub async fn pull_threads(
        &self,
        pull: &PullRef,
        cursor: Option<&PageCursor>,
        force: bool,
    ) -> Result<Page<ReviewThread>, GitHubError> {
        let mut variables = self.pull_variables(pull)?;
        variables.as_object_mut().unwrap().remove("reviewer");
        let query = format!(
            r#"query NativePullThreads($owner:String!,$repo:String!,$number:Int!,$after:String) {{
         repository(owner:$owner,name:$repo) {{pullRequest(number:$number) {{
          reviewThreads(first:20,after:$after) {{pageInfo {{hasNextPage endCursor}} nodes {{
           id isResolved isOutdated path line diffSide comments(first:20) {{pageInfo {{hasNextPage endCursor}} nodes {{{COMMENT_FIELDS}}}}}
          }}}}
         }}}}
        }}"#
        );
        self.connection(
            &query,
            variables,
            "/data/repository/pullRequest/reviewThreads",
            cursor,
            force,
        )
        .await?
        .map(|value| {
            let id = text(&value, "id")?;
            valid_node_id(id)?;
            let comments = comment_page(pull, &value["comments"])?;
            Ok(ReviewThread {
                reference: ThreadRef {
                    pull: pull.clone(),
                    id: id.into(),
                },
                resolved: value["isResolved"]
                    .as_bool()
                    .ok_or(GitHubError::InvalidData)?,
                outdated: value["isOutdated"]
                    .as_bool()
                    .ok_or(GitHubError::InvalidData)?,
                path: value["path"].as_str().map(Into::into),
                line: value["line"].as_u64(),
                side: value["diffSide"].as_str().map(Into::into),
                comments,
            })
        })
    }

    pub async fn thread_comments(
        &self,
        thread: &ThreadRef,
        cursor: Option<&PageCursor>,
        force: bool,
    ) -> Result<Page<Comment>, GitHubError> {
        valid_node_id(&thread.id)?;
        let after = match cursor {
            None => Value::Null,
            Some(PageCursor::GraphQl(after))
                if !after.is_empty()
                    && after.len() <= 1024
                    && !after.chars().any(char::is_control) =>
            {
                json!(after)
            }
            _ => return Err(GitHubError::InvalidResource),
        };
        let query = format!(
            r#"query NativeThreadComments($id:ID!,$after:String) {{
         node(id:$id) {{... on PullRequestReviewThread {{
          pullRequest {{number repository {{nameWithOwner}}}}
          comments(first:100,after:$after) {{pageInfo {{hasNextPage endCursor}} nodes {{{COMMENT_FIELDS}}}}}
         }}}}
        }}"#
        );
        let response = self
            .request(
                ApiRequest::graphql(&query, json!({"id":thread.id,"after":after}), false),
                force,
            )
            .await?
            .value;
        let node = &response.body["data"]["node"];
        let repository =
            RepositoryRef::parse(text(&node["pullRequest"]["repository"], "nameWithOwner")?)?;
        if repository != thread.pull.repository
            || node["pullRequest"]["number"].as_u64() != Some(thread.pull.number)
        {
            return Err(GitHubError::InvalidResource);
        }
        let page = comment_page(&thread.pull, &node["comments"])?;
        if page.next.as_ref() == cursor && cursor.is_some() {
            return Err(GitHubError::InvalidData);
        }
        Ok(page)
    }
}

fn comment_page(pull: &PullRef, connection: &Value) -> Result<Page<Comment>, GitHubError> {
    let nodes = connection["nodes"]
        .as_array()
        .ok_or(GitHubError::InvalidData)?;
    let more = connection["pageInfo"]["hasNextPage"]
        .as_bool()
        .ok_or(GitHubError::InvalidData)?;
    let next = if more {
        let cursor = text(&connection["pageInfo"], "endCursor")?;
        if cursor.is_empty() || cursor.len() > 1024 || cursor.chars().any(char::is_control) {
            return Err(GitHubError::InvalidData);
        }
        Some(PageCursor::GraphQl(cursor.into()))
    } else {
        None
    };
    Ok(Page {
        items: nodes
            .iter()
            .map(|node| Comment::parse(pull, node))
            .collect::<Result<_, _>>()?,
        next,
        truncated: false,
    })
}
