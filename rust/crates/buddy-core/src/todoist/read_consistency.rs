//! A connection-local Sync watermark detects changes during cursor reads.
//! Only resource reads are sent: no commands, stored tokens or service retry.
use serde::Deserialize;
use serde_json::Value;

use super::{Client, TransportError, bounded_body};
use crate::todoist::{Error, parse_profile, valid_cursor};

#[derive(Deserialize)]
struct SyncReply {
    sync_token: String,
    full_sync: bool,
    user: Option<Value>,
    #[serde(default)]
    items: Vec<Value>,
    #[serde(default)]
    projects: Vec<Value>,
}

impl Client {
    async fn sync_read(&self, token: &str, resources: &str) -> Result<SyncReply, TransportError> {
        let response = self
            .request(reqwest::Method::POST, "sync")
            .form(&[("sync_token", token), ("resource_types", resources)])
            .send()
            .await
            .map_err(|_| TransportError::Unavailable)?;
        let reply: SyncReply = serde_json::from_slice(&bounded_body(response).await?)
            .map_err(|_| Error::InvalidResponse)?;
        if !valid_cursor(&reply.sync_token) || reply.sync_token == "*" {
            return Err(Error::InvalidResponse.into());
        }
        Ok(reply)
    }

    pub(super) async fn start_read_marker(
        &self,
        marker: &mut Option<String>,
    ) -> Result<(), TransportError> {
        if marker.is_none() {
            // Sync tokens mark account progress; resource_types selects the
            // returned data. Bootstrap with user data so a large account does
            // not require an unbounded full task download just to get a token.
            let reply = self.sync_read("*", "[\"user\"]").await?;
            if !reply.full_sync {
                return Err(Error::InvalidResponse.into());
            }
            let user = reply.user.ok_or(Error::InvalidResponse)?;
            parse_profile(&serde_json::to_vec(&user).map_err(|_| Error::InvalidResponse)?)?;
            *marker = Some(reply.sync_token);
        }
        // Initial full-sync data can be delayed. Advance to a current token
        // before restarting page one; prior deltas need not be applied to REST.
        self.advance_read_marker(marker, false).await
    }

    pub(super) async fn finish_read_marker(
        &self,
        marker: &mut Option<String>,
    ) -> Result<(), TransportError> {
        self.advance_read_marker(marker, true).await
    }

    async fn advance_read_marker(
        &self,
        marker: &mut Option<String>,
        require_stable: bool,
    ) -> Result<(), TransportError> {
        let token = marker.as_deref().ok_or(Error::InvalidResponse)?;
        let reply = match self.sync_read(token, "[\"items\",\"projects\"]").await {
            Ok(reply) => reply,
            Err(error) => {
                *marker = None;
                return Err(error);
            }
        };
        if reply.full_sync {
            *marker = None;
            return Err(Error::ChangedWhileLoading.into());
        }
        *marker = Some(reply.sync_token);
        if require_stable && (!reply.items.is_empty() || !reply.projects.is_empty()) {
            return Err(Error::ChangedWhileLoading.into());
        }
        Ok(())
    }
}
