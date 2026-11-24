# whenpaid

whenpaid reads a merchant's balance ledger and payout history and answers the three "where is my payout" questions: when do I get paid, how much, and why was it held or smaller than I expected. Every line of the answer names the rule it came from, and every rule links to the page of the docs it was taken from.

I built it against Dodo Payments' public docs as they stood on 2026-11-24. It's unofficial and uses only public information. Nothing in it comes from inside the company.

## Why

Most payout questions don't need a person. The ledger already has the answer, but it's spread over dozens of rows in three currencies. A threshold, a weekend, a reserve and a chargeback can all land in the same period. When support explains it by hand, the reasoning goes into a ticket and is gone.

The cite-the-source idea comes from Sense AI: an answer should point at where it came from. whenpaid applies that to money. If the tool says a payout was 197.51 USD, you can follow every cent back to a ledger row and every decision back to a sentence in the docs.

## What a report looks like

From `examples/merchant.json`, a made-up account. Every report opens with the short answer:

```
In short
  Next payout: 2026-12-04, for 2026-11-16 to 2026-11-30. 630.34 USD before fees so far, minus a 5.00 USD payout fee and a conversion fee that isn't published, so the exact amount isn't known yet. [PS-PAYOUT-FEE, PS-CONVERSION]
  Last payout: 2026-11-18, pyt_7002 for 197.51 USD failed and was credited back to the wallet. It goes out with the next eligible run. [PS-FAILED]
```

The same lines are in the JSON output and the API response as `summary`, so a support tool can show them as is. Below that, each run is broken down line by line. Here is the failed one from the full report in `examples/merchant.txt`:

```
  2026-11-18 combined: paid, then failed and credited back (pyt_7002)
    Covers 2026-11-01 to 2026-11-15, created 00:00 UTC, counts entries before 2026-11-16 00:00 UTC
    USD wallet
      carried in                                              0.00 USD
      payments (1)                                          499.00 USD
        pay_1005 499.00 USD
      disputed amounts held (1)                            -199.00 USD
        dsp_1003 -199.00 USD
      tax collected and remitted (1)                        -39.92 USD
        pay_1005:tax -39.92 USD
      payment fees (1)                                      -27.57 USD
        pay_1005:fee -27.57 USD
      dispute fees (not returned if won) (1)                -30.00 USD
        dsp_1003:fee -30.00 USD
      payable                                               202.51 USD
    Minimum: 202.51 USD against 100.00 USD, reached [PS-THRESHOLD]
    Expected payout in USD
      before fees                                           202.51 USD
      payout fee                                             -5.00 USD
        5.00 USD because the payout before fees is below 1,000.00 USD
      sent                                                  197.51 USD
      Not known: charges deducted by your bank or intermediary banks (PS-BANK-CHARGES)
    Actual: pyt_7002 sent 197.51 USD, fee 5.00 USD
    Due since 2026-11-18, 6 days ago [PS-MISSED-DATE]
    Rules: BAL-TYPES, DC-FEE-COMPARE, PS-CYCLE, PS-FAILED, PS-INITIATION, PS-MISSED-DATE, PS-PAYOUT-FEE, PS-THRESHOLD
    Next: The payout failed and the amount and fee were credited back to the wallet. Check the bank details on file; the money goes out with the next eligible run.
```

Every report starts with the policy version and a "docs as of" stamp listing each source page's last-modified date, so you know which version of the rules produced it.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/rakshit-gen/whenpaid/main/scripts/install.sh | sh
```

That puts the binary in `~/.local/bin` and checks its SHA-256. You can also build from source with `cargo install --path .` (Rust 1.88 or newer, edition 2024).

## Use it

```sh
whenpaid explain examples/merchant.json            # last three runs plus the next one
whenpaid explain examples/merchant.json --all      # every run
whenpaid explain examples/merchant.json --entries  # every ledger row under each line
whenpaid explain examples/merchant.json --json     # the full trace, with input_sha256
whenpaid rules                                     # every rule, its source and its tests
```

The exit status is 0 when everything adds up, 2 when the report found something that needs a look (a missing payout, a fee off the published schedule, a gap in the balance chain), and 1 for bad input.

The input is one JSON file:

| Field | What goes in it |
|---|---|
| `ledger` | Rows from the balance ledger API, as is: `id`, `created_at`, `currency`, `event_type`, `amount`, `is_credit`, and `before_balance`/`after_balance` when you have them |
| `payouts` | Payout records with their status events (from the payouts API or webhooks), plus `bank_confirmed_at` if you saw the money arrive |
| `rates` | USD per unit of EUR and GBP, by date, as decimal strings |
| `settings` | Cycle history, threshold history, primary currency, linked wallets, SWIFT or local |
| `forms`, `restrictions` | Verification status history and any holds, with reason and end date only if you know them |
| `params` | Fee numbers, if yours differ from the published ones. Conversion fee is unset by default because it isn't published |
| `as_of` | The time the report is for. Nothing reads the clock |

## How the money works

All amounts are `i64` in the currency's smallest unit, with decimals taken from ISO 4217: 0 for JPY, 2 for USD, 3 for KWD and BHD. Arithmetic is checked, so an overflow is an error rather than a wrong number. There are no floats anywhere in the calculation.

Rounding happens in one function and is half to even by default. Every conversion keeps the exact fraction next to the rounded result, and the leftover is printed (for example "exact 208.92677 USD, rounded up by 0.323 of a cent"). The leftover isn't moved anywhere. In a rule estimate it is reported and dropped. When the provider actually converts, the settled amount comes from the `currency_conversion` rows in the ledger, and whenpaid shows the rate those rows imply. Estimated conversions, settled conversions and bank charges nobody has told you about are kept as three separate things and never added together.

The ledger is rebuilt as a double-entry book. Each row becomes a balanced transaction between the wallet and whoever was on the other side (customers, tax, fees, reserve, the bank). The book has to sum to zero in every currency, and the wallet has to reconcile: opening balance plus movements equals closing balance. If `before_balance` on a row doesn't follow from the row before it, whenpaid doesn't invent a matching entry. It reports a gap with its size and the two rows on either side, which tells you which date range to re-export.

## Running it as a service

```sh
export DATABASE_URL=postgres://localhost/whenpaid
whenpaid tenant add bus_123 whsec_...      # prints an API token once
whenpaid import bus_123 export.json        # backfill from a ledger export
whenpaid serve &                           # webhooks on /webhooks/bus_123
whenpaid worker &                          # recomputes reports when data changes

curl -H "Authorization: Bearer wp_..." localhost:8080/v1/report
curl -X POST -H "Authorization: Bearer wp_..." -H 'content-type: application/json' \
     -d '{"at":"2026-11-20T09:00:00Z"}' localhost:8080/v1/payouts/pyt_123/received
```

Webhooks are checked with the Standard Webhooks scheme the docs describe: HMAC-SHA256 over id, timestamp and body, constant-time compare, 5 minutes of clock tolerance. Each webhook is stored once per `webhook-id`. Payout events can arrive out of order (the docs say failed can follow success), so the current status only moves to an event with a later timestamp. Older events are kept as history. The `business_id` inside the body has to match the endpoint's tenant.

The worker leases one tenant's job at a time with `FOR UPDATE SKIP LOCKED`. Each new piece of data bumps the job's version. The worker clears the job only if the version hasn't moved while it worked, so a webhook that lands mid-run is never lost. A worker that dies mid-job leaves the job behind for the next one once the lease runs out. The tests kill the process at two points to prove it.

Payout runs are created at 00:00 UTC, so a report can go out of date with no new data at all. Once a minute the worker queues every account whose newest report is from before today (UTC). It keeps the last 30 reports per account. If a recompute fails, `GET /v1/report` still returns the last good report, with the error in `recompute_error` so the caller knows it's behind.

Bad data is refused before it's stored. An import is checked in full first (negative or absurd amounts, conflicting duplicate ids, invalid settings) and either goes in completely or not at all. A webhook with a negative amount gets a 200 so the sender stops retrying, but nothing is kept.

`GET /healthz` returns 503 if Postgres doesn't answer within two seconds. On SIGTERM, `serve` stops taking new requests and finishes the ones in flight, and `worker` finishes its current job before it exits.

whenpaid never moves money, so there's no outbox. It only reads what the provider reports.

## Tests

```sh
cargo test                                                    # 82 tests
DATABASE_URL=postgres://localhost/whenpaid_test cargo test    # also runs the 12 Postgres tests
```

- **Unit tests** cover every module.
- **Fixtures I worked out by hand** include the provider's own MoR example (230.00 USD gross becomes 186.95 net), a mixed-wallet payout, both readings of the GBP fee rule, and a holiday move.
- **Property tests** check that every cent is either paid out or still in the wallet, and that every run's lines add up exactly to its payable balance. They also check that a shuffled or duplicated export gives the same report, and that removing any single row shows up as a gap of exactly that row's amount.
- **Eleven scenarios** are built from public "where is my payout" questions, with names and amounts replaced. The list is in `docs/SCENARIOS.md`.
- **Postgres tests** cover 25 concurrent copies of the same webhook, reordered payout events, eight workers racing over ten tenants, new data arriving mid-job, crashes at two points, re-imports with a changed row, tenant isolation, stale reports being queued again, pruning, refusing bad data, and the health check.

The rule catalog in `src/policy.rs` names the tests for each rule, and a test fails if any of those tests doesn't exist. CI also fails if `docs/SOURCES.md` falls out of sync with the code.

## Measured vs claimed

These are local numbers from one laptop, not production: Apple M4 (10 cores), 16 GB RAM, macOS, Rust 1.100 release build, Postgres 14.20 on the same machine. Measured 2026-11-24.

| Claim | Value | How it was measured |
|---|---|---|
| Engine replay, 100k ledger rows (two years, 3 wallets, 52 runs including projected ones) | 0.22 s wall, 132 MB peak RSS, 5 of 5 runs | `cargo run --release --example generate -- bench 100000 > bench_100k.json` then `/usr/bin/time -l target/release/whenpaid explain bench_100k.json` |
| Engine replay, 1M ledger rows | 2.46 to 2.80 s wall (median 2.52), 1.3 GB peak RSS, 5 runs | same, with `bench 1000000` |
| Webhook ingest, signed payout events, 32 concurrent senders for 30 s | 2,853 req/s, p50 9.4 ms, p95 21.8 ms, p99 33.0 ms, 0 failed, 85,627 of 85,627 rows in the inbox afterwards | `k6 run -e SECRET=whsec_... -e BUSINESS=bus_bench bench/webhooks.js` against `whenpaid serve` |
| Accuracy on the benchmark data | every run passed `verify()`, and all three wallets reconciled to the reported closing balance | from the same runs |

Each "time" figure includes reading and parsing the JSON file, which is 27 MB for 100k rows and 278 MB for 1M. Peak memory grows roughly in line with the input because the whole export is held in memory. The ingest test sends every event to one tenant, so all of them bump the same jobs row, which is the worst case for that row. I haven't measured many tenants in parallel.

## What's in the repo

- `src/policy.rs`: every rule with its id, source URL, last-modified date, and whether it's documented, conflicting, or my own call
- `src/engine.rs`: schedule, threshold, fees, matching, overdue tracking, the per-run trace
- `src/ledger.rs`: export validation, double-entry book, balance-chain checks, double-deduction checks
- `src/account.rs`: verification, restrictions and settings, kept apart from the money
- `src/store.rs`, `src/server.rs`: Postgres, webhooks, the worker
- `docs/SOURCES.md`: the source to rule to code to test matrix (generated)
- `docs/ASSUMPTIONS.md`: what the docs don't say, where they disagree, and what I picked
- `docs/SCENARIOS.md`: the anonymized cases and where each one came from
- `docs/DECISIONS.md`: why things are built the way they are

## License

MIT
