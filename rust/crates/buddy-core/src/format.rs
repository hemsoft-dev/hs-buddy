//! Number and duration formatting matching the Electron renderer's output
//! (`toLocaleString`, `formatCurrency`, `formatUptime`, `formatPrice`).
//!
//! Digits, separators, the USD symbol and month names follow the system
//! locale through ICU4X, as `Intl.NumberFormat(undefined, ...)` and
//! `toLocaleDateString(undefined, ...)` do in the renderer.

use fixed_decimal::{Decimal, FloatPrecision, Sign, SignedRoundingMode, UnsignedRoundingMode};
use icu_calendar::Date;
use icu_datetime::DateTimeFormatter;
use icu_datetime::fieldsets::{T, YM};
use icu_datetime::options::Alignment;
use icu_decimal::DecimalFormatter;
use icu_decimal::options::DecimalFormatterOptions;
use icu_experimental::dimension::currency::CurrencyType;
use icu_experimental::dimension::currency::formatter::CurrencyFormatter;
use icu_experimental::dimension::currency::options::CurrencyFormatterOptions;
use icu_locale_core::{Locale, locale};
use icu_time::Time;

/// Locale-aware number and date formatting.
pub struct LocaleFormat {
    formatter: DecimalFormatter,
    usd: CurrencyFormatter<DecimalFormatter>,
    month_year: DateTimeFormatter<YM>,
    clock: DateTimeFormatter<T>,
}

thread_local! {
    // ICU4X formatters hold `Rc` data, so the shared instance is per thread.
    static SYSTEM: LocaleFormat = LocaleFormat::system();
}

/// JavaScript's `toFixed`/`Intl` tie rule: halves round away from zero.
const HALF_EXPAND: SignedRoundingMode =
    SignedRoundingMode::Unsigned(UnsignedRoundingMode::HalfExpand);

impl LocaleFormat {
    pub fn for_locale(locale: &Locale) -> Self {
        Self::try_for_locale(locale).unwrap_or_else(|err| {
            log::warn!("no number formatting data for {locale}: {err}; using en-US");
            Self::try_for_locale(&locale!("en-US")).expect("en-US number data is compiled in")
        })
    }

    fn try_for_locale(locale: &Locale) -> Result<Self, String> {
        let formatter =
            DecimalFormatter::try_new(locale.into(), DecimalFormatterOptions::default())
                .map_err(|err| err.to_string())?;
        let usd = CurrencyFormatter::try_new_symbol(
            locale.into(),
            CurrencyType::try_from_str("USD").expect("USD is a valid ISO 4217 code"),
            CurrencyFormatterOptions::default(),
        )
        .map_err(|err| err.to_string())?;
        let month_year = DateTimeFormatter::try_new(locale.into(), YM::medium())
            .map_err(|err| err.to_string())?;
        let clock =
            DateTimeFormatter::try_new(locale.into(), T::hms().with_alignment(Alignment::Column))
                .map_err(|err| err.to_string())?;
        Ok(Self {
            formatter,
            usd,
            month_year,
            clock,
        })
    }

    /// The process locale (`LANG`, the Windows user locale, macOS
    /// preferences), en-US when it is unset or unparsable.
    pub fn system() -> Self {
        let locale = sys_locale::get_locale()
            .and_then(|tag| Locale::try_from_str(&tag).ok())
            .unwrap_or(locale!("en-US"));
        Self::for_locale(&locale)
    }

    /// `n.toLocaleString()` for an integer.
    pub fn thousands(&self, n: i64) -> String {
        self.formatter.format(&Decimal::from(n)).to_string()
    }

    /// A number with grouped integer digits and exactly `decimals` fraction digits.
    pub fn decimal(&self, value: f64, decimals: usize) -> String {
        if !value.is_finite() {
            return non_finite(value).to_string();
        }
        let position = -(decimals.min(i16::MAX as usize) as i16);
        let Ok(number) = Decimal::try_from_f64(value, FloatPrecision::RoundTrip) else {
            return format!("{value:.decimals$}");
        };
        let mut number = number.rounded_with_mode(position, HALF_EXPAND);
        number.absolute.pad_end(position);
        if value == 0.0 {
            number.sign = Sign::None;
        }
        self.formatter.format(&number).to_string()
    }

    /// `formatCurrency` from `quotaUtils.ts`: `Intl.NumberFormat` with
    /// `style: "currency", currency: "USD"`, so the symbol, its placement
    /// and the separators are the locale's (`$1,234.57`, `1.234,57 $`,
    /// `1 234,57 $US`).
    pub fn currency(&self, amount: f64) -> String {
        if !amount.is_finite() {
            return match non_finite(amount) {
                "NaN" => "NaN".to_string(),
                "∞" => "$∞".to_string(),
                _ => "-$∞".to_string(),
            };
        }
        let Ok(number) = Decimal::try_from_f64(amount, FloatPrecision::RoundTrip) else {
            return format!("${amount:.2}");
        };
        let mut number = number.rounded_with_mode(-2, HALF_EXPAND);
        number.absolute.pad_end(-2);
        self.usd.format_fixed_decimal(&number).to_string()
    }

    /// `formatPrice` from `FinanceCard.tsx`: two decimals at or above 1000,
    /// otherwise up to four significant fraction digits with trailing zeros
    /// trimmed down to two.
    pub fn price(&self, value: f64) -> String {
        if !value.is_finite() {
            return "—".to_string();
        }
        if value >= 1000.0 {
            return self.decimal(value, 2);
        }
        let Ok(number) = Decimal::try_from_f64(value, FloatPrecision::RoundTrip) else {
            return format!("{value:.2}");
        };
        let mut number = number.rounded_with_mode(-4, HALF_EXPAND);
        number.absolute.trim_end();
        number.absolute.pad_end(-2);
        self.formatter.format(&number).to_string()
    }

    /// `toLocaleDateString(undefined, { month: "short", year: "numeric" })`:
    /// the locale's medium year-month (`Oct 2026`, `oct. 2026`, `10/2026`
    /// for German, `2026/10` for Japanese). The full date is passed so a
    /// locale on a non-Gregorian calendar lands in the right month and year.
    pub fn month_year(&self, date: jiff::civil::Date) -> Option<String> {
        let date = Date::try_new_iso(
            date.year().into(),
            u8::try_from(date.month()).ok()?,
            u8::try_from(date.day()).ok()?,
        )
        .ok()?;
        Some(self.month_year.format(&date).to_string())
    }

    /// `toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit",
    /// second: "2-digit" })`: the locale's hour cycle (`03:04:05 PM`,
    /// `15:04:05`).
    pub fn clock(&self, time: jiff::civil::Time) -> Option<String> {
        let time = Time::try_new(
            u8::try_from(time.hour()).ok()?,
            u8::try_from(time.minute()).ok()?,
            u8::try_from(time.second()).ok()?,
            0,
        )
        .ok()?;
        Some(self.clock.format(&time).to_string())
    }
}

/// JavaScript's spelling of the values a `Decimal` cannot hold.
fn non_finite(value: f64) -> &'static str {
    if value.is_nan() {
        "NaN"
    } else if value > 0.0 {
        "∞"
    } else {
        "-∞"
    }
}

/// `n.toLocaleString()` in the system locale.
pub fn thousands(n: i64) -> String {
    SYSTEM.with(|f| f.thousands(n))
}

/// Grouped digits with exactly `decimals` fraction digits, system locale.
pub fn decimal(value: f64, decimals: usize) -> String {
    SYSTEM.with(|f| f.decimal(value, decimals))
}

/// `formatCurrency`: USD with two fraction digits, system locale digits.
pub fn currency(amount: f64) -> String {
    SYSTEM.with(|f| f.currency(amount))
}

/// `formatPrice`, system locale digits.
pub fn price(value: f64) -> String {
    SYSTEM.with(|f| f.price(value))
}

/// Short month and year of `date` in the system locale.
pub fn month_year(date: jiff::civil::Date) -> Option<String> {
    SYSTEM.with(|f| f.month_year(date))
}

/// Wall-clock time with seconds in the system locale's hour cycle.
pub fn clock(time: jiff::civil::Time) -> Option<String> {
    SYSTEM.with(|f| f.clock(time))
}

const SECOND_MS: u64 = 1_000;
const MINUTE_MS: u64 = 60 * SECOND_MS;
const HOUR_MS: u64 = 60 * MINUTE_MS;
const DAY_MS: u64 = 24 * HOUR_MS;

fn with_remainder(value: u64, unit: &str, remainder: u64, remainder_unit: &str) -> String {
    if remainder > 0 {
        format!("{value}{unit} {remainder}{remainder_unit}")
    } else {
        format!("{value}{unit}")
    }
}

/// `formatUptime` from `dateUtils.ts`.
pub fn uptime(ms: u64) -> String {
    if ms == 0 {
        return "0s".to_string();
    }
    if ms < MINUTE_MS {
        return format!("{}s", ms / SECOND_MS);
    }
    if ms < HOUR_MS {
        return format!("{}m", ms / MINUTE_MS);
    }
    if ms < DAY_MS {
        return with_remainder(ms / HOUR_MS, "h", (ms % HOUR_MS) / MINUTE_MS, "m");
    }
    with_remainder(ms / DAY_MS, "d", (ms % DAY_MS) / HOUR_MS, "h")
}

/// Relative "x ago" label for refresh status lines.
pub fn ago(ms: u64) -> String {
    if ms < MINUTE_MS {
        "just now".to_string()
    } else if ms < HOUR_MS {
        format!("{}m ago", ms / MINUTE_MS)
    } else {
        format!("{}h ago", ms / HOUR_MS)
    }
}

/// Countdown label such as `4m 12s` or `45s`, rounding partial seconds up so
/// the label never shows `0s` while time remains.
pub fn countdown(ms: u64) -> String {
    let total_seconds = ms.div_ceil(SECOND_MS);
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    if minutes > 0 {
        format!("{minutes}m {seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn en_us() -> LocaleFormat {
        LocaleFormat::for_locale(&locale!("en-US"))
    }

    #[test]
    fn formats_the_clock_in_the_locale_hour_cycle() {
        let time = jiff::civil::time(15, 4, 5, 0);
        assert_eq!(en_us().clock(time).as_deref(), Some("03:04:05\u{202f}PM"));
        let de = LocaleFormat::for_locale(&locale!("de-DE"));
        assert_eq!(de.clock(time).as_deref(), Some("15:04:05"));
        let h24 = LocaleFormat::for_locale(&locale!("en-US-u-hc-h23"));
        assert_eq!(h24.clock(time).as_deref(), Some("15:04:05"));
    }

    #[test]
    fn formats_month_and_year_like_to_locale_date_string() {
        let day = jiff::civil::date(2026, 10, 7);
        assert_eq!(en_us().month_year(day).as_deref(), Some("Oct 2026"));
        let fr = LocaleFormat::for_locale(&locale!("fr-FR"));
        assert_eq!(fr.month_year(day).as_deref(), Some("oct. 2026"));
        let de = LocaleFormat::for_locale(&locale!("de-DE"));
        assert_eq!(de.month_year(day).as_deref(), Some("10/2026"));
        // A locale on another calendar places the day in its own month/year.
        let thai = LocaleFormat::for_locale(&locale!("th-TH-u-ca-buddhist"));
        assert_eq!(thai.month_year(day).as_deref(), Some("ต.ค. 2569"));
    }

    #[test]
    fn groups_thousands() {
        let f = en_us();
        assert_eq!(f.thousands(0), "0");
        assert_eq!(f.thousands(999), "999");
        assert_eq!(f.thousands(1_000), "1,000");
        assert_eq!(f.thousands(1_234_567), "1,234,567");
        assert_eq!(f.thousands(-1_234), "-1,234");
    }

    #[test]
    fn formats_currency_like_intl() {
        let f = en_us();
        assert_eq!(f.currency(0.0), "$0.00");
        assert_eq!(f.currency(12.4), "$12.40");
        assert_eq!(f.currency(1234.567), "$1,234.57");
        assert_eq!(f.currency(-3.5), "-$3.50");
        assert_eq!(f.currency(-0.0), "-$0.00");
        assert_eq!(f.decimal(-0.0, 2), "0.00");
        assert_eq!(f.currency(f64::INFINITY), "$∞");
        assert_eq!(f.currency(f64::NEG_INFINITY), "-$∞");
        assert_eq!(f.currency(f64::NAN), "NaN");
        assert_eq!(f.decimal(f64::INFINITY, 2), "∞");
    }

    #[test]
    fn formats_prices_like_finance_card() {
        let f = en_us();
        assert_eq!(f.price(5234.18), "5,234.18");
        assert_eq!(f.price(63_412.9), "63,412.90");
        assert_eq!(f.price(0.1234), "0.1234");
        assert_eq!(f.price(12.5), "12.50");
        assert_eq!(f.price(f64::NAN), "—");
    }

    #[test]
    fn follows_the_locale_separators() {
        let de = LocaleFormat::for_locale(&locale!("de-DE"));
        assert_eq!(de.thousands(1_234_567), "1.234.567");
        assert_eq!(de.decimal(1234.5, 2), "1.234,50");
        assert_eq!(de.currency(1234.567), "1.234,57\u{a0}$");
        assert_eq!(de.currency(-3.5), "-3,50\u{a0}$");
        assert_eq!(de.price(0.1234), "0,1234");
        let fr = LocaleFormat::for_locale(&locale!("fr-FR"));
        assert_eq!(fr.decimal(1234.5, 2), "1\u{202f}234,50");
        assert_eq!(fr.currency(1234.567), "1\u{202f}234,57\u{a0}$US");
        let ja = LocaleFormat::for_locale(&locale!("ja-JP"));
        assert_eq!(ja.currency(1234.567), "$1,234.57");
        let hi = LocaleFormat::for_locale(&locale!("hi-IN"));
        assert_eq!(hi.thousands(12_345_678), "1,23,45,678");
    }

    #[test]
    fn unknown_locales_fall_back_to_en_us() {
        let f = LocaleFormat::for_locale(&locale!("xx-ZZ"));
        assert_eq!(f.thousands(1_000), "1,000");
    }

    #[test]
    fn countdown_rounds_up_partial_seconds() {
        assert_eq!(countdown(0), "0s");
        assert_eq!(countdown(1), "1s");
        assert_eq!(countdown(59_001), "1m 00s");
        assert_eq!(countdown(4 * MINUTE_MS + 12 * SECOND_MS), "4m 12s");
    }

    #[test]
    fn formats_uptime() {
        assert_eq!(uptime(0), "0s");
        assert_eq!(uptime(45_000), "45s");
        assert_eq!(uptime(5 * MINUTE_MS), "5m");
        assert_eq!(uptime(HOUR_MS + 5 * MINUTE_MS), "1h 5m");
        assert_eq!(uptime(2 * HOUR_MS), "2h");
        assert_eq!(uptime(3 * DAY_MS + 4 * HOUR_MS), "3d 4h");
    }
}
