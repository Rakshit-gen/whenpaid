//! Plain-text rendering of a report. Everything here comes from the trace; nothing is recomputed.

use crate::engine::{Discrepancy, Report, Run, RunStatus};
use crate::ledger::Finding;
use crate::money::Money;
use std::fmt::Write;

pub struct Options {
    /// Show every past run, not just the last few.
    pub all: bool,
    /// List every entry id under each line.
    pub entries: bool,
}

const RECENT: usize = 3;
const IDS_SHOWN: usize = 4;

pub fn render(r: &Report, o: &Options) -> String {
    let mut s = String::new();
    let w = &mut s;
    let _ = writeln!(
        w,
        "{} report as of {}",
        r.tool,
        r.as_of.format("%Y-%m-%d %H:%M UTC")
    );
    if let Some(b) = &r.business_id {
        let _ = writeln!(w, "Business {b}");
    }
    let docs: Vec<String> = r
        .docs_as_of
        .iter()
        .filter(|(_, d)| *d != "not shown")
        .map(|(k, d)| format!("{k} {d}"))
        .collect();
    let _ = writeln!(
        w,
        "Rules {}, docs as of: {}",
        r.policy_version,
        docs.join(", ")
    );
    let rounding = match r.rounding {
        crate::money::Rounding::HalfEven => "half to even",
        crate::money::Rounding::HalfUp => "half up",
        crate::money::Rounding::TowardZero => "toward zero",
    };
    let _ = writeln!(
        w,
        "Rounding: {rounding}, to the smallest unit of each currency. `whenpaid rules` shows where each rule id comes from.\n"
    );

    let _ = writeln!(w, "In short");
    for line in &r.summary {
        let _ = writeln!(w, "  {line}");
    }
    let _ = writeln!(w);

    let yn = |b: Option<bool>| {
        b.map_or("not known (no verification data given)", |b| {
            if b { "yes" } else { "no" }
        })
    };
    let _ = writeln!(w, "Live payments: {}", yn(r.eligibility.payments_live));
    let _ = writeln!(w, "Payouts enabled: {}", yn(r.eligibility.payouts_enabled));
    for b in &r.eligibility.blockers {
        let _ = writeln!(
            w,
            "  Blocked: {}. Reason: {}. Until: {}. [{}]\n    Next: {}",
            b.what, b.reason, b.until, b.rule, b.action
        );
    }

    let next: Vec<&Run> = r
        .runs
        .iter()
        .filter(|x| x.status == RunStatus::Projected)
        .collect();
    if !next.is_empty() {
        let _ = writeln!(w, "\nNext payout");
        for run in &next {
            run_block(w, run, o);
        }
    }

    let past: Vec<&Run> = r
        .runs
        .iter()
        .filter(|x| {
            x.status != RunStatus::Projected && (x.status != RunStatus::NothingToPay || o.all)
        })
        .collect();
    if !past.is_empty() {
        let skip = if o.all {
            0
        } else {
            past.len().saturating_sub(RECENT)
        };
        let _ = writeln!(
            w,
            "\nPast runs{}",
            if skip > 0 {
                format!(" (last {RECENT} of {}; --all shows every one)", past.len())
            } else {
                String::new()
            }
        );
        for run in past.iter().skip(skip).rev() {
            run_block(w, run, o);
        }
    }

    if !r.payouts.is_empty() {
        let _ = writeln!(w, "\nPayouts");
        for p in &r.payouts {
            let amt = p
                .sent
                .map_or("amount not reported".into(), |m| m.to_string());
            let _ = writeln!(
                w,
                "  {} created {}: {amt}, {}",
                p.payout_id,
                p.created_at.date_naive(),
                p.state
            );
            let _ = writeln!(
                w,
                "    For: {}. Next: {}",
                p.run.as_deref().unwrap_or("no run matched"),
                p.next_action
            );
        }
    }

    let _ = writeln!(w, "\nWallet reconciliation");
    for rec in &r.reconciliation {
        let check = match rec.reported_closing {
            Some(rc) if rec.balanced => format!("matches the reported balance {rc}"),
            Some(rc) => format!("DOES NOT match the reported balance {rc}"),
            None => "no balance reported to compare".into(),
        };
        let _ = writeln!(
            w,
            "  {}: opened {}, moved {}, closed {} ({check})",
            rec.currency,
            rec.opening,
            Money::new(
                rec.movements.iter().map(|l| l.amount.minor).sum(),
                rec.currency
            ),
            rec.closing
        );
        if rec.unattributed_gaps.minor != 0 {
            let _ = writeln!(
                w,
                "    {} of that is balance movement with no entry in the data (see findings)",
                rec.unattributed_gaps
            );
        }
    }
    for x in &r.excluded {
        let _ = writeln!(
            w,
            "  {} {} across {} {} left out: {} [{}]",
            x.currency,
            x.balance,
            x.entries,
            if x.entries == 1 {
                "entry is"
            } else {
                "entries are"
            },
            x.why,
            x.rule
        );
    }
    for c in &r.settled_conversions {
        let _ = writeln!(
            w,
            "  Settled conversion {}: {} became {} at {}",
            c.reference, c.from, c.to, c.implied_rate
        );
    }

    let errors: Vec<&Discrepancy> = r.discrepancies.iter().filter(|d| d.is_error()).collect();
    let notes: Vec<&Discrepancy> = r.discrepancies.iter().filter(|d| !d.is_error()).collect();
    if !errors.is_empty() || !r.findings.is_empty() {
        let _ = writeln!(w, "\nNeeds a look");
        for d in errors {
            let _ = writeln!(w, "  {}", discrepancy(d));
        }
        for f in &r.findings {
            let _ = writeln!(w, "  {}", finding(f));
        }
    }
    for d in notes {
        let _ = writeln!(w, "  Note: {}", discrepancy(d));
    }
    s
}

fn status(run: &Run) -> String {
    match run.status {
        RunStatus::Paid => format!("paid ({})", run.matched.as_ref().unwrap().payout_id),
        RunStatus::PaidSimulated => {
            "should have paid (no payout data given, so this is the rules' figure)".into()
        }
        RunStatus::Missing => "should have paid, and no payout was found".into(),
        RunStatus::BelowThreshold => "not paid: below the minimum, carried forward".into(),
        RunStatus::NothingToPay => "nothing to pay".into(),
        RunStatus::Blocked => "not paid: payouts were blocked".into(),
        RunStatus::Reversed => format!(
            "paid, then failed and credited back ({})",
            run.matched.as_ref().unwrap().payout_id
        ),
        RunStatus::Undecided => "can't decide: a rate is missing".into(),
        RunStatus::Projected => "upcoming".into(),
    }
}

fn run_block(w: &mut String, run: &Run, o: &Options) {
    let moved = if run.slot.runs != run.slot.scheduled {
        format!(
            " (scheduled {}, moved past a weekend or holiday)",
            run.slot.scheduled
        )
    } else {
        String::new()
    };
    let _ = writeln!(
        w,
        "\n  {} {}: {}{}",
        run.slot.runs,
        run.group,
        status(run),
        moved
    );
    let _ = writeln!(
        w,
        "    Covers {} to {}, created 00:00 UTC, counts entries before {}",
        run.slot.period_start,
        run.slot.period_end,
        run.cutoff.format("%Y-%m-%d %H:%M UTC")
    );
    for p in &run.wallets {
        if p.carried_in.minor == 0 && p.lines.is_empty() {
            continue;
        }
        let _ = writeln!(w, "    {} wallet", p.currency);
        let _ = writeln!(
            w,
            "      {:<46}{:>18}",
            "carried in",
            p.carried_in.to_string()
        );
        for l in &p.lines {
            let _ = writeln!(
                w,
                "      {:<46}{:>18}",
                format!("{} ({})", l.label, l.count),
                l.amount.to_string()
            );
            let ids: Vec<&crate::engine::EntryRef> = p
                .entries
                .iter()
                .filter(|e| {
                    e.amount.currency == l.amount.currency
                        && crate::ledger::EventType::parse(&e.event_type) == l.kind
                })
                .collect();
            let shown = if o.entries {
                ids.len()
            } else {
                IDS_SHOWN.min(ids.len())
            };
            let list: Vec<String> = ids[..shown]
                .iter()
                .map(|e| format!("{} {}", e.id, e.amount))
                .collect();
            let more = if ids.len() > shown {
                format!(", and {} more (--entries)", ids.len() - shown)
            } else {
                String::new()
            };
            let _ = writeln!(w, "        {}{more}", list.join(", "));
        }
        let _ = writeln!(w, "      {:<46}{:>18}", "payable", p.payable.to_string());
        if p.drained.minor != 0 || p.carried_out != p.payable {
            let _ = writeln!(
                w,
                "      {:<46}{:>18}",
                "paid out (amount plus fees)",
                Money::new(-p.drained.minor, p.currency).to_string()
            );
            let _ = writeln!(
                w,
                "      {:<46}{:>18}",
                "carries forward",
                p.carried_out.to_string()
            );
        }
    }
    if let Some(t) = &run.threshold {
        let _ = writeln!(
            w,
            "    Minimum: {} against {}, {} [{}]",
            t.total,
            t.threshold,
            if t.met { "reached" } else { "not reached" },
            t.rule
        );
        for c in t.conversions.iter().filter(|c| c.from.minor != 0) {
            let _ = writeln!(
                w,
                "      {} is {} at {} (rate dated {}; exact {}, rounded {})",
                c.from,
                c.to,
                c.rate,
                c.rate_date,
                exact(&c.exact, c.to),
                residual(&c.residual, c.to)
            );
        }
    }
    if let Some(e) = &run.expected {
        let _ = writeln!(w, "    Expected payout in {}", run.currency);
        for c in e.conversions.iter().filter(|c| c.from.minor != 0) {
            let _ = writeln!(
                w,
                "      {} converts to about {} (estimate at {}; the transfer-day rate decides)",
                c.from, c.to, c.rate
            );
        }
        let _ = writeln!(w, "      {:<46}{:>18}", "before fees", e.gross.to_string());
        match e.conversion_fee {
            Some(f) if f.minor != 0 => {
                let _ = writeln!(w, "      {:<46}{:>18}", "conversion fee", format!("-{f}"));
            }
            None => {
                let _ = writeln!(w, "      {:<46}{:>18}", "conversion fee", "unknown");
            }
            _ => {}
        }
        let _ = writeln!(
            w,
            "      {:<46}{:>18}",
            "payout fee",
            format!("-{}", e.payout_fee)
        );
        let _ = writeln!(w, "        {}", e.payout_fee_note);
        if e.wire_fee.minor != 0 {
            let _ = writeln!(
                w,
                "      {:<46}{:>18}",
                "SWIFT wire fee",
                format!("-{}", e.wire_fee)
            );
        }
        let _ = writeln!(
            w,
            "      {:<46}{:>18}",
            "sent",
            e.sent.map_or("unknown".into(), |s| s.to_string())
        );
        for u in &e.unknown {
            let _ = writeln!(w, "      Not known: {u}");
        }
    }
    if let Some(m) = &run.matched {
        let sent = m.sent.map_or("not reported".into(), |s| s.to_string());
        let fee = m.fee.map_or("not reported".into(), |s| s.to_string());
        let _ = writeln!(w, "    Actual: {} sent {sent}, fee {fee}", m.payout_id);
        if let Some(c) = m.implied_conversion_cost {
            let _ = writeln!(
                w,
                "      Conversion cost implied by what was sent: {c} (before fees minus fees minus sent)"
            );
        }
        for u in m.unattributed.iter().filter(|u| u.minor != 0) {
            let dir = if u.minor > 0 {
                "more than this run's balance (it drew on an earlier leftover)"
            } else {
                "less than this run's balance (the rest stays for a later payout)"
            };
            let _ = writeln!(
                w,
                "      Unattributed {u}: the payout took {dir} [PB-UNATTRIBUTED]"
            );
        }
    }
    for b in &run.blockers {
        let _ = writeln!(
            w,
            "    Blocked by: {}. Reason: {}. Until: {}. [{}]",
            b.what, b.reason, b.until, b.rule
        );
    }
    if let (Some(d), Some(n)) = (run.due_since, run.overdue_days) {
        let _ = writeln!(w, "    Due since {d}, {n} days ago [PS-MISSED-DATE]");
    }
    let _ = writeln!(w, "    Rules: {}", run.rules.join(", "));
    let _ = writeln!(w, "    Next: {}", run.next_action);
}

/// An exact amount in minor units as a decimal in major units, e.g. 20892677/1000 cents -> "208.92677 USD".
fn exact(f: &crate::money::Fraction, m: Money) -> String {
    let e = m.currency.exponent();
    let places = 8u32;
    let scaled = crate::money::div_round(
        f.num * 10i128.pow(places),
        f.den * 10i128.pow(e),
        crate::money::Rounding::HalfEven,
    );
    let p = 10i128.pow(places);
    let sign = if scaled < 0 { "-" } else { "" };
    let frac = format!("{:0w$}", (scaled % p).abs(), w = places as usize);
    let frac = frac.trim_end_matches('0');
    let whole = (scaled / p).abs();
    let terminates = (f.num * 10i128.pow(places)) % (f.den * 10i128.pow(e)) == 0;
    format!(
        "{sign}{whole}{}{}{} {}",
        if frac.is_empty() { "" } else { "." },
        frac,
        if terminates { "" } else { "..." },
        m.currency
    )
}

fn residual(f: &crate::money::Fraction, m: Money) -> String {
    let unit = if m.currency.exponent() == 0 {
        "unit"
    } else {
        "cent"
    };
    if f.is_zero() {
        return "exactly".into();
    }
    let th = crate::money::div_round(f.num.abs() * 1000, f.den, crate::money::Rounding::HalfEven);
    let amount = format!("{}.{:03}", th / 1000, th % 1000);
    if f.num > 0 {
        format!("down by {amount} of a {unit}")
    } else {
        format!("up by {amount} of a {unit}")
    }
}

pub fn discrepancy(d: &Discrepancy) -> String {
    use Discrepancy::*;
    match d {
        MissingPayout { run, expected } => format!(
            "{run}: should have paid {} and no payout was found.",
            expected.map_or("an unknown amount".into(), |m| m.to_string())
        ),
        AmountDiffers {
            run,
            payout_id,
            currency,
            unattributed,
        } => format!(
            "{run}: {payout_id} took {} {} than the balance the rules give. That is the unattributed amount, not an error by itself; check the payout's breakup.",
            Money::new(unattributed.abs(), *currency),
            if *unattributed > 0 { "more" } else { "less" }
        ),
        SentDiffers {
            run,
            payout_id,
            expected,
            reported,
        } => format!("{run}: {payout_id} sent {reported}, the rules give {expected}."),
        FeeDiffers {
            payout_id,
            expected,
            reported,
            rule,
        } => format!(
            "{payout_id}: payout fee {reported}, the published schedule gives {expected} [{rule}]."
        ),
        FeeNotCheckable { payout_id, why } => format!("{payout_id}: fee not checked: {why}."),
        FailedWithoutReversal { payout_id } => format!(
            "{payout_id} failed but the ledger has no payout reversal for it yet [PS-FAILED]."
        ),
        TwoPayoutsOneRun { run, payout_ids } => format!(
            "{run}: {} all fall in this run's window; not guessing which is which [DC-MATCH].",
            payout_ids.join(" and ")
        ),
        UnmatchedPayout { payout_id } => {
            format!("{payout_id} doesn't line up with any scheduled run (manual or off-cycle?).")
        }
        MissingRate {
            run,
            currency,
            date,
        } => format!("{run}: no {currency} rate on or before {date}."),
        PayoutWhileBlocked { run, payout_id } => format!(
            "{run}: {payout_id} was paid while the account data says payouts were blocked. One of the two inputs is out of date."
        ),
        RecordWithoutLedgerRows { payout_id } => format!(
            "{payout_id} is in the payout records but has no rows in the ledger export. The export may be incomplete; import a fresh one."
        ),
    }
}

pub fn finding(f: &Finding) -> String {
    use Finding::*;
    match f {
        Duplicate { id, copies } => {
            format!("{id} appeared {copies} times with identical content; counted once.")
        }
        WrongSign {
            id,
            event_type,
            is_credit,
        } => format!(
            "{id} is a {event_type} marked as a {}, which is the wrong direction for that type.",
            if *is_credit { "credit" } else { "debit" }
        ),
        UnknownType { id, event_type } => {
            format!("{id} has an unrecognised type {event_type:?}; counted as is.")
        }
        Gap {
            currency,
            after,
            before,
            expected,
            reported,
            difference,
        } => format!(
            "{currency} balance jumps by {} between {} and {before}: running balance {}, reported {}. Entries are missing from the export; backfill this range.",
            Money::new(*difference, *currency),
            after.as_deref().unwrap_or("the start"),
            Money::new(*expected, *currency),
            Money::new(*reported, *currency)
        ),
        BadArithmetic {
            id,
            before,
            amount,
            after,
        } => format!("{id}: before {before} plus {amount} is not after {after}."),
        PossibleDoubleDeduction {
            kind,
            reference,
            ids,
        } => format!(
            "{kind} charged more than once for {reference}: {}.",
            ids.join(", ")
        ),
    }
}
