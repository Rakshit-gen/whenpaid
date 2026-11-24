# Scenarios

Each scenario in `tests/scenarios.rs` started as a payout question merchants asked in public, mostly on review sites and X. I kept the shape of the problem and changed everything else: names, amounts, dates and ids. None of these is a real account, and I don't link to the posts.

For each one I've split three things apart:

- **What was reported.** What the merchant said happened. I can't check it, so it stays an allegation.
- **What the docs say.** The rule that applies, with its id.
- **What whenpaid does.** Including anything that's my own call.

## S1. Smaller payout after a chargeback early in the period

- **Reported:** the payout was much smaller than the sales in the period, with no explanation.
- **Docs:** a dispute takes the disputed amount and a dispute fee from the wallet (BAL-TYPES, BAL-DISPUTE-FEE). The fee isn't returned if the merchant wins.
- **whenpaid:** lists the held amount and the fee as separate lines, each with its ledger row. The test works the payable out by hand: 300.00 − 12.40 − 300.00 − 30.00 + 200.00 − 8.40 = 149.20 USD.

## S2. Payouts stopped, no reason, no end date

- **Reported:** payouts stopped and support gave no reason or timeline.
- **Docs:** verification and compliance holds can pause payouts (VP-GATES). The docs don't promise a reason or a date.
- **whenpaid:** the run is blocked, the reason reads "not disclosed" and the end date "unknown" (DC-UNKNOWN). It counts the days since the payout was first due, which comes to 36 in the test.

## S3. A long hold that eventually ends

- **Reported:** money held through a long dispute window, then released.
- **Docs:** reserves and holds sit outside the payable balance until released (PS-RESERVE).
- **whenpaid:** the blocked runs keep their original due dates. Once the hold ends, everything owed goes out on the next eligible run, and nothing is counted twice.

## S4. A payout that looked late

- **Reported:** the payout arrived a day after the scheduled date.
- **Docs:** if the date is a weekend or US bank holiday, the payout is created the next business day (PS-HOLIDAY, FED-CALENDAR).
- **whenpaid:** names the holiday (MLK Day in the test) and gives the date it moved to.

## S5. Account suspended, customers refunded

- **Reported:** after a suspension, the merchant expected their balance to be paid out.
- **Docs:** refunds come out of the wallet (BAL-TYPES). A suspended account doesn't get payouts (VP-GATES).
- **whenpaid:** the refunds push the wallet negative, the run shows nothing to pay, and the blocker is listed separately from the money.

## S6. A balance that looked rounded down

- **Reported:** a credit balance of 21.98 USD, after 20.00 USD was used, showed as 1.00 instead of 1.98.
- **Docs:** each wallet keeps its own ledger, and every row moves the balance by its exact amount (BAL-WALLETS, BAL-CHAIN). Nothing in the docs mentions whole-dollar rounding.
- **whenpaid:** works in integer cents end to end, so the test's balance closes at exactly 1.98 USD. whenpaid can't say why a dashboard showed 1.00. It can only show what the ledger adds up to.

## S7. A preview mixing currencies

- **Reported:** an expected payout figure combined EUR and USD without saying which rate it used.
- **Docs:** unlinked EUR and GBP balances are converted to the primary currency at payout (PS-CONVERSION). The rate shown before payout is indicative (PS-FX-INDICATIVE).
- **whenpaid:** prints the rate, its date and the exact unrounded value, and marks the figure as an estimate. The test checks the leftover from rounding is recorded.

## S8. "Success", then the money came back

- **Reported:** the payout showed success, but the bank never got it, and later it failed.
- **Docs:** success means sent, not received (PS-SUCCESS-IS-SENT). A failed payout is credited back to the wallet (PS-FAILED). Failed can arrive after success (WH-PAYOUT-ORDER).
- **whenpaid:** settles the status by event timestamp, shows the reversal, and moves the amount into the next run.

## S9. A correction after the period closed

- **Reported:** an adjustment didn't show up in the payout it seemed to belong to.
- **Docs:** each run counts entries up to the end of its period (PS-CYCLE).
- **whenpaid:** puts the late correction in the next run and says why.

## S10. Two reserve holds at once

- **Reported:** confusion over how much was still held after a partial release.
- **Docs:** reserve holds and releases are their own ledger rows (PS-RESERVE).
- **whenpaid:** tracks each hold, and only the released one comes back.

## S11. A payout fee off the published schedule

- **Reported:** a fee was taken that the merchant didn't expect.
- **Docs:** the fee is 5 USD only when the payout is under 1,000 USD (PS-PAYOUT-FEE).
- **whenpaid:** when a fee doesn't match, it reports the expected and actual fee. It never corrects the number.
