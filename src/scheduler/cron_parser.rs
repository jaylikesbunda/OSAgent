use chrono::{DateTime, Datelike, Duration, Local, LocalResult, TimeZone, Timelike, Utc};

/// A deliberately small, dependency-free scheduler parser.
///
/// The previous implementation accepted five-field cron but only read the
/// minute and hour. That made expressions such as `0 9 * * 1-5` run every day.
/// This parser supports the normal five-field grammar and evaluates the date
/// fields as well. Natural-language forms remain supported for compatibility.
#[derive(Clone, Default)]
pub struct CronParser;

#[derive(Clone)]
struct Field {
    values: Vec<bool>,
    wildcard: bool,
}

impl Field {
    fn matches(&self, value: usize) -> bool {
        self.values.get(value).copied().unwrap_or(false)
    }
}

impl CronParser {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self
    }

    /// Compute the next occurrence using the current time as the lower bound.
    pub fn next_run(&self, expr: &str) -> Option<DateTime<Utc>> {
        self.next_run_after(expr, Utc::now())
    }

    /// Compute the next occurrence strictly after `from`.
    ///
    /// Keeping the reference time explicit prevents recurring jobs from
    /// drifting every time the polling loop happens to run.
    pub fn next_run_after(&self, expr: &str, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let expr = expr.trim();
        let lower = expr.to_ascii_lowercase();

        if lower.starts_with("in ") {
            return self.parse_relative_from(&lower, from);
        }
        if lower.starts_with("at ") {
            return self.parse_at_time_from(&lower, from);
        }
        if lower.starts_with("@") {
            return self.parse_special_from(&lower, from);
        }
        if lower.starts_with("every ") {
            return self.parse_every_from(&lower, from);
        }

        self.parse_standard_cron(&lower, from)
    }

    fn parse_relative_from(&self, expr: &str, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let rest = expr.strip_prefix("in ")?.trim();
        let duration = parse_duration(
            rest,
            &[
                "m", "min", "minute", "minutes", "h", "hr", "hour", "hours", "d", "day", "days",
                "w", "week", "weeks",
            ],
        )?;
        (duration > 0).then(|| from + Duration::seconds(duration))
    }

    fn parse_at_time_from(&self, expr: &str, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let rest = expr.strip_prefix("at ")?.trim().to_ascii_lowercase();
        let (hour, minute) = if rest == "noon" {
            (12, 0)
        } else if rest == "midnight" {
            (0, 0)
        } else {
            let (clock, meridiem) = split_meridiem(&rest)?;
            let mut parts = clock.split(':');
            let raw_hour: u32 = parts.next()?.trim().parse().ok()?;
            let raw_minute: u32 = parts.next().map(str::trim).unwrap_or("0").parse().ok()?;
            if parts.next().is_some() || raw_minute > 59 {
                return None;
            }

            let mut hour = raw_hour;
            match meridiem {
                Meridiem::Pm if hour < 12 => hour += 12,
                Meridiem::Am if hour == 12 => hour = 0,
                _ => {}
            }
            if hour > 23 {
                return None;
            }
            (hour, raw_minute)
        };

        let local = Local.from_utc_datetime(&from.naive_utc());
        let base_date = local.date_naive();
        for day_offset in 0..=1 {
            let date = base_date.checked_add_days(chrono::Days::new(day_offset))?;
            let naive = date.and_hms_opt(hour, minute, 0)?;
            let candidate = match Local.from_local_datetime(&naive) {
                LocalResult::Single(value) => value,
                // Skipping a nonexistent DST time is safer than silently
                // running at a different local time. Ambiguous times are also
                // skipped; the next valid occurrence is selected.
                _ => continue,
            }
            .with_timezone(&Utc);
            if candidate > from {
                return Some(candidate);
            }
        }
        None
    }

    fn parse_special_from(&self, expr: &str, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let local = Local.from_utc_datetime(&from.naive_utc());
        match expr {
            "@hourly" => {
                let mut date = local.date_naive();
                let mut hour = local.hour() + 1;
                if hour >= 24 {
                    hour = 0;
                    date = date.succ_opt()?;
                }
                local_datetime(date, hour, 0)
            }
            "@daily" | "@everyday" => local_datetime(local.date_naive().succ_opt()?, 0, 0),
            "@weekly" => {
                let days_until_monday = (7 - local.weekday().num_days_from_monday()) % 7;
                let days = if days_until_monday == 0 {
                    7
                } else {
                    days_until_monday
                };
                local_datetime(
                    local
                        .date_naive()
                        .checked_add_days(chrono::Days::new(u64::from(days)))?,
                    0,
                    0,
                )
            }
            "@monthly" => {
                let next_month = local
                    .date_naive()
                    .with_day(1)?
                    .checked_add_months(chrono::Months::new(1))?;
                local_datetime(next_month, 0, 0)
            }
            _ => None,
        }
    }

    fn parse_every_from(&self, expr: &str, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let rest = expr.strip_prefix("every ")?.trim();
        let seconds = parse_duration(
            rest,
            &[
                "m", "min", "minute", "minutes", "h", "hr", "hour", "hours", "d", "day", "days",
                "w", "week", "weeks",
            ],
        )?;
        (seconds > 0).then(|| from + Duration::seconds(seconds))
    }

    fn parse_standard_cron(&self, expr: &str, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let parts: Vec<&str> = expr.split_whitespace().collect();
        if parts.len() != 5 {
            return None;
        }

        let minute = parse_field(parts[0], 0, 59)?;
        let hour = parse_field(parts[1], 0, 23)?;
        let day_of_month = parse_field(parts[2], 1, 31)?;
        let month = parse_field(parts[3], 1, 12)?;
        let day_of_week = parse_field(parts[4], 0, 7)?;

        let local = Local.from_utc_datetime(&from.naive_utc());
        let mut candidate = local
            .date_naive()
            .and_hms_opt(local.hour(), local.minute(), 0)?
            + Duration::minutes(1);

        // Five years covers leap-year schedules while keeping malformed input
        // bounded. Calendar validity is checked by the conversion below.
        for _ in 0..(5 * 366 * 24 * 60) {
            if month.matches(candidate.month() as usize)
                && day_matches(
                    candidate.day() as usize,
                    candidate.weekday().num_days_from_monday() as usize,
                    &day_of_month,
                    &day_of_week,
                )
                && hour.matches(candidate.hour() as usize)
                && minute.matches(candidate.minute() as usize)
            {
                if let LocalResult::Single(value) = Local.from_local_datetime(&candidate) {
                    let utc = value.with_timezone(&Utc);
                    if utc > from {
                        return Some(utc);
                    }
                }
            }
            candidate += Duration::minutes(1);
        }
        None
    }
}

fn local_datetime(date: chrono::NaiveDate, hour: u32, minute: u32) -> Option<DateTime<Utc>> {
    let naive = date.and_hms_opt(hour, minute, 0)?;
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(value) => Some(value.with_timezone(&Utc)),
        _ => None,
    }
}

fn split_meridiem(value: &str) -> Option<(&str, Meridiem)> {
    if let Some(clock) = value.strip_suffix("pm") {
        return Some((clock.trim(), Meridiem::Pm));
    }
    if let Some(clock) = value.strip_suffix("am") {
        return Some((clock.trim(), Meridiem::Am));
    }
    Some((value.trim(), Meridiem::None))
}

#[derive(Clone, Copy)]
enum Meridiem {
    Am,
    Pm,
    None,
}

fn parse_field(value: &str, min: usize, max: usize) -> Option<Field> {
    let mut values = vec![false; max + 1];
    let wildcard = value == "*";
    for part in value.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => (range, step.parse::<usize>().ok()?),
            None => (part, 1),
        };
        if step == 0 {
            return None;
        }
        let (start, end) = if range == "*" {
            (min, max)
        } else if let Some((start, end)) = range.split_once('-') {
            (start.parse::<usize>().ok()?, end.parse::<usize>().ok()?)
        } else {
            let start = range.parse::<usize>().ok()?;
            (start, start)
        };
        if start < min || end > max || start > end {
            return None;
        }
        for item in (start..=end).step_by(step) {
            values[item] = true;
        }
    }
    Some(Field { values, wildcard })
}

fn day_matches(day: usize, weekday: usize, day_of_month: &Field, day_of_week: &Field) -> bool {
    let dom_matches = day_of_month.matches(day);
    // Cron weekdays are Sunday=0/7, Monday=1, ..., Saturday=6, while
    // chrono exposes Monday=0, ..., Sunday=6.
    let cron_weekday = (weekday + 1) % 7;
    let dow_matches =
        day_of_week.matches(cron_weekday) || (cron_weekday == 0 && day_of_week.matches(7));
    match (day_of_month.wildcard, day_of_week.wildcard) {
        (true, true) => true,
        (true, false) => dow_matches,
        (false, true) => dom_matches,
        // Standard cron uses OR when both day fields are restricted.
        (false, false) => dom_matches || dow_matches,
    }
}

fn parse_duration(value: &str, suffixes: &[&str]) -> Option<i64> {
    let value = value.trim().to_ascii_lowercase();
    let singular = match value.as_str() {
        "m" | "min" | "minute" => Some(("m", 60)),
        "h" | "hr" | "hour" => Some(("h", 3600)),
        "d" | "day" => Some(("d", 86_400)),
        "w" | "week" => Some(("w", 604_800)),
        _ => None,
    };
    if let Some((_, multiplier)) = singular {
        return Some(multiplier);
    }

    for suffix in suffixes {
        if let Some(number) = value.strip_suffix(suffix) {
            let number = number.trim();
            if number.is_empty() {
                continue;
            }
            let amount: i64 = number.parse().ok()?;
            let multiplier = match *suffix {
                "m" | "min" | "minute" | "minutes" => 60,
                "h" | "hr" | "hour" | "hours" => 3600,
                "d" | "day" | "days" => 86_400,
                "w" | "week" | "weeks" => 604_800,
                _ => continue,
            };
            return amount.checked_mul(multiplier);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_cron_fields() {
        let parser = CronParser::new();
        let from = Utc::now();
        let next = parser
            .next_run_after("*/15 * * * *", from)
            .expect("next run");
        assert_eq!(next.minute() % 15, 0);

        let weekday = parser
            .next_run_after("0 9 * * 1-5", from)
            .expect("weekday run");
        let local = Local.from_utc_datetime(&weekday.naive_utc());
        assert!(local.weekday().num_days_from_monday() < 5);
        assert_eq!(local.hour(), 9);
    }

    #[test]
    fn rejects_malformed_expressions() {
        let parser = CronParser::new();
        let from = Utc::now();
        assert!(parser.next_run_after("0 9 * *", from).is_none());
        assert!(parser.next_run_after("0 9 * * 1-99", from).is_none());
        assert!(parser.next_run_after("*/0 * * * *", from).is_none());
    }

    #[test]
    fn relative_schedules_are_strictly_positive() {
        let parser = CronParser::new();
        let from = Utc::now();
        assert!(parser.next_run_after("in 0m", from).is_none());
        assert!(parser.next_run_after("in -1m", from).is_none());
    }
}
