//! Builds ledger rows in the provider's export format, with correct running
//! balances. Used by tests, the benchmark and the examples, never by the engine.

use crate::ledger::LedgerEntry;
use crate::money::Currency;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

#[derive(Default, Clone)]
pub struct Builder {
    pub entries: Vec<LedgerEntry>,
    bal: HashMap<Currency, i64>,
}

fn ts(s: &str) -> DateTime<Utc> {
    s.parse().unwrap_or_else(|_| panic!("bad timestamp {s}"))
}

impl Builder {
    pub fn new() -> Builder {
        Builder::default()
    }

    /// Append one row. Rows must be added in time order for the balances to be right.
    #[allow(clippy::too_many_arguments)]
    pub fn entry(
        &mut self,
        id: &str,
        at: &str,
        cur: Currency,
        event_type: &str,
        amount: i64,
        is_credit: bool,
        reference: Option<&str>,
        payout: Option<&str>,
    ) {
        self.entry_at(
            id,
            ts(at),
            cur,
            event_type,
            amount,
            is_credit,
            reference,
            payout,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn entry_at(
        &mut self,
        id: &str,
        at: DateTime<Utc>,
        cur: Currency,
        event_type: &str,
        amount: i64,
        is_credit: bool,
        reference: Option<&str>,
        payout: Option<&str>,
    ) {
        if amount == 0 {
            return;
        }
        let before = *self.bal.get(&cur).unwrap_or(&0);
        let after = before + if is_credit { amount } else { -amount };
        self.bal.insert(cur, after);
        self.entries.push(LedgerEntry {
            id: id.into(),
            created_at: at,
            currency: cur,
            event_type: event_type.into(),
            amount,
            is_credit,
            before_balance: Some(before),
            after_balance: Some(after),
            payout_id: payout.map(Into::into),
            reference_object_id: reference.map(Into::into),
            description: None,
            usd_equivalent_amount: None,
        });
    }

    pub fn balance(&self, cur: Currency) -> i64 {
        *self.bal.get(&cur).unwrap_or(&0)
    }

    /// A payment with its tax and fee rows.
    pub fn sale(&mut self, id: &str, at: &str, cur: Currency, amount: i64, tax: i64, fee: i64) {
        self.entry(id, at, cur, "payment", amount, true, Some(id), None);
        self.entry(
            &format!("{id}:tax"),
            at,
            cur,
            "tax",
            tax,
            false,
            Some(id),
            None,
        );
        self.entry(
            &format!("{id}:fee"),
            at,
            cur,
            "payment_fees",
            fee,
            false,
            Some(id),
            None,
        );
    }

    pub fn refund(
        &mut self,
        id: &str,
        at: &str,
        cur: Currency,
        amount: i64,
        tax_returned: i64,
        fee: i64,
    ) {
        self.entry(id, at, cur, "refund", amount, false, Some(id), None);
        self.entry(
            &format!("{id}:tax"),
            at,
            cur,
            "tax_reversal",
            tax_returned,
            true,
            Some(id),
            None,
        );
        self.entry(
            &format!("{id}:fee"),
            at,
            cur,
            "refund_fees",
            fee,
            false,
            Some(id),
            None,
        );
    }

    pub fn dispute(&mut self, id: &str, at: &str, cur: Currency, amount: i64, fee: i64) {
        self.entry(id, at, cur, "dispute", amount, false, Some(id), None);
        self.entry(
            &format!("{id}:fee"),
            at,
            cur,
            "dispute_fees",
            fee,
            false,
            Some(id),
            None,
        );
    }

    pub fn dispute_won(&mut self, id: &str, at: &str, cur: Currency, amount: i64) {
        self.entry(
            &format!("{id}:won"),
            at,
            cur,
            "dispute_reversal",
            amount,
            true,
            Some(id),
            None,
        );
    }

    pub fn reserve_hold(&mut self, id: &str, at: &str, cur: Currency, amount: i64) {
        self.entry(id, at, cur, "reserve_hold", amount, false, Some(id), None);
    }

    pub fn reserve_release(&mut self, id: &str, at: &str, cur: Currency, amount: i64) {
        self.entry(id, at, cur, "reserve_release", amount, true, Some(id), None);
    }

    /// The rows a payout leaves: the amount sent and the payout fee.
    pub fn payout(&mut self, pyt: &str, at: &str, cur: Currency, sent: i64, fee: i64) {
        self.entry(pyt, at, cur, "payout", sent, false, None, Some(pyt));
        self.entry(
            &format!("{pyt}:fee"),
            at,
            cur,
            "payout_fees",
            fee,
            false,
            None,
            Some(pyt),
        );
    }

    pub fn payout_reversal(&mut self, pyt: &str, at: &str, cur: Currency, sent: i64, fee: i64) {
        self.entry(
            &format!("{pyt}:rev"),
            at,
            cur,
            "payout_reversal",
            sent,
            true,
            None,
            Some(pyt),
        );
        self.entry(
            &format!("{pyt}:fee:rev"),
            at,
            cur,
            "payout_fees_reversal",
            fee,
            true,
            None,
            Some(pyt),
        );
    }

    /// Move `from_amount` out of one wallet and `to_amount` into another.
    pub fn conversion(
        &mut self,
        id: &str,
        at: &str,
        from: Currency,
        from_amount: i64,
        to: Currency,
        to_amount: i64,
    ) {
        self.entry(
            &format!("{id}:out"),
            at,
            from,
            "currency_conversion",
            from_amount,
            false,
            Some(id),
            None,
        );
        self.entry(
            &format!("{id}:in"),
            at,
            to,
            "currency_conversion",
            to_amount,
            true,
            Some(id),
            None,
        );
    }

    pub fn adjustment(&mut self, id: &str, at: &str, cur: Currency, signed: i64) {
        self.entry(
            id,
            at,
            cur,
            "adjustment",
            signed.abs(),
            signed > 0,
            Some(id),
            None,
        );
    }
}
