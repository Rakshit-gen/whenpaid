//! Fixtures worked out by hand, one per number that matters. If one of these
//! changes, a rule changed: update docs/SOURCES.md and the decision log too.

use whenpaid::build::Builder;
use whenpaid::engine::{Input, RunStatus};
use whenpaid::money::Currency;

fn input(b: &Builder, as_of: &str, extra: serde_json::Value) -> Input {
    let mut v = serde_json::json!({
        "as_of": as_of,
        "ledger": b.entries,
        "rates": [{ "date": "2026-01-01", "currency": "EUR", "usd": "1.08" }, { "date": "2026-01-01", "currency": "GBP", "usd": "1.27" }],
    });
    v.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    serde_json::from_value(v).unwrap()
}

/// The provider's own worked example (mor-intro): a 230.00 USD sale with 30.00 tax,
/// 4% + 1.5% international + 0.40 fixed, all on the tax-inclusive total.
/// 230.00 - 30.00 - 9.20 - 3.45 - 0.40 = 186.95. Below 1,000, so a 5.00 payout fee: 181.95 sent.
#[test]
fn provider_worked_example_end_to_end() {
    let mut b = Builder::new();
    b.sale(
        "pay_1",
        "2026-11-03T10:00:00Z",
        Currency::USD,
        23_000,
        3_000,
        920 + 345 + 40,
    );
    let r = whenpaid::explain(&input(&b, "2026-11-30T00:00:00Z", serde_json::json!({}))).unwrap();
    let run = r
        .runs
        .iter()
        .find(|x| x.slot.runs.to_string() == "2026-11-18")
        .unwrap();
    assert_eq!(run.wallets[0].payable.minor, 18_695);
    let e = run.expected.as_ref().unwrap();
    assert_eq!((e.payout_fee.minor, e.sent.unwrap().minor), (500, 18_195));
}

/// 60.00 USD and 40.00 EUR: 40.00 EUR at 1.08 is 43.20 USD, 103.20 combined, over 100.
/// Payout to USD: 60.00 + 43.20 = 103.20 before fees, conversion fee 1.5% of 43.20 = 0.648,
/// half-even to 0.65, payout fee 5.00, sent 97.55.
#[test]
fn mixed_wallets_hand_calculated() {
    let mut b = Builder::new();
    b.sale("pay_1", "2026-11-03T10:00:00Z", Currency::USD, 6_000, 0, 0);
    b.sale("pay_2", "2026-11-04T10:00:00Z", Currency::EUR, 4_000, 0, 0);
    let r = whenpaid::explain(&input(
        &b,
        "2026-11-30T00:00:00Z",
        serde_json::json!({ "params": { "conversion_fee_bps": 150 } }),
    ))
    .unwrap();
    let run = r
        .runs
        .iter()
        .find(|x| x.slot.runs.to_string() == "2026-11-18")
        .unwrap();
    let e = run.expected.as_ref().unwrap();
    assert_eq!(e.gross.minor, 10_320);
    assert_eq!(e.conversion_fee.unwrap().minor, 65);
    assert_eq!(e.sent.unwrap().minor, 10_320 - 65 - 500);
}

/// GBP payout under the nominal reading: 5.00 GBP below 1,000.00 GBP.
/// Under the converted reading at 1.27: 5.00 USD is 3.94 GBP (3.937 rounds to 3.94), 1,000 USD is 787.40 GBP.
#[test]
fn gbp_payout_fee_both_readings() {
    let mut b = Builder::new();
    b.sale("pay_1", "2026-11-03T10:00:00Z", Currency::GBP, 80_000, 0, 0);
    let linked = serde_json::json!({ "settings": { "linked": ["GBP"] } });
    let r = whenpaid::explain(&input(&b, "2026-11-30T00:00:00Z", linked.clone())).unwrap();
    let run = r
        .runs
        .iter()
        .find(|x| x.group == "linked GBP" && x.slot.runs.to_string() == "2026-11-18")
        .unwrap();
    assert_eq!(
        run.expected.as_ref().unwrap().payout_fee.minor,
        0,
        "800 GBP is over 787.40 GBP"
    );
    let mut i = input(&b, "2026-11-30T00:00:00Z", linked);
    i.params.fee_basis = whenpaid::policy::FeeBasis::Nominal;
    let r = whenpaid::explain(&i).unwrap();
    let run = r
        .runs
        .iter()
        .find(|x| x.group == "linked GBP" && x.slot.runs.to_string() == "2026-11-18")
        .unwrap();
    assert_eq!(
        run.expected.as_ref().unwrap().payout_fee.minor,
        500,
        "800 GBP is under 1,000 GBP"
    );
}

/// Monthly runs are due on the 11th. Columbus Day 2027 is Monday 11 Oct, so it runs Tuesday the 12th.
#[test]
fn monthly_run_on_columbus_day_moves() {
    let mut b = Builder::new();
    b.sale("pay_1", "2027-09-03T10:00:00Z", Currency::USD, 50_000, 0, 0);
    let r = whenpaid::explain(&input(&b, "2027-10-20T00:00:00Z", serde_json::json!({ "settings": { "cycles": [{ "from": "2027-01-01", "cycle": "monthly" }] } }))).unwrap();
    let run = r
        .runs
        .iter()
        .find(|x| x.slot.scheduled.to_string() == "2027-10-11")
        .unwrap();
    assert_eq!(run.slot.runs.to_string(), "2027-10-12");
    assert_eq!(run.status, RunStatus::PaidSimulated);
    assert!(run.rules.contains(&"PS-HOLIDAY"));
}

/// The sample account in examples/ renders the same text every time.
#[test]
fn sample_report_is_stable() {
    let text = std::fs::read_to_string("examples/merchant.json").unwrap();
    let i: Input = serde_json::from_str(&text).unwrap();
    let a = whenpaid::report::render(
        &whenpaid::explain(&i).unwrap(),
        &whenpaid::report::Options {
            all: true,
            entries: true,
        },
    );
    let want = std::fs::read_to_string("examples/merchant.txt").unwrap();
    assert_eq!(
        a, want,
        "regenerate with: cargo run -- explain examples/merchant.json --all --entries > examples/merchant.txt"
    );
}
