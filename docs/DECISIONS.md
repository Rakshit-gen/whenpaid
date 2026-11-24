# Decision log

This is why whenpaid is built the way it is. Each entry covers what I decided, what else I considered, and what it costs. The rule-level choices (rounding, matching, fee comparison and so on) are also in `src/policy.rs` with a `DC-` id. This page is about the bigger calls.

## 1. Answer one question well

I started from the questions merchants actually ask about payouts. Almost all of them are some form of: when do I get paid, how much, and why was it held or smaller. I didn't build a dashboard, a payments simulator or a general ledger tool. Every feature had to help answer one of those three questions, and anything that didn't got cut.

## 2. Rust, and no machine learning

A payout explanation has to be exactly right or it's worse than nothing. I wanted a type system that makes it hard to add USD to EUR by accident, and arithmetic that fails on overflow instead of wrapping. Rust gives me both.

I left ML out on purpose. Every number in a report has to trace back to a ledger row and a sentence in the docs. A model that is right 98% of the time is the wrong tool for telling someone how much money they're owed.

## 3. Every line cites its rule

I took this from Sense AI, which makes an answer point at its source. Here that means each rule has an id, a source URL, the page's last-modified date, and a label: documented, conflicting, or my own call. Every line in a report names the rules it used.

Two things keep this honest. The rule catalog lists the tests for each rule, and a unit test fails if any of them is missing. CI regenerates `docs/SOURCES.md` from the code and fails if it changed. That makes it hard for the docs and the code to drift without someone noticing.

The cost is that every new behavior needs a rule entry before it ships. I think that's the right amount of friction for money.

## 4. Separate what the docs say from what I decided

Reading the docs, I found places where two pages disagree (the threshold, the GBP fee, bank days) and places where they say nothing (rounding, whether SWIFT fees stack). If I'd blended those into the code, nobody could tell later which numbers came from the provider and which came from me.

So every rule is one of three kinds: Documented, Conflict, or DesignChoice. Conflicts say which reading I took and why. Where both readings are plausible, both are in the code behind a setting. `docs/ASSUMPTIONS.md` collects all of them, plus the questions I'd ask the payouts team.

## 5. Integer minor units, one rounding function

Amounts are `i64` in the smallest unit of the currency, with decimals from ISO 4217. Floats were never on the table. I also considered a decimal library, but an integer is simpler, faster, and matches what the ledger API returns.

All rounding goes through one function, half to even by default. I picked half to even because it doesn't drift in one direction over many conversions. Every conversion keeps the exact fraction next to the rounded value, and the leftover is printed. I decided not to move leftovers into some account, because a rule estimate isn't real money. When the provider actually converts, the settled amounts come from the ledger rows, and whenpaid shows the rate they imply.

## 6. Rebuild the ledger as double entry

The provider's ledger is single-sided: rows move one wallet up or down. I rebuild it as double entry, where each row posts against a counter book (customers, tax, fees, reserve, paid out). The point is the checks it buys. The whole book has to sum to zero, and each wallet has to reconcile from opening to closing balance.

When `before_balance` on a row doesn't follow from the previous row, I report a gap with its size and the rows on each side. I never invent a balancing entry. A gap is exactly the thing a merchant or support agent needs to see, and plugging it would hide it.

## 7. Keep schedule and eligibility apart

Whether a payout is due and whether the account is allowed to receive it are different questions. The calendar works out periods and run dates from the cycle history and the Federal Reserve holiday list. The account layer works out blockers from verification and restrictions. The engine combines them per run.

This is what lets the report say "this run was due on the 18th, it's been blocked for 36 days, the reason wasn't disclosed". A blocked run keeps its original due date, so the overdue count is honest.

## 8. A deterministic engine

The engine takes one input and never reads the clock, the network or the disk. `as_of` is part of the input. Same input, same report. The JSON report includes a SHA-256 of the input so you can tell later exactly what produced it.

A property test shuffles the export and adds a duplicate row, then checks the report doesn't change. Another drops a random row and checks the report finds a gap of exactly that row's amount. That second one caught a real bug: when several rows share a timestamp, I was ordering them in a way that broke the balance chain and reported gaps that weren't there.

## 9. Unknown stays unknown

If a hold has no reason, the report says "not disclosed". If there's no end date, it says "unknown". Conversion fee, intermediary bank charges, reserve terms: same thing. I was tempted to fill in typical values, but a wrong guess about someone's money does more damage than an honest blank.

The same goes for matching. If two payouts could belong to one run, I report both and pick neither.

## 10. Expected and actual are compared, never merged

For each run I work out what the payout should have been, then match it against what was actually sent. Differences go under "Needs a look" with the expected value, the actual value and the rule. whenpaid never corrects the actual number to fit its own model. If the model and reality disagree, either the model is wrong or something happened that a person should look at. In both cases a person should see it.

## 11. Postgres, and only Postgres

For the service I needed webhook deduplication, tenant isolation, a job queue and crash safety. Postgres covers all of it: a primary key on `(tenant_id, webhook_id)` for dedup, `FOR UPDATE SKIP LOCKED` for the queue, transactions for the rest. I considered Redis or a separate queue and decided a second moving part wasn't worth it at this size.

## 12. Version counter on jobs

Each new piece of data bumps a version on the tenant's job row. A worker remembers the version it started with and deletes the job only if the version hasn't moved. If a webhook lands while a report is being computed, the job survives and the report gets recomputed. One worker per tenant at a time, and no lost updates.

I prove it in the tests by killing the worker process at two points: after computing and after writing the report. Then I check the job is still there and the next run finishes it.

## 13. No outbox

whenpaid only reads. It never sends money, emails or messages, so there's no outgoing side effect to make exactly-once, and an outbox would be dead code. I wrote this down so nobody adds one out of habit. If whenpaid ever starts sending support replies, that's when it needs one.

## 14. Webhook endpoint behavior

- An unknown business gets the same 401 as a bad signature, so the endpoint doesn't reveal which accounts exist.
- A correctly signed body for a different business gets 403.
- A database error returns 503, so the provider retries.
- A malformed event that will never parse gets a 200 with `ignored`, so retries stop. Nothing is stored.

Payout events can arrive out of order. The docs say failed can follow success. So the stored status only moves forward in event time, and older events are kept as history.

## 15. Anonymized scenarios, no names

The scenarios come from public questions merchants asked. I changed every name, amount, date and id, and I don't link to the posts. The aim is to show the tool handles real situations, not to point at anyone.

## 16. Benchmarks with their conditions

Every number in the README says which machine it came from, which dataset, the exact command, and what accuracy check ran alongside it. I generate the benchmark data with a seeded generator so anyone can rebuild the same file. I didn't benchmark across many tenants, and the README says so.

## 17. Ship as one binary

One binary does the CLI, the server and the worker. Releases are built with goreleaser for Linux and macOS on amd64 and arm64, and an install script puts the binary in `~/.local/bin` after checking its SHA-256. No Rust toolchain needed to try it.
