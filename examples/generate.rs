//! Writes the sample account and the benchmark dataset.
//!
//!   cargo run --example generate -- sample > examples/merchant.json
//!   cargo run --release --example generate -- bench 1000000 > /tmp/bench.json
//!
//! Everything is made up. The benchmark generator is seeded, so the same
//! count always gives the same file.

use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use whenpaid::build::Builder;
use whenpaid::money::Currency;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("sample") => sample(),
        Some("bench") => bench(
            args.get(1)
                .and_then(|n| n.parse().ok())
                .unwrap_or(1_000_000),
        ),
        _ => {
            eprintln!("usage: generate sample | generate bench <entries>");
            std::process::exit(1);
        }
    }
}

const USD: Currency = Currency::USD;
const EUR: Currency = Currency::EUR;

/// A small software seller, October to late November 2026.
fn sample() {
    let mut b = Builder::new();
    // First half of October: two sales, below the 100 USD minimum on their own.
    b.sale("pay_1001", "2026-10-03T09:12:00Z", USD, 4_900, 0, 236);
    b.sale("pay_1002", "2026-10-09T17:40:00Z", EUR, 2_900, 503, 156);
    // Second half: a bigger sale, a reserve hold on it, a refund.
    b.sale(
        "pay_1003",
        "2026-10-21T02:05:00Z",
        USD,
        19_900,
        1_592,
        1_101,
    );
    b.reserve_hold("rsv_1003", "2026-10-21T02:05:00Z", USD, 1_990);
    b.sale("pay_1004", "2026-10-28T11:00:00Z", EUR, 12_000, 2_000, 593);
    b.refund("ref_1001", "2026-10-30T08:00:00Z", USD, 4_900, 0, 100);
    // Paid on 4 Nov for 16-31 Oct (and the carried first half).
    let usd_before = b.balance(USD);
    b.conversion(
        "cnv_7001",
        "2026-11-04T00:00:03Z",
        EUR,
        b.balance(EUR),
        USD,
        12_428,
    );
    b.entries
        .iter_mut()
        .rev()
        .take(2)
        .for_each(|e| e.payout_id = Some("pyt_7001".into()));
    let gross = usd_before + 12_428;
    b.payout("pyt_7001", "2026-11-04T00:00:04Z", USD, gross - 500, 500);
    // November: a large sale, a chargeback five days later, an INR sale that will never be paid.
    b.sale(
        "pay_1005",
        "2026-11-05T13:30:00Z",
        USD,
        49_900,
        3_992,
        2_757,
    );
    b.dispute("dsp_1003", "2026-11-10T10:00:00Z", USD, 19_900, 3_000);
    b.sale(
        "pay_1006",
        "2026-11-12T19:00:00Z",
        Currency::INR,
        99_900,
        15_238,
        4_496,
    );
    // Payout on 18 Nov fails, and the amount and fee come back the next day.
    let bal = b.balance(USD);
    b.payout("pyt_7002", "2026-11-18T00:00:05Z", USD, bal - 500, 500);
    b.payout_reversal("pyt_7002", "2026-11-19T15:20:00Z", USD, bal - 500, 500);
    b.reserve_release("rsv_1003:rel", "2026-11-20T00:00:00Z", USD, 1_990);
    b.sale(
        "pay_1007",
        "2026-11-21T06:45:00Z",
        EUR,
        24_900,
        4_322,
        1_215,
    );
    b.dispute_won("dsp_1003", "2026-11-22T10:00:00Z", USD, 19_900);

    let input = json!({
        "business_id": "bus_sample",
        "as_of": "2026-11-24T12:00:00Z",
        "settings": { "payout_data": "complete" },
        "rates": [
            { "date": "2026-10-01", "currency": "EUR", "usd": "1.0832" },
            { "date": "2026-10-01", "currency": "GBP", "usd": "1.2710" },
            { "date": "2026-11-15", "currency": "EUR", "usd": "1.0790" }
        ],
        "forms": [
            { "form": "product_information", "status": "approved", "at": "2026-09-20T10:00:00Z" },
            { "form": "identity", "status": "approved", "at": "2026-09-20T10:00:00Z" },
            { "form": "bank", "status": "approved", "at": "2026-09-21T10:00:00Z" },
            { "form": "monitoring_review", "status": "approved", "at": "2026-09-30T10:00:00Z" }
        ],
        "ledger": b.entries,
        "payouts": [
            { "payout_id": "pyt_7001", "currency": "USD", "amount": gross - 500, "fee": 500, "created_at": "2026-11-04T00:00:04Z",
              "events": [ { "status": "in_progress", "timestamp": "2026-11-04T00:00:04Z" }, { "status": "success", "timestamp": "2026-11-05T09:00:00Z" } ],
              "bank_confirmed_at": "2026-11-06T08:00:00Z" },
            { "payout_id": "pyt_7002", "currency": "USD", "amount": bal - 500, "fee": 500, "created_at": "2026-11-18T00:00:05Z",
              "events": [ { "status": "failed", "timestamp": "2026-11-19T15:20:00Z" }, { "status": "success", "timestamp": "2026-11-18T13:00:00Z" } ],
              "remarks": "beneficiary account closed" }
        ]
    });
    println!("{}", serde_json::to_string_pretty(&input).unwrap());
}

/// xorshift64*, so the dataset doesn't depend on a crate's RNG staying stable.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// `n` ledger rows over two years: sales with tax and fees in USD, EUR and GBP,
/// plus refunds, disputes and reserve movements. No payouts, so the engine
/// simulates every run.
fn bench(n: usize) {
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    let mut b = Builder::new();
    let start: DateTime<Utc> = "2025-01-01T00:00:00Z".parse().unwrap();
    let span = 730 * 86_400;
    let step = span / (n as i64 / 3).max(1);
    let mut t = start;
    let mut i = 0usize;
    while b.entries.len() < n {
        t += Duration::seconds(step.max(1));
        let cur = [USD, EUR, Currency::GBP][r.below(3) as usize];
        let id = format!("e{i}");
        let at = t.to_rfc3339();
        match r.below(100) {
            0..=84 => {
                let amt = 500 + r.below(50_000) as i64;
                b.sale(&id, &at, cur, amt, amt / 10, amt * 4 / 100 + 40);
            }
            85..=92 => b.refund(&id, &at, cur, 500 + r.below(5_000) as i64, 0, 100),
            93..=95 => b.dispute(&id, &at, cur, 500 + r.below(5_000) as i64, 3_000),
            _ => b.reserve_hold(&id, &at, cur, 100 + r.below(2_000) as i64),
        }
        i += 1;
    }
    b.entries.truncate(n);
    let input = json!({
        "as_of": (t + Duration::days(1)).to_rfc3339(),
        "settings": { "payout_data": "none" },
        "rates": [
            { "date": "2024-12-01", "currency": "EUR", "usd": "1.08" },
            { "date": "2024-12-01", "currency": "GBP", "usd": "1.27" }
        ],
        "ledger": b.entries,
    });
    println!("{}", serde_json::to_string(&input).unwrap());
}
