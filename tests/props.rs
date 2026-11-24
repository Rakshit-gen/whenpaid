//! Properties that must hold for any ledger, not just the hand-written ones.

use proptest::prelude::*;
use whenpaid::build::Builder;
use whenpaid::engine::{Input, RunStatus};
use whenpaid::ledger::Finding;
use whenpaid::money::Currency;

#[derive(Clone, Debug)]
enum Ev {
    Sale(usize, i64, i64, i64),
    Refund(usize, i64),
    Dispute(usize, i64, bool),
    Hold(usize, i64, bool),
    Adjust(usize, i64),
}

const CUR: [Currency; 4] = [Currency::USD, Currency::EUR, Currency::GBP, Currency::INR];

fn ev() -> impl Strategy<Value = Ev> {
    prop_oneof![
        6 => (0..4usize, 100..200_000i64, 0..30i64, 0..10i64).prop_map(|(c, a, tax, fee)| Ev::Sale(c, a, a * tax / 100, a * fee / 100 + 40)),
        1 => (0..4usize, 100..50_000i64).prop_map(|(c, a)| Ev::Refund(c, a)),
        1 => (0..4usize, 100..50_000i64, any::<bool>()).prop_map(|(c, a, w)| Ev::Dispute(c, a, w)),
        1 => (0..4usize, 100..20_000i64, any::<bool>()).prop_map(|(c, a, r)| Ev::Hold(c, a, r)),
        1 => (0..4usize, -5_000..5_000i64).prop_map(|(c, a)| Ev::Adjust(c, a)),
    ]
}

/// Events spread over ~120 days from 2027-12-20, so runs cross a year end and the 2028 leap day.
fn build(evs: &[(Ev, u32)]) -> Builder {
    let mut b = Builder::new();
    let start: chrono::DateTime<chrono::Utc> = "2027-12-20T00:00:00Z".parse().unwrap();
    let mut t = start;
    for (i, (e, gap)) in evs.iter().enumerate() {
        t += chrono::Duration::minutes(*gap as i64);
        let at = t.to_rfc3339();
        let id = format!("x{i}");
        match *e {
            Ev::Sale(c, a, tax, fee) => b.sale(&id, &at, CUR[c], a, tax, fee),
            Ev::Refund(c, a) => b.refund(&id, &at, CUR[c], a, 0, 100),
            Ev::Dispute(c, a, won) => {
                b.dispute(&id, &at, CUR[c], a, 3_000);
                if won {
                    b.dispute_won(&id, &at, CUR[c], a);
                }
            }
            Ev::Hold(c, a, release) => {
                b.reserve_hold(&id, &at, CUR[c], a);
                if release {
                    b.reserve_release(&format!("{id}:rel"), &at, CUR[c], a);
                }
            }
            Ev::Adjust(c, a) => b.adjustment(&id, &at, CUR[c], a),
        }
    }
    b
}

fn input(b: &Builder, linked: bool) -> Input {
    let mut i: Input = serde_json::from_value(serde_json::json!({
        "as_of": "2028-05-01T00:00:00Z",
        "ledger": b.entries,
        "rates": [
            { "date": "2027-01-01", "currency": "EUR", "usd": "1.0832" },
            { "date": "2027-01-01", "currency": "GBP", "usd": "1.2710" },
            { "date": "2028-02-01", "currency": "EUR", "usd": "1.0790" }
        ],
    }))
    .unwrap();
    if linked {
        i.settings.linked = vec![Currency::GBP];
    }
    i
}

fn events() -> impl Strategy<Value = Vec<(Ev, u32)>> {
    prop::collection::vec((ev(), 1..4_000u32), 1..80)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// Every cent in a wallet is either paid out or still in the wallet, and every run's lines add up exactly.
    #[test]
    fn money_is_conserved(evs in events(), linked in any::<bool>()) {
        let b = build(&evs);
        let r = whenpaid::explain(&input(&b, linked)).unwrap(); // explain() runs verify()
        for c in Currency::PAYOUT_WALLETS {
            let past: Vec<_> = r.runs.iter().filter(|x| x.status != RunStatus::Projected).flat_map(|x| x.wallets.iter().filter(move |w| w.currency == c)).collect();
            let drained: i64 = past.iter().map(|w| w.drained.minor).sum();
            let counted: i64 = past.iter().flat_map(|w| w.lines.iter()).map(|l| l.amount.minor).sum();
            let last_carry = past.last().map_or(0, |w| w.carried_out.minor);
            prop_assert_eq!(counted - drained, last_carry, "{}", c);
        }
        for run in &r.runs {
            if let Some(e) = &run.expected && let (Some(sent), Some(cf)) = (e.sent, e.conversion_fee) {
                prop_assert_eq!(sent.minor + cf.minor + e.payout_fee.minor + e.wire_fee.minor, e.gross.minor);
            }
        }
        // The ledger book always balances, and reconciliation always closes.
        for rec in &r.reconciliation {
            prop_assert!(rec.balanced, "{:?}", rec);
        }
    }

    /// Order of the export doesn't matter: shuffled and duplicated rows give the same report.
    #[test]
    fn deterministic_replay_under_shuffle_and_duplicates(evs in events(), seed in any::<u64>()) {
        let b = build(&evs);
        let a = serde_json::to_value(whenpaid::explain(&input(&b, false)).unwrap()).unwrap();
        let mut rows = b.entries.clone();
        let mut s = seed | 1;
        for i in (1..rows.len()).rev() {
            s ^= s << 13; s ^= s >> 7; s ^= s << 17;
            rows.swap(i, (s % (i as u64 + 1)) as usize);
        }
        let dup = rows[(s % rows.len() as u64) as usize].clone();
        rows.push(dup);
        let mut b2 = b.clone();
        b2.entries = rows;
        let r2 = whenpaid::explain(&input(&b2, false)).unwrap();
        let mut v2 = serde_json::to_value(&r2).unwrap();
        // The only allowed difference is the duplicate finding itself.
        v2["findings"] = serde_json::Value::Array(v2["findings"].as_array().unwrap().iter().filter(|f| f["finding"] != "duplicate").cloned().collect());
        prop_assert!(a == v2, "shuffled input gave a different report");
    }

    /// Dropping a row is always caught by the balance chain, with the exact amount.
    #[test]
    fn a_missing_row_shows_up_as_a_gap_of_its_amount(evs in events(), pick in any::<prop::sample::Index>()) {
        let b = build(&evs);
        prop_assume!(b.entries.len() >= 2);
        let i = pick.index(b.entries.len() - 1); // never the last row: nothing after it to notice
        let gone = b.entries[i].clone();
        // A row is only detectable if the wallet has rows on both sides of it: the first row's
        // before_balance is the opening balance, so losing it just moves the opening.
        let later = b.entries[i + 1..].iter().any(|e| e.currency == gone.currency);
        let earlier = b.entries[..i].iter().any(|e| e.currency == gone.currency);
        prop_assume!(later && earlier);
        let mut rows = b.entries.clone();
        rows.remove(i);
        let r = whenpaid::explain(&Input { ledger: rows, ..input(&b, false) }).unwrap();
        let gaps: Vec<_> = r.findings.iter().filter_map(|f| match f { Finding::Gap { currency, difference, .. } => Some((*currency, *difference)), _ => None }).collect();
        prop_assert_eq!(gaps, vec![(gone.currency, gone.signed().minor)]);
    }
}
