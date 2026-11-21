//! Date and time text exactly as the services and saved files spell it.
//!
//! API bodies and recovery files were first written with chrono, and readers
//! compare them byte for byte. jiff's own parsers accept more (basic ISO 8601,
//! zone annotations, missing seconds) and print `Z` with trimmed fractions, so
//! this module keeps the earlier grammar and spelling.

use jiff::civil::{Date, DateTime, Time};
use jiff::tz::{Offset, TimeZone};
use jiff::{Timestamp, Zoned};

/// A value parsed from text. jiff has no 60th second, so `:60` reads as `:59`
/// and is flagged for callers that must refuse it.
pub struct Parsed<T> {
    pub value: T,
    pub leap_second: bool,
}
impl<T> Parsed<T> {
    /// The value, unless the text named a leap second.
    pub fn exact(self) -> Option<T> {
        (!self.leap_second).then_some(self.value)
    }
}

struct Scan<'a>(&'a str);
impl Scan<'_> {
    fn digits(&mut self, width: usize) -> Option<i64> {
        let length = self
            .0
            .bytes()
            .take(width)
            .take_while(u8::is_ascii_digit)
            .count();
        let value = self.0[..length].parse().ok()?;
        self.0 = &self.0[length..];
        Some(value)
    }
    /// A numeric field: any leading whitespace, then one to `width` digits.
    fn number(&mut self, width: usize) -> Option<i64> {
        self.0 = self.0.trim_start();
        self.digits(width)
    }
    /// A sign lifts the four digit limit of a year.
    fn year(&mut self) -> Option<i64> {
        self.0 = self.0.trim_start();
        if let Some(rest) = self.0.strip_prefix('-') {
            self.0 = rest;
            self.digits(usize::MAX)?.checked_neg()
        } else if let Some(rest) = self.0.strip_prefix('+') {
            self.0 = rest;
            self.digits(usize::MAX)
        } else {
            self.digits(4)
        }
    }
    fn literal(&mut self, text: char) -> Option<()> {
        self.0 = self.0.strip_prefix(text)?;
        Some(())
    }
    /// An optional `.` with at least one digit. Digits past the ninth are dropped.
    fn fraction(&mut self) -> Option<i32> {
        let Some(rest) = self.0.strip_prefix('.') else {
            return Some(0);
        };
        let length = rest.bytes().take_while(u8::is_ascii_digit).count();
        let kept = &rest[..length.min(9)];
        let scale = 10_i32.pow(9 - u32::try_from(kept.len()).ok()?);
        let nanoseconds = kept.parse::<i32>().ok()? * scale;
        self.0 = &rest[length..];
        Some(nanoseconds)
    }
    fn date(&mut self) -> Option<Date> {
        let year = self.year()?;
        self.literal('-')?;
        let month = self.number(2)?;
        self.literal('-')?;
        let day = self.number(2)?;
        Date::new(
            i16::try_from(year).ok()?,
            i8::try_from(month).ok()?,
            i8::try_from(day).ok()?,
        )
        .ok()
    }
    fn time(&mut self, seconds: bool, fraction: bool) -> Option<Parsed<Time>> {
        let hour = self.number(2)?;
        self.literal(':')?;
        let minute = self.number(2)?;
        let second = if seconds {
            self.literal(':')?;
            self.number(2)?
        } else {
            0
        };
        let nanosecond = if fraction { self.fraction()? } else { 0 };
        clock(hour, minute, second, nanosecond)
    }
    fn end(self) -> Option<()> {
        self.0.is_empty().then_some(())
    }
}

fn clock(hour: i64, minute: i64, second: i64, nanosecond: i32) -> Option<Parsed<Time>> {
    let leap_second = second == 60;
    let value = Time::new(
        i8::try_from(hour).ok()?,
        i8::try_from(minute).ok()?,
        i8::try_from(if leap_second { 59 } else { second }).ok()?,
        nanosecond,
    )
    .ok()?;
    Some(Parsed { value, leap_second })
}

/// `%Y-%m-%d`: padding and whitespace before a number are optional.
pub fn date(text: &str) -> Option<Date> {
    let mut scan = Scan(text);
    let date = scan.date()?;
    scan.end()?;
    Some(date)
}

/// A date as chrono's `NaiveDate::from_str` read it: [`date`], with
/// whitespace also allowed before each `-` and after the day.
pub fn typed_date(text: &str) -> Option<Date> {
    let mut scan = Scan(text);
    let year = scan.year()?;
    scan.0 = scan.0.trim_start();
    scan.literal('-')?;
    let month = scan.number(2)?;
    scan.0 = scan.0.trim_start();
    scan.literal('-')?;
    let day = scan.number(2)?;
    scan.0 = scan.0.trim_start();
    scan.end()?;
    Date::new(
        i16::try_from(year).ok()?,
        i8::try_from(month).ok()?,
        i8::try_from(day).ok()?,
    )
    .ok()
}

/// `%H:%M`.
pub fn hour_minute(text: &str) -> Option<Time> {
    let mut scan = Scan(text);
    let time = scan.time(false, false)?;
    scan.end()?;
    Some(time.value)
}

/// `%H:%M:%S`.
pub fn hour_minute_second(text: &str) -> Option<Parsed<Time>> {
    let mut scan = Scan(text);
    let time = scan.time(true, false)?;
    scan.end()?;
    Some(time)
}

/// `%H:%M:%S%.f`.
pub fn time(text: &str) -> Option<Parsed<Time>> {
    let mut scan = Scan(text);
    let time = scan.time(true, true)?;
    scan.end()?;
    Some(time)
}

/// `%Y-%m-%dT%H:%M:%S%.f`, a wall time without an offset.
pub fn date_time(text: &str) -> Option<Parsed<DateTime>> {
    let mut scan = Scan(text);
    let date = scan.date()?;
    scan.literal('T')?;
    let time = scan.time(true, true)?;
    scan.end()?;
    Some(Parsed {
        value: date.to_datetime(time.value),
        leap_second: time.leap_second,
    })
}

fn fixed(bytes: &[u8], start: usize, length: usize) -> Option<i16> {
    bytes[start..start + length]
        .iter()
        .try_fold(0, |sum, byte| {
            byte.is_ascii_digit()
                .then(|| sum * 10 + i16::from(byte - b'0'))
        })
}

/// RFC 3339 as section 5.6 writes it: four digit year, two digit fields, `T`,
/// `t` or a space between date and time, and `Z`, `z` or `±HH:MM` at the end.
/// The result keeps the written offset.
pub fn rfc3339(text: &str) -> Option<Parsed<Zoned>> {
    let bytes = text.as_bytes();
    if bytes.len() < 19
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't' | b' ')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let date = Date::new(
        fixed(bytes, 0, 4)?,
        i8::try_from(fixed(bytes, 5, 2)?).ok()?,
        i8::try_from(fixed(bytes, 8, 2)?).ok()?,
    )
    .ok()?;
    let hour = fixed(bytes, 11, 2)?;
    let minute = fixed(bytes, 14, 2)?;
    let second = fixed(bytes, 17, 2)?;
    // Every byte so far is ASCII, so this is a character boundary.
    let mut scan = Scan(&text[19..]);
    let time = clock(hour.into(), minute.into(), second.into(), scan.fraction()?)?;
    let offset = offset(scan.0)?;
    let instant = offset.to_timestamp(date.to_datetime(time.value)).ok()?;
    Some(Parsed {
        value: Zoned::new(instant, TimeZone::fixed(offset)),
        leap_second: time.leap_second,
    })
}

fn offset(text: &str) -> Option<Offset> {
    if matches!(text, "Z" | "z") {
        return Some(Offset::UTC);
    }
    let (sign, rest) = if let Some(rest) = text.strip_prefix('+') {
        (1, rest)
    } else {
        // U+2212 MINUS SIGN is accepted beside the ASCII hyphen.
        (-1, text.strip_prefix(['-', '−'])?)
    };
    let bytes = rest.as_bytes();
    if bytes.len() != 5 || bytes[2] != b':' {
        return None;
    }
    let hours = fixed(bytes, 0, 2)?;
    let minutes = fixed(bytes, 3, 2)?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Offset::from_seconds(sign * (i32::from(hours) * 3600 + i32::from(minutes) * 60)).ok()
}

fn seconds_text(at: DateTime) -> String {
    let year = at.year();
    // Years outside four digits carry an explicit sign.
    let year = if (0..=9999).contains(&year) {
        format!("{year:04}")
    } else {
        format!("{year:+05}")
    };
    format!(
        "{year}-{:02}-{:02}T{:02}:{:02}:{:02}",
        at.month(),
        at.day(),
        at.hour(),
        at.minute(),
        at.second()
    )
}

/// A UTC instant with a `+00:00` offset and no, three, six or nine fraction
/// digits, whichever is the shortest that loses nothing.
pub fn rfc3339_utc(at: Timestamp) -> String {
    let at = Offset::UTC.to_datetime(at);
    let nanosecond = at.subsec_nanosecond();
    let fraction = if nanosecond == 0 {
        String::new()
    } else if nanosecond % 1_000_000 == 0 {
        format!(".{:03}", nanosecond / 1_000_000)
    } else if nanosecond % 1_000 == 0 {
        format!(".{:06}", nanosecond / 1_000)
    } else {
        format!(".{nanosecond:09}")
    };
    format!("{}{fraction}+00:00", seconds_text(at))
}

/// A UTC instant as `YYYY-MM-DDTHH:MM:SS.mmmZ`, like JavaScript's `toISOString`.
pub fn rfc3339_millis(at: Timestamp) -> String {
    let at = Offset::UTC.to_datetime(at);
    format!("{}.{:03}Z", seconds_text(at), at.millisecond())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_allow_unpadded_numbers_whitespace_before_them_and_a_signed_year() {
        for (text, canonical) in [
            ("2026-10-16", "2026-10-16"),
            ("2026-1-6", "2026-01-06"),
            ("26-1-6", "0026-01-06"),
            (" 2026-10-16", "2026-10-16"),
            ("2026- 10-\t16", "2026-10-16"),
            ("2026-\u{a0}10-\u{3000}16", "2026-10-16"),
            ("+2026-10-16", "2026-10-16"),
            ("+002026-10-16", "2026-10-16"),
            ("0000-01-01", "0000-01-01"),
            ("9999-12-31", "9999-12-31"),
            ("2028-02-29", "2028-02-29"),
        ] {
            assert_eq!(date(text).unwrap().to_string(), canonical, "{text:?}");
        }
        assert_eq!(date("-0005-02-03"), Some(jiff::civil::date(-5, 2, 3)));
        assert_eq!(date("-9999-01-01"), Some(Date::MIN));
        for text in [
            "",
            "2026-10",
            "2026-10-16 ",
            "2026-10-16T00:00:00",
            "20261016",
            "2026/10/16",
            "2026 -10-16",
            "2026-10 -16",
            "2026--10-16",
            "2026-02-30",
            "2027-02-29",
            "2026-13-01",
            "2026-00-01",
            "2026-10-00",
            "2026-10-32",
            "2026-100-1",
            "2026-10-161",
            "12026-10-16",
            "+2026\u{2212}10-16",
            "２０２６-10-16",
            // Outside the years jiff can hold.
            "+10000-01-01",
            "-10000-12-31",
            "+99999999999999999999-01-01",
        ] {
            assert_eq!(date(text), None, "{text:?}");
        }
    }

    #[test]
    fn typed_dates_also_allow_whitespace_around_the_separators_and_at_the_end() {
        // chrono's `NaiveDate::from_str` accepted each of these in the Tempo
        // range fields; jiff's own `Date::from_str` refuses all but the first.
        for (text, canonical) in [
            ("2026-11-01", "2026-11-01"),
            ("2026-11-1", "2026-11-01"),
            (" 2026-11-01", "2026-11-01"),
            ("2026-11-01 ", "2026-11-01"),
            ("2026 - 11 - 1", "2026-11-01"),
            ("2026\t-11-01\n", "2026-11-01"),
            ("+2026-11-01", "2026-11-01"),
            ("26-1-6", "0026-01-06"),
        ] {
            assert_eq!(typed_date(text).unwrap().to_string(), canonical, "{text:?}");
        }
        // jiff's own parser accepts these; chrono refused them.
        for text in [
            "",
            "20261101",
            "2026-11-01T09:00",
            "2026-11-01 09:00",
            "2026-11-01[Europe/Paris]",
            "2026-11-01T09:00+02:00",
            "2026-11",
            "2026-11-01-",
            "2026-13-01",
            "2026-02-30",
            "2026-11-011",
            "+10000-01-01",
        ] {
            assert_eq!(typed_date(text), None, "{text:?}");
        }
    }

    #[test]
    fn times_allow_unpadded_numbers_and_flag_a_sixtieth_second() {
        for (text, canonical) in [
            ("08:15", "08:15:00"),
            ("8:5", "08:05:00"),
            (" 8: 5", "08:05:00"),
            ("23:59", "23:59:00"),
            ("00:00", "00:00:00"),
        ] {
            assert_eq!(
                hour_minute(text).unwrap().to_string(),
                canonical,
                "{text:?}"
            );
        }
        for text in [
            "", "8", "08:", "08:15:00", "24:00", "08:60", "08:15 ", "0815", "08.15", "008:15",
        ] {
            assert_eq!(hour_minute(text), None, "{text:?}");
        }
        for (text, canonical, leap_second) in [
            ("08:15:01", "08:15:01", false),
            ("8:5:1", "08:05:01", false),
            (" 8: 5: 1", "08:05:01", false),
            ("23:59:59", "23:59:59", false),
            ("23:59:60", "23:59:59", true),
            ("08:15:60", "08:15:59", true),
        ] {
            let parsed = hour_minute_second(text).unwrap();
            assert_eq!(parsed.value.to_string(), canonical, "{text:?}");
            assert_eq!(parsed.leap_second, leap_second, "{text:?}");
            assert_eq!(parsed.exact().is_some(), !leap_second, "{text:?}");
        }
        for text in [
            "",
            "08:15",
            "08:15:",
            "08:15:01.0",
            "08:15:01 ",
            "24:00:00",
            "08:60:00",
            "08:15:61",
            "081501",
            "08:15:001",
            "T08:15:01",
            "08:15:01Z",
        ] {
            assert!(hour_minute_second(text).is_none(), "{text:?}");
        }
        for (text, canonical, leap_second) in [
            ("08:15:01", "08:15:01", false),
            ("08:15:01.5", "08:15:01.5", false),
            ("08:15:01.050", "08:15:01.05", false),
            ("08:15:01.123456789", "08:15:01.123456789", false),
            ("08:15:01.1234567899999", "08:15:01.123456789", false),
            ("08:15:01.0000000009", "08:15:01", false),
            ("8:5:1.25", "08:05:01.25", false),
            ("23:59:60.5", "23:59:59.5", true),
        ] {
            let parsed = time(text).unwrap();
            assert_eq!(parsed.value.to_string(), canonical, "{text:?}");
            assert_eq!(parsed.leap_second, leap_second, "{text:?}");
        }
        for text in [
            "08:15",
            "08:15:01.",
            "08:15:01,5",
            "08:15:01 .5",
            "08:15:01.5 ",
            "08:15:01.x",
        ] {
            assert!(time(text).is_none(), "{text:?}");
        }
    }

    #[test]
    fn wall_times_need_an_upper_case_t_and_seconds() {
        for (text, canonical, leap_second) in [
            ("2026-10-16T12:00:00", "2026-10-16T12:00:00", false),
            ("2026-10-16T12:00:00.5", "2026-10-16T12:00:00.5", false),
            ("2026-1-6T1:2:3", "2026-01-06T01:02:03", false),
            (" 2026-10-16T 12: 00: 00", "2026-10-16T12:00:00", false),
            ("+2026-10-16T12:00:00", "2026-10-16T12:00:00", false),
            ("2026-10-16T12:00:60", "2026-10-16T12:00:59", true),
            (
                "9999-12-31T23:59:59.999999999",
                "9999-12-31T23:59:59.999999999",
                false,
            ),
        ] {
            let parsed = date_time(text).unwrap();
            assert_eq!(parsed.value.to_string(), canonical, "{text:?}");
            assert_eq!(parsed.leap_second, leap_second, "{text:?}");
        }
        for text in [
            "2026-10-16",
            "2026-10-16T12:00",
            "2026-10-16t12:00:00",
            "2026-10-16 12:00:00",
            "2026-10-16T12:00:00Z",
            "2026-10-16T12:00:00+00:00",
            "2026-10-16T24:00:00",
            "2026-02-30T12:00:00",
            "+10000-01-01T00:00:00",
        ] {
            assert!(date_time(text).is_none(), "{text:?}");
        }
    }

    #[test]
    fn rfc_3339_keeps_the_written_offset_and_refuses_every_looser_iso_8601_form() {
        for (text, utc, offset_seconds, leap_second) in [
            ("2026-10-16T12:00:00Z", "2026-10-16T12:00:00Z", 0, false),
            ("2026-10-16t12:00:00z", "2026-10-16T12:00:00Z", 0, false),
            ("2026-10-16 12:00:00Z", "2026-10-16T12:00:00Z", 0, false),
            (
                "2026-10-16T12:00:00+00:00",
                "2026-10-16T12:00:00Z",
                0,
                false,
            ),
            (
                "2026-10-16T12:00:00-00:00",
                "2026-10-16T12:00:00Z",
                0,
                false,
            ),
            (
                "2026-10-16T12:00:00.5+05:30",
                "2026-10-16T06:30:00.5Z",
                19800,
                false,
            ),
            (
                "2026-10-16T12:00:00-04:00",
                "2026-10-16T16:00:00Z",
                -14400,
                false,
            ),
            (
                "2026-10-16T12:00:00\u{2212}04:00",
                "2026-10-16T16:00:00Z",
                -14400,
                false,
            ),
            (
                "2026-10-16T12:00:00.123456789987-23:59",
                "2026-10-17T11:59:00.123456789Z",
                -86340,
                false,
            ),
            (
                "2026-10-16T12:00:00+23:59",
                "2026-10-15T12:01:00Z",
                86340,
                false,
            ),
            ("2016-12-31T23:59:60Z", "2016-12-31T23:59:59Z", 0, true),
            (
                "2026-10-16T12:30:60.25+01:00",
                "2026-10-16T11:30:59.25Z",
                3600,
                true,
            ),
            ("0000-01-01T00:00:00Z", "0000-01-01T00:00:00Z", 0, false),
            (
                "0000-01-01T00:00:00+23:59",
                "-000001-12-31T00:01:00Z",
                86340,
                false,
            ),
            (
                "9999-12-30T22:00:00.999999999Z",
                "9999-12-30T22:00:00.999999999Z",
                0,
                false,
            ),
            (
                "9999-12-31T21:59:00+23:59",
                "9999-12-30T22:00:00Z",
                86340,
                false,
            ),
        ] {
            let parsed = rfc3339(text).unwrap_or_else(|| panic!("{text:?} was refused"));
            assert_eq!(parsed.value.timestamp().to_string(), utc, "{text:?}");
            assert_eq!(parsed.value.offset().seconds(), offset_seconds, "{text:?}");
            assert_eq!(parsed.leap_second, leap_second, "{text:?}");
        }
        assert_eq!(
            rfc3339("0001-01-01T00:00:00+14:00").unwrap().value.year(),
            1
        );
        for text in [
            "",
            "2026-10-16",
            "2026-10-16T12:00:00",
            "2026-10-16T12:00Z",
            "2026-10-16T12Z",
            "20261016T120000Z",
            "2026-10-16T120000Z",
            "2026-10-16T12:00:00+0530",
            "2026-10-16T12:00:00+05",
            "2026-10-16T12:00:00+5:30",
            "2026-10-16T12:00:00+05:3",
            "2026-10-16T12:00:00+05:300",
            "2026-10-16T12:00:00+05:60",
            "2026-10-16T12:00:00+24:00",
            "2026-10-16T12:00:00+00:00:00",
            "2026-10-16T12:00:00 Z",
            "2026-10-16T12:00:00Z ",
            " 2026-10-16T12:00:00Z",
            "2026-10-16T12:00:00UTC",
            "2026-10-16T12:00:00Z[UTC]",
            "2026-10-16T12:00:00+00:00[Europe/London]",
            "2026-10-16T12:00:00,5Z",
            "2026-10-16T12:00:00.Z",
            "2026-10-16T12:00:00.5",
            "2026-10-16T24:00:00Z",
            "2026-10-16T12:60:00Z",
            "2026-10-16T12:00:61Z",
            "2026-02-30T12:00:00Z",
            "2026-1-16T12:00:00Z",
            "2026-10-16T1:00:00Z",
            "+002026-10-16T12:00:00Z",
            "-0001-10-16T12:00:00Z",
            "2026-10-16T12:00:00é",
            "2026-10-16T12:00:0\u{661}Z",
            // After the last instant jiff can hold.
            "9999-12-30T22:00:01Z",
            "9999-12-31T23:59:59Z",
            "9999-12-31T23:59:59-23:59",
        ] {
            assert!(rfc3339(text).is_none(), "{text:?}");
        }
    }

    #[test]
    fn instants_are_written_in_utc_with_grouped_fraction_digits() {
        for (instant, with_offset, milliseconds) in [
            (
                "2026-10-16T02:00:00Z",
                "2026-10-16T02:00:00+00:00",
                "2026-10-16T02:00:00.000Z",
            ),
            (
                "2026-10-16T02:00:00.1Z",
                "2026-10-16T02:00:00.100+00:00",
                "2026-10-16T02:00:00.100Z",
            ),
            (
                "2026-10-16T02:00:00.123Z",
                "2026-10-16T02:00:00.123+00:00",
                "2026-10-16T02:00:00.123Z",
            ),
            (
                "2026-10-16T02:00:00.1234Z",
                "2026-10-16T02:00:00.123400+00:00",
                "2026-10-16T02:00:00.123Z",
            ),
            (
                "2026-10-16T02:00:00.123456Z",
                "2026-10-16T02:00:00.123456+00:00",
                "2026-10-16T02:00:00.123Z",
            ),
            (
                "2026-10-16T02:00:00.1239999Z",
                "2026-10-16T02:00:00.123999900+00:00",
                "2026-10-16T02:00:00.123Z",
            ),
            (
                "2026-10-16T02:00:00.000000001Z",
                "2026-10-16T02:00:00.000000001+00:00",
                "2026-10-16T02:00:00.000Z",
            ),
            (
                "1969-12-31T23:59:58.5Z",
                "1969-12-31T23:59:58.500+00:00",
                "1969-12-31T23:59:58.500Z",
            ),
            (
                "0001-01-01T00:00:00Z",
                "0001-01-01T00:00:00+00:00",
                "0001-01-01T00:00:00.000Z",
            ),
            (
                "0000-12-31T19:00:00Z",
                "0000-12-31T19:00:00+00:00",
                "0000-12-31T19:00:00.000Z",
            ),
            (
                "-000001-12-31T00:01:00Z",
                "-0001-12-31T00:01:00+00:00",
                "-0001-12-31T00:01:00.000Z",
            ),
            (
                "-009999-01-02T01:59:59Z",
                "-9999-01-02T01:59:59+00:00",
                "-9999-01-02T01:59:59.000Z",
            ),
            (
                "9999-12-30T22:00:00.999999999Z",
                "9999-12-30T22:00:00.999999999+00:00",
                "9999-12-30T22:00:00.999Z",
            ),
        ] {
            let at: Timestamp = instant.parse().unwrap();
            assert_eq!(rfc3339_utc(at), with_offset);
            assert_eq!(rfc3339_millis(at), milliseconds);
        }
    }
}
