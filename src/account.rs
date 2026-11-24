//! Account state that decides whether money can move, kept apart from the money itself.
//!
//! Four separate questions, answered separately:
//! - can the account take live payments (verification forms),
//! - are payouts enabled (forms plus the monitoring review),
//! - is anything pausing payouts right now (restrictions),
//! - what happened to one particular payout (its own status, in the engine).
//!
//! A reserve release adds money to the wallet. It never touches a restriction.

use crate::calendar::Cycle;
use crate::money::Currency;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountType {
    #[default]
    Individual,
    RegisteredEntity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Form {
    ProductInformation,
    Identity,
    Business,
    Bank,
    MonitoringReview,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FormStatus {
    Draft,
    UnderReview,
    OnHold,
    Approved,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FormChange {
    pub form: Form,
    pub status: FormStatus,
    pub at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestrictionKind {
    ComplianceReview,
    InformationRequest,
    Suspension,
    Other,
}

/// Something pausing the account, with whatever is actually known about it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Restriction {
    pub id: String,
    pub kind: RestrictionKind,
    pub from: DateTime<Utc>,
    /// `None` means nobody has said when it ends.
    #[serde(default)]
    pub until: Option<DateTime<Utc>>,
    /// `None` means no reason was given.
    #[serde(default)]
    pub reason: Option<String>,
    /// Defaults to true, except for information requests (VP-INFO-REQUEST).
    #[serde(default)]
    pub pauses_payouts: Option<bool>,
}

impl Restriction {
    pub fn pauses(&self) -> bool {
        self.pauses_payouts
            .unwrap_or(self.kind != RestrictionKind::InformationRequest)
    }
    pub fn active_at(&self, t: DateTime<Utc>) -> bool {
        self.from <= t && self.until.is_none_or(|u| t < u)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CycleChange {
    pub from: NaiveDate,
    pub cycle: Cycle,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ThresholdChange {
    pub from: DateTime<Utc>,
    /// USD minor units.
    pub amount: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Transfer {
    #[default]
    Local,
    Swift,
}

/// Whether the ledger export is expected to contain the payouts that were made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PayoutData {
    /// Complete if any payout row or payout record is present, otherwise none.
    #[default]
    Auto,
    /// Every payout made is in the input. A missing one is a discrepancy.
    Complete,
    /// No payouts in the input: the engine simulates past ones to keep balances right.
    None,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Settings {
    pub account_type: AccountType,
    /// Cycle history, oldest first. Empty means bi-monthly throughout.
    pub cycles: Vec<CycleChange>,
    /// Threshold history, oldest first. Empty means the default.
    pub thresholds: Vec<ThresholdChange>,
    /// Currency of the primary bank account. Defaults to USD.
    pub primary: Option<Currency>,
    /// Wallets linked to their own bank account (settlement currencies).
    pub linked: Vec<Currency>,
    pub transfer: Transfer,
    pub payout_data: PayoutData,
}

impl Settings {
    pub fn primary(&self) -> Currency {
        self.primary.unwrap_or(Currency::USD)
    }

    pub fn validate(&self, min_threshold: i64) -> Result<(), String> {
        let p = self.primary();
        if !p.is_payout_wallet() {
            return Err(format!(
                "{p} can't be the primary payout currency; payouts go from USD, GBP or EUR"
            ));
        }
        for &w in &self.linked {
            if !w.is_payout_wallet() {
                return Err(format!("{w} can't be a linked wallet"));
            }
            if w == p {
                return Err(format!(
                    "{w} is the primary currency; its wallet is already paid to the primary account"
                ));
            }
        }
        for t in &self.thresholds {
            if t.amount < min_threshold {
                return Err(format!(
                    "threshold {} is below the lowest allowed ({min_threshold} USD minor units)",
                    t.amount
                ));
            }
        }
        for w in self.cycles.windows(2) {
            if w[1].from < w[0].from {
                return Err("cycle changes must be in date order".into());
            }
            // PS-CYCLE-CHANGE
            if w[1].cycle <= w[0].cycle {
                return Err(format!(
                    "cycle change on {} goes from {:?} to {:?}; only moves to a less frequent cycle are allowed",
                    w[1].from, w[0].cycle, w[1].cycle
                ));
            }
        }
        Ok(())
    }

    /// The threshold for a run created at `t`. A change made before the run applies to it (PS-THRESHOLD-EFFECTIVE).
    pub fn threshold_at(&self, t: DateTime<Utc>, default: i64) -> i64 {
        self.thresholds
            .iter()
            .rev()
            .find(|c| c.from < t)
            .map_or(default, |c| c.amount)
    }
}

/// One reason a payout can't happen, with the next step if there is one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Blocker {
    pub what: String,
    pub reason: String,
    pub until: String,
    pub rule: &'static str,
    pub action: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Eligibility {
    /// `None` when no verification data was supplied.
    pub payments_live: Option<bool>,
    pub payouts_enabled: Option<bool>,
    pub blockers: Vec<Blocker>,
}

fn required(t: AccountType) -> Vec<Form> {
    let mut f = vec![Form::ProductInformation, Form::Identity];
    if t == AccountType::RegisteredEntity {
        f.push(Form::Business);
    }
    f.push(Form::Bank);
    f
}

fn status_at(changes: &[FormChange], form: Form, t: DateTime<Utc>) -> Option<FormStatus> {
    changes
        .iter()
        .filter(|c| c.form == form && c.at <= t)
        .max_by_key(|c| c.at)
        .map(|c| c.status)
}

fn name(f: Form) -> &'static str {
    match f {
        Form::ProductInformation => "Product Information",
        Form::Identity => "Identity Verification",
        Form::Business => "Business Verification",
        Form::Bank => "Bank Verification",
        Form::MonitoringReview => "Monitoring Review",
    }
}

/// Everything standing between the account and a payout at time `t` (VP-GATES, PS-INACTIVE-AGAIN).
pub fn eligibility(
    t: AccountType,
    forms: &[FormChange],
    restrictions: &[Restriction],
    at: DateTime<Utc>,
) -> Eligibility {
    let mut blockers = Vec::new();
    let (payments_live, payouts_enabled) = if forms.is_empty() {
        (None, None)
    } else {
        let mut live = true;
        for f in required(t) {
            let s = status_at(forms, f, at);
            if s != Some(FormStatus::Approved) {
                live = false;
                let state = s.map_or("not started".to_string(), |s| {
                    format!("{s:?}").to_lowercase()
                });
                blockers.push(Blocker {
                    what: format!("{} is {state}", name(f)),
                    reason: "required before live payments and payouts".into(),
                    until: "until it is approved".into(),
                    rule: "VP-GATES",
                    action: match s {
                        Some(FormStatus::OnHold) => format!(
                            "Complete the additional verification form raised for {}.",
                            name(f)
                        ),
                        Some(FormStatus::Failed) => format!(
                            "Fix the stated reason, then appeal {} once, in detail.",
                            name(f)
                        ),
                        Some(FormStatus::UnderReview) => format!(
                            "Wait for review of {} (usually 1-3 business days).",
                            name(f)
                        ),
                        _ => format!("Submit {}.", name(f)),
                    },
                });
            }
        }
        let monitoring = status_at(forms, Form::MonitoringReview, at) == Some(FormStatus::Approved);
        if live && !monitoring {
            blockers.push(Blocker {
                what: "Monitoring Review not complete".into(),
                reason: "payouts need it even when live payments are on".into(),
                until: "unknown".into(),
                rule: "VP-GATES",
                action: "Nothing to submit; if every form is approved and payouts stay inactive, email support from your registered address.".into(),
            });
        }
        (Some(live), Some(live && monitoring))
    };
    for r in restrictions
        .iter()
        .filter(|r| r.active_at(at) && r.pauses())
    {
        blockers.push(Blocker {
            what: format!("{} {} ({})", kind_name(r.kind), r.id, r.from.format("since %Y-%m-%d")),
            reason: r.reason.clone().unwrap_or_else(|| "not disclosed".into()),
            until: r.until.map_or("unknown".into(), |u| u.format("%Y-%m-%d").to_string()),
            rule: if r.reason.is_none() || r.until.is_none() { "DC-UNKNOWN" } else { "PS-INACTIVE-AGAIN" },
            action: match r.kind {
                RestrictionKind::InformationRequest => "Answer the information request in the dashboard.".into(),
                _ => "Check the Verification section for an open form; if none, email support from your registered address and quote this hold.".into(),
            },
        });
    }
    let payouts_enabled = payouts_enabled.map(|e| e && blockers.is_empty());
    Eligibility {
        payments_live,
        payouts_enabled,
        blockers,
    }
}

fn kind_name(k: RestrictionKind) -> &'static str {
    match k {
        RestrictionKind::ComplianceReview => "Compliance review",
        RestrictionKind::InformationRequest => "Information request",
        RestrictionKind::Suspension => "Suspension",
        RestrictionKind::Other => "Hold",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }
    fn fc(form: Form, status: FormStatus, at: &str) -> FormChange {
        FormChange {
            form,
            status,
            at: t(at),
        }
    }

    #[test]
    fn payments_live_but_payouts_inactive() {
        let forms = vec![
            fc(
                Form::ProductInformation,
                FormStatus::Approved,
                "2026-10-01T00:00:00Z",
            ),
            fc(Form::Identity, FormStatus::Approved, "2026-10-01T00:00:00Z"),
            fc(Form::Bank, FormStatus::Approved, "2026-10-02T00:00:00Z"),
        ];
        let e = eligibility(
            AccountType::Individual,
            &forms,
            &[],
            t("2026-11-01T00:00:00Z"),
        );
        assert_eq!(
            (e.payments_live, e.payouts_enabled),
            (Some(true), Some(false))
        );
        assert_eq!(e.blockers[0].what, "Monitoring Review not complete");
    }

    #[test]
    fn information_request_does_not_block_by_default() {
        let r = Restriction {
            id: "ir_1".into(),
            kind: RestrictionKind::InformationRequest,
            from: t("2026-11-01T00:00:00Z"),
            until: None,
            reason: None,
            pauses_payouts: None,
        };
        assert!(
            eligibility(
                AccountType::Individual,
                &[],
                std::slice::from_ref(&r),
                t("2026-11-10T00:00:00Z")
            )
            .blockers
            .is_empty()
        );
        let r = Restriction {
            pauses_payouts: Some(true),
            ..r
        };
        assert_eq!(
            eligibility(
                AccountType::Individual,
                &[],
                &[r],
                t("2026-11-10T00:00:00Z")
            )
            .blockers
            .len(),
            1
        );
    }

    #[test]
    fn unknown_hold_reason_and_date_stay_unknown() {
        let r = Restriction {
            id: "h1".into(),
            kind: RestrictionKind::ComplianceReview,
            from: t("2026-11-01T00:00:00Z"),
            until: None,
            reason: None,
            pauses_payouts: None,
        };
        let b = &eligibility(
            AccountType::Individual,
            &[],
            &[r],
            t("2026-11-10T00:00:00Z"),
        )
        .blockers[0];
        assert_eq!(
            (b.reason.as_str(), b.until.as_str(), b.rule),
            ("not disclosed", "unknown", "DC-UNKNOWN")
        );
    }

    #[test]
    fn moving_to_a_more_frequent_cycle_is_rejected() {
        let d = |s: &str| s.parse::<NaiveDate>().unwrap();
        let s = Settings {
            cycles: vec![
                CycleChange {
                    from: d("2026-01-01"),
                    cycle: Cycle::Monthly,
                },
                CycleChange {
                    from: d("2026-06-01"),
                    cycle: Cycle::Weekly,
                },
            ],
            ..Default::default()
        };
        assert!(s.validate(5000).unwrap_err().contains("less frequent"));
        let s = Settings {
            cycles: vec![
                CycleChange {
                    from: d("2026-01-01"),
                    cycle: Cycle::Weekly,
                },
                CycleChange {
                    from: d("2026-06-01"),
                    cycle: Cycle::Monthly,
                },
            ],
            ..Default::default()
        };
        assert!(s.validate(5000).is_ok());
    }

    #[test]
    fn threshold_change_applies_to_runs_created_after_it() {
        let s = Settings {
            thresholds: vec![ThresholdChange {
                from: t("2026-11-18T00:00:00Z"),
                amount: 25_000,
            }],
            ..Default::default()
        };
        // A change at exactly 00:00 on the run date is not before the run: the scheduled payout keeps the old one.
        assert_eq!(s.threshold_at(t("2026-11-18T00:00:00Z"), 10_000), 10_000);
        assert_eq!(s.threshold_at(t("2026-12-04T00:00:00Z"), 10_000), 25_000);
    }
}
