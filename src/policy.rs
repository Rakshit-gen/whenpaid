//! Every rule the engine applies, where it comes from, and how sure we are.
//!
//! Each run's trace lists the rule ids it used. `whenpaid rules` prints this
//! catalog as the source -> rule -> code -> test matrix, so the docs can't
//! drift from the code: the matrix is generated from here, and a test checks
//! that every test named below exists.

use serde::Serialize;

/// Bumped whenever a rule or a default below changes.
pub const POLICY_VERSION: &str = "public-docs/2026-11-24";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// Stated in the provider's public docs or pricing page.
    Documented,
    /// Two public sources disagree. The engine picks one and says which.
    Conflict,
    /// Not documented. The engine needs an answer, so this is ours.
    DesignChoice,
}

#[derive(Debug, Serialize)]
pub struct Source {
    pub key: &'static str,
    pub url: &'static str,
    /// The page's own "last modified" date.
    pub last_modified: &'static str,
    pub retrieved: &'static str,
}

#[derive(Debug, Serialize)]
pub struct Rule {
    pub id: &'static str,
    pub basis: Basis,
    pub source: &'static str,
    pub says: &'static str,
    /// Where the engine applies it.
    pub code: &'static str,
    /// Tests that fail if the rule is broken.
    pub tests: &'static [&'static str],
}

pub const SOURCES: &[Source] = &[
    Source {
        key: "payouts",
        url: "https://docs.dodopayments.com/features/payouts/payout-structure",
        last_modified: "2026-11-19",
        retrieved: "2026-11-24",
    },
    Source {
        key: "balances",
        url: "https://docs.dodopayments.com/features/account-summary-payout-wallet",
        last_modified: "2026-10-22",
        retrieved: "2026-11-24",
    },
    Source {
        key: "breakup",
        url: "https://docs.dodopayments.com/api-reference/payouts/retrieve-breakup",
        last_modified: "2026-04-01",
        retrieved: "2026-11-24",
    },
    Source {
        key: "breakup-details",
        url: "https://docs.dodopayments.com/api-reference/payouts/list-breakup-details",
        last_modified: "2026-10-16",
        retrieved: "2026-11-24",
    },
    Source {
        key: "ledger-api",
        url: "https://docs.dodopayments.com/api-reference/balance-ledger/list-ledger-entries",
        last_modified: "2026-03-25",
        retrieved: "2026-11-24",
    },
    Source {
        key: "verification",
        url: "https://docs.dodopayments.com/miscellaneous/verification-process",
        last_modified: "2026-11-02",
        retrieved: "2026-11-24",
    },
    Source {
        key: "webhooks",
        url: "https://docs.dodopayments.com/developer-resources/webhooks",
        last_modified: "2026-11-20",
        retrieved: "2026-11-24",
    },
    Source {
        key: "payout-webhooks",
        url: "https://docs.dodopayments.com/developer-resources/webhooks/intents/payout",
        last_modified: "2026-10-28",
        retrieved: "2026-11-24",
    },
    Source {
        key: "faq",
        url: "https://docs.dodopayments.com/miscellaneous/faq",
        last_modified: "not shown",
        retrieved: "2026-11-24",
    },
    Source {
        key: "pricing",
        url: "https://dodopayments.com/pricing",
        last_modified: "not shown",
        retrieved: "2026-11-24",
    },
    Source {
        key: "mor-intro",
        url: "https://docs.dodopayments.com/features/mor-introduction",
        last_modified: "2026-10-07",
        retrieved: "2026-11-24",
    },
    Source {
        key: "fed",
        url: "https://www.federalreserve.gov/aboutthefed/k8.htm",
        last_modified: "not shown",
        retrieved: "2026-11-24",
    },
    Source {
        key: "none",
        url: "",
        last_modified: "",
        retrieved: "",
    },
];

use Basis::*;

pub const RULES: &[Rule] = &[
    // Schedule
    Rule {
        id: "PS-CYCLE",
        basis: Documented,
        source: "payouts",
        says: "Bi-monthly (default): 1-15 paid on the 18th, 16-end paid on the 4th. Weekly: 1-7/8-14/15-21/22-end paid on the 11th/18th/25th/4th. Monthly: paid on the 11th of the next month.",
        code: "calendar::month_slots",
        tests: &["bimonthly_slots", "every_cycle_covers_every_day_once"],
    },
    Rule {
        id: "PS-CYCLE-CHANGE",
        basis: Documented,
        source: "payouts",
        says: "A business can only move to a less frequent cycle: weekly, then bi-monthly, then monthly.",
        code: "account::Settings::validate",
        tests: &["moving_to_a_more_frequent_cycle_is_rejected"],
    },
    Rule {
        id: "PS-INITIATION",
        basis: Documented,
        source: "payouts",
        says: "Payout dates are initiation dates. The payout is created at 00:00 UTC on that date.",
        code: "engine::run_time",
        tests: &["entry_recorded_after_period_end_goes_to_next_run"],
    },
    Rule {
        id: "PS-HOLIDAY",
        basis: Documented,
        source: "payouts",
        says: "A payout date on a weekend or US Federal Reserve holiday moves to the next business day. The billing period does not change.",
        code: "calendar::next_business_day",
        tests: &[
            "known_holidays",
            "payout_dates_move_past_weekends_and_holidays",
        ],
    },
    Rule {
        id: "FED-CALENDAR",
        basis: Documented,
        source: "fed",
        says: "Fed holidays: New Year, MLK, Washington's Birthday, Memorial, Juneteenth, Independence, Labor, Columbus, Veterans, Thanksgiving, Christmas. Sunday holidays are observed Monday; Saturday holidays are not moved.",
        code: "calendar::fed_holidays",
        tests: &["known_holidays"],
    },
    Rule {
        id: "PS-MISSED-DATE",
        basis: Documented,
        source: "payouts",
        says: "A payout that missed its date and is still due runs on the next payout run.",
        code: "engine (due_since)",
        tests: &["blocked_payout_keeps_its_original_due_date"],
    },
    // Threshold
    Rule {
        id: "PS-THRESHOLD",
        basis: Documented,
        source: "payouts",
        says: "Default minimum payout 100 USD, lowest 50 USD. Compared against the USD, GBP and EUR wallets combined and converted to USD; a payout starts when the total is equal to or greater than the minimum.",
        code: "engine::run (threshold check)",
        tests: &[
            "threshold_is_inclusive",
            "single_wallet_over_threshold_does_not_trigger_alone",
        ],
    },
    Rule {
        id: "PS-THRESHOLD-LINKED",
        basis: Documented,
        source: "payouts",
        says: "A wallet linked to its own bank account is not part of the combined balance and must reach the minimum on its own, unconverted: 300 means 300 EUR.",
        code: "engine::run (threshold check)",
        tests: &["linked_wallet_is_compared_unconverted"],
    },
    Rule {
        id: "PS-THRESHOLD-EFFECTIVE",
        basis: Documented,
        source: "payouts",
        says: "A threshold change applies from the current cycle; a payout already scheduled keeps its amount.",
        code: "account::Settings::threshold_at",
        tests: &["threshold_change_applies_to_runs_created_after_it"],
    },
    Rule {
        id: "PS-CARRY",
        basis: Documented,
        source: "payouts",
        says: "A balance below the minimum carries forward to the next cycle.",
        code: "engine (no debit when not paid)",
        tests: &["money_is_conserved"],
    },
    Rule {
        id: "PS-INR",
        basis: Documented,
        source: "payouts",
        says: "INR wallet discontinued: an INR balance is not paid out and does not count toward the minimum.",
        code: "engine (excluded entries)",
        tests: &["inr_is_listed_as_excluded"],
    },
    // Eligibility
    Rule {
        id: "VP-GATES",
        basis: Documented,
        source: "verification",
        says: "Live payments need every required form approved. Payouts also need the Monitoring Review completed. The two turn on separately.",
        code: "account::eligibility",
        tests: &["payments_live_but_payouts_inactive"],
    },
    Rule {
        id: "PS-INACTIVE-AGAIN",
        basis: Documented,
        source: "payouts",
        says: "Payouts that were running can return to inactive while an information request is open or a compliance review runs.",
        code: "account::eligibility",
        tests: &["reserve_release_does_not_clear_a_restriction"],
    },
    Rule {
        id: "VP-INFO-REQUEST",
        basis: Documented,
        source: "verification",
        says: "Information requests do not block payouts by themselves, but an unanswered one may lead to pauses.",
        code: "account::Restriction::pauses_payouts",
        tests: &["information_request_does_not_block_by_default"],
    },
    // Money
    Rule {
        id: "BAL-WALLETS",
        basis: Documented,
        source: "balances",
        says: "Each business has USD, GBP and EUR wallets. Each wallet has its own ledger in its own currency.",
        code: "money::Currency::PAYOUT_WALLETS",
        tests: &["money_is_conserved"],
    },
    Rule {
        id: "BAL-CHAIN",
        basis: Documented,
        source: "ledger-api",
        says: "Each ledger entry carries before_balance and after_balance for its wallet.",
        code: "ledger::check_chain",
        tests: &["gap_in_balance_chain_is_reported_not_guessed"],
    },
    Rule {
        id: "BAL-TYPES",
        basis: Documented,
        source: "balances",
        says: "Entry types and signs: payment +, refund -, dispute -, dispute_reversal +, tax -, tax_reversal +, fees -, fee reversals +, payout -, payout_reversal +, reserve_hold -, reserve_release +, adjustment either.",
        code: "ledger::EventType",
        tests: &["wrong_sign_is_flagged"],
    },
    Rule {
        id: "BAL-DISPUTE-FEE",
        basis: Documented,
        source: "balances",
        says: "The dispute fee is not returned when the merchant wins.",
        code: "ledger::EventType::DisputeFees",
        tests: &["dispute_won_returns_amount_not_fee"],
    },
    Rule {
        id: "PS-RESERVE",
        basis: Documented,
        source: "payouts",
        says: "A reserve is held for a set period, released automatically, and already excluded from the payable balance so it is not deducted twice. The share and period are not published.",
        code: "ledger (reserve account)",
        tests: &["reserve_release_does_not_clear_a_restriction"],
    },
    Rule {
        id: "PS-CONVERSION",
        basis: Documented,
        source: "payouts",
        says: "Unlinked wallets are converted to the primary account's currency with a conversion fee. Linked wallets are paid in their own currency with no conversion fee. The fee rate is not published.",
        code: "engine::expected_payout",
        tests: &["conversion_fee_unknown_is_shown_as_unknown"],
    },
    Rule {
        id: "PS-FX-INDICATIVE",
        basis: Documented,
        source: "balances",
        says: "Converted totals are for reference; the final amount depends on the rate at transfer.",
        code: "engine (estimated conversions)",
        tests: &["conversion_fee_unknown_is_shown_as_unknown"],
    },
    Rule {
        id: "PS-PAYOUT-FEE",
        basis: Documented,
        source: "payouts",
        says: "Under standard pricing the payout fee is 5 USD for payouts below 1,000 USD, or the GBP or EUR equivalent.",
        code: "engine::payout_fee",
        tests: &["payout_fee_boundary_at_1000"],
    },
    Rule {
        id: "PR-SWIFT",
        basis: Documented,
        source: "pricing",
        says: "USD SWIFT payouts: 25 USD per wire.",
        code: "engine::expected_payout",
        tests: &["swift_wire_fee_and_unknown_bank_charges"],
    },
    Rule {
        id: "PS-BANK-CHARGES",
        basis: Documented,
        source: "payouts",
        says: "Your bank and intermediary banks may deduct charges or convert at their own rate. These are not shown on the payout invoice.",
        code: "engine (unknown_charges)",
        tests: &["swift_wire_fee_and_unknown_bank_charges"],
    },
    Rule {
        id: "PS-FAILED",
        basis: Documented,
        source: "payouts",
        says: "A failed payout credits back the amount and the fee as Payout Reversal and Payout Fees Reversal.",
        code: "engine::latest_status",
        tests: &["failed_payout_without_reversal_is_flagged"],
    },
    Rule {
        id: "PS-SUCCESS-IS-SENT",
        basis: Documented,
        source: "payouts",
        says: "Success means sent to the bank, not received. If it hasn't arrived, ask the bank to trace it, then contact support with the pyt_ id.",
        code: "engine::latest_status",
        tests: &["success_is_not_bank_receipt"],
    },
    Rule {
        id: "PB-UNATTRIBUTED",
        basis: Documented,
        source: "breakup",
        says: "A payout's entries need not sum to the payout. The difference is unattributed: positive means it drew on an earlier leftover balance, negative means a remainder funds a later payout.",
        code: "engine (unattributed)",
        tests: &["actual_payout_differs_by_one_cent"],
    },
    // Webhooks
    Rule {
        id: "WH-SIGNATURE",
        basis: Documented,
        source: "webhooks",
        says: "Sign {webhook-id}.{webhook-timestamp}.{raw body} with HMAC-SHA256 using the base64 key after the whsec_ prefix. Header has v1,<base64> entries; any match is valid. Compare in constant time. Reject timestamps more than 5 minutes off.",
        code: "webhook::verify",
        tests: &[
            "accepts_a_valid_signature",
            "rejects_tampered_body",
            "rejects_old_timestamp",
        ],
    },
    Rule {
        id: "WH-DEDUP",
        basis: Documented,
        source: "webhooks",
        says: "Use webhook-id to detect and skip duplicates. Non-2xx responses are retried.",
        code: "store::ingest_webhook",
        tests: &["duplicate_webhook_is_stored_once"],
    },
    Rule {
        id: "WH-PAYOUT-ORDER",
        basis: Documented,
        source: "payout-webhooks",
        says: "Payout events are not terminal and not ordered: failed can follow success and the reverse. Compare each event's timestamp with the last one processed.",
        code: "store::upsert_payout, engine::latest_status",
        tests: &[
            "late_older_payout_event_does_not_win",
            "reordered_payout_events_settle_by_timestamp",
        ],
    },
    // Conflicts
    Rule {
        id: "CF-THRESHOLD-EQUAL",
        basis: Conflict,
        source: "faq",
        says: "The FAQ says the balance must exceed 50 USD; the payouts page says equal to or greater than the threshold. We use equal to or greater (payouts page is newer and more specific).",
        code: "engine::run (threshold check)",
        tests: &["threshold_is_inclusive"],
    },
    Rule {
        id: "CF-PAYOUT-FEE-CURRENCY",
        basis: Conflict,
        source: "pricing",
        says: "Pricing says 5 USD if payout < 1,000 USD; the payouts page adds 'or the GBP or EUR equivalent'. Unclear whether that means 5 EUR under 1,000 EUR or 5 USD converted. Configurable (fee_basis); default converts USD amounts at the run-date rate.",
        code: "engine::payout_fee",
        tests: &["payout_fee_boundary_at_1000"],
    },
    Rule {
        id: "CF-PAYOUTS-FREE",
        basis: Conflict,
        source: "pricing",
        says: "The pricing card headline says payouts are Free, with a 5 USD fee under 1,000 USD in the small print. We apply the fee.",
        code: "policy::Params",
        tests: &["payout_fee_boundary_at_1000"],
    },
    Rule {
        id: "CF-BANK-DAYS",
        basis: Conflict,
        source: "faq",
        says: "The FAQ gives both 1-2 and 1-5 business days for funds to reach the bank. We use 5 before suggesting a trace.",
        code: "engine::latest_status",
        tests: &["success_is_not_bank_receipt"],
    },
    Rule {
        id: "CF-INR-FILTER",
        basis: Conflict,
        source: "balances",
        says: "The Balances page lists three wallets but its currency filter offers INR. We treat INR entries as a non-payout wallet (PS-INR).",
        code: "engine (excluded entries)",
        tests: &["inr_is_listed_as_excluded"],
    },
    // Design choices
    Rule {
        id: "DC-ROUNDING",
        basis: DesignChoice,
        source: "none",
        says: "Rounding is half-to-even unless configured. Every conversion and percentage keeps its exact value and residual in the trace.",
        code: "money::div_round",
        tests: &[
            "rounding_modes",
            "conversion_across_exponents_records_the_residual",
        ],
    },
    Rule {
        id: "DC-PAYOUT-DRAIN",
        basis: DesignChoice,
        source: "none",
        says: "A paid run drains each wallet in its group to zero as of the period end: payout + payout_fees (+ conversion) equals the drained balance.",
        code: "engine::expected_payout",
        tests: &["money_is_conserved"],
    },
    Rule {
        id: "DC-FEE-COMPARE",
        basis: DesignChoice,
        source: "none",
        says: "The 1,000 USD payout-fee boundary is compared against the payout before fees.",
        code: "engine::payout_fee",
        tests: &["payout_fee_boundary_at_1000"],
    },
    Rule {
        id: "DC-SWIFT-STACK",
        basis: DesignChoice,
        source: "none",
        says: "The SWIFT wire fee is applied on top of the payout fee. Pricing lists them separately and doesn't say whether they stack.",
        code: "engine::expected_payout",
        tests: &["swift_wire_fee_and_unknown_bank_charges"],
    },
    Rule {
        id: "DC-RATES",
        basis: DesignChoice,
        source: "none",
        says: "Conversions use the latest rate you supply on or before the run date, marked estimated. Settled amounts come only from currency_conversion ledger entries.",
        code: "engine::Rates",
        tests: &["conversion_fee_unknown_is_shown_as_unknown"],
    },
    Rule {
        id: "DC-CYCLE-SWITCH",
        basis: DesignChoice,
        source: "none",
        says: "On a cycle change the period in progress finishes on the old cycle. The new cycle starts the next day, with a short first period if needed, so every day is covered once.",
        code: "calendar::schedule",
        tests: &["cycle_switch_covers_every_day_once"],
    },
    Rule {
        id: "DC-MATCH",
        basis: DesignChoice,
        source: "none",
        says: "A recorded payout matches a run when its currency matches and it was created between the run's scheduled date and the next run's. Two candidates means no match: both are reported, neither is guessed.",
        code: "engine::run (payout matching)",
        tests: &["two_candidate_payouts_are_not_guessed"],
    },
    Rule {
        id: "DC-REVERSAL-WALLET",
        basis: DesignChoice,
        source: "none",
        says: "Docs don't say which wallet a reversed converted payout returns to. We take the reversal entries' own currency from the ledger and never assume.",
        code: "ledger",
        tests: &["failed_payout_without_reversal_is_flagged"],
    },
    Rule {
        id: "DC-UNKNOWN",
        basis: DesignChoice,
        source: "none",
        says: "Hold reasons, release dates, reserve terms, conversion fees and bank charges that aren't in the input stay 'unknown' in the output.",
        code: "account, engine",
        tests: &["unknown_hold_reason_and_date_stay_unknown"],
    },
];

pub fn rule(id: &str) -> &'static Rule {
    RULES
        .iter()
        .find(|r| r.id == id)
        .unwrap_or_else(|| panic!("unknown rule {id}"))
}

/// The pages behind this policy version and their last-modified dates.
pub fn docs_as_of() -> Vec<(&'static str, &'static str)> {
    SOURCES
        .iter()
        .filter(|s| !s.url.is_empty())
        .map(|s| (s.key, s.last_modified))
        .collect()
}

/// The matrix as Markdown, for docs/SOURCES.md.
pub fn matrix_markdown() -> String {
    let mut out = format!(
        "# Sources and rules\n\nPolicy version `{POLICY_VERSION}`. Generated by `whenpaid rules`; edit `src/policy.rs`, not this file.\n\n## Sources\n\n| Key | Page | Last modified | Retrieved |\n|---|---|---|---|\n"
    );
    for s in SOURCES.iter().filter(|s| !s.url.is_empty()) {
        out += &format!(
            "| {} | {} | {} | {} |\n",
            s.key, s.url, s.last_modified, s.retrieved
        );
    }
    out += "\n## Rules\n\n| Rule | Basis | Source | What it says | Code | Tests |\n|---|---|---|---|---|---|\n";
    for r in RULES {
        let basis = match r.basis {
            Documented => "documented",
            Conflict => "conflict",
            DesignChoice => "our choice",
        };
        out += &format!(
            "| {} | {} | {} | {} | `{}` | {} |\n",
            r.id,
            basis,
            r.source,
            r.says,
            r.code,
            r.tests
                .iter()
                .map(|t| format!("`{t}`"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    out
}

/// The numbers the rules use. Defaults follow the docs; anything unpublished is `None`.
#[derive(Clone, Debug, serde::Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Params {
    /// Minor units of USD.
    pub default_threshold: i64,
    pub min_threshold: i64,
    pub payout_fee: i64,
    pub payout_fee_below: i64,
    pub fee_basis: FeeBasis,
    pub swift_fee: i64,
    /// Not published (PS-CONVERSION). Leave unset unless you know your rate.
    pub conversion_fee_bps: Option<u32>,
    pub rounding: crate::money::Rounding,
    /// Business days after a successful payout before suggesting a trace (CF-BANK-DAYS).
    pub bank_days: u32,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            default_threshold: 10_000,
            min_threshold: 5_000,
            payout_fee: 500,
            payout_fee_below: 100_000,
            fee_basis: FeeBasis::Converted,
            swift_fee: 2_500,
            conversion_fee_bps: None,
            rounding: crate::money::Rounding::HalfEven,
            bank_days: 5,
        }
    }
}

/// How "5 USD below 1,000 USD, or the GBP or EUR equivalent" is read (CF-PAYOUT-FEE-CURRENCY).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FeeBasis {
    /// Convert the USD fee and boundary into the payout currency at the run-date rate.
    Converted,
    /// Use the same numbers in the payout currency: 5 EUR below 1,000 EUR.
    Nominal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_ids_are_unique_and_sources_exist() {
        let mut ids: Vec<_> = RULES.iter().map(|r| r.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), RULES.len());
        for r in RULES {
            assert!(
                SOURCES.iter().any(|s| s.key == r.source),
                "{} cites unknown source {}",
                r.id,
                r.source
            );
            assert!(!r.tests.is_empty(), "{} has no test", r.id);
        }
    }

    #[test]
    fn every_test_named_in_the_matrix_exists() {
        let code = [
            include_str!("money.rs"),
            include_str!("calendar.rs"),
            include_str!("ledger.rs"),
            include_str!("account.rs"),
            include_str!("engine.rs"),
            include_str!("webhook.rs"),
            include_str!("../tests/rules.rs"),
            include_str!("../tests/props.rs"),
            include_str!("../tests/scenarios.rs"),
            include_str!("../tests/db.rs"),
        ]
        .concat();
        for r in RULES {
            for t in r.tests {
                assert!(
                    code.contains(&format!("fn {t}(")),
                    "{} names missing test {t}",
                    r.id
                );
            }
        }
    }
}
