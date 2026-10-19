//! Only documented Tempo cloud origins can receive a real bearer token.
use reqwest::Url;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum ApiOrigin {
    #[default]
    #[serde(rename = "https://api.tempo.io")]
    Global,
    #[serde(rename = "https://api.eu.tempo.io")]
    Europe,
    #[serde(rename = "https://api.us.tempo.io")]
    Americas,
}
impl ApiOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "https://api.tempo.io",
            Self::Europe => "https://api.eu.tempo.io",
            Self::Americas => "https://api.us.tempo.io",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Global => "Global",
            Self::Europe => "Europe",
            Self::Americas => "Americas",
        }
    }
    pub(super) fn base(self) -> Url {
        Url::parse(&format!("{}/4/", self.as_str())).expect("documented static HTTPS origin")
    }
    pub(super) fn matches(self, url: &Url) -> bool {
        url.scheme() == "https"
            && url.host_str() == self.base().host_str()
            && url.port_or_known_default() == Some(443)
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_the_captured_documented_region_accepts_a_reference() {
        for origin in [ApiOrigin::Global, ApiOrigin::Europe, ApiOrigin::Americas] {
            assert!(origin.matches(&origin.base()));
            for other in [ApiOrigin::Global, ApiOrigin::Europe, ApiOrigin::Americas] {
                assert_eq!(origin.matches(&other.base()), origin == other);
            }
            for invalid in [
                "https://api.tempo.io.evil.invalid",
                "http://api.tempo.io",
                "https://user@api.tempo.io",
                "https://api.tempo.io:444",
                "https://api.tempo.io/#fragment",
            ] {
                assert!(!origin.matches(&Url::parse(invalid).unwrap()));
                assert!(
                    serde_json::from_str::<ApiOrigin>(&serde_json::to_string(invalid).unwrap())
                        .is_err()
                );
            }
        }
    }
}
