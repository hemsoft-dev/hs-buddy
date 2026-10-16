use serde_json::Value;

use super::{ApiRequest, GitHubClient, GitHubError};

pub const MAX_PAGES: u32 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageCursor {
    Rest(u32),
    GraphQl(String),
}

#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<PageCursor>,
    pub truncated: bool,
}

impl<T> Page<T> {
    pub fn map<U>(
        self,
        mut parse: impl FnMut(T) -> Result<U, GitHubError>,
    ) -> Result<Page<U>, GitHubError> {
        Ok(Page {
            items: self
                .items
                .into_iter()
                .map(&mut parse)
                .collect::<Result<_, _>>()?,
            next: self.next,
            truncated: self.truncated,
        })
    }
}

impl GitHubClient {
    pub(crate) async fn rest_page(
        &self,
        path: &str,
        cursor: Option<&PageCursor>,
        key: Option<&str>,
        force: bool,
    ) -> Result<Page<Value>, GitHubError> {
        let page = match cursor {
            None => 1,
            Some(PageCursor::Rest(page)) if (1..=MAX_PAGES).contains(page) => *page,
            _ => return Err(GitHubError::InvalidResource),
        };
        let separator = if path.contains('?') { '&' } else { '?' };
        let response = self
            .request(
                ApiRequest::get(format!("{path}{separator}per_page=100&page={page}")),
                force,
            )
            .await?
            .value;
        let values = match key {
            Some(key) => response.body.get(key),
            None => Some(&response.body),
        }
        .and_then(Value::as_array)
        .ok_or(GitHubError::InvalidData)?
        .clone();
        let more = response
            .headers
            .get("link")
            .is_some_and(|header| header.split(',').any(|part| part.contains("rel=\"next\"")));
        Ok(Page {
            items: values,
            next: (more && page < MAX_PAGES).then_some(PageCursor::Rest(page + 1)),
            truncated: more && page == MAX_PAGES,
        })
    }

    pub(crate) async fn connection(
        &self,
        query: &str,
        mut variables: Value,
        pointer: &str,
        cursor: Option<&PageCursor>,
        force: bool,
    ) -> Result<Page<Value>, GitHubError> {
        let after = match cursor {
            None => Value::Null,
            Some(PageCursor::GraphQl(cursor))
                if !cursor.is_empty()
                    && cursor.len() <= 1024
                    && !cursor.chars().any(char::is_control) =>
            {
                Value::String(cursor.clone())
            }
            _ => return Err(GitHubError::InvalidResource),
        };
        let previous = after.as_str().map(str::to_string);
        variables["after"] = after;
        let response = self
            .request(ApiRequest::graphql(query, variables, false), force)
            .await?
            .value;
        let connection = response
            .body
            .pointer(pointer)
            .ok_or(GitHubError::NotFound)?;
        if connection.is_null() {
            return Err(GitHubError::NotFound);
        }
        let items = connection["nodes"]
            .as_array()
            .ok_or(GitHubError::InvalidData)?
            .clone();
        let more = connection["pageInfo"]["hasNextPage"]
            .as_bool()
            .ok_or(GitHubError::InvalidData)?;
        let next = if more {
            let value = connection["pageInfo"]["endCursor"]
                .as_str()
                .filter(|v| !v.is_empty() && v.len() <= 1024 && !v.chars().any(char::is_control))
                .ok_or(GitHubError::InvalidData)?;
            if previous.as_deref() == Some(value) {
                return Err(GitHubError::InvalidData);
            }
            Some(PageCursor::GraphQl(value.into()))
        } else {
            None
        };
        Ok(Page {
            items,
            next,
            truncated: false,
        })
    }
}
