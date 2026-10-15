//! Pollen data (Google Pollen API) and label tables from `usePollen.ts` and
//! `pollenHandlers.ts`.

use serde::Deserialize;

use crate::http::describe_error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PollenType {
    Tree,
    Grass,
    Weed,
}

impl PollenType {
    pub const ALL: [PollenType; 3] = [PollenType::Tree, PollenType::Grass, PollenType::Weed];

    pub fn group_label(self) -> &'static str {
        match self {
            PollenType::Tree => "Trees",
            PollenType::Grass => "Grasses",
            PollenType::Weed => "Weeds",
        }
    }

    pub fn badge_label(self) -> &'static str {
        match self {
            PollenType::Tree => "Tree",
            PollenType::Grass => "Grass",
            PollenType::Weed => "Weed",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PollenSpecies {
    pub code: String,
    pub display_name: String,
    pub index: u8,
    pub category: String,
    pub in_season: bool,
    pub kind: PollenType,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PollenData {
    pub tree: u8,
    pub grass: u8,
    pub weed: u8,
    pub species: Vec<PollenSpecies>,
    pub health_recommendations: Vec<String>,
}

impl PollenData {
    pub fn index_for(&self, kind: PollenType) -> u8 {
        match kind {
            PollenType::Tree => self.tree,
            PollenType::Grass => self.grass,
            PollenType::Weed => self.weed,
        }
    }

    /// Species grouped by type in Trees, Grasses, Weeds order, skipping empty groups.
    pub fn species_by_type(&self) -> Vec<(PollenType, Vec<&PollenSpecies>)> {
        PollenType::ALL
            .into_iter()
            .filter_map(|kind| {
                let items: Vec<_> = self.species.iter().filter(|s| s.kind == kind).collect();
                (!items.is_empty()).then_some((kind, items))
            })
            .collect()
    }
}

const POLLEN_LABELS: [&str; 6] = ["None", "Very Low", "Low", "Medium", "High", "Very High"];

/// `getPollenLabel`: index 0..=5 to label.
pub fn level_label(index: u8) -> &'static str {
    POLLEN_LABELS[index.min(5) as usize]
}

/// `getPollenColor`: `None` means "use the muted text color".
pub fn level_color_hex(index: u8) -> Option<&'static str> {
    match index {
        0 => None,
        1 => Some("#4caf50"),
        2 => Some("#8bc34a"),
        3 => Some("#ffc107"),
        4 => Some("#ff9800"),
        _ => Some("#f44336"),
    }
}

// ── Google Pollen API ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum PollenError {
    /// No key configured: the card simply hides the pollen section.
    NoApiKey,
    Message(String),
}

#[derive(Debug, Deserialize, Default)]
struct IndexInfo {
    #[serde(default)]
    value: Option<f64>,
    #[serde(default)]
    category: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PollenTypeInfo {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    index_info: Option<IndexInfo>,
    #[serde(default)]
    health_recommendations: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
struct PlantDescription {
    #[serde(default, rename = "type")]
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlantInfo {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    index_info: Option<IndexInfo>,
    #[serde(default)]
    in_season: Option<bool>,
    #[serde(default)]
    plant_description: Option<PlantDescription>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DailyInfo {
    #[serde(default)]
    pollen_type_info: Vec<PollenTypeInfo>,
    #[serde(default)]
    plant_info: Vec<PlantInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ForecastResponse {
    #[serde(default)]
    daily_info: Vec<DailyInfo>,
}

fn parse_type(raw: Option<&str>) -> PollenType {
    match raw.map(str::to_ascii_uppercase).as_deref() {
        Some("GRASS") => PollenType::Grass,
        Some("WEED") => PollenType::Weed,
        _ => PollenType::Tree,
    }
}

fn index_value(info: Option<&IndexInfo>) -> u8 {
    info.and_then(|i| i.value)
        .map(|v| v.round().clamp(0.0, 255.0) as u8)
        .unwrap_or(0)
}

/// `parseGooglePollenResponse`: `Ok(None)` when the day has no pollen data.
pub fn parse_google_response(json: &str) -> Result<Option<PollenData>, String> {
    let response: ForecastResponse =
        serde_json::from_str(json).map_err(|_| "Unreadable pollen response".to_string())?;
    let Some(day) = response.daily_info.into_iter().next() else {
        return Ok(None);
    };
    if day.pollen_type_info.is_empty() && day.plant_info.is_empty() {
        return Ok(None);
    }

    let mut data = PollenData::default();
    for info in &day.pollen_type_info {
        let value = index_value(info.index_info.as_ref());
        match info.code.as_deref() {
            Some("TREE") => data.tree = value,
            Some("GRASS") => data.grass = value,
            Some("WEED") => data.weed = value,
            _ => {}
        }
        data.health_recommendations
            .extend(info.health_recommendations.iter().cloned());
    }
    for plant in day.plant_info {
        let (Some(code), Some(display_name)) = (plant.code, plant.display_name) else {
            continue;
        };
        data.species.push(PollenSpecies {
            code,
            display_name,
            index: index_value(plant.index_info.as_ref()),
            category: plant
                .index_info
                .as_ref()
                .and_then(|i| i.category.clone())
                .unwrap_or_else(|| "None".to_string()),
            in_season: plant.in_season.unwrap_or(false),
            kind: parse_type(
                plant
                    .plant_description
                    .as_ref()
                    .and_then(|d| d.kind.as_deref()),
            ),
        });
    }
    Ok(Some(data))
}

/// Google Pollen forecast URL; the key is the caller's trimmed API key.
pub fn pollen_url(latitude: f64, longitude: f64, api_key: &str) -> String {
    let api_key = crate::http::encode_component(api_key);
    format!(
        "https://pollen.googleapis.com/v1/forecast:lookup?key={api_key}&location.latitude={latitude}&location.longitude={longitude}&days=1"
    )
}

fn valid_coordinates(latitude: f64, longitude: f64) -> bool {
    latitude.is_finite()
        && longitude.is_finite()
        && (-90.0..=90.0).contains(&latitude)
        && (-180.0..=180.0).contains(&longitude)
}

/// `fetchPollenData`.
pub async fn fetch_pollen(
    client: &reqwest::Client,
    latitude: f64,
    longitude: f64,
    api_key: &str,
) -> Result<PollenData, PollenError> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err(PollenError::NoApiKey);
    }
    if !valid_coordinates(latitude, longitude) {
        return Err(PollenError::Message("Invalid location".to_string()));
    }
    let url = pollen_url(latitude, longitude, api_key);
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| PollenError::Message(describe_error(&err, "Pollen fetch failed")))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|err| PollenError::Message(describe_error(&err, "Pollen fetch failed")))?;
    if !status.is_success() {
        // Provider error text may reflect the API key. Report only trusted
        // status and recovery text, never an upstream body or request URL.
        return Err(PollenError::Message(format!(
            "Google Pollen API: HTTP {}. Check API enablement, key restrictions, and quota.",
            status.as_u16()
        )));
    }
    match parse_google_response(&body) {
        Ok(Some(data)) => Ok(data),
        Ok(None) => Err(PollenError::Message(
            "No pollen data available for this location".to_string(),
        )),
        Err(message) => Err(PollenError::Message(message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOGLE_FIXTURE: &str = r#"{"regionCode":"US","dailyInfo":[{"date":{"year":2026,"month":10,"day":7},
      "pollenTypeInfo":[
        {"code":"GRASS","displayName":"Grass","inSeason":true,"indexInfo":{"code":"UPI","displayName":"Universal Pollen Index","value":1,"category":"Very Low"},
         "healthRecommendations":["Pollen levels are very low right now."]},
        {"code":"TREE","displayName":"Tree","inSeason":false,"indexInfo":{"value":2,"category":"Low"}},
        {"code":"WEED","displayName":"Weed","inSeason":true,"indexInfo":{"value":3,"category":"Medium"}}],
      "plantInfo":[
        {"code":"RAGWEED","displayName":"Ragweed","inSeason":true,"indexInfo":{"value":3,"category":"Medium"},"plantDescription":{"type":"WEED"}},
        {"code":"OAK","displayName":"Oak","inSeason":false,"indexInfo":{"value":0,"category":"None"},"plantDescription":{"type":"TREE"}},
        {"code":"BROKEN","displayName":null}]}]}"#;

    #[test]
    fn parses_google_pollen_forecast() {
        let data = parse_google_response(GOOGLE_FIXTURE).unwrap().unwrap();
        assert_eq!((data.tree, data.grass, data.weed), (2, 1, 3));
        assert_eq!(data.health_recommendations.len(), 1);
        assert_eq!(data.species.len(), 2);
        assert_eq!(data.species[0].kind, PollenType::Weed);
        assert!(!data.species[1].in_season);
        assert!(
            parse_google_response(r#"{"dailyInfo":[]}"#)
                .unwrap()
                .is_none()
        );
        assert!(
            parse_google_response(r#"{"dailyInfo":[{}]}"#)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn pollen_url_has_no_whitespace_and_keeps_the_key_intact() {
        let url = pollen_url(35.8235, -78.8256, "AIza-test-key");
        assert!(!url.contains(char::is_whitespace), "{url}");
        assert!(url.contains("?key=AIza-test-key&location.latitude=35.8235"));
        let key = "fixture&key=π #fragment";
        let encoded = reqwest::Url::parse(&pollen_url(35.8235, -78.8256, key)).unwrap();
        let pairs: Vec<_> = encoded.query_pairs().collect();
        assert_eq!(pairs.iter().filter(|(name, _)| name == "key").count(), 1);
        assert_eq!(pairs[0].1, key);
        assert_eq!(encoded.fragment(), None);
    }

    #[test]
    fn labels_and_colors_follow_the_tables() {
        assert_eq!(level_label(0), "None");
        assert_eq!(level_label(3), "Medium");
        assert_eq!(level_label(9), "Very High");
        assert_eq!(level_color_hex(0), None);
        assert_eq!(level_color_hex(2), Some("#8bc34a"));
        assert_eq!(level_color_hex(5), Some("#f44336"));
    }

    #[test]
    fn groups_species_in_fixed_order_skipping_empty() {
        let data = PollenData {
            species: vec![
                PollenSpecies {
                    code: "RAGWEED".into(),
                    display_name: "Ragweed".into(),
                    index: 3,
                    category: "Medium".into(),
                    in_season: true,
                    kind: PollenType::Weed,
                },
                PollenSpecies {
                    code: "OAK".into(),
                    display_name: "Oak".into(),
                    index: 2,
                    category: "Low".into(),
                    in_season: true,
                    kind: PollenType::Tree,
                },
            ],
            ..Default::default()
        };
        let groups = data.species_by_type();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, PollenType::Tree);
        assert_eq!(groups[1].0, PollenType::Weed);
    }
}
