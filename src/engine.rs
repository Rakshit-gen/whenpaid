//! Replays an account through the payout rules and explains every run.
//!
//! Pure and deterministic: the same input gives the same report, byte for
//! byte. Time, rates, settings and account state all come from the input;
//! nothing reads the clock or the network.

use crate::account::{
    self, Blocker, Eligibility, FormChange, PayoutData, Restriction, Settings, Transfer,
};
use crate::calendar::{self, Cycle, Slot};
use crate::ledger::{EventType, Finding, Ledger, LedgerEntry};
use crate::money::{self, Conversion, Currency, Money, Rate, Rounding};
use crate::policy::{self, FeeBasis, Params};
use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Everything the engine reads.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    #[serde(default)]
    pub business_id: Option<String>,
    /// "Now" for this report. Runs after it are projections.
    pub as_of: DateTime<Utc>,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub params: Params,
    pub ledger: Vec<LedgerEntry>,
    #[serde(default)]
    pub payouts: Vec<PayoutRecord>,
    #[serde(default)]
    pub rates: Vec<RateQuote>,
    #[serde(default)]
    pub forms: Vec<FormChange>,
    #[serde(default)]
    pub restrictions: Vec<Restriction>,
}

/// USD per one unit of `currency`, on and after `date` until the next quote.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RateQuote {
    pub date: NaiveDate,
    pub currency: Currency,
    pub usd: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PayoutStatus {
    #[serde(alias = "not_initiated")]
    Created,
    InProgress,
    OnHold,
    Success,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StatusEvent {
    pub status: PayoutStatus,
    pub timestamp: DateTime<Utc>,
}

/// A payout as the provider reports it (payout API or webhooks).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PayoutRecord {
    pub payout_id: String,
    pub currency: Currency,
    /// Amount sent, in `currency`'s smallest unit.
    pub amount: i64,
    #[serde(default)]
    pub fee: Option<i64>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub events: Vec<StatusEvent>,
    /// When the merchant saw the money in their bank. Never set by the provider.
    #[serde(default)]
    pub bank_confirmed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub remarks: Option<String>,
}

// ---------- report types ----------

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub tool: String,
    pub policy_version: &'static str,
    pub docs_as_of: Vec<(&'static str, &'static str)>,
    pub as_of: DateTime<Utc>,
    pub business_id: Option<String>,
    pub rounding: Rounding,
    pub eligibility: Eligibility,
    pub runs: Vec<Run>,
    pub payouts: Vec<PayoutView>,
    pub reconciliation: Vec<Reconciliation>,
    pub excluded: Vec<Excluded>,
    pub settled_conversions: Vec<SettledConversion>,
    pub findings: Vec<Finding>,
    pub discrepancies: Vec<Discrepancy>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// A payout was made and matched to this run.
    Paid,
    /// The rules say it should have paid; the provider's data was not given, so this is the rule's version.
    PaidSimulated,
    /// The rules say it should have paid, the data is complete, and no payout was found.
    Missing,
    BelowThreshold,
    NothingToPay,
    Blocked,
    /// A payout was made for this run, failed, and was credited back. The money carries forward.
    Reversed,
    /// A rate needed to decide is missing.
    Undecided,
    /// After `as_of`: what happens if nothing else changes.
    Projected,
}

#[derive(Clone, Debug, Serialize)]
pub struct Line {
    pub kind: EventType,
    pub label: &'static str,
    pub count: usize,
    pub amount: Money,
}

#[derive(Clone, Debug, Serialize)]
pub struct EntryRef {
    pub id: String,
    pub event_type: String,
    pub amount: Money,
    pub created_at: DateTime<Utc>,
    pub rule: &'static str,
}

/// One wallet's part of a run. `carried_in + sum(lines) == payable`, always.
#[derive(Clone, Debug, Serialize)]
pub struct WalletPart {
    pub currency: Currency,
    pub carried_in: Money,
    pub lines: Vec<Line>,
    pub entries: Vec<EntryRef>,
    pub payable: Money,
    /// What left the wallet for this run: the actual payout if one matched, otherwise the rule's figure.
    pub drained: Money,
    pub carried_out: Money,
}

#[derive(Clone, Debug, Serialize)]
pub struct ThresholdCheck {
    pub rule: &'static str,
    /// In USD minor units for the combined group, in the wallet's own units for a linked wallet.
    pub threshold: Money,
    pub total: Money,
    pub conversions: Vec<Conversion>,
    pub met: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Expected {
    /// Payable balance in the payout currency, before fees.
    pub gross: Money,
    pub conversions: Vec<Conversion>,
    /// `None`: a conversion fee applies but its rate isn't published or configured.
    pub conversion_fee: Option<Money>,
    pub payout_fee: Money,
    pub payout_fee_note: String,
    pub wire_fee: Money,
    /// `None` when the conversion fee is unknown.
    pub sent: Option<Money>,
    pub unknown: Vec<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Matched {
    pub payout_id: String,
    pub sent: Option<Money>,
    pub fee: Option<Money>,
    /// Actual drain minus expected drain, per wallet. Positive: the payout took
    /// more than this run's balance (an earlier leftover). Negative: some of
    /// this run's balance stays for a later payout. Same sign as the provider's breakup.
    pub unattributed: Vec<Money>,
    /// When the conversion fee isn't known up front: gross minus fees minus what was actually sent.
    pub implied_conversion_cost: Option<Money>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Run {
    pub group: String,
    pub currency: Currency,
    pub slot: Slot,
    pub created_at: DateTime<Utc>,
    pub cutoff: DateTime<Utc>,
    pub status: RunStatus,
    pub wallets: Vec<WalletPart>,
    pub threshold: Option<ThresholdCheck>,
    pub expected: Option<Expected>,
    pub matched: Option<Matched>,
    pub blockers: Vec<Blocker>,
    /// The first run date this money was due and not paid because of a blocker or a missing payout.
    pub due_since: Option<NaiveDate>,
    pub overdue_days: Option<i64>,
    pub rules: Vec<&'static str>,
    pub next_action: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct PayoutView {
    pub payout_id: String,
    pub currency: Currency,
    pub created_at: DateTime<Utc>,
    pub sent: Option<Money>,
    pub fee: Option<Money>,
    /// Latest provider status by event timestamp. `None` when only ledger rows exist.
    pub provider_status: Option<PayoutStatus>,
    pub status_at: Option<DateTime<Utc>>,
    pub reversed_in_ledger: bool,
    pub bank_confirmed_at: Option<DateTime<Utc>>,
    pub run: Option<String>,
    pub state: String,
    pub next_action: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Reconciliation {
    pub currency: Currency,
    pub opening: Money,
    pub movements: Vec<Line>,
    /// Sum of chain gaps: money the reported balances moved by with no entry in the data.
    pub unattributed_gaps: Money,
    pub closing: Money,
    /// The last after_balance in the data, if reported.
    pub reported_closing: Option<Money>,
    pub balanced: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Excluded {
    pub currency: Currency,
    pub entries: usize,
    pub balance: Money,
    pub rule: &'static str,
    pub why: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct SettledConversion {
    pub reference: String,
    pub from: Money,
    pub to: Money,
    /// to / from, from the ledger rows themselves.
    pub implied_rate: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "discrepancy", rename_all = "snake_case")]
pub enum Discrepancy {
    MissingPayout {
        run: String,
        expected: Option<Money>,
    },
    AmountDiffers {
        run: String,
        payout_id: String,
        currency: Currency,
        unattributed: i64,
    },
    SentDiffers {
        run: String,
        payout_id: String,
        expected: Money,
        reported: Money,
    },
    FeeDiffers {
        payout_id: String,
        expected: Money,
        reported: Money,
        rule: &'static str,
    },
    FeeNotCheckable {
        payout_id: String,
        why: String,
    },
    FailedWithoutReversal {
        payout_id: String,
    },
    TwoPayoutsOneRun {
        run: String,
        payout_ids: Vec<String>,
    },
    UnmatchedPayout {
        payout_id: String,
    },
    MissingRate {
        run: String,
        currency: Currency,
        date: NaiveDate,
    },
    PayoutWhileBlocked {
        run: String,
        payout_id: String,
    },
    RecordWithoutLedgerRows {
        payout_id: String,
    },
}

impl Discrepancy {
    /// Fee checks that can't be done aren't errors in the data.
    pub fn is_error(&self) -> bool {
        !matches!(self, Discrepancy::FeeNotCheckable { .. })
    }
}

// ---------- rates ----------

struct Rates(BTreeMap<Currency, Vec<(NaiveDate, Rate, String)>>);

impl Rates {
    fn new(q: &[RateQuote]) -> Result<Rates, String> {
        let mut m: BTreeMap<Currency, Vec<(NaiveDate, Rate, String)>> = BTreeMap::new();
        for r in q {
            let rate = Rate::parse(&r.usd)
                .map_err(|e| format!("rate for {} on {}: {e}", r.currency, r.date))?;
            m.entry(r.currency)
                .or_default()
                .push((r.date, rate, r.usd.clone()));
        }
        for v in m.values_mut() {
            v.sort_by_key(|x| x.0);
            for w in v.windows(2) {
                if w[0].0 == w[1].0 && w[0].1 != w[1].1 {
                    return Err(format!("two different rates on {}", w[0].0));
                }
            }
        }
        Ok(Rates(m))
    }

    /// USD per unit of `c`, latest on or before `d` (DC-RATES).
    fn usd(&self, c: Currency, d: NaiveDate) -> Option<(Rate, NaiveDate)> {
        if c == Currency::USD {
            return Some((Rate::ONE, NaiveDate::MIN));
        }
        self.0
            .get(&c)?
            .iter()
            .rev()
            .find(|x| x.0 <= d)
            .map(|x| (x.1, x.0))
    }

    fn convert(
        &self,
        m: Money,
        to: Currency,
        d: NaiveDate,
        mode: Rounding,
    ) -> Result<Option<Conversion>, String> {
        let (Some((a, da)), Some((b, db))) = (self.usd(m.currency, d), self.usd(to, d)) else {
            return Ok(None);
        };
        let dated = if da.max(db) == NaiveDate::MIN {
            d
        } else {
            da.max(db)
        };
        let mut cv = money::convert(m, to, Rate::cross(a, b), dated, mode)?;
        cv.rate = format!("{} {to} per {}", rate_text(Rate::cross(a, b)), m.currency);
        Ok(Some(cv))
    }
}

// ---------- engine ----------

fn midnight(d: NaiveDate) -> DateTime<Utc> {
    d.and_time(NaiveTime::MIN).and_utc()
}

/// Created at 00:00 UTC on the run date (PS-INITIATION).
fn run_time(s: &Slot) -> DateTime<Utc> {
    midnight(s.runs)
}

fn is_payout_family(e: &LedgerEntry) -> bool {
    matches!(
        e.kind(),
        EventType::Payout
            | EventType::PayoutFees
            | EventType::PayoutReversal
            | EventType::PayoutFeesReversal
    ) || (e.kind() == EventType::CurrencyConversion && e.payout_id.is_some())
}

struct Group {
    name: String,
    currency: Currency,
    wallets: Vec<Currency>,
    linked: bool,
}

fn groups(s: &Settings) -> Vec<Group> {
    let p = s.primary();
    let mut combined: Vec<Currency> = Currency::PAYOUT_WALLETS
        .iter()
        .copied()
        .filter(|c| !s.linked.contains(c))
        .collect();
    combined.sort_by_key(|c| (*c != p, *c));
    let mut out = vec![Group {
        name: "combined".into(),
        currency: p,
        wallets: combined,
        linked: false,
    }];
    let mut linked = s.linked.clone();
    linked.sort();
    for c in linked {
        out.push(Group {
            name: format!("linked {c}"),
            currency: c,
            wallets: vec![c],
            linked: true,
        });
    }
    out
}

/// A payout seen in the input, from its record, its ledger rows, or both.
struct Seen {
    id: String,
    currency: Currency,
    created_at: DateTime<Utc>,
    record: Option<PayoutRecord>,
    /// Net drain per wallet from the payout's ledger rows.
    drain: BTreeMap<Currency, i64>,
    has_rows: bool,
    reversed: bool,
    sent_rows: Option<i64>,
    fee_rows: Option<i64>,
}

fn collect_payouts(ledger: &Ledger, records: &[PayoutRecord]) -> Vec<Seen> {
    let mut m: BTreeMap<String, Seen> = BTreeMap::new();
    for r in records {
        m.insert(
            r.payout_id.clone(),
            Seen {
                id: r.payout_id.clone(),
                currency: r.currency,
                created_at: r.created_at,
                record: Some(r.clone()),
                drain: BTreeMap::new(),
                has_rows: false,
                reversed: false,
                sent_rows: None,
                fee_rows: None,
            },
        );
    }
    for e in ledger.entries.iter().filter(|e| is_payout_family(e)) {
        let Some(id) = e.payout_id.clone() else {
            continue;
        };
        let s = m.entry(id.clone()).or_insert_with(|| Seen {
            id,
            currency: e.currency,
            created_at: e.created_at,
            record: None,
            drain: BTreeMap::new(),
            has_rows: false,
            reversed: false,
            sent_rows: None,
            fee_rows: None,
        });
        s.has_rows = true;
        if s.record.is_none() {
            s.created_at = s.created_at.min(e.created_at);
            if e.kind() == EventType::Payout {
                s.currency = e.currency;
            }
        }
        *s.drain.entry(e.currency).or_insert(0) -= e.signed().minor;
        match e.kind() {
            EventType::PayoutReversal => s.reversed = true,
            EventType::Payout => *s.sent_rows.get_or_insert(0) += e.amount,
            EventType::PayoutFees => *s.fee_rows.get_or_insert(0) += e.amount,
            _ => {}
        }
    }
    let mut v: Vec<Seen> = m.into_values().collect();
    v.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
    v
}

fn latest_status(r: &PayoutRecord) -> Option<&StatusEvent> {
    // WH-PAYOUT-ORDER: the event with the latest timestamp wins, whatever order it arrived in.
    r.events
        .iter()
        .max_by_key(|e| (e.timestamp, e.status as u8))
}

pub fn run(input: &Input) -> Result<Report, String> {
    let params = &input.params;
    let s = &input.settings;
    s.validate(params.min_threshold)?;
    let mode = params.rounding;
    let ledger = Ledger::build(&input.ledger)?;
    let rates = Rates::new(&input.rates)?;
    let as_of = input.as_of;
    let mut discrepancies = Vec::new();

    let seen = collect_payouts(&ledger, &input.payouts);
    let payout_data = match s.payout_data {
        PayoutData::Auto if seen.is_empty() => PayoutData::None,
        PayoutData::Auto => PayoutData::Complete,
        x => x,
    };

    // Schedule from the first entry's date to the first run after as_of.
    let first = ledger
        .entries
        .first()
        .map_or(as_of.date_naive(), |e| e.created_at.date_naive())
        .min(as_of.date_naive());
    let mut changes: Vec<(NaiveDate, Cycle)> = s.cycles.iter().map(|c| (c.from, c.cycle)).collect();
    // No history means the default cycle throughout. History that starts after the data
    // does is taken to have applied from the start (documented in ASSUMPTIONS.md).
    match changes.first_mut() {
        None => changes.push((first, Cycle::Bimonthly)),
        Some(c) => c.0 = c.0.min(first),
    }
    let slots = calendar::schedule(&changes, first, as_of.date_naive());

    // Entries that count toward payouts, by wallet, in book order. Payout rows are handled per payout.
    let mut earning: BTreeMap<Currency, Vec<&LedgerEntry>> = BTreeMap::new();
    for e in ledger.entries.iter().filter(|e| !is_payout_family(e)) {
        earning.entry(e.currency).or_default().push(e);
    }

    let mut runs = Vec::new();
    let mut matched_ids: BTreeSet<String> = BTreeSet::new();
    for g in groups(s) {
        // How far into each wallet's entries we've consumed, and the carried balance.
        let mut pos: HashMap<Currency, usize> = HashMap::new();
        let mut carry: HashMap<Currency, i64> = g
            .wallets
            .iter()
            .map(|c| (*c, *ledger.opening.get(c).unwrap_or(&0)))
            .collect();
        // Payouts already counted against this group's balance.
        let mut counted: BTreeSet<String> = BTreeSet::new();
        let mut due_since: Option<NaiveDate> = None;

        for (i, slot) in slots.iter().enumerate() {
            let created = run_time(slot);
            let cutoff = midnight(slot.period_end + Duration::days(1));
            let projected = created > as_of;
            let window_end = slots.get(i + 1).map(|n| midnight(n.scheduled));
            let name = format!("{} run {}", g.name, slot.runs);

            // Unmatched payouts created before this cutoff still took money out.
            let mut pre_drain: HashMap<Currency, i64> = HashMap::new();
            let earlier: Vec<&Seen> = seen
                .iter()
                .filter(|p| {
                    p.created_at < cutoff
                        && !counted.contains(&p.id)
                        && !matched_ids.contains(&p.id)
                        && run_owner(&g, p)
                })
                .collect();
            for p in earlier {
                for (c, d) in &p.drain {
                    *pre_drain.entry(*c).or_insert(0) += d;
                }
                if p.drain.is_empty()
                    && let Some(r) = &p.record
                    && !p.has_rows
                {
                    *pre_drain.entry(r.currency).or_insert(0) += r.amount + r.fee.unwrap_or(0);
                }
                counted.insert(p.id.clone());
            }

            let limit = if projected { cutoff.min(as_of) } else { cutoff };
            let mut wallets = Vec::new();
            for &c in &g.wallets {
                let list = earning.get(&c).map(Vec::as_slice).unwrap_or(&[]);
                let start = *pos.get(&c).unwrap_or(&0);
                let mut end = start;
                while end < list.len() && list[end].created_at < limit {
                    end += 1;
                }
                if !projected {
                    pos.insert(c, end);
                }
                let carried_in = carry[&c] - pre_drain.get(&c).copied().unwrap_or(0);
                let mut by_kind: BTreeMap<EventType, (usize, i64)> = BTreeMap::new();
                let mut entries = Vec::new();
                for e in &list[start..end] {
                    let k = by_kind.entry(e.kind()).or_default();
                    k.0 += 1;
                    k.1 += e.signed().minor;
                    entries.push(EntryRef {
                        id: e.id.clone(),
                        event_type: e.event_type.clone(),
                        amount: e.signed(),
                        created_at: e.created_at,
                        rule: rule_for(e.kind()),
                    });
                }
                let lines: Vec<Line> = by_kind
                    .into_iter()
                    .map(|(k, (n, a))| Line {
                        kind: k,
                        label: k.label(),
                        count: n,
                        amount: Money::new(a, c),
                    })
                    .collect();
                let payable = lines.iter().try_fold(carried_in, |a, l| {
                    a.checked_add(l.amount.minor).ok_or("balance overflowed")
                })?;
                wallets.push(WalletPart {
                    currency: c,
                    carried_in: Money::new(carried_in, c),
                    lines,
                    entries,
                    payable: Money::new(payable, c),
                    drained: Money::zero(c),
                    carried_out: Money::new(payable, c),
                });
            }

            let mut rules = vec!["PS-CYCLE", "PS-INITIATION", "BAL-TYPES"];
            if slot.runs != slot.scheduled {
                rules.push("PS-HOLIDAY");
            }
            let elig =
                account::eligibility(s.account_type, &input.forms, &input.restrictions, created);
            let blocked = elig.payouts_enabled == Some(false) || !elig.blockers.is_empty();

            // Threshold (PS-THRESHOLD / PS-THRESHOLD-LINKED / CF-THRESHOLD-EQUAL).
            let threshold_usd = s.threshold_at(created, params.default_threshold);
            let mut missing_rate = None;
            let threshold = if g.linked {
                let w = &wallets[0];
                rules.push("PS-THRESHOLD-LINKED");
                Some(ThresholdCheck {
                    rule: "PS-THRESHOLD-LINKED",
                    threshold: Money::new(threshold_usd, g.currency),
                    total: w.payable,
                    conversions: vec![],
                    met: w.payable.minor >= threshold_usd,
                })
            } else {
                rules.push("PS-THRESHOLD");
                let mut total = 0i64;
                let mut convs = Vec::new();
                for w in &wallets {
                    match rates.convert(w.payable, Currency::USD, slot.runs, mode)? {
                        Some(cv) => {
                            total += cv.to.minor;
                            if w.currency != Currency::USD {
                                convs.push(cv);
                            }
                        }
                        None => missing_rate = Some(w.currency),
                    }
                }
                Some(ThresholdCheck {
                    rule: "PS-THRESHOLD",
                    threshold: Money::new(threshold_usd, Currency::USD),
                    total: Money::new(total, Currency::USD),
                    conversions: convs,
                    met: total >= threshold_usd,
                })
            };
            let met = threshold.as_ref().is_some_and(|t| t.met);
            let positive = wallets.iter().any(|w| w.payable.minor > 0);

            // Actual payout(s) for this run (DC-MATCH).
            let candidates: Vec<&Seen> = if projected {
                vec![]
            } else {
                seen.iter()
                    .filter(|p| {
                        p.currency == g.currency
                            && !matched_ids.contains(&p.id)
                            && !counted.contains(&p.id)
                            && p.created_at >= midnight(slot.scheduled)
                            && window_end.is_none_or(|w| p.created_at < w)
                    })
                    .filter(|p| run_owner(&g, p))
                    .collect()
            };
            let live: Vec<&&Seen> = candidates
                .iter()
                .filter(|p| !(p.reversed && p.drain.values().all(|d| *d == 0)))
                .collect();
            let chosen: Option<&Seen> = match live.len() {
                0 => None,
                1 => Some(live[0]),
                _ => {
                    discrepancies.push(Discrepancy::TwoPayoutsOneRun {
                        run: name.clone(),
                        payout_ids: live.iter().map(|p| p.id.clone()).collect(),
                    });
                    rules.push("DC-MATCH");
                    None
                }
            };
            // Fully reversed attempts in the window belong to this run too; they drain nothing.
            let mut reversed_attempt = None;
            for p in &candidates {
                if chosen.is_none_or(|c| c.id != p.id) && live.iter().all(|l| l.id != p.id) {
                    matched_ids.insert(p.id.clone());
                    reversed_attempt.get_or_insert(*p);
                }
            }

            let (status, expected) = if let (Some(c), false) = (missing_rate, projected) {
                discrepancies.push(Discrepancy::MissingRate {
                    run: name.clone(),
                    currency: c,
                    date: slot.runs,
                });
                (RunStatus::Undecided, None)
            } else if !positive {
                (
                    if projected {
                        RunStatus::Projected
                    } else {
                        RunStatus::NothingToPay
                    },
                    None,
                )
            } else if blocked && chosen.is_none() {
                (
                    if projected {
                        RunStatus::Projected
                    } else {
                        RunStatus::Blocked
                    },
                    None,
                )
            } else if !met && chosen.is_none() {
                rules.push("PS-CARRY");
                (
                    if projected {
                        RunStatus::Projected
                    } else {
                        RunStatus::BelowThreshold
                    },
                    None,
                )
            } else {
                let exp = expected_payout(
                    &g, &wallets, &rates, slot.runs, params, s.transfer, mode, &mut rules,
                )?;
                let st = if projected {
                    RunStatus::Projected
                } else if chosen.is_some() {
                    RunStatus::Paid
                } else if reversed_attempt.is_some() {
                    rules.push("PS-FAILED");
                    RunStatus::Reversed
                } else if payout_data == PayoutData::None {
                    RunStatus::PaidSimulated
                } else {
                    RunStatus::Missing
                };
                (st, exp)
            };

            // Drain the wallets.
            let mut matched = None;
            if let Some(p) = chosen {
                matched_ids.insert(p.id.clone());
                counted.insert(p.id.clone());
                if blocked {
                    discrepancies.push(Discrepancy::PayoutWhileBlocked {
                        run: name.clone(),
                        payout_id: p.id.clone(),
                    });
                }
                rules.push("PB-UNATTRIBUTED");
                let mut unattributed = Vec::new();
                for w in &mut wallets {
                    let actual = match p.drain.get(&w.currency) {
                        Some(d) => *d,
                        None if !p.has_rows && w.currency == p.currency => p
                            .record
                            .as_ref()
                            .map_or(0, |r| r.amount + r.fee.unwrap_or(0)),
                        None => 0,
                    };
                    w.drained = Money::new(actual, w.currency);
                    w.carried_out = Money::new(w.payable.minor - actual, w.currency);
                    let diff = actual - w.payable.minor;
                    unattributed.push(Money::new(diff, w.currency));
                    if diff != 0 {
                        discrepancies.push(Discrepancy::AmountDiffers {
                            run: name.clone(),
                            payout_id: p.id.clone(),
                            currency: w.currency,
                            unattributed: diff,
                        });
                    }
                }
                let sent = p
                    .record
                    .as_ref()
                    .map(|r| Money::new(r.amount, r.currency))
                    .or(p.sent_rows.map(|x| Money::new(x, p.currency)));
                let fee = p
                    .record
                    .as_ref()
                    .and_then(|r| r.fee)
                    .or(p.fee_rows)
                    .map(|x| Money::new(x, p.currency));
                if let (Some(e), Some(sent)) = (&expected, sent)
                    && let Some(es) = e.sent
                    && es != sent
                    && unattributed.iter().all(|u| u.minor == 0)
                {
                    discrepancies.push(Discrepancy::SentDiffers {
                        run: name.clone(),
                        payout_id: p.id.clone(),
                        expected: es,
                        reported: sent,
                    });
                }
                if let (Some(e), Some(fee)) = (&expected, fee) {
                    audit_fee(p, e, fee, params, &mut discrepancies);
                }
                let implied_conversion_cost = match (&expected, sent) {
                    (Some(e), Some(sent))
                        if e.conversion_fee.is_none()
                            && unattributed.iter().all(|u| u.minor == 0) =>
                    {
                        Some(Money::new(
                            e.gross.minor - e.payout_fee.minor - e.wire_fee.minor - sent.minor,
                            sent.currency,
                        ))
                    }
                    _ => None,
                };
                matched = Some(Matched {
                    payout_id: p.id.clone(),
                    sent,
                    fee,
                    unattributed,
                    implied_conversion_cost,
                });
            } else if status == RunStatus::PaidSimulated {
                for w in &mut wallets {
                    w.drained = w.payable;
                    w.carried_out = Money::zero(w.currency);
                }
                rules.push("DC-PAYOUT-DRAIN");
            } else if let (RunStatus::Reversed, Some(p)) = (status, reversed_attempt) {
                let sent = p
                    .record
                    .as_ref()
                    .map(|r| Money::new(r.amount, r.currency))
                    .or(p.sent_rows.map(|x| Money::new(x, p.currency)));
                let fee = p
                    .record
                    .as_ref()
                    .and_then(|r| r.fee)
                    .or(p.fee_rows)
                    .map(|x| Money::new(x, p.currency));
                matched = Some(Matched {
                    payout_id: p.id.clone(),
                    sent,
                    fee,
                    unattributed: vec![],
                    implied_conversion_cost: None,
                });
            } else if status == RunStatus::Missing {
                discrepancies.push(Discrepancy::MissingPayout {
                    run: name.clone(),
                    expected: expected.as_ref().and_then(|e| e.sent),
                });
            }
            if !projected {
                for w in &wallets {
                    carry.insert(w.currency, w.carried_out.minor);
                }
            }

            // Overdue tracking (PS-MISSED-DATE): money due and not paid keeps its first due date.
            let unpaid_due = matches!(
                status,
                RunStatus::Blocked | RunStatus::Missing | RunStatus::Reversed
            ) || (projected && blocked && positive && met);
            if unpaid_due {
                due_since.get_or_insert(slot.runs);
                rules.push("PS-MISSED-DATE");
            } else if matches!(status, RunStatus::Paid | RunStatus::PaidSimulated) {
                due_since = None;
            }
            let this_due = if unpaid_due { due_since } else { None };
            let overdue_days = this_due.map(|d| (as_of.date_naive() - d).num_days().max(0));
            if blocked {
                rules.push("VP-GATES");
            }
            if wallets.iter().any(|w| {
                w.lines
                    .iter()
                    .any(|l| matches!(l.kind, EventType::ReserveHold | EventType::ReserveRelease))
            }) {
                rules.push("PS-RESERVE");
            }
            let next_action =
                next_action(status, &elig.blockers, threshold.as_ref(), slot, &expected);
            rules.sort();
            rules.dedup();
            runs.push(Run {
                group: g.name.clone(),
                currency: g.currency,
                slot: *slot,
                created_at: created,
                cutoff,
                status,
                wallets,
                threshold,
                expected,
                matched,
                blockers: if blocked {
                    elig.blockers.clone()
                } else {
                    vec![]
                },
                due_since: this_due,
                overdue_days,
                rules,
                next_action,
            });
        }
    }

    // Payouts nobody could place.
    for p in &seen {
        if !matched_ids.contains(&p.id)
            && runs
                .iter()
                .all(|r| r.matched.as_ref().is_none_or(|m| m.payout_id != p.id))
            && !p.reversed
        {
            discrepancies.push(Discrepancy::UnmatchedPayout {
                payout_id: p.id.clone(),
            });
        }
    }
    let payouts = payout_views(&seen, &runs, as_of, params, payout_data, &mut discrepancies);

    let reconciliation = reconcile(&ledger);
    let excluded = excluded(&ledger);
    let settled_conversions = settled(&ledger);
    runs.sort_by(|a, b| (a.slot.runs, &a.group).cmp(&(b.slot.runs, &b.group)));
    discrepancies.sort_by_key(|d| format!("{d:?}"));
    discrepancies.dedup();

    Ok(Report {
        tool: format!("whenpaid {}", env!("CARGO_PKG_VERSION")),
        policy_version: policy::POLICY_VERSION,
        docs_as_of: policy::docs_as_of(),
        as_of,
        business_id: input.business_id.clone(),
        rounding: mode,
        eligibility: account::eligibility(s.account_type, &input.forms, &input.restrictions, as_of),
        runs,
        payouts,
        reconciliation,
        excluded,
        settled_conversions,
        findings: ledger.findings.clone(),
        discrepancies,
    })
}

/// Can this payout belong to this group? Its currency must be the group's payout currency.
fn run_owner(g: &Group, p: &Seen) -> bool {
    p.currency == g.currency || p.drain.keys().any(|c| g.wallets.contains(c))
}

fn rule_for(k: EventType) -> &'static str {
    match k {
        EventType::DisputeFees => "BAL-DISPUTE-FEE",
        EventType::ReserveHold | EventType::ReserveRelease => "PS-RESERVE",
        EventType::CurrencyConversion => "PS-FX-INDICATIVE",
        _ => "BAL-TYPES",
    }
}

#[allow(clippy::too_many_arguments)]
fn expected_payout(
    g: &Group,
    wallets: &[WalletPart],
    rates: &Rates,
    day: NaiveDate,
    p: &Params,
    transfer: Transfer,
    mode: Rounding,
    rules: &mut Vec<&'static str>,
) -> Result<Option<Expected>, String> {
    let cur = g.currency;
    let mut gross = 0i64;
    let mut converted_gross = 0i64;
    let mut conversions = Vec::new();
    for w in wallets {
        if w.currency == cur {
            gross += w.payable.minor;
        } else {
            let Some(cv) = rates.convert(w.payable, cur, day, mode)? else {
                return Ok(None);
            };
            gross += cv.to.minor;
            converted_gross += cv.to.minor;
            conversions.push(cv);
        }
    }
    let mut unknown = vec!["charges deducted by your bank or intermediary banks (PS-BANK-CHARGES)"];
    let conversion_fee = if converted_gross == 0 {
        Some(Money::zero(cur))
    } else {
        rules.push("PS-CONVERSION");
        rules.push("PS-FX-INDICATIVE");
        match p.conversion_fee_bps {
            Some(bps) => Some(money::apply_bps(Money::new(converted_gross, cur), bps, mode)?.0),
            None => {
                unknown.push(
                    "the conversion fee on non-primary wallets (rate not published, PS-CONVERSION)",
                );
                None
            }
        }
    };
    rules.push("PS-PAYOUT-FEE");
    rules.push("DC-FEE-COMPARE");
    let Some((fee, note)) = payout_fee(gross, cur, rates, day, p, mode)? else {
        return Ok(None);
    };
    if cur != Currency::USD {
        rules.push("CF-PAYOUT-FEE-CURRENCY");
    }
    let wire = if transfer == Transfer::Swift && cur == Currency::USD {
        rules.push("PR-SWIFT");
        rules.push("DC-SWIFT-STACK");
        p.swift_fee
    } else {
        0
    };
    let sent = conversion_fee.map(|cf| Money::new(gross - cf.minor - fee - wire, cur));
    Ok(Some(Expected {
        gross: Money::new(gross, cur),
        conversions,
        conversion_fee,
        payout_fee: Money::new(fee, cur),
        payout_fee_note: note,
        wire_fee: Money::new(wire, cur),
        sent,
        unknown,
    }))
}

/// 5 USD below 1,000 USD, read per `fee_basis` for GBP and EUR (PS-PAYOUT-FEE, CF-PAYOUT-FEE-CURRENCY).
fn payout_fee(
    gross: i64,
    cur: Currency,
    rates: &Rates,
    day: NaiveDate,
    p: &Params,
    mode: Rounding,
) -> Result<Option<(i64, String)>, String> {
    let (fee, below) = if cur == Currency::USD || p.fee_basis == FeeBasis::Nominal {
        (p.payout_fee, p.payout_fee_below)
    } else {
        let f = rates.convert(Money::new(p.payout_fee, Currency::USD), cur, day, mode)?;
        let b = rates.convert(
            Money::new(p.payout_fee_below, Currency::USD),
            cur,
            day,
            mode,
        )?;
        match (f, b) {
            (Some(f), Some(b)) => (f.to.minor, b.to.minor),
            _ => return Ok(None),
        }
    };
    if gross < below {
        Ok(Some((
            fee,
            format!(
                "{} because the payout before fees is below {}",
                Money::new(fee, cur),
                Money::new(below, cur)
            ),
        )))
    } else {
        Ok(Some((
            0,
            format!(
                "none because the payout before fees is {} or more",
                Money::new(below, cur)
            ),
        )))
    }
}

fn audit_fee(p: &Seen, e: &Expected, fee: Money, params: &Params, out: &mut Vec<Discrepancy>) {
    if fee.currency != Currency::USD && params.fee_basis == FeeBasis::Converted {
        if fee.minor != e.payout_fee.minor + e.wire_fee.minor {
            out.push(Discrepancy::FeeNotCheckable { payout_id: p.id.clone(), why: format!("{} payout fee depends on how 'GBP or EUR equivalent' is read (CF-PAYOUT-FEE-CURRENCY)", fee.currency) });
        }
        return;
    }
    let want = Money::new(e.payout_fee.minor + e.wire_fee.minor, e.payout_fee.currency);
    if fee != want {
        out.push(Discrepancy::FeeDiffers {
            payout_id: p.id.clone(),
            expected: want,
            reported: fee,
            rule: "PS-PAYOUT-FEE",
        });
    }
}

fn next_action(
    status: RunStatus,
    blockers: &[Blocker],
    t: Option<&ThresholdCheck>,
    slot: &Slot,
    e: &Option<Expected>,
) -> String {
    match status {
        RunStatus::Paid | RunStatus::PaidSimulated => "None for this run.".into(),
        RunStatus::Missing => "The rules say this run should have paid and no payout is in the data. Check the Payouts page for this date; if nothing is there, contact support with this run's date and amount.".into(),
        RunStatus::Reversed => "The payout failed and the amount and fee were credited back to the wallet. Check the bank details on file; the money goes out with the next eligible run.".into(),
        RunStatus::Blocked => blockers.first().map_or("Unknown.".into(), |b| b.action.clone()),
        RunStatus::BelowThreshold => {
            let t = t.expect("threshold checked");
            format!("Nothing to do. The balance carries forward until it reaches {} (it is {}).", t.threshold, t.total)
        }
        RunStatus::NothingToPay => "Nothing to do.".into(),
        RunStatus::Undecided => "Add a rate for the missing currency on or before the run date and run again.".into(),
        RunStatus::Projected => {
            if !blockers.is_empty() {
                blockers[0].action.clone()
            } else if let Some(t) = t
                && !t.met
            {
                format!("Runs {} if the balance reaches {}. It is {} so far.", slot.runs, t.threshold, t.total)
            } else if let Some(e) = e {
                match e.sent {
                    Some(sent) => format!("Runs {} for {} to {}. About {sent} so far if nothing else changes.", slot.runs, slot.period_start, slot.period_end),
                    None => format!("Runs {} for {} to {}. {} before fees so far; the exact amount depends on the unpublished conversion fee.", slot.runs, slot.period_start, slot.period_end, e.gross),
                }
            } else {
                format!("Runs {}.", slot.runs)
            }
        }
    }
}

fn payout_views(
    seen: &[Seen],
    runs: &[Run],
    as_of: DateTime<Utc>,
    p: &Params,
    data: PayoutData,
    out: &mut Vec<Discrepancy>,
) -> Vec<PayoutView> {
    seen.iter()
        .map(|s| {
            let last = s.record.as_ref().and_then(latest_status);
            let status = last.map(|e| e.status);
            let run = runs.iter().find(|r| r.matched.as_ref().is_some_and(|m| m.payout_id == s.id)).map(|r| format!("{} run {}", r.group, r.slot.runs));
            let bank = s.record.as_ref().and_then(|r| r.bank_confirmed_at);
            if status == Some(PayoutStatus::Failed) && !s.reversed && data == PayoutData::Complete && s.has_rows {
                out.push(Discrepancy::FailedWithoutReversal { payout_id: s.id.clone() });
            }
            if s.record.is_some() && !s.has_rows && data == PayoutData::Complete && !s.reversed && status != Some(PayoutStatus::Failed) {
                out.push(Discrepancy::RecordWithoutLedgerRows { payout_id: s.id.clone() });
            }
            let (state, action) = match (status, s.reversed, bank) {
                (_, _, Some(b)) => (format!("received by your bank on {} (confirmed by you)", b.date_naive()), "None.".to_string()),
                (Some(PayoutStatus::Failed), true, _) => ("failed; amount and fee credited back to the wallet (PS-FAILED)".into(), "Fix the bank details if the remarks point to them. The money is back in the wallet and goes out with the next eligible run.".into()),
                (Some(PayoutStatus::Failed), false, _) => ("failed; no credit-back rows in the ledger yet (PS-FAILED)".into(), "Expect a payout reversal and a payout fees reversal in the ledger. If they don't appear, contact support with the payout id.".into()),
                (_, true, _) => ("reversed in the ledger (PS-FAILED)".into(), "The money is back in the wallet.".into()),
                (Some(PayoutStatus::Success), false, None) => {
                    let at = last.map(|e| e.timestamp).unwrap();
                    let days = calendar::business_days_between(at.date_naive(), as_of.date_naive());
                    let state = format!("sent to your bank {} business days ago; not confirmed received (PS-SUCCESS-IS-SENT)", days);
                    let action = if days > p.bank_days {
                        format!("It has been more than {} business days (CF-BANK-DAYS). Ask your bank to trace the incoming transfer, then contact support with {} if it can't be found.", p.bank_days, s.id)
                    } else {
                        format!("Wait up to {} business days for it to arrive.", p.bank_days)
                    };
                    (state, action)
                }
                (Some(PayoutStatus::OnHold), _, _) => ("on hold; reason: ".to_string() + s.record.as_ref().and_then(|r| r.remarks.as_deref()).unwrap_or("not disclosed"), "Check the Verification section for an open request; if none, contact support with the payout id.".into()),
                (Some(PayoutStatus::InProgress), _, _) => ("in progress".into(), "Wait for the next status update.".into()),
                (Some(PayoutStatus::Created), _, _) => ("created".into(), "Wait for processing to start.".into()),
                (None, false, None) => ("in the ledger; no status reported".into(), "Add the payout's status from the payouts API or webhooks for a fuller answer.".into()),
            };
            PayoutView {
                payout_id: s.id.clone(),
                currency: s.currency,
                created_at: s.created_at,
                sent: s.record.as_ref().map(|r| Money::new(r.amount, r.currency)).or(s.sent_rows.map(|x| Money::new(x, s.currency))),
                fee: s.record.as_ref().and_then(|r| r.fee).or(s.fee_rows).map(|x| Money::new(x, s.currency)),
                provider_status: status,
                status_at: last.map(|e| e.timestamp),
                reversed_in_ledger: s.reversed,
                bank_confirmed_at: bank,
                run,
                state,
                next_action: action,
            }
        })
        .collect()
}

fn reconcile(l: &Ledger) -> Vec<Reconciliation> {
    let mut out = Vec::new();
    let currencies: BTreeSet<Currency> = l.entries.iter().map(|e| e.currency).collect();
    for c in currencies {
        let open = *l.opening.get(&c).unwrap_or(&0);
        let mut by: BTreeMap<EventType, (usize, i64)> = BTreeMap::new();
        for e in l.entries.iter().filter(|e| e.currency == c) {
            let x = by.entry(e.kind()).or_default();
            x.0 += 1;
            x.1 += e.signed().minor;
        }
        let gaps: i64 = l
            .findings
            .iter()
            .filter_map(|f| match f {
                Finding::Gap {
                    currency,
                    difference,
                    ..
                } if *currency == c => Some(*difference),
                _ => None,
            })
            .sum();
        let moved: i64 = by.values().map(|x| x.1).sum();
        let closing = open + moved + gaps;
        let reported = l
            .entries
            .iter()
            .rev()
            .find(|e| e.currency == c)
            .and_then(|e| e.after_balance)
            .map(|a| Money::new(a, c));
        out.push(Reconciliation {
            currency: c,
            opening: Money::new(open, c),
            movements: by
                .into_iter()
                .map(|(k, (n, a))| Line {
                    kind: k,
                    label: k.label(),
                    count: n,
                    amount: Money::new(a, c),
                })
                .collect(),
            unattributed_gaps: Money::new(gaps, c),
            closing: Money::new(closing, c),
            reported_closing: reported,
            balanced: reported.is_none_or(|r| r.minor == closing),
        });
    }
    out
}

fn excluded(l: &Ledger) -> Vec<Excluded> {
    let mut m: BTreeMap<Currency, (usize, i64)> = BTreeMap::new();
    for e in l.entries.iter().filter(|e| !e.currency.is_payout_wallet()) {
        let x = m.entry(e.currency).or_default();
        x.0 += 1;
        x.1 += e.signed().minor;
    }
    m.into_iter()
        .map(|(c, (n, b))| Excluded {
            currency: c,
            entries: n,
            balance: Money::new(b + l.opening.get(&c).copied().unwrap_or(0), c),
            rule: if c == Currency::INR {
                "PS-INR"
            } else {
                "BAL-WALLETS"
            },
            why: if c == Currency::INR {
                "the INR wallet is discontinued: not paid out and not counted toward the minimum"
            } else {
                "not one of the USD, GBP or EUR payout wallets"
            },
        })
        .collect()
}

fn settled(l: &Ledger) -> Vec<SettledConversion> {
    let mut by: BTreeMap<String, Vec<&LedgerEntry>> = BTreeMap::new();
    for e in l
        .entries
        .iter()
        .filter(|e| e.kind() == EventType::CurrencyConversion)
    {
        let key = e
            .reference_object_id
            .clone()
            .or(e.payout_id.clone())
            .unwrap_or_else(|| e.id.clone());
        by.entry(key).or_default().push(e);
    }
    by.into_iter()
        .filter_map(|(r, v)| {
            let out = v.iter().find(|e| !e.is_credit)?;
            let inn = v.iter().find(|e| e.is_credit)?;
            let (f, t) = (
                Money::new(out.amount, out.currency),
                Money::new(inn.amount, inn.currency),
            );
            let num = t.minor as i128 * 10i128.pow(f.currency.exponent());
            let den = f.minor as i128 * 10i128.pow(t.currency.exponent());
            let rate = if den == 0 {
                "n/a".into()
            } else {
                format!(
                    "{} (exact {})",
                    decimal(num, den, 6),
                    money::Fraction::new(num, den)
                )
            };
            Some(SettledConversion {
                reference: r,
                from: f,
                to: t,
                implied_rate: rate,
            })
        })
        .collect()
}

/// "1.0832", or as many places as it takes up to 10, with an exact fraction if it doesn't terminate.
fn rate_text(r: Rate) -> String {
    let d = decimal(r.num, r.den, 10);
    let t = d.trim_end_matches('0').trim_end_matches('.').to_string();
    if (r.num * 10i128.pow(10)) % r.den == 0 {
        t
    } else {
        format!("{t}... ({}/{})", r.num, r.den)
    }
}

fn decimal(num: i128, den: i128, places: u32) -> String {
    let scaled = money::div_round(num * 10i128.pow(places), den, Rounding::HalfEven);
    let s = 10i128.pow(places);
    format!(
        "{}.{:0w$}",
        scaled / s,
        (scaled % s).abs(),
        w = places as usize
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::Builder;

    fn t(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    pub(crate) fn input(b: &Builder, as_of: &str) -> Input {
        Input {
            business_id: None,
            as_of: t(as_of),
            settings: Settings::default(),
            params: Params::default(),
            ledger: b.entries.clone(),
            payouts: vec![],
            rates: vec![
                RateQuote {
                    date: "2026-01-01".parse().unwrap(),
                    currency: Currency::EUR,
                    usd: "1.08".into(),
                },
                RateQuote {
                    date: "2026-01-01".parse().unwrap(),
                    currency: Currency::GBP,
                    usd: "1.27".into(),
                },
            ],
            forms: vec![],
            restrictions: vec![],
        }
    }

    fn run_on<'a>(r: &'a Report, date: &str) -> &'a Run {
        r.runs
            .iter()
            .find(|x| x.slot.runs.to_string() == date && x.group == "combined")
            .unwrap_or_else(|| panic!("no run on {date}"))
    }

    #[test]
    fn threshold_is_inclusive() {
        let mut b = Builder::new();
        b.sale("pay_1", "2026-11-03T10:00:00Z", Currency::USD, 10_000, 0, 0);
        let r = run(&input(&b, "2026-11-30T00:00:00Z")).unwrap();
        let x = run_on(&r, "2026-11-18");
        assert_eq!(x.status, RunStatus::PaidSimulated);
        let mut b = Builder::new();
        b.sale("pay_1", "2026-11-03T10:00:00Z", Currency::USD, 9_999, 0, 0);
        let r = run(&input(&b, "2026-11-30T00:00:00Z")).unwrap();
        assert_eq!(run_on(&r, "2026-11-18").status, RunStatus::BelowThreshold);
    }

    #[test]
    fn single_wallet_over_threshold_does_not_trigger_alone() {
        // 60 USD + 40 EUR (43.20 USD) = 103.20 USD combined: pays even though neither wallet reaches 100 alone.
        let mut b = Builder::new();
        b.sale("pay_1", "2026-11-03T10:00:00Z", Currency::USD, 6_000, 0, 0);
        b.sale("pay_2", "2026-11-04T10:00:00Z", Currency::EUR, 4_000, 0, 0);
        let r = run(&input(&b, "2026-11-30T00:00:00Z")).unwrap();
        let x = run_on(&r, "2026-11-18");
        assert_eq!(x.threshold.as_ref().unwrap().total.minor, 10_320);
        assert_eq!(x.status, RunStatus::PaidSimulated);
    }

    #[test]
    fn linked_wallet_is_compared_unconverted() {
        let mut b = Builder::new();
        b.sale("pay_1", "2026-11-03T10:00:00Z", Currency::EUR, 9_500, 0, 0); // 95 EUR = 102.60 USD
        let mut i = input(&b, "2026-11-30T00:00:00Z");
        i.settings.linked = vec![Currency::EUR];
        let r = run(&i).unwrap();
        let x = r
            .runs
            .iter()
            .find(|x| x.group == "linked EUR" && x.slot.runs.to_string() == "2026-11-18")
            .unwrap();
        assert_eq!(x.status, RunStatus::BelowThreshold);
        assert_eq!(
            x.threshold.as_ref().unwrap().threshold,
            Money::new(10_000, Currency::EUR)
        );
    }

    #[test]
    fn payout_fee_boundary_at_1000() {
        for (bal, fee) in [(99_999, 500), (100_000, 0)] {
            let mut b = Builder::new();
            b.sale("pay_1", "2026-11-03T10:00:00Z", Currency::USD, bal, 0, 0);
            let r = run(&input(&b, "2026-11-30T00:00:00Z")).unwrap();
            let e = run_on(&r, "2026-11-18").expected.clone().unwrap();
            assert_eq!(e.payout_fee.minor, fee, "balance {bal}");
            assert_eq!(e.sent.unwrap().minor, bal - fee);
        }
    }

    #[test]
    fn entry_recorded_after_period_end_goes_to_next_run() {
        let mut b = Builder::new();
        b.sale("pay_1", "2026-11-15T23:59:59Z", Currency::USD, 20_000, 0, 0);
        b.sale("pay_2", "2026-11-16T00:00:00Z", Currency::USD, 30_000, 0, 0);
        let r = run(&input(&b, "2026-12-10T00:00:00Z")).unwrap();
        assert_eq!(run_on(&r, "2026-11-18").wallets[0].payable.minor, 20_000);
        assert_eq!(run_on(&r, "2026-12-04").wallets[0].payable.minor, 30_000);
    }

    #[test]
    fn blocked_payout_keeps_its_original_due_date() {
        let mut b = Builder::new();
        b.sale("pay_1", "2026-11-03T10:00:00Z", Currency::USD, 50_000, 0, 0);
        let mut i = input(&b, "2026-12-10T00:00:00Z");
        i.restrictions = vec![Restriction {
            id: "h1".into(),
            kind: account::RestrictionKind::ComplianceReview,
            from: t("2026-11-10T00:00:00Z"),
            until: Some(t("2026-12-01T00:00:00Z")),
            reason: None,
            pauses_payouts: None,
        }];
        let r = run(&i).unwrap();
        let x = run_on(&r, "2026-11-18");
        assert_eq!(
            (x.status, x.due_since),
            (RunStatus::Blocked, Some("2026-11-18".parse().unwrap()))
        );
        assert_eq!(x.overdue_days, Some(22));
        assert_eq!(x.blockers[0].reason, "not disclosed");
        let y = run_on(&r, "2026-12-04");
        assert_eq!(
            (y.status, y.wallets[0].payable.minor),
            (RunStatus::PaidSimulated, 50_000)
        );
    }

    #[test]
    fn reserve_release_does_not_clear_a_restriction() {
        let mut b = Builder::new();
        b.sale("pay_1", "2026-10-03T10:00:00Z", Currency::USD, 50_000, 0, 0);
        b.reserve_hold("rsv_1", "2026-10-03T10:00:01Z", Currency::USD, 5_000);
        b.reserve_release("rsv_1:rel", "2026-11-05T00:00:00Z", Currency::USD, 5_000);
        let mut i = input(&b, "2026-11-30T00:00:00Z");
        i.restrictions = vec![Restriction {
            id: "h1".into(),
            kind: account::RestrictionKind::ComplianceReview,
            from: t("2026-10-10T00:00:00Z"),
            until: None,
            reason: None,
            pauses_payouts: None,
        }];
        let r = run(&i).unwrap();
        let x = run_on(&r, "2026-11-18");
        assert_eq!(x.status, RunStatus::Blocked);
        assert_eq!(x.wallets[0].payable.minor, 50_000);
        assert!(x.rules.contains(&"PS-RESERVE"));
    }

    #[test]
    fn inr_is_listed_as_excluded() {
        let mut b = Builder::new();
        b.sale("pay_1", "2026-11-03T10:00:00Z", Currency::INR, 99_900, 0, 0);
        let r = run(&input(&b, "2026-11-30T00:00:00Z")).unwrap();
        assert_eq!(r.excluded[0].rule, "PS-INR");
        assert_eq!(r.excluded[0].balance.minor, 99_900);
        assert!(
            r.runs
                .iter()
                .all(|x| x.wallets.iter().all(|w| w.currency != Currency::INR))
        );
    }

    #[test]
    fn conversion_fee_unknown_is_shown_as_unknown() {
        let mut b = Builder::new();
        b.sale("pay_1", "2026-11-03T10:00:00Z", Currency::EUR, 50_000, 0, 0);
        let r = run(&input(&b, "2026-11-30T00:00:00Z")).unwrap();
        let e = run_on(&r, "2026-11-18").expected.clone().unwrap();
        assert_eq!((e.conversion_fee, e.sent), (None, None));
        assert_eq!(e.gross.minor, 54_000);
        assert!(e.unknown.iter().any(|u| u.contains("conversion fee")));
        let mut i = input(&b, "2026-11-30T00:00:00Z");
        i.params.conversion_fee_bps = Some(150);
        let e = run_on(&run(&i).unwrap(), "2026-11-18")
            .expected
            .clone()
            .unwrap();
        assert_eq!(e.conversion_fee.unwrap().minor, 810);
        assert_eq!(e.sent.unwrap().minor, 54_000 - 810 - 500);
    }

    #[test]
    fn swift_wire_fee_and_unknown_bank_charges() {
        let mut b = Builder::new();
        b.sale(
            "pay_1",
            "2026-11-03T10:00:00Z",
            Currency::USD,
            200_000,
            0,
            0,
        );
        let mut i = input(&b, "2026-11-30T00:00:00Z");
        i.settings.transfer = Transfer::Swift;
        let e = run_on(&run(&i).unwrap(), "2026-11-18")
            .expected
            .clone()
            .unwrap();
        assert_eq!(
            (e.payout_fee.minor, e.wire_fee.minor, e.sent.unwrap().minor),
            (0, 2_500, 197_500)
        );
        assert!(e.unknown[0].contains("bank"));
    }

    #[test]
    fn actual_payout_differs_by_one_cent() {
        let mut b = Builder::new();
        b.sale("pay_1", "2026-11-03T10:00:00Z", Currency::USD, 50_000, 0, 0);
        b.payout("pyt_1", "2026-11-18T00:00:05Z", Currency::USD, 49_499, 500);
        let r = run(&input(&b, "2026-11-30T00:00:00Z")).unwrap();
        let x = run_on(&r, "2026-11-18");
        assert_eq!(x.status, RunStatus::Paid);
        assert_eq!(x.matched.as_ref().unwrap().unattributed[0].minor, -1);
        assert_eq!(x.wallets[0].carried_out.minor, 1);
        assert!(r.discrepancies.iter().any(|d| matches!(
            d,
            Discrepancy::AmountDiffers {
                unattributed: -1,
                ..
            }
        )));
    }

    #[test]
    fn two_candidate_payouts_are_not_guessed() {
        let mut b = Builder::new();
        b.sale("pay_1", "2026-11-03T10:00:00Z", Currency::USD, 50_000, 0, 0);
        b.payout("pyt_1", "2026-11-18T00:00:05Z", Currency::USD, 20_000, 500);
        b.payout("pyt_2", "2026-11-19T00:00:05Z", Currency::USD, 29_000, 500);
        let r = run(&input(&b, "2026-11-30T00:00:00Z")).unwrap();
        assert!(r.discrepancies.iter().any(|d| matches!(d, Discrepancy::TwoPayoutsOneRun { payout_ids, .. } if payout_ids.len() == 2)));
        assert!(run_on(&r, "2026-11-18").matched.is_none());
    }

    #[test]
    fn failed_payout_without_reversal_is_flagged() {
        let mut b = Builder::new();
        b.sale("pay_1", "2026-11-03T10:00:00Z", Currency::USD, 50_000, 0, 0);
        b.payout("pyt_1", "2026-11-18T00:00:05Z", Currency::USD, 49_500, 500);
        let mut i = input(&b, "2026-11-30T00:00:00Z");
        i.payouts = vec![PayoutRecord {
            payout_id: "pyt_1".into(),
            currency: Currency::USD,
            amount: 49_500,
            fee: Some(500),
            created_at: t("2026-11-18T00:00:05Z"),
            events: vec![StatusEvent {
                status: PayoutStatus::Failed,
                timestamp: t("2026-11-19T10:00:00Z"),
            }],
            bank_confirmed_at: None,
            remarks: None,
        }];
        let r = run(&i).unwrap();
        assert!(
            r.discrepancies
                .contains(&Discrepancy::FailedWithoutReversal {
                    payout_id: "pyt_1".into()
                })
        );
        // Once the credit-back rows arrive, the money is back and the next run pays it again.
        b.payout_reversal("pyt_1", "2026-11-19T10:00:00Z", Currency::USD, 49_500, 500);
        i.ledger = b.entries.clone();
        i.as_of = t("2026-12-10T00:00:00Z");
        let r = run(&i).unwrap();
        assert!(
            !r.discrepancies
                .iter()
                .any(|d| matches!(d, Discrepancy::FailedWithoutReversal { .. }))
        );
        assert_eq!(run_on(&r, "2026-12-04").wallets[0].payable.minor, 50_000);
    }

    #[test]
    fn success_is_not_bank_receipt() {
        let mut b = Builder::new();
        b.sale("pay_1", "2026-11-03T10:00:00Z", Currency::USD, 50_000, 0, 0);
        b.payout("pyt_1", "2026-11-18T00:00:05Z", Currency::USD, 49_500, 500);
        let mut i = input(&b, "2026-11-30T00:00:00Z");
        i.payouts = vec![PayoutRecord {
            payout_id: "pyt_1".into(),
            currency: Currency::USD,
            amount: 49_500,
            fee: Some(500),
            created_at: t("2026-11-18T00:00:05Z"),
            events: vec![StatusEvent {
                status: PayoutStatus::Success,
                timestamp: t("2026-11-19T10:00:00Z"),
            }],
            bank_confirmed_at: None,
            remarks: None,
        }];
        let r = run(&i).unwrap();
        let p = &r.payouts[0];
        assert!(p.state.contains("not confirmed received"));
        assert!(p.next_action.contains("trace"), "{}", p.next_action);
    }

    #[test]
    fn reordered_payout_events_settle_by_timestamp() {
        let rec = |events: Vec<StatusEvent>| PayoutRecord {
            payout_id: "pyt_1".into(),
            currency: Currency::USD,
            amount: 1,
            fee: None,
            created_at: t("2026-11-18T00:00:00Z"),
            events,
            bank_confirmed_at: None,
            remarks: None,
        };
        let s = StatusEvent {
            status: PayoutStatus::Success,
            timestamp: t("2026-11-19T00:00:00Z"),
        };
        let f = StatusEvent {
            status: PayoutStatus::Failed,
            timestamp: t("2026-11-21T00:00:00Z"),
        };
        assert_eq!(
            latest_status(&rec(vec![f.clone(), s.clone()]))
                .unwrap()
                .status,
            PayoutStatus::Failed
        );
        assert_eq!(
            latest_status(&rec(vec![s, f])).unwrap().status,
            PayoutStatus::Failed
        );
    }
}
