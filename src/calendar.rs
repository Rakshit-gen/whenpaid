//! Payout cycles and the business-day calendar payout dates move on.

use chrono::{Datelike, Duration, NaiveDate, Weekday};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cycle {
    // Declared from most to least frequent: a business may only move down this list (PS-CYCLE-CHANGE).
    Weekly,
    Bimonthly,
    Monthly,
}

/// One scheduled payout: the billing period it covers and the day it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Slot {
    pub cycle: Cycle,
    pub period_start: NaiveDate,
    pub period_end: NaiveDate,
    /// The date the schedule names.
    pub scheduled: NaiveDate,
    /// The date it actually runs: `scheduled`, or the next business day.
    pub runs: NaiveDate,
}

fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
}

fn month_end(y: i32, m: u32) -> NaiveDate {
    let next = if m == 12 {
        ymd(y + 1, 1, 1)
    } else {
        ymd(y, m + 1, 1)
    };
    next.pred_opt().unwrap()
}

fn nth(y: i32, m: u32, wd: Weekday, n: u8) -> NaiveDate {
    NaiveDate::from_weekday_of_month_opt(y, m, wd, n).unwrap()
}

fn last(y: i32, m: u32, wd: Weekday) -> NaiveDate {
    let mut d = month_end(y, m);
    while d.weekday() != wd {
        d = d.pred_opt().unwrap();
    }
    d
}

/// Days the Federal Reserve is closed in year `y`, with names (FED-CALENDAR).
///
/// A fixed-date holiday on a Sunday is observed on Monday. One on a Saturday
/// is not moved: the Fed stays open the Friday before. Juneteenth counts from 2022.
pub fn fed_holidays(y: i32) -> Vec<(NaiveDate, &'static str)> {
    let mut fixed = vec![
        (1, 1, "New Year's Day"),
        (7, 4, "Independence Day"),
        (11, 11, "Veterans Day"),
        (12, 25, "Christmas Day"),
    ];
    if y >= 2022 {
        fixed.push((6, 19, "Juneteenth"));
    }
    let mut out = Vec::new();
    for (m, d, name) in fixed {
        let h = ymd(y, m, d);
        match h.weekday() {
            Weekday::Sun => out.push((h + Duration::days(1), name)),
            Weekday::Sat => {}
            _ => out.push((h, name)),
        }
    }
    out.extend([
        (nth(y, 1, Weekday::Mon, 3), "Martin Luther King Jr. Day"),
        (nth(y, 2, Weekday::Mon, 3), "Washington's Birthday"),
        (last(y, 5, Weekday::Mon), "Memorial Day"),
        (nth(y, 9, Weekday::Mon, 1), "Labor Day"),
        (nth(y, 10, Weekday::Mon, 2), "Columbus Day"),
        (nth(y, 11, Weekday::Thu, 4), "Thanksgiving Day"),
    ]);
    out.sort();
    out
}

/// Why `d` is not a business day, or `None` if it is.
pub fn closed_because(d: NaiveDate) -> Option<&'static str> {
    match d.weekday() {
        Weekday::Sat => Some("Saturday"),
        Weekday::Sun => Some("Sunday"),
        _ => fed_holidays(d.year())
            .into_iter()
            .find(|(h, _)| *h == d)
            .map(|(_, n)| n),
    }
}

pub fn is_business_day(d: NaiveDate) -> bool {
    closed_because(d).is_none()
}

pub fn next_business_day(mut d: NaiveDate) -> NaiveDate {
    while !is_business_day(d) {
        d = d.succ_opt().unwrap();
    }
    d
}

/// Business days strictly after `from`, up to and including `to`.
pub fn business_days_between(from: NaiveDate, to: NaiveDate) -> u32 {
    let mut n = 0;
    let mut d = from;
    while d < to {
        d = d.succ_opt().unwrap();
        if is_business_day(d) {
            n += 1;
        }
    }
    n
}

fn slot(cycle: Cycle, start: NaiveDate, end: NaiveDate, scheduled: NaiveDate) -> Slot {
    Slot {
        cycle,
        period_start: start,
        period_end: end,
        scheduled,
        runs: next_business_day(scheduled),
    }
}

/// Every slot of month `m`'s billing periods (PS-CYCLE).
fn month_slots(cycle: Cycle, y: i32, m: u32) -> Vec<Slot> {
    let end = month_end(y, m);
    let next4 = end + Duration::days(4);
    let next11 = end + Duration::days(11);
    let d = |day| ymd(y, m, day);
    match cycle {
        Cycle::Bimonthly => vec![
            slot(cycle, d(1), d(15), d(18)),
            slot(cycle, d(16), end, next4),
        ],
        Cycle::Weekly => vec![
            slot(cycle, d(1), d(7), d(11)),
            slot(cycle, d(8), d(14), d(18)),
            slot(cycle, d(15), d(21), d(25)),
            slot(cycle, d(22), end, next4),
        ],
        Cycle::Monthly => vec![slot(cycle, d(1), end, next11)],
    }
}

/// The slots of one cycle whose period ends on or after `from`, clipping the
/// first period to start at `from`.
fn cycle_slots_from(cycle: Cycle, from: NaiveDate) -> impl Iterator<Item = Slot> {
    let (mut y, mut m) = (from.year(), from.month());
    std::iter::from_fn(move || {
        let s = month_slots(cycle, y, m);
        (y, m) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
        Some(s)
    })
    .flatten()
    .filter(move |s| s.period_end >= from)
    .map(move |mut s| {
        s.period_start = s.period_start.max(from);
        s
    })
}

/// The schedule from `from` until the first slot that runs after `until`
/// (included, so callers always see the next payout).
///
/// `changes` are (date, cycle) pairs sorted by date; the first must be on or
/// before `from`. A change takes effect after the period in progress on its
/// date ends (DC-CYCLE-SWITCH), so every day belongs to exactly one slot.
pub fn schedule(changes: &[(NaiveDate, Cycle)], from: NaiveDate, until: NaiveDate) -> Vec<Slot> {
    assert!(
        !changes.is_empty() && changes[0].0 <= from,
        "need the cycle in force at the start"
    );
    let mut out: Vec<Slot> = Vec::new();
    let mut idx = 0;
    let mut start = from;
    loop {
        while idx + 1 < changes.len() && changes[idx + 1].0 <= start {
            idx += 1;
        }
        let cycle = changes[idx].1;
        let next_change = changes.get(idx + 1).map(|c| c.0);
        for s in cycle_slots_from(cycle, start) {
            let done = s.runs > until;
            start = s.period_end.succ_opt().unwrap();
            out.push(s);
            if done {
                return out;
            }
            // A change dated inside this period applies from the next one.
            if next_change.is_some_and(|c| c <= s.period_end) {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has(y: i32, d: NaiveDate) -> bool {
        fed_holidays(y).iter().any(|(h, _)| *h == d)
    }

    #[test]
    fn known_holidays() {
        assert!(has(2026, ymd(2026, 11, 26))); // Thanksgiving
        assert!(has(2026, ymd(2026, 10, 12))); // Columbus Day
        assert!(!has(2026, ymd(2026, 7, 3))); // July 4 is a Saturday: Fed open Friday
        assert!(!has(2026, ymd(2026, 7, 4)));
        assert!(has(2027, ymd(2027, 7, 5))); // July 4 2027 is a Sunday: observed Monday
        assert!(has(2022, ymd(2022, 12, 26))); // Christmas 2022 on a Sunday
        assert!(!has(2021, ymd(2021, 6, 18)));
        assert_eq!(closed_because(ymd(2026, 11, 26)), Some("Thanksgiving Day"));
    }

    #[test]
    fn payout_dates_move_past_weekends_and_holidays() {
        assert_eq!(next_business_day(ymd(2026, 10, 4)), ymd(2026, 10, 5)); // Sunday
        assert_eq!(next_business_day(ymd(2026, 7, 4)), ymd(2026, 7, 6)); // Saturday
        assert_eq!(next_business_day(ymd(2027, 1, 18)), ymd(2027, 1, 19)); // MLK Day
        assert_eq!(next_business_day(ymd(2026, 10, 12)), ymd(2026, 10, 13)); // Columbus Day
        // The payouts page's notification example says 2026-10-05 moves to 2026-10-06, but
        // 10-05 is a normal Monday. We follow the rule, not the example (docs/ASSUMPTIONS.md).
        assert_eq!(next_business_day(ymd(2026, 10, 5)), ymd(2026, 10, 5));
    }

    #[test]
    fn bimonthly_slots() {
        let s = schedule(
            &[(ymd(2026, 1, 1), Cycle::Bimonthly)],
            ymd(2026, 10, 16),
            ymd(2026, 11, 24),
        );
        assert_eq!(s.len(), 3);
        assert_eq!(
            (s[0].period_start, s[0].period_end, s[0].runs),
            (ymd(2026, 10, 16), ymd(2026, 10, 31), ymd(2026, 11, 4))
        );
        assert_eq!(s[1].runs, ymd(2026, 11, 18));
        assert_eq!(
            (s[2].period_end, s[2].runs),
            (ymd(2026, 11, 30), ymd(2026, 12, 4))
        );
    }

    #[test]
    fn leap_year_month_end() {
        let s = schedule(
            &[(ymd(2028, 1, 1), Cycle::Bimonthly)],
            ymd(2028, 2, 16),
            ymd(2028, 3, 1),
        );
        assert_eq!(
            (s[0].period_end, s[0].scheduled),
            (ymd(2028, 2, 29), ymd(2028, 3, 4))
        );
        // 2028-03-04 is a Saturday.
        assert_eq!(s[0].runs, ymd(2028, 3, 6));
        let s = schedule(
            &[(ymd(2027, 1, 1), Cycle::Monthly)],
            ymd(2027, 2, 1),
            ymd(2027, 3, 1),
        );
        assert_eq!(
            (s[0].period_end, s[0].scheduled),
            (ymd(2027, 2, 28), ymd(2027, 3, 11))
        );
    }

    fn assert_tiles(s: &[Slot]) {
        for w in s.windows(2) {
            assert_eq!(
                w[0].period_end.succ_opt().unwrap(),
                w[1].period_start,
                "{w:?}"
            );
            assert!(w[0].runs <= w[1].runs);
        }
        for x in s {
            assert!(
                x.period_start <= x.period_end && x.runs > x.period_end && is_business_day(x.runs)
            );
        }
    }

    #[test]
    fn every_cycle_covers_every_day_once() {
        for cycle in [Cycle::Weekly, Cycle::Bimonthly, Cycle::Monthly] {
            assert_tiles(&schedule(
                &[(ymd(2020, 1, 1), cycle)],
                ymd(2024, 1, 1),
                ymd(2029, 1, 1),
            ));
        }
    }

    #[test]
    fn cycle_switch_covers_every_day_once() {
        // Weekly until 2026-11-19 (inside the 15-21 period), then bi-monthly.
        let s = schedule(
            &[
                (ymd(2026, 1, 1), Cycle::Weekly),
                (ymd(2026, 11, 19), Cycle::Bimonthly),
            ],
            ymd(2026, 11, 1),
            ymd(2027, 1, 10),
        );
        assert_tiles(&s);
        let short = s
            .iter()
            .find(|x| x.period_start == ymd(2026, 11, 22))
            .unwrap();
        assert_eq!(
            (short.cycle, short.period_end, short.scheduled),
            (Cycle::Bimonthly, ymd(2026, 11, 30), ymd(2026, 12, 4))
        );
        assert_eq!(
            s.iter()
                .find(|x| x.period_start == ymd(2026, 11, 15))
                .unwrap()
                .cycle,
            Cycle::Weekly
        );
        // Bi-monthly to monthly mid-period.
        let s = schedule(
            &[
                (ymd(2026, 1, 1), Cycle::Bimonthly),
                (ymd(2026, 11, 3), Cycle::Monthly),
            ],
            ymd(2026, 11, 1),
            ymd(2027, 2, 1),
        );
        assert_tiles(&s);
        assert_eq!(
            (s[1].cycle, s[1].period_start, s[1].period_end),
            (Cycle::Monthly, ymd(2026, 11, 16), ymd(2026, 11, 30))
        );
    }

    #[test]
    fn business_days() {
        // Fri 2026-11-20 -> Fri 2026-11-27: Mon, Tue, Wed, (Thu Thanksgiving), Fri = 4.
        assert_eq!(
            business_days_between(ymd(2026, 11, 20), ymd(2026, 11, 27)),
            4
        );
    }
}
