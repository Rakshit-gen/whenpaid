//! whenpaid: explains a merchant's payouts from the ledger, one cited rule at a time.

pub mod account;
pub mod build;
pub mod calendar;
pub mod engine;
pub mod ledger;
pub mod money;
pub mod policy;
pub mod report;
pub mod server;
pub mod store;
pub mod webhook;

pub use engine::{Input, Report};

use sha2::{Digest, Sha256};

/// Run the engine and check its own arithmetic before returning anything.
pub fn explain(input: &Input) -> Result<Report, String> {
    let r = engine::run(input)?;
    verify(&r)
        .map_err(|e| format!("internal check failed, please report this with your input: {e}"))?;
    Ok(r)
}

/// SHA-256 of the input as parsed, so two reports can be tied to the same data.
pub fn input_hash(input: &Input) -> String {
    hex::encode(Sha256::digest(
        serde_json::to_vec(input).expect("input serializes"),
    ))
}

/// Invariants every report must satisfy. A failure here is a bug in whenpaid, not in the data.
pub fn verify(r: &Report) -> Result<(), String> {
    for run in &r.runs {
        for w in &run.wallets {
            let sum: i64 = w.lines.iter().map(|l| l.amount.minor).sum();
            if w.carried_in.minor + sum != w.payable.minor {
                return Err(format!(
                    "{} {}: lines don't add up to the payable balance",
                    run.group, run.slot.runs
                ));
            }
            let entries: i64 = w.entries.iter().map(|e| e.amount.minor).sum();
            if entries != sum {
                return Err(format!(
                    "{} {}: entries don't add up to the lines",
                    run.group, run.slot.runs
                ));
            }
            if w.payable.minor - w.drained.minor != w.carried_out.minor {
                return Err(format!(
                    "{} {}: payable minus drained isn't what carries forward",
                    run.group, run.slot.runs
                ));
            }
        }
        if let Some(e) = &run.expected {
            let primary: i64 = run
                .wallets
                .iter()
                .filter(|w| w.currency == run.currency)
                .map(|w| w.payable.minor)
                .sum();
            let converted: i64 = e.conversions.iter().map(|c| c.to.minor).sum();
            if primary + converted != e.gross.minor {
                return Err(format!(
                    "{} {}: converted wallets don't add up to the gross",
                    run.group, run.slot.runs
                ));
            }
            if let (Some(sent), Some(cf)) = (e.sent, e.conversion_fee)
                && sent.minor + cf.minor + e.payout_fee.minor + e.wire_fee.minor != e.gross.minor
            {
                return Err(format!(
                    "{} {}: sent plus fees isn't the gross",
                    run.group, run.slot.runs
                ));
            }
        }
    }
    for rec in &r.reconciliation {
        let moved: i64 = rec.movements.iter().map(|l| l.amount.minor).sum();
        if rec.opening.minor + moved + rec.unattributed_gaps.minor != rec.closing.minor {
            return Err(format!(
                "{}: opening plus movements isn't the closing balance",
                rec.currency
            ));
        }
    }
    Ok(())
}
