use chrono::{DateTime, Datelike, Duration, Utc, Weekday};

/// Adds `days` business days (Mon–Fri) to `start` (IMP-REQ-009-01). V1
/// ships weekday-only — no Canadian statutory holiday calendar exists yet
/// (Autonomous Execution Notes: documented limitation, not a blocker; see
/// the REQ-009 Risks table). A Friday start rolls forward to the following
/// Tuesday for a 2-business-day SLA, matching the plan's example.
pub fn add_business_days(start: DateTime<Utc>, days: i64) -> DateTime<Utc> {
    let mut remaining = days;
    let mut current = start;
    while remaining > 0 {
        current += Duration::days(1);
        if !matches!(current.weekday(), Weekday::Sat | Weekday::Sun) {
            remaining -= 1;
        }
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn friday_start_rolls_two_business_days_to_tuesday() {
        // 2026-07-10 is a Friday.
        let friday = Utc.with_ymd_and_hms(2026, 7, 10, 12, 0, 0).unwrap();
        let due = add_business_days(friday, 2);
        // +1 business day -> Monday 2026-07-13, +1 more -> Tuesday 2026-07-14.
        assert_eq!(due.weekday(), Weekday::Tue);
        assert_eq!(due.date_naive(), Utc.with_ymd_and_hms(2026, 7, 14, 12, 0, 0).unwrap().date_naive());
    }

    #[test]
    fn monday_start_adds_two_weekdays_to_wednesday() {
        // 2026-07-13 is a Monday.
        let monday = Utc.with_ymd_and_hms(2026, 7, 13, 9, 0, 0).unwrap();
        let due = add_business_days(monday, 2);
        assert_eq!(due.weekday(), Weekday::Wed);
    }

    #[test]
    fn zero_days_returns_the_same_instant() {
        let now = Utc::now();
        assert_eq!(add_business_days(now, 0), now);
    }
}
