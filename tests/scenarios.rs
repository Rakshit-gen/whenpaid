//! Situations taken from public "where is my payout" questions, with every
//! name, amount and date replaced. docs/SCENARIOS.md says where each pattern
//! came from and which parts are documented behaviour versus what a merchant
//! reported.

use whenpaid::account::{Restriction, RestrictionKind};
use whenpaid::build::Builder;
use whenpaid::engine::{Discrepancy, Input, RunStatus};
use whenpaid::money::Currency;

const USD: Currency = Currency::USD;

fn input(b: &Builder, as_of: &str) -> Input {
    serde_json::from_value(serde_json::json!({
        "as_of": as_of,
        "ledger": b.entries,
        "rates": [{ "date": "2026-01-01", "currency": "EUR", "usd": "1.08" }, { "date": "2026-01-01", "currency": "GBP", "usd": "1.27" }],
    }))
    .unwrap()
}

fn run<'a>(r: &'a whenpaid::Report, date: &str) -> &'a whenpaid::engine::Run {
    r.runs
        .iter()
        .find(|x| x.group == "combined" && x.slot.runs.to_string() == date)
        .unwrap()
}

fn t(s: &str) -> chrono::DateTime<chrono::Utc> {
    s.parse().unwrap()
}

/// S1. Payout smaller than expected after a chargeback on day 5 of the period.
#[test]
fn s1_chargeback_on_day_five_shrinks_the_payout() {
    let mut b = Builder::new();
    b.sale("pay_a", "2026-11-01T09:00:00Z", USD, 30_000, 0, 1_240);
    b.dispute("dsp_a", "2026-11-05T09:00:00Z", USD, 30_000, 3_000);
    b.sale("pay_b", "2026-11-09T09:00:00Z", USD, 20_000, 0, 840);
    let r = whenpaid::explain(&input(&b, "2026-11-20T00:00:00Z")).unwrap();
    let x = run(&r, "2026-11-18");
    // 300.00 - 12.40 - 300.00 - 30.00 + 200.00 - 8.40 = 149.20
    assert_eq!(x.wallets[0].payable.minor, 14_920);
    let lines: Vec<_> = x.wallets[0]
        .lines
        .iter()
        .map(|l| (l.label, l.amount.minor))
        .collect();
    assert!(lines.contains(&("disputed amounts held", -30_000)));
    assert!(lines.contains(&("dispute fees (not returned if won)", -3_000)));
    assert!(x.rules.contains(&"BAL-TYPES"));
}

/// S2. Payouts stop with no reason given and no end date.
#[test]
fn s2_hold_with_no_reason_stays_unknown_and_counts_days() {
    let mut b = Builder::new();
    b.sale("pay_a", "2026-10-02T09:00:00Z", USD, 80_000, 0, 0);
    let mut i = input(&b, "2026-11-24T00:00:00Z");
    i.restrictions = vec![Restriction {
        id: "hold_1".into(),
        kind: RestrictionKind::Other,
        from: t("2026-10-10T00:00:00Z"),
        until: None,
        reason: None,
        pauses_payouts: None,
    }];
    let r = whenpaid::explain(&i).unwrap();
    let x = run(&r, "2026-10-19");
    assert_eq!(x.status, RunStatus::Blocked);
    assert_eq!(
        (x.blockers[0].reason.as_str(), x.blockers[0].until.as_str()),
        ("not disclosed", "unknown")
    );
    let last = r
        .runs
        .iter()
        .rfind(|x| x.status == RunStatus::Blocked)
        .unwrap();
    assert_eq!(last.due_since.unwrap().to_string(), "2026-10-19");
    assert_eq!(last.overdue_days, Some(36));
    assert_eq!(
        last.wallets[0].payable.minor, 80_000,
        "nothing is lost while held"
    );
}

/// The top of the report answers S2 in plain words, without the reader digging through runs.
#[test]
fn s2_summary_says_on_hold_and_for_how_long() {
    let mut b = Builder::new();
    b.sale("pay_a", "2026-10-02T09:00:00Z", USD, 80_000, 0, 0);
    let mut i = input(&b, "2026-11-24T00:00:00Z");
    i.restrictions = vec![Restriction {
        id: "hold_1".into(),
        kind: RestrictionKind::Other,
        from: t("2026-10-10T00:00:00Z"),
        until: None,
        reason: None,
        pauses_payouts: None,
    }];
    let r = whenpaid::explain(&i).unwrap();
    let s = r.summary.join("\n");
    assert!(s.contains("Reason: not disclosed. Until: unknown."), "{s}");
    assert!(s.contains("won't go out while the hold lasts"), "{s}");
    assert!(s.contains("due since 2026-10-19, 36 days"), "{s}");
}

/// S3. A hold that lasts for a long dispute window, then ends: everything held goes out on the next run.
#[test]
fn s3_long_dispute_window_hold_then_release() {
    let mut b = Builder::new();
    b.sale("pay_a", "2026-06-02T09:00:00Z", USD, 50_000, 0, 0);
    b.sale("pay_b", "2026-08-02T09:00:00Z", USD, 25_000, 0, 0);
    let mut i = input(&b, "2026-11-24T00:00:00Z");
    i.restrictions = vec![Restriction {
        id: "risk_1".into(),
        kind: RestrictionKind::ComplianceReview,
        from: t("2026-06-10T00:00:00Z"),
        until: Some(t("2026-10-08T00:00:00Z")),
        reason: Some("dispute window review".into()),
        pauses_payouts: None,
    }];
    let r = whenpaid::explain(&i).unwrap();
    let paid: Vec<_> = r
        .runs
        .iter()
        .filter(|x| x.status == RunStatus::PaidSimulated)
        .collect();
    assert_eq!(paid.len(), 1);
    assert_eq!(paid[0].slot.runs.to_string(), "2026-10-19");
    assert_eq!(paid[0].wallets[0].payable.minor, 75_000);
}

/// S4. Payout arrived "late": the scheduled day was a holiday, and it was created the next business day.
#[test]
fn s4_holiday_moves_the_payout_and_the_report_says_so() {
    // 1-15 Jan 2027 is due 18 Jan 2027, which is MLK Day, so it runs on the 19th.
    let mut b2 = Builder::new();
    b2.sale("pay_a", "2027-01-05T09:00:00Z", USD, 50_000, 0, 0);
    b2.payout("pyt_a", "2027-01-19T00:00:04Z", USD, 49_500, 500);
    let r = whenpaid::explain(&input(&b2, "2027-01-25T00:00:00Z")).unwrap();
    let x = run(&r, "2027-01-19");
    assert_eq!(
        (x.slot.scheduled.to_string(), x.status),
        ("2027-01-18".to_string(), RunStatus::Paid)
    );
    assert!(x.rules.contains(&"PS-HOLIDAY"));
}

/// S5. Account suspended and customers refunded: the wallet goes negative and nothing is owed.
#[test]
fn s5_suspension_with_refunds_leaves_nothing_to_pay() {
    let mut b = Builder::new();
    b.sale("pay_a", "2026-11-02T09:00:00Z", USD, 10_000, 0, 440);
    b.sale("pay_b", "2026-11-03T09:00:00Z", USD, 10_000, 0, 440);
    b.refund("ref_a", "2026-11-06T09:00:00Z", USD, 10_000, 0, 100);
    b.refund("ref_b", "2026-11-06T09:00:01Z", USD, 10_000, 0, 100);
    let mut i = input(&b, "2026-11-24T00:00:00Z");
    i.restrictions = vec![Restriction {
        id: "susp".into(),
        kind: RestrictionKind::Suspension,
        from: t("2026-11-06T00:00:00Z"),
        until: None,
        reason: Some("policy review".into()),
        pauses_payouts: None,
    }];
    let r = whenpaid::explain(&i).unwrap();
    let x = run(&r, "2026-11-18");
    assert_eq!(x.status, RunStatus::NothingToPay);
    assert_eq!(x.wallets[0].payable.minor, -1_080);
    assert_eq!(
        r.eligibility.payouts_enabled, None,
        "no verification data, so not claimed either way"
    );
    assert_eq!(r.eligibility.blockers.len(), 1);
}

/// S6. A balance shown rounded down to whole dollars (21.98 - 20.00 shown as 1.00, not 1.98).
#[test]
fn s6_credits_are_exact_to_the_cent() {
    let mut b = Builder::new();
    b.entry(
        "cr_a",
        "2026-11-02T09:00:00Z",
        USD,
        "dodo_credits",
        2_198,
        true,
        None,
        None,
    );
    b.entry(
        "use_a",
        "2026-11-03T09:00:00Z",
        USD,
        "adjustment",
        2_000,
        false,
        None,
        None,
    );
    let r = whenpaid::explain(&input(&b, "2026-11-24T00:00:00Z")).unwrap();
    assert_eq!(run(&r, "2026-11-18").wallets[0].payable.minor, 198);
    assert_eq!(r.reconciliation[0].closing.to_string(), "1.98 USD");
}

/// S7. A preview that mixes currencies without saying which rate it used.
#[test]
fn s7_mixed_currency_preview_names_the_rate_and_date() {
    let mut b = Builder::new();
    b.sale("pay_a", "2026-11-02T09:00:00Z", Currency::EUR, 33_333, 0, 0);
    let r = whenpaid::explain(&input(&b, "2026-11-24T00:00:00Z")).unwrap();
    let c = &run(&r, "2026-11-18")
        .threshold
        .as_ref()
        .unwrap()
        .conversions[0];
    // 333.33 EUR * 1.08 = 359.9964 USD -> 360.00, rounding moved it up 0.36 of a cent.
    assert_eq!(c.to.minor, 36_000);
    assert_eq!(c.rate, "1.08 USD per EUR");
    assert_eq!(c.rate_date.to_string(), "2026-01-01");
    assert_eq!(c.residual.to_string(), "-9/25");
}

/// S8. The payout said "success" but never reached the bank, then failed back.
#[test]
fn s8_success_then_bank_return() {
    let mut b = Builder::new();
    b.sale("pay_a", "2026-11-02T09:00:00Z", USD, 50_000, 0, 0);
    b.payout("pyt_a", "2026-11-18T00:00:04Z", USD, 49_500, 500);
    b.payout_reversal("pyt_a", "2026-11-23T10:00:00Z", USD, 49_500, 500);
    let mut i = input(&b, "2026-11-24T00:00:00Z");
    i.payouts = serde_json::from_value(serde_json::json!([{
        "payout_id": "pyt_a", "currency": "USD", "amount": 49_500, "fee": 500, "created_at": "2026-11-18T00:00:04Z",
        "events": [ { "status": "failed", "timestamp": "2026-11-23T10:00:00Z" }, { "status": "success", "timestamp": "2026-11-18T12:00:00Z" } ],
        "remarks": "returned by beneficiary bank"
    }]))
    .unwrap();
    let r = whenpaid::explain(&i).unwrap();
    assert_eq!(run(&r, "2026-11-18").status, RunStatus::Reversed);
    assert!(
        r.payouts[0]
            .state
            .starts_with("failed; amount and fee credited back")
    );
    let next = r
        .runs
        .iter()
        .find(|x| x.status == RunStatus::Projected)
        .unwrap();
    assert_eq!(next.wallets[0].carried_in.minor, 50_000);
}

/// S9. A correction posted after the period closed belongs to the next run, not the one already made.
#[test]
fn s9_late_correction_lands_in_the_next_run() {
    let mut b = Builder::new();
    b.sale("pay_a", "2026-11-02T09:00:00Z", USD, 50_000, 0, 0);
    b.payout("pyt_a", "2026-11-18T00:00:04Z", USD, 49_500, 500);
    b.adjustment("adj_a", "2026-11-19T09:00:00Z", USD, -1_234);
    let r = whenpaid::explain(&input(&b, "2026-11-24T00:00:00Z")).unwrap();
    assert_eq!(run(&r, "2026-11-18").status, RunStatus::Paid);
    let next = r
        .runs
        .iter()
        .find(|x| x.status == RunStatus::Projected)
        .unwrap();
    assert_eq!(next.wallets[0].payable.minor, -1_234);
    assert!(r.discrepancies.is_empty(), "{:?}", r.discrepancies);
}

/// S10. Two reserve holds overlap; only the released one comes back.
#[test]
fn s10_overlapping_reserve_holds() {
    let mut b = Builder::new();
    b.sale("pay_a", "2026-09-02T09:00:00Z", USD, 100_000, 0, 0);
    b.reserve_hold("rsv_a", "2026-09-02T09:00:01Z", USD, 10_000);
    b.sale("pay_b", "2026-09-20T09:00:00Z", USD, 100_000, 0, 0);
    b.reserve_hold("rsv_b", "2026-09-20T09:00:01Z", USD, 10_000);
    b.reserve_release("rsv_a:rel", "2026-11-02T00:00:00Z", USD, 10_000);
    let r = whenpaid::explain(&input(&b, "2026-11-24T00:00:00Z")).unwrap();
    let x = run(&r, "2026-11-18");
    assert_eq!(x.wallets[0].payable.minor, 10_000);
    let held: i64 = r.reconciliation[0]
        .movements
        .iter()
        .filter(|l| l.label.contains("reserve"))
        .map(|l| l.amount.minor)
        .sum();
    assert_eq!(held, -10_000, "one hold still out");
}

/// S11. A fee that doesn't match the published schedule is reported, never corrected.
#[test]
fn s11_payout_fee_off_schedule_is_flagged() {
    let mut b = Builder::new();
    b.sale("pay_a", "2026-11-02T09:00:00Z", USD, 150_000, 0, 0);
    b.payout("pyt_a", "2026-11-18T00:00:04Z", USD, 149_500, 500);
    let r = whenpaid::explain(&input(&b, "2026-11-24T00:00:00Z")).unwrap();
    assert!(
        r.discrepancies
            .iter()
            .any(|d| matches!(d, Discrepancy::FeeDiffers { expected, .. } if expected.minor == 0))
    );
}
