//! The merchant's wallet ledger, rebuilt as an immutable double-entry book.
//!
//! Input is the provider's balance ledger export: one row per wallet movement
//! with a credit/debit flag and, usually, the wallet balance before and after.
//! Each row becomes one balanced transaction: the wallet on one side and the
//! account the money came from or went to on the other. Nothing is edited in
//! place. A correction is a new row (a reversal or an adjustment), exactly as
//! the provider records it.

use crate::money::{Currency, Money};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// One row of the balance ledger export (BAL-CHAIN). Field names follow the provider's API.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct LedgerEntry {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub currency: Currency,
    pub event_type: String,
    /// Always non-negative; the direction is `is_credit`.
    pub amount: i64,
    pub is_credit: bool,
    #[serde(default)]
    pub before_balance: Option<i64>,
    #[serde(default)]
    pub after_balance: Option<i64>,
    #[serde(default)]
    pub payout_id: Option<String>,
    #[serde(default)]
    pub reference_object_id: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub usd_equivalent_amount: Option<i64>,
}

impl LedgerEntry {
    pub fn signed(&self) -> Money {
        Money::new(
            if self.is_credit {
                self.amount
            } else {
                -self.amount
            },
            self.currency,
        )
    }
    pub fn kind(&self) -> EventType {
        EventType::parse(&self.event_type)
    }
}

/// The documented entry types (BAL-TYPES) and what each one moves money against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    Payment,
    Refund,
    RefundReversal,
    Dispute,
    DisputeReversal,
    Tax,
    TaxReversal,
    PaymentFees,
    RefundFees,
    RefundFeesReversal,
    DisputeFees,
    DisputeFeesReversal,
    OtherFee,
    OtherFeeReversal,
    Payout,
    PayoutFees,
    PayoutReversal,
    PayoutFeesReversal,
    DodoCredits,
    Adjustment,
    CurrencyConversion,
    ReserveHold,
    ReserveRelease,
    Unknown,
}

impl EventType {
    pub fn parse(s: &str) -> EventType {
        use EventType::*;
        match s {
            "payment" => Payment,
            "refund" => Refund,
            "refund_reversal" => RefundReversal,
            "dispute" => Dispute,
            "dispute_reversal" => DisputeReversal,
            "tax" => Tax,
            "tax_reversal" => TaxReversal,
            "payment_fees" => PaymentFees,
            "refund_fees" => RefundFees,
            "refund_fees_reversal" => RefundFeesReversal,
            "dispute_fees" => DisputeFees,
            "dispute_fees_reversal" => DisputeFeesReversal,
            "abandoned_cart_recovery_fee"
            | "dunning_fees"
            | "payment_retry_fee"
            | "byop_fee"
            | "ethoca_fees"
            | "visa_rdr_fee"
            | "deflection_fees"
            | "moderation_fees"
            | "dodo_fixed_fee"
            | "interchange_fee"
            | "scheme_fee" => OtherFee,
            "ethoca_fees_reversal" | "deflection_fees_reversal" => OtherFeeReversal,
            "payout" => Payout,
            "payout_fees" => PayoutFees,
            "payout_reversal" => PayoutReversal,
            "payout_fees_reversal" => PayoutFeesReversal,
            "dodo_credits" => DodoCredits,
            "adjustment" => Adjustment,
            "currency_conversion" => CurrencyConversion,
            "reserve_hold" => ReserveHold,
            "reserve_release" => ReserveRelease,
            _ => Unknown,
        }
    }

    /// The documented direction, if fixed.
    pub fn expected_credit(self) -> Option<bool> {
        use EventType::*;
        match self {
            Payment | RefundReversal | DisputeReversal | TaxReversal | RefundFeesReversal
            | DisputeFeesReversal | OtherFeeReversal | PayoutReversal | PayoutFeesReversal
            | DodoCredits | ReserveRelease => Some(true),
            Refund | Dispute | Tax | PaymentFees | RefundFees | DisputeFees | OtherFee | Payout
            | PayoutFees | ReserveHold => Some(false),
            Adjustment | CurrencyConversion | Unknown => None,
        }
    }

    /// Where the other side of the entry is booked.
    pub fn counter(self) -> Book {
        use EventType::*;
        match self {
            Payment | Refund | RefundReversal => Book::Customers,
            Dispute | DisputeReversal => Book::Disputes,
            Tax | TaxReversal => Book::TaxAuthority,
            PaymentFees | RefundFees | RefundFeesReversal | DisputeFees | DisputeFeesReversal
            | OtherFee | OtherFeeReversal | PayoutFees | PayoutFeesReversal => Book::PlatformFees,
            Payout | PayoutReversal => Book::PaidOut,
            DodoCredits | Adjustment => Book::PlatformAdjustments,
            CurrencyConversion => Book::FxClearing,
            ReserveHold | ReserveRelease => Book::Reserve,
            Unknown => Book::Unclassified,
        }
    }

    /// One of these per reference is normal; a second one, net of reversals, needs a look.
    fn once_per(self) -> Option<(&'static str, EventType)> {
        use EventType::*;
        match self {
            Tax => Some(("tax", TaxReversal)),
            PaymentFees => Some(("payment_fees", Unknown)),
            DisputeFees => Some(("dispute_fees", DisputeFeesReversal)),
            RefundFees => Some(("refund_fees", RefundFeesReversal)),
            Payout => Some(("payout", PayoutReversal)),
            PayoutFees => Some(("payout_fees", PayoutFeesReversal)),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        use EventType::*;
        match self {
            Payment => "payments",
            Refund => "refunds",
            RefundReversal => "refunds reversed",
            Dispute => "disputed amounts held",
            DisputeReversal => "disputes won, amount returned",
            Tax => "tax collected and remitted",
            TaxReversal => "tax returned",
            PaymentFees => "payment fees",
            RefundFees => "refund fees",
            RefundFeesReversal => "refund fees returned",
            DisputeFees => "dispute fees (not returned if won)",
            DisputeFeesReversal => "dispute fees cancelled",
            OtherFee => "other fees",
            OtherFeeReversal => "other fees returned",
            Payout => "payouts sent",
            PayoutFees => "payout fees",
            PayoutReversal => "failed payouts credited back",
            PayoutFeesReversal => "payout fees credited back",
            DodoCredits => "credits from the provider",
            Adjustment => "manual adjustments",
            CurrencyConversion => "currency conversions",
            ReserveHold => "held in reserve",
            ReserveRelease => "released from reserve",
            Unknown => "entries of an unrecognised type",
        }
    }
}

/// The accounts of the book. The merchant's wallets are `Wallet`; the rest are counterparties.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Book {
    Wallet,
    Customers,
    Disputes,
    TaxAuthority,
    PlatformFees,
    PlatformAdjustments,
    Reserve,
    FxClearing,
    PaidOut,
    Unclassified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct Account {
    pub book: Book,
    pub currency: Currency,
}

#[derive(Clone, Debug, Serialize)]
pub struct Posting {
    pub account: Account,
    pub amount: Money,
}

/// A balanced set of postings. Immutable once in the ledger.
#[derive(Clone, Debug, Serialize)]
pub struct Transaction {
    pub id: String,
    pub at: DateTime<Utc>,
    pub postings: Vec<Posting>,
}

/// Something in the input that deserves a human look. None of these change a number.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "finding", rename_all = "snake_case")]
pub enum Finding {
    /// The same entry arrived more than once with identical content. Counted once.
    Duplicate {
        id: String,
        copies: usize,
    },
    /// The entry's credit/debit flag disagrees with its documented type.
    WrongSign {
        id: String,
        event_type: String,
        is_credit: bool,
    },
    UnknownType {
        id: String,
        event_type: String,
    },
    /// `before_balance` doesn't continue from the previous entry: rows are missing or out of order.
    Gap {
        currency: Currency,
        after: Option<String>,
        before: String,
        expected: i64,
        reported: i64,
        difference: i64,
    },
    /// `after_balance` isn't `before_balance` plus the amount.
    BadArithmetic {
        id: String,
        before: i64,
        amount: i64,
        after: i64,
    },
    /// A second deduction of the same kind for the same reference, net of reversals.
    PossibleDoubleDeduction {
        kind: String,
        reference: String,
        ids: Vec<String>,
    },
}

/// The book plus what we learned building it.
#[derive(Clone, Debug, Default)]
pub struct Ledger {
    /// Entries in book order: by time, then by balance chain, then by id.
    pub entries: Vec<LedgerEntry>,
    pub transactions: Vec<Transaction>,
    pub findings: Vec<Finding>,
    /// Each wallet's balance before the first entry in the data.
    pub opening: BTreeMap<Currency, i64>,
    balances: HashMap<Account, Money>,
}

impl Ledger {
    /// Validate and post `raw`. Errors only when the input is self-contradictory
    /// (the same id with two different contents); everything else is a finding.
    pub fn build(raw: &[LedgerEntry]) -> Result<Ledger, String> {
        let mut by_id: HashMap<&str, (&LedgerEntry, usize)> = HashMap::new();
        for e in raw {
            if e.amount < 0 {
                return Err(format!(
                    "entry {}: amount is negative; use is_credit for direction",
                    e.id
                ));
            }
            match by_id.get_mut(e.id.as_str()) {
                Some((first, n)) if *first == e => *n += 1,
                Some(_) => {
                    return Err(format!(
                        "entry {} appears twice with different contents; fix the export before trusting any number",
                        e.id
                    ));
                }
                None => {
                    by_id.insert(&e.id, (e, 1));
                }
            }
        }
        let mut l = Ledger::default();
        let mut uniq: Vec<&LedgerEntry> = by_id.values().map(|(e, _)| *e).collect();
        for (id, (_, n)) in &by_id {
            if *n > 1 {
                l.findings.push(Finding::Duplicate {
                    id: id.to_string(),
                    copies: *n,
                });
            }
        }
        uniq.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
        let ordered = chain_order(uniq);
        for e in ordered {
            l.post(e)?;
        }
        l.check_chain();
        l.check_double_deductions();
        l.findings.sort_by_key(|f| format!("{f:?}"));
        Ok(l)
    }

    fn post(&mut self, e: LedgerEntry) -> Result<(), String> {
        let kind = e.kind();
        match kind.expected_credit() {
            Some(c) if c != e.is_credit => self.findings.push(Finding::WrongSign {
                id: e.id.clone(),
                event_type: e.event_type.clone(),
                is_credit: e.is_credit,
            }),
            _ => {}
        }
        if kind == EventType::Unknown {
            self.findings.push(Finding::UnknownType {
                id: e.id.clone(),
                event_type: e.event_type.clone(),
            });
        }
        let amt = e.signed();
        let tx = Transaction {
            id: e.id.clone(),
            at: e.created_at,
            postings: vec![
                Posting {
                    account: Account {
                        book: Book::Wallet,
                        currency: e.currency,
                    },
                    amount: amt,
                },
                Posting {
                    account: Account {
                        book: kind.counter(),
                        currency: e.currency,
                    },
                    amount: amt.checked_neg()?,
                },
            ],
        };
        self.opening
            .entry(e.currency)
            .or_insert(e.before_balance.unwrap_or(0));
        for p in &tx.postings {
            let b = self
                .balances
                .entry(p.account)
                .or_insert(Money::zero(p.account.currency));
            *b = b.checked_add(p.amount)?;
        }
        self.transactions.push(tx);
        self.entries.push(e);
        Ok(())
    }

    fn check_chain(&mut self) {
        let mut last: HashMap<Currency, (Option<String>, i64)> = HashMap::new();
        for e in &self.entries {
            let open = self.opening[&e.currency];
            let (prev_id, running) = last.get(&e.currency).cloned().unwrap_or((None, open));
            let mut next = running + e.signed().minor;
            if let Some(b) = e.before_balance {
                if b != running {
                    self.findings.push(Finding::Gap {
                        currency: e.currency,
                        after: prev_id.clone(),
                        before: e.id.clone(),
                        expected: running,
                        reported: b,
                        difference: b - running,
                    });
                    next = b + e.signed().minor;
                }
                if let Some(a) = e.after_balance
                    && a != b + e.signed().minor
                {
                    self.findings.push(Finding::BadArithmetic {
                        id: e.id.clone(),
                        before: b,
                        amount: e.signed().minor,
                        after: a,
                    });
                }
            }
            last.insert(
                e.currency,
                (Some(e.id.clone()), e.after_balance.unwrap_or(next)),
            );
        }
    }

    fn check_double_deductions(&mut self) {
        let mut seen: BTreeMap<(&'static str, String), (i64, Vec<String>)> = BTreeMap::new();
        for e in &self.entries {
            let k = e.kind();
            let reference = |e: &LedgerEntry, k: EventType| match k {
                EventType::Payout
                | EventType::PayoutFees
                | EventType::PayoutReversal
                | EventType::PayoutFeesReversal => e.payout_id.clone(),
                _ => e.reference_object_id.clone(),
            };
            if let Some((name, _)) = k.once_per()
                && let Some(r) = reference(e, k)
            {
                let s = seen.entry((name, r)).or_default();
                s.0 += 1;
                s.1.push(e.id.clone());
            }
            // A reversal cancels one earlier deduction of its kind.
            for k2 in [
                EventType::Tax,
                EventType::DisputeFees,
                EventType::RefundFees,
                EventType::Payout,
                EventType::PayoutFees,
            ] {
                if let Some((name, rev)) = k2.once_per()
                    && rev == k
                    && let Some(r) = reference(e, k)
                    && let Some(s) = seen.get_mut(&(name, r))
                {
                    s.0 -= 1;
                }
            }
        }
        for ((kind, reference), (net, ids)) in seen {
            if net > 1 {
                self.findings.push(Finding::PossibleDoubleDeduction {
                    kind: kind.into(),
                    reference,
                    ids,
                });
            }
        }
    }

    pub fn balance(&self, book: Book, currency: Currency) -> Money {
        self.balances
            .get(&Account { book, currency })
            .copied()
            .unwrap_or(Money::zero(currency))
    }

    /// Sum of every posting in each currency. Always zero for a sound book.
    pub fn trial_balance(&self) -> BTreeMap<Currency, i64> {
        let mut t = BTreeMap::new();
        for (a, m) in &self.balances {
            *t.entry(a.currency).or_insert(0) += m.minor;
        }
        t
    }
}

/// Within one timestamp, follow the balance chain so equal-time rows don't look like gaps.
fn chain_order(sorted: Vec<&LedgerEntry>) -> Vec<LedgerEntry> {
    let mut out: Vec<LedgerEntry> = Vec::with_capacity(sorted.len());
    let mut running: HashMap<Currency, i64> = HashMap::new();
    let mut i = 0;
    while i < sorted.len() {
        let mut j = i;
        while j < sorted.len() && sorted[j].created_at == sorted[i].created_at {
            j += 1;
        }
        let mut group: Vec<&LedgerEntry> = sorted[i..j].to_vec();
        while !group.is_empty() {
            // Prefer the row that continues the running balance. Failing that (rows missing),
            // take a row no other remaining row leads into, so one gap is reported, not several.
            let is_head = |e: &LedgerEntry| {
                e.before_balance.is_some()
                    && !group.iter().any(|o| {
                        o.currency == e.currency
                            && o.id != e.id
                            && o.after_balance == e.before_balance
                    })
            };
            let pos = group
                .iter()
                .position(|e| {
                    running
                        .get(&e.currency)
                        .is_some_and(|r| e.before_balance == Some(*r))
                })
                .or_else(|| group.iter().position(|e| is_head(e)))
                .unwrap_or(0);
            let e = group.remove(pos);
            let base = e
                .before_balance
                .or(running.get(&e.currency).copied())
                .unwrap_or(0);
            running.insert(
                e.currency,
                e.after_balance.unwrap_or(base + e.signed().minor),
            );
            out.push(e.clone());
        }
        i = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::Builder;

    #[test]
    fn every_transaction_balances_and_the_book_sums_to_zero() {
        let mut b = Builder::new();
        b.sale(
            "pay_1",
            "2026-11-02T10:00:00Z",
            Currency::USD,
            23000,
            3000,
            1405,
        );
        b.refund("ref_1", "2026-11-03T10:00:00Z", Currency::USD, 5000, 0, 100);
        let l = Ledger::build(&b.entries).unwrap();
        assert!(l.trial_balance().values().all(|v| *v == 0));
        assert_eq!(
            l.balance(Book::Wallet, Currency::USD).minor,
            23000 - 3000 - 1405 - 5000 - 100
        );
        assert!(l.findings.is_empty(), "{:?}", l.findings);
    }

    #[test]
    fn identical_duplicates_count_once_conflicting_ones_stop() {
        let mut b = Builder::new();
        b.sale("pay_1", "2026-11-02T10:00:00Z", Currency::USD, 1000, 0, 0);
        let mut raw = b.entries.clone();
        raw.push(raw[0].clone());
        let l = Ledger::build(&raw).unwrap();
        assert_eq!(l.balance(Book::Wallet, Currency::USD).minor, 1000);
        assert!(matches!(
            l.findings[0],
            Finding::Duplicate { copies: 2, .. }
        ));
        raw[1].amount = 999;
        assert!(
            Ledger::build(&raw)
                .unwrap_err()
                .contains("different contents")
        );
    }

    #[test]
    fn gap_in_balance_chain_is_reported_not_guessed() {
        let mut b = Builder::new();
        b.sale("pay_1", "2026-11-02T10:00:00Z", Currency::USD, 1000, 0, 0);
        b.sale("pay_2", "2026-11-03T10:00:00Z", Currency::USD, 500, 0, 0);
        b.sale("pay_3", "2026-11-04T10:00:00Z", Currency::USD, 700, 0, 0);
        let mut raw = b.entries.clone();
        raw.remove(1); // lose pay_2
        let l = Ledger::build(&raw).unwrap();
        assert_eq!(
            l.findings,
            vec![Finding::Gap {
                currency: Currency::USD,
                after: Some("pay_1".into()),
                before: "pay_3".into(),
                expected: 1000,
                reported: 1500,
                difference: 500
            }]
        );
    }

    #[test]
    fn wrong_sign_is_flagged() {
        let mut b = Builder::new();
        b.sale("pay_1", "2026-11-02T10:00:00Z", Currency::USD, 1000, 0, 0);
        let mut raw = b.entries.clone();
        raw[0].is_credit = false;
        raw[0].before_balance = None;
        raw[0].after_balance = None;
        let l = Ledger::build(&raw).unwrap();
        assert!(matches!(&l.findings[0], Finding::WrongSign { id, .. } if id == "pay_1"));
    }

    #[test]
    fn second_dispute_fee_for_one_dispute_is_flagged() {
        let mut b = Builder::new();
        b.sale("pay_1", "2026-11-02T10:00:00Z", Currency::USD, 5000, 0, 0);
        b.dispute("dsp_1", "2026-11-05T10:00:00Z", Currency::USD, 5000, 3000);
        b.entry(
            "dsp_1:fee2",
            "2026-11-06T10:00:00Z",
            Currency::USD,
            "dispute_fees",
            3000,
            false,
            Some("dsp_1"),
            None,
        );
        let l = Ledger::build(&b.entries).unwrap();
        assert!(l.findings.iter().any(
            |f| matches!(f, Finding::PossibleDoubleDeduction { kind, .. } if kind == "dispute_fees")
        ));
    }

    #[test]
    fn dispute_won_returns_amount_not_fee() {
        let mut b = Builder::new();
        b.sale("pay_1", "2026-11-02T10:00:00Z", Currency::USD, 5000, 0, 0);
        b.dispute("dsp_1", "2026-11-05T10:00:00Z", Currency::USD, 5000, 3000);
        b.dispute_won("dsp_1", "2026-11-20T10:00:00Z", Currency::USD, 5000);
        let l = Ledger::build(&b.entries).unwrap();
        assert_eq!(l.balance(Book::Wallet, Currency::USD).minor, 5000 - 3000);
        assert_eq!(l.balance(Book::PlatformFees, Currency::USD).minor, 3000);
    }

    #[test]
    fn same_timestamp_rows_follow_the_chain() {
        let mut b = Builder::new();
        b.sale("pay_b", "2026-11-02T10:00:00Z", Currency::USD, 1000, 0, 0);
        b.sale("pay_a", "2026-11-02T10:00:00Z", Currency::USD, 500, 0, 0);
        let l = Ledger::build(&b.entries).unwrap();
        assert!(l.findings.is_empty(), "{:?}", l.findings);
    }
}
