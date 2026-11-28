//! Market quotes for the Finance card (`useFinance.ts`, `financeCalc.ts`,
//! `financeHandlers.ts`).

pub mod cache;

use serde::{Deserialize, Serialize};

use crate::http::{describe_error, encode_component};

/// Serialized with Electron's `QuoteData` field names (`changePercent`, …).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteData {
    pub symbol: String,
    pub name: String,
    pub price: f64,
    pub change: f64,
    pub change_percent: f64,
    pub previous_close: f64,
    pub market_open: bool,
}

impl QuoteData {
    pub fn is_up(&self) -> bool {
        self.change >= 0.0
    }

    /// `+1.23 (+0.45%)` or `-1.23 (-0.45%)`.
    pub fn change_text(&self) -> String {
        fn sign(value: f64) -> &'static str {
            if value >= 0.0 { "+" } else { "" }
        }
        format!(
            "{}{:.2} ({}{:.2}%)",
            sign(self.change),
            self.change,
            sign(self.change_percent),
            self.change_percent
        )
    }
}

/// Friendly names for common symbols (`SYMBOL_NAMES`).
pub fn symbol_name(symbol: &str) -> Option<&'static str> {
    Some(match symbol {
        "^GSPC" => "S&P 500",
        "^IXIC" => "NASDAQ",
        "^DJI" => "DOW",
        "^RUT" => "Russell 2000",
        "^VIX" => "VIX",
        "BTC-USD" => "Bitcoin",
        "ETH-USD" => "Ethereum",
        "GC=F" => "Gold",
        "SI=F" => "Silver",
        "CL=F" => "Crude Oil",
        _ => return None,
    })
}

/// `normalizeSymbol`: trimmed and upper-cased.
pub fn normalize_symbol(symbol: &str) -> String {
    symbol.trim().to_ascii_uppercase()
}

/// `isValidSymbol`: `^[A-Z0-9^.=-]{1,20}$`.
pub fn is_valid_symbol(symbol: &str) -> bool {
    !symbol.is_empty()
        && symbol.len() <= 20
        && symbol.chars().all(|c| {
            c.is_ascii_uppercase() || c.is_ascii_digit() || matches!(c, '^' | '.' | '-' | '=')
        })
}

/// `buildYahooFinanceUrl`.
pub fn chart_url(symbol: &str) -> String {
    format!(
        "https://query1.finance.yahoo.com/v8/finance/chart/{}?interval=1d&range=1d",
        encode_component(symbol)
    )
}

#[derive(Debug, Deserialize)]
struct ChartResponse {
    chart: Chart,
}

#[derive(Debug, Deserialize)]
struct Chart {
    #[serde(default)]
    result: Option<Vec<ChartResult>>,
    #[serde(default)]
    error: Option<ChartError>,
}

#[derive(Debug, Deserialize)]
struct ChartError {
    #[serde(default)]
    description: String,
}

#[derive(Debug, Deserialize)]
struct ChartResult {
    meta: ChartMeta,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChartMeta {
    symbol: String,
    #[serde(default)]
    regular_market_price: Option<f64>,
    #[serde(default)]
    previous_close: Option<f64>,
    #[serde(default)]
    chart_previous_close: Option<f64>,
    #[serde(default)]
    short_name: Option<String>,
    #[serde(default)]
    current_trading_period: Option<TradingPeriods>,
}

#[derive(Debug, Deserialize)]
struct TradingPeriods {
    #[serde(default)]
    regular: Option<TradingPeriod>,
}

#[derive(Debug, Deserialize)]
struct TradingPeriod {
    #[serde(default)]
    start: Option<i64>,
    #[serde(default)]
    end: Option<i64>,
}

/// `calculateMarketOpen`: open unless a regular trading period says otherwise.
fn market_open(meta: &ChartMeta, now_epoch: i64) -> bool {
    match meta
        .current_trading_period
        .as_ref()
        .and_then(|p| p.regular.as_ref())
    {
        Some(TradingPeriod {
            start: Some(start),
            end: Some(end),
        }) => now_epoch >= *start && now_epoch < *end,
        _ => true,
    }
}

/// `parseChartResponse` + `buildQuoteFromMeta`.
pub fn parse_chart_response(json: &str, symbol: &str, now_epoch: i64) -> Result<QuoteData, String> {
    let response: ChartResponse =
        serde_json::from_str(json).map_err(|_| format!("Unreadable response for {symbol}"))?;
    if let Some(error) = response.chart.error {
        return Err(error.description);
    }
    let meta = response
        .chart
        .result
        .and_then(|results| results.into_iter().next())
        .map(|result| result.meta)
        .ok_or_else(|| format!("No data for {symbol}"))?;

    let price = meta.regular_market_price;
    let prev_close = meta.previous_close.or(meta.chart_previous_close);
    let (Some(price), Some(prev_close)) = (price, prev_close) else {
        return Err(format!("Incomplete data for {symbol}"));
    };
    let change = price - prev_close;
    let change_percent = if prev_close != 0.0 {
        change / prev_close * 100.0
    } else {
        0.0
    };
    let name = symbol_name(&meta.symbol)
        .map(str::to_string)
        .or_else(|| meta.short_name.clone())
        .unwrap_or_else(|| meta.symbol.clone());

    Ok(QuoteData {
        market_open: market_open(&meta, now_epoch),
        symbol: meta.symbol,
        name,
        price,
        change,
        change_percent,
        previous_close: prev_close,
    })
}

/// Fetch one quote (`FINANCE_FETCH_QUOTE`).
pub async fn fetch_quote(client: &reqwest::Client, symbol: &str) -> Result<QuoteData, String> {
    let symbol = normalize_symbol(symbol);
    if !is_valid_symbol(&symbol) {
        return Err(format!("Invalid symbol: {symbol}"));
    }
    let response = client
        .get(chart_url(&symbol))
        .send()
        .await
        .map_err(|err| describe_error(&err, "Fetch failed"))?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status().as_u16()));
    }
    let body = response
        .text()
        .await
        .map_err(|err| describe_error(&err, "Fetch failed"))?;
    parse_chart_response(&body, &symbol, jiff::Timestamp::now().as_second())
}

/// The outcome of fetching a watchlist: the quotes that arrived and the
/// symbols whose request failed, each with its error, in watchlist order.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct QuoteBatch {
    pub quotes: Vec<QuoteData>,
    pub failed: Vec<(String, String)>,
}

/// Fetch every symbol concurrently and report each outcome.
pub async fn fetch_quote_batch(client: &reqwest::Client, symbols: &[String]) -> QuoteBatch {
    let results = futures::future::join_all(symbols.iter().map(|s| fetch_quote(client, s))).await;
    let mut batch = QuoteBatch::default();
    for (symbol, result) in symbols.iter().zip(results) {
        match result {
            Ok(quote) => batch.quotes.push(quote),
            Err(err) => batch.failed.push((normalize_symbol(symbol), err)),
        }
    }
    batch
}

/// `fetchQuotes`: symbols that fail are dropped unless all fail, in which
/// case the first error is returned.
pub async fn fetch_quotes(
    client: &reqwest::Client,
    symbols: &[String],
) -> Result<Vec<QuoteData>, String> {
    let batch = fetch_quote_batch(client, symbols).await;
    match (batch.quotes.is_empty(), batch.failed.into_iter().next()) {
        (true, Some((_, err))) => Err(err),
        _ => Ok(batch.quotes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_change_text() {
        let up = QuoteData {
            symbol: "X".into(),
            name: "X".into(),
            price: 10.0,
            change: 1.234,
            change_percent: 0.456,
            previous_close: 8.8,
            market_open: true,
        };
        assert_eq!(up.change_text(), "+1.23 (+0.46%)");
        let down = QuoteData {
            change: -1.0,
            change_percent: -2.0,
            ..up.clone()
        };
        assert_eq!(down.change_text(), "-1.00 (-2.00%)");
        // A negative previous close flips the percent sign; each value keeps its own.
        let mixed = QuoteData {
            change: 3.0,
            change_percent: -3.0,
            ..up
        };
        assert_eq!(mixed.change_text(), "+3.00 (-3.00%)");
    }

    #[test]
    fn validates_symbols() {
        assert!(is_valid_symbol(&normalize_symbol(" aapl ")));
        assert!(is_valid_symbol("^GSPC"));
        assert!(is_valid_symbol("BTC-USD"));
        assert!(!is_valid_symbol(""));
        assert!(!is_valid_symbol("WAY-TOO-LONG-SYMBOL-XYZ"));
        assert!(!is_valid_symbol("bad symbol"));
        assert!(!is_valid_symbol("aapl"));
    }

    const CHART_FIXTURE: &str = r#"{"chart":{"result":[{"meta":{"currency":"USD","symbol":"^GSPC",
      "regularMarketPrice":5234.18,"chartPreviousClose":5212.31,"previousClose":5212.31,
      "shortName":"S&P 500","currentTradingPeriod":{"regular":{"start":1760000000,"end":1760023400}}}}],
      "error":null}}"#;

    #[test]
    fn parses_yahoo_chart_response() {
        let quote = parse_chart_response(CHART_FIXTURE, "^GSPC", 1_760_010_000).unwrap();
        assert_eq!(quote.symbol, "^GSPC");
        assert_eq!(quote.name, "S&P 500");
        assert!((quote.change - 21.87).abs() < 1e-9);
        assert!((quote.change_percent - 0.4196).abs() < 1e-3);
        assert!(quote.market_open);
        let closed = parse_chart_response(CHART_FIXTURE, "^GSPC", 1_760_030_000).unwrap();
        assert!(!closed.market_open);
    }

    #[test]
    fn reports_yahoo_errors_and_missing_data() {
        let err = parse_chart_response(
            r#"{"chart":{"result":null,"error":{"code":"Not Found","description":"No data found"}}}"#,
            "NOPE",
            0,
        );
        assert_eq!(err.unwrap_err(), "No data found");
        let err = parse_chart_response(r#"{"chart":{"result":[{"meta":{"symbol":"X"}}]}}"#, "X", 0);
        assert_eq!(err.unwrap_err(), "Incomplete data for X");
        let err = parse_chart_response(r#"{"chart":{}}"#, "X", 0);
        assert_eq!(err.unwrap_err(), "No data for X");
    }

    #[test]
    fn chart_url_has_no_whitespace() {
        assert!(!chart_url("BTC-USD").contains(char::is_whitespace));
    }

    #[test]
    fn builds_encoded_chart_url() {
        assert_eq!(
            chart_url("^GSPC"),
            "https://query1.finance.yahoo.com/v8/finance/chart/%5EGSPC?interval=1d&range=1d"
        );
    }
}
