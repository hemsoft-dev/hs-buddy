//! Five-field cron expressions as the shared `cronUtils.ts` accepts them
//! (`cron-parser` 5 behind Electron's stricter checks), and their future
//! occurrences in an IANA zone. The server stays the authority on `nextRunAt`;
//! this only lists upcoming times for display.
//!
//! Electron accepts `minute hour day-of-month month day-of-week`, the aliases
//! in [`PREDEFINED`], `*`, `?`, lists, ranges, steps and three-letter month and
//! weekday names, and rejects `L`, `W`-style `#` and hashed `H` values. When
//! both day fields are restricted, either one matching is enough, as in Vixie
//! cron. A wall time that a daylight-saving gap skips runs at the same offset
//! after the gap. A repeated wall time runs once, at its first instant, unless
//! every hour is allowed and the clock goes back a whole hour: cron-parser
//! tracks the repeated hour only for a restricted hour field
//! (`hoursLength !== 24`), so `*/15 * * * *` runs in both passes.
//!
//! cron-parser mishandles Lord Howe's 30-minute shifts: after its spring gap
//! it skips existing times such as 02:30 and 02:45, and after its autumn fold
//! it can stop listing occurrences altogether. This follows that zone's real
//! clock instead.
use jiff::civil::{Date, DateTime};
use jiff::tz::{AmbiguousOffset, TimeZone};
use jiff::{Timestamp, ToSpan as _};

/// `PREDEFINED_EXPRESSIONS` in `shared/utils/cronUtils.ts`.
const PREDEFINED: [(&str, &str); 9] = [
    ("@yearly", "0 0 1 1 *"),
    ("@annually", "0 0 1 1 *"),
    ("@monthly", "0 0 1 * *"),
    ("@weekly", "0 0 * * 0"),
    ("@daily", "0 0 * * *"),
    ("@hourly", "0 * * * *"),
    ("@minutely", "* * * * *"),
    ("@weekdays", "0 0 * * 1-5"),
    ("@weekends", "0 0 * * 0,6"),
];

/// An hour field that allows all 24 hours.
const ALL_HOURS: u32 = (1 << 24) - 1;

/// `cron-parser`'s `MAX_EXPRESSION_LENGTH`.
const MAX_LENGTH: usize = 1024;

/// cron-parser resolves both name sets in both fields (`Months[x] || DayOfWeek[x]`).
const NAMES: [(&str, u32); 19] = [
    ("jan", 1),
    ("feb", 2),
    ("mar", 3),
    ("apr", 4),
    ("may", 5),
    ("jun", 6),
    ("jul", 7),
    ("aug", 8),
    ("sep", 9),
    ("oct", 10),
    ("nov", 11),
    ("dec", 12),
    ("sun", 0),
    ("mon", 1),
    ("tue", 2),
    ("wed", 3),
    ("thu", 4),
    ("fri", 5),
    ("sat", 6),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidCron;

impl std::fmt::Display for InvalidCron {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Invalid cron expression")
    }
}
impl std::error::Error for InvalidCron {}

/// A parsed expression: one bit per allowed value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cron {
    minutes: u64,
    hours: u32,
    days: u32,
    months: u16,
    weekdays: u8,
    any_day: bool,
    any_weekday: bool,
}

#[derive(Clone, Copy)]
struct Field {
    min: u32,
    max: u32,
    names: bool,
    weekday: bool,
}

const MINUTE: Field = Field {
    min: 0,
    max: 59,
    names: false,
    weekday: false,
};
const HOUR: Field = Field {
    min: 0,
    max: 23,
    ..MINUTE
};
const DAY: Field = Field {
    min: 1,
    max: 31,
    ..MINUTE
};
const MONTH: Field = Field {
    min: 1,
    max: 12,
    names: true,
    weekday: false,
};
const WEEKDAY: Field = Field {
    min: 0,
    max: 7,
    names: true,
    weekday: true,
};

/// JavaScript's `parseInt(text, 10)`: an optional sign, then the leading digits.
fn parse_int(text: &str) -> Option<i64> {
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let end = digits
        .bytes()
        .position(|byte| !byte.is_ascii_digit())
        .unwrap_or(digits.len());
    // Long digit runs saturate; any such value is out of every field's range.
    let value = digits[..end]
        .bytes()
        .try_fold(0i64, |sum, digit| {
            Some(
                sum.saturating_mul(10)
                    .saturating_add(i64::from(digit - b'0')),
            )
        })
        .filter(|_| end > 0)?;
    Some(if negative { -value } else { value })
}

/// Replaces each run of exactly three letters with its number, as
/// `value.replace(/[a-z]{3}/gi, …)` does; an unknown name is invalid.
fn replace_names(value: &str) -> Result<String, InvalidCron> {
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut index = 0;
    while index < bytes.len() {
        if index + 3 <= bytes.len() && bytes[index..index + 3].iter().all(u8::is_ascii_alphabetic) {
            let name = value[index..index + 3].to_ascii_lowercase();
            let (_, number) = NAMES
                .iter()
                .find(|(known, _)| *known == name)
                .ok_or(InvalidCron)?;
            out.push_str(&number.to_string());
            index += 3;
        } else {
            out.push(char::from(bytes[index]));
            index += 1;
        }
    }
    Ok(out)
}

fn parse_field(raw: &str, field: Field) -> Result<u64, InvalidCron> {
    stored_bits(field_values(raw, field)?, field)
}

/// cron-parser checks for a repeated value only after it has parsed every
/// atom, and `CronDayOfMonth.fromMonth` first drops days a single month can't
/// have (unless none would remain), so `1,31,31` in February keeps only 1.
fn stored_bits(values: Vec<i64>, field: Field) -> Result<u64, InvalidCron> {
    let mut stored = 0u64;
    for number in values {
        if stored & (1 << number) != 0 {
            return Err(InvalidCron);
        }
        stored |= 1 << number;
    }
    let mut bits = stored;
    if field.weekday && bits & (1 << 7) != 0 {
        bits = (bits & !(1 << 7)) | 1;
    }
    Ok(bits)
}

fn field_values(raw: &str, field: Field) -> Result<Vec<i64>, InvalidCron> {
    let value = if field.names {
        replace_names(raw)?
    } else {
        raw.to_owned()
    };
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || b"?,*/-".contains(&byte))
    {
        return Err(InvalidCron);
    }
    let value = value.replace(['*', '?'], &format!("{}-{}", field.min, field.max));
    // cron-parser keeps the values it parsed, and a repeated one is invalid:
    // a single weekday is stored modulo 7 and a weekday range keeps its 7
    // (adding 0 when the range reaches 7), so `0,7` repeats but `0-7` does not.
    let mut values = Vec::new();
    let mut store = |number: i64| -> Result<(), InvalidCron> {
        values.push(number);
        Ok(())
    };
    let in_range = |number: i64| number >= i64::from(field.min) && number <= i64::from(field.max);
    for atom in value.split(',') {
        if atom.is_empty() {
            return Err(InvalidCron);
        }
        let parts: Vec<&str> = atom.split('/').collect();
        let (range, step) = match parts.as_slice() {
            [range] => ((*range).to_owned(), 1),
            [range, step] => (
                if range.contains('-') {
                    (*range).to_owned()
                } else {
                    format!("{range}-{}", field.max)
                },
                parse_int(step).ok_or(InvalidCron)?,
            ),
            _ => return Err(InvalidCron),
        };
        let bounds: Vec<&str> = range.split('-').collect();
        if bounds.len() <= 1 {
            // A plain number; an empty one, as in `5,`, was rejected above.
            let number = range.parse::<i64>().map_err(|_| InvalidCron)?;
            if !in_range(number) {
                return Err(InvalidCron);
            }
            store(if field.weekday { number % 7 } else { number })?;
            continue;
        }
        // cron-parser reads only the first two bounds of `*-5`, which is `0-59-5`.
        let low = parse_int(bounds[0]).ok_or(InvalidCron)?;
        let high = parse_int(bounds[1]).ok_or(InvalidCron)?;
        if !in_range(low) || !in_range(high) || low > high || step <= 0 {
            return Err(InvalidCron);
        }
        let reaches_seven = field.weekday && high % 7 == 0 && (high - low) % step == 0;
        if reaches_seven && low != 0 {
            store(0)?;
        }
        // cron-parser accepts any positive step; a huge one, as in
        // `1/9223372036854775807`, simply stops after its first value.
        let mut number = low;
        loop {
            store(number)?;
            match number.checked_add(step) {
                Some(next) if next <= high => number = next,
                _ => break,
            }
        }
    }
    Ok(values)
}

/// `DAYS_IN_MONTH` in cron-parser: February counts 29 days.
const DAYS_IN_MONTH: [i64; 12] = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

fn parse_days(raw: &str, months: u16) -> Result<u64, InvalidCron> {
    let mut values = field_values(raw, DAY)?;
    if months.count_ones() == 1 {
        let length = DAYS_IN_MONTH[months.trailing_zeros() as usize - 1];
        let possible: Vec<i64> = values
            .iter()
            .copied()
            .filter(|day| *day <= length)
            .collect();
        if !possible.is_empty() {
            values = possible;
        }
    }
    stored_bits(values, DAY)
}

impl Cron {
    /// `normalizeStandardCronExpression` followed by `CronExpressionParser.parse`.
    pub fn parse(expression: &str) -> Result<Self, InvalidCron> {
        let lowered = expression.trim().to_lowercase();
        let normalized = PREDEFINED
            .iter()
            .find(|(alias, _)| *alias == lowered)
            .map_or(expression, |(_, standard)| standard)
            .trim();
        if normalized.len() > MAX_LENGTH {
            return Err(InvalidCron);
        }
        let fields: Vec<&str> = normalized.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = fields.as_slice() else {
            return Err(InvalidCron);
        };
        // Electron rejects a doubled range such as `1-2-3`, and the `L`, `#`
        // and `H` extensions; `THU` is the one name with an H in it.
        let doubled = fields
            .iter()
            .any(|field| field.split(',').any(|atom| atom.matches('-').count() > 1));
        let hashed = fields.iter().enumerate().any(|(index, field)| {
            let field = if index == 4 {
                field.to_ascii_lowercase().replace("thu", "")
            } else {
                (*field).to_owned()
            };
            field.contains(['h', 'H'])
        });
        if doubled || hashed || normalized.contains('#') || day.contains('L') {
            return Err(InvalidCron);
        }
        let months = parse_field(month, MONTH)? as u16;
        Ok(Self {
            minutes: parse_field(minute, MINUTE)?,
            hours: parse_field(hour, HOUR)? as u32,
            days: parse_days(day, months)? as u32,
            months,
            weekdays: parse_field(weekday, WEEKDAY)? as u8,
            any_day: matches!(*day, "*" | "?"),
            any_weekday: matches!(*weekday, "*" | "?"),
        })
    }

    fn matches_date(&self, date: Date) -> bool {
        if self.months & (1 << date.month()) == 0 {
            return false;
        }
        let day = self.days & (1 << date.day()) != 0;
        let weekday = self.weekdays & (1 << date.weekday().to_sunday_zero_offset()) != 0;
        match (self.any_day, self.any_weekday) {
            (false, false) => day || weekday,
            (_, true) => day,
            (true, false) => weekday,
        }
    }

    /// Occurrences strictly after `after` and no later than `until`, in order,
    /// at most `limit` of them: `enumerateCronOccurrences(…, includeStart = false)`.
    pub fn occurrences(
        &self,
        zone: &TimeZone,
        after: Timestamp,
        until: Timestamp,
        limit: usize,
    ) -> Vec<Timestamp> {
        let mut found = Vec::new();
        if after >= until || limit == 0 {
            return found;
        }
        let mut date = zone.to_datetime(after).date();
        let last = zone.to_datetime(until).date();
        while date <= last && found.len() < limit {
            if self.matches_date(date) {
                // When the clocks skip the start of a day, cron-parser's move
                // to that day lands after the gap, so the day's skipped times
                // never run.
                // When they repeat it, that move lands in the second pass.
                let midnight = zone
                    .to_ambiguous_timestamp(date.to_datetime(jiff::civil::time(0, 0, 0, 0)))
                    .offset();
                let midnight_gap = matches!(midnight, AmbiguousOffset::Gap { .. });
                // A start inside that day's first pass stays in the first pass.
                let midnight_fold = match midnight {
                    AmbiguousOffset::Fold { before, .. } => {
                        zone.to_datetime(after).date() != date || zone.to_offset(after) != before
                    }
                    _ => false,
                };
                let mut day = Vec::new();
                for hour in (0..24).filter(|hour| self.hours & (1 << hour) != 0) {
                    for minute in (0..60).filter(|minute| self.minutes & (1 << minute) != 0) {
                        let civil =
                            DateTime::from_parts(date, jiff::civil::time(hour, minute, 0, 0));
                        let ambiguous = zone.to_ambiguous_timestamp(civil);
                        let (rolled_back, both) = match ambiguous.offset() {
                            AmbiguousOffset::Fold {
                                before,
                                after: later_offset,
                            } => (
                                zone.to_offset(after) == later_offset,
                                self.hours == ALL_HOURS
                                    && before.seconds() - later_offset.seconds() == 3600,
                            ),
                            AmbiguousOffset::Gap { .. } if midnight_gap => continue,
                            // cron-parser moves a skipped wall time past the gap only
                            // when enumeration crosses it; starting after the clocks
                            // went forward, that day's skipped time does not run.
                            AmbiguousOffset::Gap {
                                after: later_offset,
                                ..
                            } if zone.to_offset(after) == later_offset
                                && later_offset
                                    .to_timestamp(civil)
                                    .is_ok_and(|gap_end| gap_end <= after) =>
                            {
                                continue;
                            }
                            _ => (false, false),
                        };
                        let first = ambiguous.earlier().ok();
                        // `compatible` resolves a gap forward and a fold to its first instant.
                        // cron-parser suppresses a restricted fold's second instant only
                        // when enumeration crosses the rollback. Starting after it, in
                        // the repeated hour's second pass, it still lists that instant.
                        let instants = if both {
                            [first, ambiguous.later().ok()]
                        } else if midnight_fold
                            || rolled_back && first.is_some_and(|first| first <= after)
                        {
                            [ambiguous.later().ok(), None]
                        } else {
                            [ambiguous.compatible().ok(), None]
                        };
                        day.extend(
                            instants
                                .into_iter()
                                .flatten()
                                .filter(|time| *time > after && *time <= until),
                        );
                    }
                }
                // Gap times can land on, or before, a later wall time of the same day.
                day.sort();
                day.dedup();
                for time in day {
                    if found.last().is_none_or(|previous| *previous < time) {
                        found.push(time);
                    }
                }
            }
            match date.checked_add(1.day()) {
                Ok(next) => date = next,
                Err(_) => break,
            }
        }
        found.truncate(limit);
        found
    }
}

#[cfg(test)]
#[path = "cron_tests.rs"]
mod tests;
