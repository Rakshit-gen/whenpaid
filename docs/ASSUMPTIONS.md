# Assumptions and open questions

whenpaid only knows what the public docs say and what you put in the input file. This page lists everything it had to assume, every place where two pages disagree, and the questions only the payouts team can answer. Each item has a rule id you can look up in `docs/SOURCES.md` or with `whenpaid rules`.

Docs as of 2026-11-24. If a page changes, its last-modified date in `src/policy.rs` changes with it, and so does the policy version printed at the top of every report.

## Where the docs disagree

**Is the minimum payout inclusive?** (CF-THRESHOLD-EQUAL) The FAQ says the balance has to exceed 50 USD. The payout structure page says equal to or greater than the threshold. I went with equal to or greater, because the payout structure page is newer and is the page about payouts. A balance of exactly 100.00 USD against a 100.00 USD threshold gets paid.

**What does the 5 USD payout fee mean in EUR or GBP?** (CF-PAYOUT-FEE-CURRENCY) Pricing says 5 USD under 1,000 USD. The payout page adds "or the GBP or EUR equivalent". That could mean 5 EUR under 1,000 EUR, or 5 USD converted at the day's rate. Both readings are in the code (`fee_basis`). The default converts. When the actual fee on a payout doesn't match either reading, the report says so rather than picking one.

**Are payouts free?** (CF-PAYOUTS-FREE) The pricing card headline says free. The small print under it has the 5 USD fee. I apply the fee.

**How long until the money is in the bank?** (CF-BANK-DAYS) The FAQ gives both 1 to 2 and 1 to 5 business days. whenpaid waits 5 business days after a payout shows success before it suggests asking for a trace.

**Is there an INR wallet?** (CF-INR-FILTER) The balances page lists three payout wallets (USD, EUR, GBP), but its currency filter offers INR. INR rows are shown in an "excluded" section with their total, never added to a payout.

**The holiday example.** The payout structure page has a notification example where a payout due 2026-10-05 moves to 2026-10-06. 2026-10-05 is a normal Monday, so the page's own rule wouldn't move it. I follow the rule and not the example. If the example is right, there is a calendar rule the docs don't mention.

## What the docs don't say, and what I picked

**Rounding** (DC-ROUNDING). Not documented anywhere. Half to even, configurable, and every conversion prints its exact value and the leftover.

**How the payout fee is compared to 1,000 USD** (DC-FEE-COMPARE). Against the payout before fees. Comparing after fees would make a payout of 1,002 USD cheaper to receive than one of 1,000 USD.

**Whether the SWIFT fee stacks with the payout fee** (DC-SWIFT-STACK). Pricing lists them on separate lines. I add both.

**Which exchange rate** (DC-RATES). The provider's rate isn't published. whenpaid uses the latest rate you supply on or before the run date and marks the result as an estimate. The settled amount only ever comes from `currency_conversion` rows in the ledger.

**Conversion fee.** Not published. It stays unset, and the report says "not known" instead of guessing.

**Cycle changes** (DC-CYCLE-SWITCH). The docs say a change applies from the next cycle, and that you can only move to a less frequent cycle. When a merchant switches, the period in progress finishes on the old cycle and the new one starts the next day. That can make the first new period short. Every day still belongs to exactly one period.

**No cycle history in the input.** whenpaid assumes bimonthly from the first ledger row, because that's the default the docs describe. If the account changed cycle before the export starts, give the history in `settings.cycles`, or the early runs will be wrong.

**Matching a payout to a run** (DC-MATCH). A payout belongs to a run if it's in the same currency and was created between that run's date and the next run's. If two live payouts fit, neither is chosen. Both are listed under "Needs a look".

**Where a reversed converted payout goes** (DC-REVERSAL-WALLET). The docs don't say which wallet gets the money back when a converted payout fails. whenpaid reads the currency off the reversal rows themselves.

**Holds with no reason or end date** (DC-UNKNOWN). Shown as "not disclosed" and "unknown", with a count of days since the payout was first due. whenpaid never fills them in.

## Limits I know about

**The opening balance.** The first row in an export has a `before_balance`, which becomes the opening balance. If that first row is missing, the opening just moves and nothing looks wrong. A missing row is only caught when the same wallet has rows on both sides of it. The property tests check exactly that case.

**Live API.** whenpaid reads exported JSON in the shape the API reference documents. I haven't run it against a live account, so field names come from the docs, not from real responses. The top-level input, `settings` and `params` reject fields they don't know, so a typo there fails loudly. Ledger rows and payout records accept extra fields, so a renamed field in those would show up as missing data, not an error.

**Bank charges.** Intermediary bank fees on SWIFT wires and charges from your own bank are never in the provider's data. They're always listed as "not known".

**No outbox.** whenpaid never moves money, so there is nothing to make exactly-once on the way out. If it ever sends emails or support replies, that send will need an outbox.

**Rows after as_of.** A report is for one moment. Ledger rows dated after `as_of` are left out of every run and the reconciliation, and listed with their count and total (DC-AS-OF). The service always uses the current time, so this only matters for the CLI or for clock skew.

**Webhook secrets are stored as given.** Checking an HMAC needs the key itself, so the webhook secret sits in the tenants table in plain text. API tokens are stored only as SHA-256 hashes. Anyone who can read the database can forge webhooks for that tenant, so treat database access accordingly.

**No rate limiting.** An unsigned request costs one indexed lookup to find the tenant's secret, and nothing is written until the signature checks out. There is no per-tenant or per-IP limit, though, so put the service behind a proxy that has one.

**Memory.** The engine holds the whole export in memory. A million rows peaks around 1.3 GB. That's fine for one merchant's history but not for a whole platform in one process.

## Questions for the payouts team

1. Is the 2026-10-05 example a typo, or is there another closure rule?
2. Which reading of the GBP and EUR payout fee is right?
3. Is the fee compared before or after it is taken off?
4. Does the SWIFT fee stack on top of the payout fee?
5. Is there a conversion fee, and which rate source is used?
6. Which wallet does a failed converted payout go back to?
7. The docs say an unanswered information request may lead to a pause. After how long?
