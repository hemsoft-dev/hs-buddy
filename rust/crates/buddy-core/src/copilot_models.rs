//! Read-only model discovery over Copilot's stdio JSON-RPC protocol 3, matching
//! the repository's @github/copilot-sdk 1.0.14. No prompt or session is created.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Clone, Debug, Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
}

fn safe_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= 200 && !value.chars().any(char::is_control)
}

fn send(writer: &mut impl Write, id: u32, method: &str, params: Value) -> Result<(), &'static str> {
    let body = json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}).to_string();
    write!(writer, "Content-Length: {}\r\n\r\n{}", body.len(), body)
        .map_err(|_| "Copilot CLI disconnected.")?;
    writer.flush().map_err(|_| "Copilot CLI disconnected.")
}

fn response(reader: &mut impl BufRead, id: u32) -> Result<Value, &'static str> {
    for _ in 0..64 {
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            if header.len() >= 8192 {
                return Err("Copilot returned an unsupported response.");
            }
            let mut byte = [0];
            reader
                .read_exact(&mut byte)
                .map_err(|_| "Copilot CLI disconnected.")?;
            header.push(byte[0]);
        }
        let header = std::str::from_utf8(&header)
            .map_err(|_| "Copilot returned an unsupported response.")?;
        let length = header
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case("Content-Length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .filter(|size| *size > 0 && *size <= 1024 * 1024)
            .ok_or("Copilot returned an oversized or unsupported response.")?;
        let mut bytes = vec![0; length];
        reader
            .read_exact(&mut bytes)
            .map_err(|_| "Copilot CLI disconnected.")?;
        let message: Value = serde_json::from_slice(&bytes)
            .map_err(|_| "Copilot returned an unsupported response.")?;
        if message.get("id").and_then(Value::as_u64) != Some(u64::from(id)) {
            continue;
        }
        if message.get("error").is_some() {
            return Err(
                "Copilot rejected model discovery. Check account access and CLI compatibility.",
            );
        }
        return message
            .get("result")
            .cloned()
            .ok_or("Copilot returned an unsupported response.");
    }
    Err("Copilot did not return the requested models.")
}

fn discover(token: String) -> Result<Vec<Model>, &'static str> {
    let cli = std::env::var_os("BUDDY_COPILOT_CLI").unwrap_or_else(|| "copilot".into());
    let mut command = Command::new(cli);
    command
        .args([
            "--headless",
            "--no-auto-update",
            "--stdio",
            "--no-auto-login",
            "--auth-token-env",
            "COPILOT_SDK_AUTH_TOKEN",
        ])
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .env("COPILOT_SDK_AUTH_TOKEN", token)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().map_err(
        |_| "Copilot CLI is unavailable. Install it or set BUDDY_COPILOT_CLI to its executable.",
    )?;
    let input = child.stdin.take();
    let output = child.stdout.take();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = (|| {
            let mut input = input.ok_or("Copilot CLI disconnected.")?;
            let mut output = BufReader::new(output.ok_or("Copilot CLI disconnected.")?);
            send(
                &mut input,
                1,
                "connect",
                json!({"supportedTaskKinds":["agent","client","shell"]}),
            )?;
            let connected = response(&mut output, 1)?;
            if connected.get("protocolVersion").and_then(Value::as_u64) != Some(3) {
                return Err(
                    "This Copilot CLI protocol is unsupported. Use the CLI matching Buddy's SDK.",
                );
            }
            send(&mut input, 2, "models.list", json!({}))?;
            let result = response(&mut output, 2)?;
            let models = result
                .get("models")
                .and_then(Value::as_array)
                .ok_or("Copilot returned an unsupported model list.")?;
            let mut parsed = Vec::new();
            for model in models.iter().take(500) {
                let id = model.get("id").and_then(Value::as_str).unwrap_or_default();
                let name = model.get("name").and_then(Value::as_str).unwrap_or(id);
                if safe_text(id) && safe_text(name) {
                    parsed.push(Model {
                        id: id.into(),
                        name: name.into(),
                    });
                }
            }
            Ok(parsed)
        })();
        let _ = tx.send(result);
    });
    let result = rx
        .recv_timeout(Duration::from_secs(30))
        .map_err(|_| "Copilot model discovery timed out. Check the CLI and account, then retry.");
    // No session exists. Reap this request's server on success and every error.
    let _ = child.kill();
    let _ = child.wait();
    let _ = worker.join();
    result?
}

pub async fn list(account: &str) -> Result<Vec<Model>, &'static str> {
    let account = if account.trim().is_empty() {
        crate::gh::active_account()
            .await
            .ok_or("Sign in with GitHub CLI before loading models.")?
    } else {
        crate::gh::assert_valid_slug(account).map_err(|_| "Choose a valid GitHub CLI account.")?;
        account.into()
    };
    let token = crate::gh::auth_token(&account).await.map_err(
        |_| "GitHub CLI could not provide this account's credentials. Sign in and retry.",
    )?;
    tokio::task::spawn_blocking(move || discover(token))
        .await
        .map_err(|_| "Copilot model discovery could not finish.")?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framing_accepts_notifications_without_exposing_error_bodies() {
        let mut bytes = Vec::new();
        send(&mut bytes, 99, "notice", json!({})).unwrap();
        let error = r#"{"jsonrpc":"2.0","id":2,"error":{"message":"private-fixture-token"}}"#;
        write!(
            &mut bytes,
            "Content-Length: {}\r\n\r\n{}",
            error.len(),
            error
        )
        .unwrap();
        let message = response(&mut Cursor::new(bytes), 2).unwrap_err();
        assert!(!message.contains("private-fixture-token"));
    }
    #[test]
    fn oversized_and_unterminated_frames_are_rejected() {
        assert!(response(&mut Cursor::new(b"Content-Length: 1048577\r\n\r\n"), 1).is_err());
        assert!(response(&mut Cursor::new(vec![b'x'; 8193]), 1).is_err());
    }
    use std::io::Cursor;
}
