//! Read-only CodexBar metrics, retaining the Electron normalization aliases.
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
const MAX_BYTES: usize = 8 * 1024 * 1024;
const ERROR: &str =
    "The enterprise metrics file is unavailable, invalid or exceeds the supported limits.";

// No Debug: rows and source JSON can contain private enterprise data.
pub struct User {
    pub login: String,
    pub gross_quantity: f64,
    pub gross_amount: f64,
    pub net_amount: f64,
    pub models: usize,
    pub top_model: Option<String>,
    pub top_model_quantity: f64,
    pub success: bool,
    pub error: Option<String>,
    pub source_json: String,
}
pub struct Snapshot {
    pub generated_at: String,
    pub enterprise: String,
    pub organization: String,
    pub year: Option<f64>,
    pub month: Option<f64>,
    pub days: Vec<f64>,
    pub users: Vec<User>,
}
fn string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value[*key]
            .as_str()
            .filter(|text| !text.trim().is_empty())
            .map(str::to_owned)
    })
}
fn number(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| value[*key].as_f64().filter(|value| value.is_finite()))
}
fn array<'a>(value: &'a Value, keys: &[&str]) -> &'a [Value] {
    keys.iter()
        .find_map(|key| value[*key].as_array().map(Vec::as_slice))
        .unwrap_or(&[])
}
#[derive(Default)]
struct Totals {
    quantity: f64,
    gross: f64,
    net: f64,
    models: Vec<(String, f64)>,
}
impl Totals {
    fn model(&mut self, name: Option<String>, quantity: f64) -> Result<(), &'static str> {
        if let Some(name) = name.filter(|_| quantity > 0.0) {
            if name.len() > 256 {
                return Err(ERROR);
            }
            if let Some((_, total)) = self.models.iter_mut().find(|(model, _)| *model == name) {
                *total += quantity;
            } else {
                self.models.push((name, quantity));
            }
        }
        Ok(())
    }
    fn item(&mut self, value: &Value) -> Result<(), &'static str> {
        if !value.is_object() {
            return Ok(());
        }
        let quantity =
            number(value, &["grossQuantity", "GrossQuantity", "gross_quantity"]).unwrap_or(0.0);
        self.quantity += quantity;
        self.gross += number(value, &["grossAmount", "GrossAmount", "gross_amount"]).unwrap_or(0.0);
        self.net += number(value, &["netAmount", "NetAmount", "net_amount"]).unwrap_or(0.0);
        self.model(string(value, &["model", "Model"]), quantity)
    }
}
fn collect(
    value: &Value,
    totals: &mut Totals,
    depth: usize,
    budget: &mut usize,
) -> Result<(), &'static str> {
    if depth > 16 || *budget == 0 {
        return Err(ERROR);
    }
    *budget -= 1;
    if let Some(items) = value.as_array() {
        for item in items {
            collect(item, totals, depth + 1, budget)?;
        }
        return Ok(());
    }
    if !value.is_object() {
        return Ok(());
    }
    let items = array(value, &["usageItems", "UsageItems", "items", "Items"]);
    if !items.is_empty() {
        for item in items {
            if *budget == 0 {
                return Err(ERROR);
            }
            *budget -= 1;
            totals.item(item)?;
        }
        return Ok(());
    }
    for key in [
        "Responses",
        "responses",
        "Response",
        "response",
        "data",
        "Data",
        "Raw",
        "raw",
    ] {
        if let Some(value) = value.get(key) {
            collect(value, totals, depth + 1, budget)?;
        }
    }
    Ok(())
}
pub fn parse(bytes: &[u8], modified: &str) -> Result<Snapshot, &'static str> {
    if bytes.len() > MAX_BYTES {
        return Err(ERROR);
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let value: Value = serde_json::from_slice(bytes).map_err(|_| ERROR)?;
    if !value.is_object() {
        return Err(ERROR);
    }
    let rows = array(&value, &["Users", "users", "members", "Members"]);
    if rows.len() > 5000 {
        return Err(ERROR);
    }
    let mut users = Vec::new();
    let mut budget = 100_000;
    for record in rows {
        if !record.is_object() {
            continue;
        }
        let Some(login) = string(
            record,
            &[
                "User",
                "user",
                "login",
                "Login",
                "memberLogin",
                "member_login",
            ],
        ) else {
            continue;
        };
        if login.len() > 256 {
            return Err(ERROR);
        }
        let mut totals = Totals::default();
        if let Some(quantity) = number(
            record,
            &["grossQuantity", "GrossQuantity", "gross_quantity"],
        ) {
            totals.quantity = quantity;
            totals.gross =
                number(record, &["grossAmount", "GrossAmount", "gross_amount"]).unwrap_or(0.0);
            totals.net = number(record, &["netAmount", "NetAmount", "net_amount"]).unwrap_or(0.0);
            totals.model(
                string(record, &["topModel", "TopModel", "top_model"]),
                number(
                    record,
                    &["topModelQuantity", "TopModelQuantity", "top_model_quantity"],
                )
                .unwrap_or(quantity),
            )?;
        } else {
            collect(record, &mut totals, 0, &mut budget)?;
        }
        if ![totals.quantity, totals.gross, totals.net]
            .iter()
            .all(|value| value.is_finite())
            || totals
                .models
                .iter()
                .any(|(_, quantity)| !quantity.is_finite())
        {
            return Err(ERROR);
        }
        let mut top_model = None;
        let mut top_quantity = 0.0;
        for (name, quantity) in &totals.models {
            if *quantity > top_quantity {
                top_model = Some(name.clone());
                top_quantity = *quantity;
            }
        }
        let source_json = serde_json::to_string_pretty(record).map_err(|_| ERROR)?;
        if source_json.len() > 1024 * 1024 {
            return Err(ERROR);
        }
        users.push(User {
            login,
            gross_quantity: totals.quantity,
            gross_amount: totals.gross,
            net_amount: totals.net,
            models: totals.models.len(),
            top_model,
            top_model_quantity: top_quantity,
            success: record["Success"]
                .as_bool()
                .or_else(|| record["success"].as_bool())
                .unwrap_or(true),
            error: string(record, &["Error", "error", "errorMessage", "error_message"]),
            source_json,
        });
    }
    users.sort_by(|left, right| {
        right
            .gross_quantity
            .total_cmp(&left.gross_quantity)
            .then_with(|| left.login.to_lowercase().cmp(&right.login.to_lowercase()))
    });
    Ok(Snapshot {
        generated_at: string(
            &value,
            &[
                "GeneratedAtUtc",
                "generatedAtUtc",
                "generatedAt",
                "updatedAt",
                "updated_at",
            ],
        )
        .unwrap_or_else(|| modified.into()),
        enterprise: string(&value, &["Enterprise", "enterprise"]).unwrap_or_default(),
        organization: string(&value, &["Organization", "organization", "org"]).unwrap_or_default(),
        year: number(&value, &["Year", "year"]),
        month: number(&value, &["Month", "month"]),
        days: array(&value, &["Days", "days"])
            .iter()
            .filter_map(Value::as_f64)
            .filter(|value| value.is_finite())
            .collect(),
        users,
    })
}
pub fn default_path() -> Option<PathBuf> {
    std::env::var_os("COPILOT_METRICS_FILE")
        .filter(|value| !value.to_string_lossy().trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            cfg!(windows)
                .then(|| PathBuf::from(r"D:\github\HemSoft\codexbar\data\copilot-metrics.json"))
        })
}
pub fn read(path: &Path) -> Result<Snapshot, &'static str> {
    if !path.is_absolute() {
        return Err("Choose an absolute enterprise metrics JSON path.");
    }
    let file = std::fs::File::open(path).map_err(|_| ERROR)?;
    let metadata = file.metadata().map_err(|_| ERROR)?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES as u64 {
        return Err(ERROR);
    }
    let modified: chrono::DateTime<chrono::Utc> = metadata.modified().map_err(|_| ERROR)?.into();
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ERROR)?;
    parse(&bytes, &modified.to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn nested_daily_usage_and_direct_aggregates_keep_models_status_and_source() {
        let bytes=serde_json::to_vec(&json!({"Organization":"Synthetic team","Days":[1,2],"Users":[
            {"User":"inactive","Responses":[{"Response":{"usageItems":[]}}]},
            {"User":"active","Responses":[{"Response":{"usageItems":[{"model":"Model A","grossQuantity":10,"grossAmount":0.1},{"model":"Model B","grossQuantity":4}]}},{"Response":{"usageItems":[{"model":"Model A","grossQuantity":5}]}}]},
            {"login":"failed","grossQuantity":1,"topModel":"Direct","success":false,"error":"Synthetic error"}]})).unwrap();
        let snapshot = parse(&bytes, "mtime").unwrap();
        assert_eq!(snapshot.generated_at, "mtime");
        assert_eq!(snapshot.users[0].login, "active");
        assert_eq!(snapshot.users[0].gross_quantity, 19.0);
        assert_eq!(snapshot.users[0].models, 2);
        assert_eq!(snapshot.users[0].top_model_quantity, 15.0);
        assert!(snapshot.users[0].source_json.contains("Responses"));
        assert!(!snapshot.users[1].success);
        assert_eq!(snapshot.users[1].error.as_deref(), Some("Synthetic error"));
        assert_eq!(snapshot.users[2].top_model, None);
    }
    #[test]
    fn bom_bounds_and_explicit_local_file_reads_are_checked() {
        assert!(parse(b"\xef\xbb\xbf{\"Users\":[]}", "mtime").is_ok());
        assert!(parse(b"null", "mtime").is_err());
        assert!(parse(&vec![b' '; MAX_BYTES + 1], "mtime").is_err());
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("synthetic-metrics.json");
        std::fs::write(&path, b"{\"Users\":[]}").unwrap();
        assert!(read(&path).is_ok());
        assert!(read(Path::new("relative.json")).is_err());
    }
}
