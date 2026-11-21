//! Weather data (Open-Meteo), geocoding (Nominatim), and the WMO weather-code
//! tables from `useWeather.ts` and `WeatherCard.tsx`.

use jiff::civil::Weekday;
use serde::Deserialize;

pub use crate::config::WeatherLocation;
use crate::http::{describe_error, encode_component};

#[derive(Debug, Clone, PartialEq)]
pub struct ForecastDay {
    /// ISO date, used as the row key.
    pub date: String,
    pub day_name: String,
    pub weather_code: u16,
    pub description: String,
    pub high: i32,
    pub low: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WeatherData {
    pub temperature: i32,
    pub temperature_unit: String,
    pub weather_code: u16,
    pub description: String,
    pub humidity: u8,
    pub wind_speed: i32,
    pub high: i32,
    pub low: i32,
    pub location_name: String,
    pub forecast: Vec<ForecastDay>,
}

/// Default location when nothing is remembered (`DEFAULT_LOCATION`).
pub fn default_location() -> WeatherLocation {
    WeatherLocation {
        latitude: 35.8235,
        longitude: -78.8256,
        name: DEFAULT_LOCATION_NAME.to_string(),
    }
}
pub const DEFAULT_LOCATION_NAME: &str = "Morrisville, NC";

/// WMO weather interpretation code to label (`weatherCodeToDescription`).
pub fn code_description(code: u16) -> &'static str {
    match code {
        0 => "Clear sky",
        1 => "Mainly clear",
        2 => "Partly cloudy",
        3 => "Overcast",
        45 => "Foggy",
        48 => "Depositing rime fog",
        51 => "Light drizzle",
        53 => "Moderate drizzle",
        55 => "Dense drizzle",
        61 => "Slight rain",
        63 => "Moderate rain",
        65 => "Heavy rain",
        71 => "Slight snow",
        73 => "Moderate snow",
        75 => "Heavy snow",
        80 => "Slight rain showers",
        81 => "Moderate rain showers",
        82 => "Violent rain showers",
        85 => "Slight snow showers",
        86 => "Heavy snow showers",
        95 => "Thunderstorm",
        96 => "Thunderstorm with hail",
        99 => "Thunderstorm with heavy hail",
        _ => "Unknown",
    }
}

/// Icon family for a weather code (`WEATHER_THRESHOLDS` in `WeatherCard.tsx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeatherGlyph {
    Sun,
    Cloud,
    Fog,
    Rain,
    Snow,
    Lightning,
}

pub fn code_glyph(code: u16) -> WeatherGlyph {
    match code {
        0 => WeatherGlyph::Sun,
        1..=3 => WeatherGlyph::Cloud,
        4..=48 => WeatherGlyph::Fog,
        49..=65 => WeatherGlyph::Rain,
        66..=75 => WeatherGlyph::Snow,
        76..=82 => WeatherGlyph::Rain,
        83..=86 => WeatherGlyph::Snow,
        _ => WeatherGlyph::Lightning,
    }
}

// ── Open-Meteo ──────────────────────────────────────────────────────────────

pub fn forecast_url(latitude: f64, longitude: f64) -> String {
    let params = [
        "current=temperature_2m,relative_humidity_2m,weather_code,wind_speed_10m",
        "daily=temperature_2m_max,temperature_2m_min,weather_code",
        "temperature_unit=fahrenheit",
        "wind_speed_unit=mph",
        "timezone=auto",
        "forecast_days=3",
    ]
    .join("&");
    format!(
        "https://api.open-meteo.com/v1/forecast?latitude={latitude}&longitude={longitude}&{params}"
    )
}

#[derive(Debug, Deserialize)]
struct ForecastResponse {
    current: Current,
    daily: Daily,
}

#[derive(Debug, Deserialize)]
struct Current {
    temperature_2m: f64,
    relative_humidity_2m: f64,
    weather_code: u16,
    wind_speed_10m: f64,
}

#[derive(Debug, Deserialize)]
struct Daily {
    time: Vec<String>,
    temperature_2m_max: Vec<f64>,
    temperature_2m_min: Vec<f64>,
    weather_code: Vec<u16>,
}

fn day_name(date: &str, index: usize) -> String {
    if index == 0 {
        return "Today".to_string();
    }
    crate::wire_time::date(date)
        .map(|d| match d.weekday() {
            Weekday::Monday => "Mon",
            Weekday::Tuesday => "Tue",
            Weekday::Wednesday => "Wed",
            Weekday::Thursday => "Thu",
            Weekday::Friday => "Fri",
            Weekday::Saturday => "Sat",
            Weekday::Sunday => "Sun",
        })
        .unwrap_or("")
        .to_string()
}

/// `Math.round` semantics: halves round toward positive infinity, so -2.5
/// becomes -2 as in the Electron renderer (Rust's `round` would give -3).
fn round(value: f64) -> i32 {
    (value + 0.5).floor() as i32
}

/// Port of the response mapping in `fetchWeather`.
pub fn parse_forecast(json: &str, location_name: &str) -> Result<WeatherData, String> {
    let response: ForecastResponse =
        serde_json::from_str(json).map_err(|_| "Unreadable weather response".to_string())?;
    let daily = response.daily;
    if daily.time.is_empty()
        || daily.temperature_2m_max.is_empty()
        || daily.temperature_2m_min.is_empty()
    {
        return Err("Weather response had no daily data".to_string());
    }
    let forecast = daily
        .time
        .iter()
        .enumerate()
        .filter_map(|(i, date)| {
            let code = *daily.weather_code.get(i)?;
            Some(ForecastDay {
                date: date.clone(),
                day_name: day_name(date, i),
                weather_code: code,
                description: code_description(code).to_string(),
                high: round(*daily.temperature_2m_max.get(i)?),
                low: round(*daily.temperature_2m_min.get(i)?),
            })
        })
        .collect();
    let current = response.current;
    Ok(WeatherData {
        temperature: round(current.temperature_2m),
        temperature_unit: "°F".to_string(),
        weather_code: current.weather_code,
        description: code_description(current.weather_code).to_string(),
        humidity: current.relative_humidity_2m.round().clamp(0.0, 100.0) as u8,
        wind_speed: round(current.wind_speed_10m),
        high: round(daily.temperature_2m_max[0]),
        low: round(daily.temperature_2m_min[0]),
        location_name: location_name.to_string(),
        forecast,
    })
}

pub async fn fetch_weather(
    client: &reqwest::Client,
    location: &WeatherLocation,
) -> Result<WeatherData, String> {
    let response = client
        .get(forecast_url(location.latitude, location.longitude))
        .send()
        .await
        .map_err(|err| describe_error(&err, "Weather fetch failed"))?;
    if !response.status().is_success() {
        return Err(format!("Weather API error: {}", response.status().as_u16()));
    }
    let body = response
        .text()
        .await
        .map_err(|err| describe_error(&err, "Weather fetch failed"))?;
    parse_forecast(&body, &location.name)
}

// ── Nominatim geocoding ─────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
struct NominatimAddress {
    #[serde(default)]
    city: Option<String>,
    #[serde(default)]
    town: Option<String>,
    #[serde(default)]
    village: Option<String>,
    #[serde(default)]
    state: Option<String>,
}

impl NominatimAddress {
    fn city(&self) -> Option<&str> {
        self.city
            .as_deref()
            .or(self.town.as_deref())
            .or(self.village.as_deref())
            .filter(|s| !s.is_empty())
    }

    /// `buildLocationName`: "City, State", "City", or the fallback.
    fn location_name(&self, fallback: &str) -> String {
        match (self.city(), self.state.as_deref().filter(|s| !s.is_empty())) {
            (Some(city), Some(state)) => format!("{city}, {state}"),
            (Some(city), None) => city.to_string(),
            (None, _) => fallback.to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct NominatimSearchResult {
    lat: String,
    lon: String,
    #[serde(default)]
    address: Option<NominatimAddress>,
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NominatimReverseResult {
    #[serde(default)]
    address: Option<NominatimAddress>,
}

/// `parseGeocodingResult` over the first search hit.
pub fn parse_search_result(json: &str, query: &str) -> Result<WeatherLocation, String> {
    let results: Vec<NominatimSearchResult> =
        serde_json::from_str(json).map_err(|_| "Unreadable geocoding response".to_string())?;
    let first = results
        .into_iter()
        .next()
        .ok_or_else(|| format!("No results for \"{query}\""))?;
    let fallback = first
        .display_name
        .as_deref()
        .and_then(|name| name.split(',').next())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(query)
        .trim()
        .to_string();
    let name = first.address.unwrap_or_default().location_name(&fallback);
    Ok(WeatherLocation {
        latitude: first
            .lat
            .parse()
            .map_err(|_| "Invalid latitude in geocoding response".to_string())?,
        longitude: first
            .lon
            .parse()
            .map_err(|_| "Invalid longitude in geocoding response".to_string())?,
        name,
    })
}

/// `setLocationBySearch`: forward geocode a city, state, or zip code.
pub async fn search_location(
    client: &reqwest::Client,
    query: &str,
) -> Result<WeatherLocation, String> {
    let query = query.trim();
    let url = format!(
        "https://nominatim.openstreetmap.org/search?q={}&format=json&limit=1&addressdetails=1",
        encode_component(query)
    );
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| describe_error(&err, "Location search failed"))?;
    if !response.status().is_success() {
        return Err(format!("Geocoding error: {}", response.status().as_u16()));
    }
    let body = response
        .text()
        .await
        .map_err(|err| describe_error(&err, "Location search failed"))?;
    parse_search_result(&body, query)
}

/// `reverseGeocodeLocation`: a "City, State" name for coordinates, if known.
pub async fn reverse_geocode(
    client: &reqwest::Client,
    latitude: f64,
    longitude: f64,
) -> Option<String> {
    let url = format!(
        "https://nominatim.openstreetmap.org/reverse?lat={latitude}&lon={longitude}&format=json"
    );
    let response = client.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let result: NominatimReverseResult = response.json().await.ok()?;
    let address = result.address?;
    address.city().map(|_| address.location_name(""))
}

#[derive(Debug, Deserialize)]
struct IpLocation {
    latitude: f64,
    longitude: f64,
    #[serde(default)]
    city: Option<String>,
    #[serde(default)]
    region: Option<String>,
}

/// Approximate "my location" from the public IP (the browser geolocation API
/// has no native equivalent).
pub async fn locate_by_ip(client: &reqwest::Client) -> Result<WeatherLocation, String> {
    let response = client
        .get("https://ipapi.co/json/")
        .send()
        .await
        .map_err(|err| describe_error(&err, "Location lookup failed"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Location lookup failed: HTTP {}",
            response.status().as_u16()
        ));
    }
    let ip: IpLocation = response
        .json()
        .await
        .map_err(|_| "Unreadable location response".to_string())?;
    let name = match (
        ip.city.as_deref().filter(|s| !s.is_empty()),
        ip.region.as_deref().filter(|s| !s.is_empty()),
    ) {
        (Some(city), Some(region)) => format!("{city}, {region}"),
        (Some(city), None) => city.to_string(),
        _ => format!("{:.2}°, {:.2}°", ip.latitude, ip.longitude),
    };
    Ok(WeatherLocation {
        latitude: ip.latitude,
        longitude: ip.longitude,
        name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_codes_to_glyphs_like_thresholds() {
        assert_eq!(code_glyph(0), WeatherGlyph::Sun);
        assert_eq!(code_glyph(2), WeatherGlyph::Cloud);
        assert_eq!(code_glyph(45), WeatherGlyph::Fog);
        assert_eq!(code_glyph(63), WeatherGlyph::Rain);
        assert_eq!(code_glyph(73), WeatherGlyph::Snow);
        assert_eq!(code_glyph(81), WeatherGlyph::Rain);
        assert_eq!(code_glyph(86), WeatherGlyph::Snow);
        assert_eq!(code_glyph(95), WeatherGlyph::Lightning);
    }

    #[test]
    fn describes_known_codes() {
        assert_eq!(code_description(2), "Partly cloudy");
        assert_eq!(code_description(999), "Unknown");
    }

    const FORECAST_FIXTURE: &str = r#"{
      "current":{"time":"2026-10-07T02:30","temperature_2m":71.6,"relative_humidity_2m":54.4,"weather_code":2,"wind_speed_10m":6.8},
      "daily":{"time":["2026-10-07","2026-10-08","2026-10-09"],
               "temperature_2m_max":[78.3,79.1,70.9],"temperature_2m_min":[60.8,59.6,58.2],"weather_code":[2,1,61]}}"#;

    #[test]
    fn rounds_like_math_round() {
        assert_eq!(round(-2.5), -2);
        assert_eq!(round(2.5), 3);
        assert_eq!(round(-2.6), -3);
        assert_eq!(round(71.6), 72);
    }

    #[test]
    fn forecast_url_has_no_whitespace() {
        let url = forecast_url(35.8235, -78.8256);
        assert!(!url.contains(char::is_whitespace), "{url}");
        assert!(url.contains("&daily=temperature_2m_max,temperature_2m_min,weather_code&"));
        assert!(url.ends_with("&forecast_days=3"));
    }

    #[test]
    fn parses_open_meteo_forecast() {
        let data = parse_forecast(FORECAST_FIXTURE, "Morrisville, NC").unwrap();
        assert_eq!(data.temperature, 72);
        assert_eq!(data.description, "Partly cloudy");
        assert_eq!(data.humidity, 54);
        assert_eq!(data.wind_speed, 7);
        assert_eq!((data.high, data.low), (78, 61));
        assert_eq!(data.forecast.len(), 3);
        assert_eq!(data.forecast[0].day_name, "Today");
        assert_eq!(data.forecast[1].day_name, "Thu");
        assert_eq!(data.forecast[2].description, "Slight rain");
    }

    #[test]
    fn forecast_days_are_named_from_their_date() {
        for (date, name) in [
            ("2026-10-12", "Mon"),
            ("2026-10-13", "Tue"),
            ("2026-10-14", "Wed"),
            ("2026-10-15", "Thu"),
            ("2026-10-16", "Fri"),
            ("2026-10-17", "Sat"),
            ("2026-10-18", "Sun"),
            ("2028-02-29", "Tue"),
            ("0001-01-01", "Mon"),
            ("9999-12-31", "Fri"),
            // Padding and leading whitespace are optional.
            ("2026-10-7", "Wed"),
            (" 2026-10-17", "Sat"),
            ("2026- 1- 5", "Mon"),
            ("+2026-10-17", "Sat"),
            ("0-1-1", "Sat"),
            ("-0001-01-01", "Fri"),
        ] {
            assert_eq!(day_name(date, 1), name, "{date:?}");
        }
        for date in [
            "",
            "2026-02-30",
            "2026-13-01",
            "20261017",
            "2026-10-17 ",
            "2026-10-17T00:00:00Z",
            "2026/10/17",
            "17 Oct 2026",
        ] {
            assert_eq!(day_name(date, 1), "", "{date:?}");
        }
        assert_eq!(day_name("not a date", 0), "Today");
    }

    #[test]
    fn parses_nominatim_search_result() {
        let json = r#"[{"lat":"35.7796","lon":"-78.6382","display_name":"Raleigh, Wake County, North Carolina, United States",
                        "address":{"city":"Raleigh","state":"North Carolina","country":"United States"}}]"#;
        let loc = parse_search_result(json, "raleigh").unwrap();
        assert_eq!(loc.name, "Raleigh, North Carolina");
        assert!((loc.latitude - 35.7796).abs() < 1e-6);
        let fallback = parse_search_result(
            r#"[{"lat":"1","lon":"2","display_name":"Somewhere, Far"}]"#,
            "q",
        )
        .unwrap();
        assert_eq!(fallback.name, "Somewhere");
        assert_eq!(
            parse_search_result("[]", "nowhere").unwrap_err(),
            "No results for \"nowhere\""
        );
    }
}
