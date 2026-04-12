use chrono::{DateTime, Datelike, Duration, Utc};

#[derive(Clone, Default)]
pub struct CronParser;

impl CronParser {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self
    }

    pub fn next_run(&self, expr: &str) -> Option<DateTime<Utc>> {
        let expr = expr.trim();

        if expr.starts_with("in ") {
            return self.parse_relative(expr);
        }

        if expr.starts_with("at ") {
            return self.parse_at_time(expr);
        }

        if expr.starts_with("@") {
            return self.parse_special(expr);
        }

        if expr.starts_with("every ") {
            return self.parse_every(expr);
        }

        self.parse_standard_cron(expr)
    }

    fn parse_relative(&self, expr: &str) -> Option<DateTime<Utc>> {
        let rest = expr.strip_prefix("in ")?;
        let rest = rest.trim();

        if let Some(mins) = rest.strip_suffix("m").or(rest.strip_suffix("min")) {
            let mins: i64 = mins.trim().parse().ok()?;
            return Some(Utc::now() + Duration::minutes(mins));
        }

        if let Some(hours) = rest.strip_suffix("h").or(rest.strip_suffix("hr")) {
            let hours: i64 = hours.trim().parse().ok()?;
            return Some(Utc::now() + Duration::hours(hours));
        }

        if let Some(days) = rest.strip_suffix("d").or(rest.strip_suffix("day")) {
            let days: i64 = days.trim().parse().ok()?;
            return Some(Utc::now() + Duration::days(days));
        }

        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() == 2 {
            let value: i64 = parts[0].parse().ok()?;
            let unit = parts[1].to_lowercase();
            match unit.as_str() {
                "minute" | "minutes" | "min" => Some(Utc::now() + Duration::minutes(value)),
                "hour" | "hours" | "hr" => Some(Utc::now() + Duration::hours(value)),
                "day" | "days" => Some(Utc::now() + Duration::days(value)),
                "week" | "weeks" => Some(Utc::now() + Duration::weeks(value)),
                _ => None,
            }
        } else {
            None
        }
    }

    fn parse_at_time(&self, expr: &str) -> Option<DateTime<Utc>> {
        let rest = expr.strip_prefix("at ")?;
        let rest = rest.trim().to_lowercase();

        let (hour, minute) = if rest == "noon" {
            (12, 0)
        } else if rest == "midnight" {
            (0, 0)
        } else {
            let parts: Vec<&str> = rest.split(':').collect();
            if parts.len() == 2 {
                let mut hour: u32 = parts[0].parse().ok()?;
                let minute: u32 = parts[1]
                    .trim_end_matches(|c: char| !c.is_ascii_digit())
                    .parse()
                    .ok()?;

                if rest.contains("pm") && hour < 12 {
                    hour += 12;
                } else if rest.contains("am") && hour == 12 {
                    hour = 0;
                }

                (hour, minute)
            } else {
                let hour: u32 = rest
                    .trim_end_matches(|c: char| !c.is_ascii_digit())
                    .parse()
                    .ok()?;
                let is_pm = rest.contains("pm");
                let mut hour = hour;
                if is_pm && hour < 12 {
                    hour += 12;
                } else if !is_pm && hour == 12 {
                    hour = 0;
                }
                (hour, 0)
            }
        };

        let now = Utc::now();
        let mut next = now.date_naive().and_hms_opt(hour, minute, 0)?.and_utc();

        if next <= now {
            next += Duration::days(1);
        }

        Some(next)
    }

    fn parse_special(&self, expr: &str) -> Option<DateTime<Utc>> {
        let now = Utc::now();
        match expr {
            "@hourly" => Some(now + Duration::hours(1)),
            "@daily" | "@everyday" => {
                let tomorrow = now.date_naive().succ_opt()?.and_hms_opt(0, 0, 0)?.and_utc();
                Some(tomorrow)
            }
            "@weekly" => {
                let days_until_monday = 7 - now.weekday().num_days_from_monday();
                Some(now + Duration::days(days_until_monday as i64))
            }
            "@monthly" => {
                let next_month = now
                    .date_naive()
                    .with_day(1)?
                    .checked_add_months(chrono::Months::new(1))?;
                Some(next_month.and_hms_opt(0, 0, 0)?.and_utc())
            }
            _ => None,
        }
    }

    fn parse_every(&self, expr: &str) -> Option<DateTime<Utc>> {
        let rest = expr.strip_prefix("every ")?;
        let rest = rest.trim();

        if let Some(mins) = rest
            .strip_suffix(" minutes")
            .or(rest.strip_suffix("mins").or(rest.strip_suffix("m")))
        {
            let mins: i64 = mins.trim().parse().ok()?;
            return Some(Utc::now() + Duration::minutes(mins));
        }

        if let Some(hours) = rest
            .strip_suffix(" hours")
            .or(rest.strip_suffix("hrs").or(rest.strip_suffix("h")))
        {
            let hours: i64 = hours.trim().parse().ok()?;
            return Some(Utc::now() + Duration::hours(hours));
        }

        match rest {
            "hour" => Some(Utc::now() + Duration::hours(1)),
            "day" => Some(Utc::now() + Duration::days(1)),
            "week" => Some(Utc::now() + Duration::weeks(1)),
            _ => None,
        }
    }

    fn parse_standard_cron(&self, expr: &str) -> Option<DateTime<Utc>> {
        let parts: Vec<&str> = expr.split_whitespace().collect();
        if parts.len() < 5 {
            return None;
        }

        let minute: u32 = parts[0].parse().ok()?;
        let hour: u32 = parts[1].parse().ok()?;

        if minute >= 60 || hour >= 24 {
            return None;
        }

        let now = Utc::now();
        let mut next = now.date_naive().and_hms_opt(hour, minute, 0)?.and_utc();

        if next <= now {
            next += Duration::days(1);
        }

        Some(next)
    }
}
