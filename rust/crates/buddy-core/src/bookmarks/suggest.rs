//! The AI description and tags Electron asks Copilot for when a URL is dropped (here:
//! pasted). Only the prompt and the reply's parsing live here; the app runs the turn.
use serde_json::Value;

use super::MAX_TAGS;

/// Electron's `quickPrompt` text, word for word.
pub fn prompt(url: &str, title: &str) -> String {
    let title = title.trim();
    let (and_title, title_line) = if title.is_empty() {
        (String::new(), String::new())
    } else {
        (" and title".to_string(), format!("\nTitle: {title}"))
    };
    format!(
        "Given this bookmark URL{and_title}, respond with ONLY a JSON object (no markdown, no code fences):\n\
         {{\"description\": \"one-sentence summary of what this page is about\", \"tags\": [\"tag1\", \"tag2\", \"tag3\"]}}\n\n\
         URL: {url}{title_line}\n\n\
         Rules:\n\
         - description: 1 short sentence, max 120 chars\n\
         - tags: 3-5 lowercase single-word tags relevant to the content\n\
         - Respond with ONLY the JSON object, nothing else"
    )
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Suggestion {
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
}

/// The reply's description and tags; anything unreadable is simply absent, so a bad
/// reply changes nothing.
pub fn parse(text: &str) -> Suggestion {
    let cleaned = text.replace("```json", "").replace("```", "");
    let Ok(Value::Object(reply)) = serde_json::from_str::<Value>(cleaned.trim()) else {
        return Suggestion::default();
    };
    let description = reply
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string);
    let tags = reply
        .get("tags")
        .and_then(Value::as_array)
        .and_then(|tags| {
            let tags: Vec<String> = tags
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|tag| !tag.is_empty() && !tag.contains(','))
                .take(MAX_TAGS)
                .map(str::to_string)
                .collect();
            (!tags.is_empty()).then_some(tags)
        });
    Suggestion { description, tags }
}
