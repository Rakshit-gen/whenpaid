//! Money as integers in each currency's smallest unit. No floats anywhere.
//!
//! Every division goes through `div_round` with an explicit [`Rounding`], and
//! every conversion keeps the exact rational result next to the rounded one,
//! so the leftover fraction is recorded instead of disappearing.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// ISO 4217 code. Decimal places come from the standard: JPY has 0, KWD has 3.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Currency([u8; 3]);

impl Currency {
    pub const USD: Currency = Currency(*b"USD");
    pub const EUR: Currency = Currency(*b"EUR");
    pub const GBP: Currency = Currency(*b"GBP");
    pub const INR: Currency = Currency(*b"INR");

    /// The wallets payouts are made from (rule BAL-WALLETS).
    pub const PAYOUT_WALLETS: [Currency; 3] = [Currency::USD, Currency::GBP, Currency::EUR];

    pub fn parse(s: &str) -> Result<Currency, String> {
        let b = s.as_bytes();
        if b.len() != 3 || !b.iter().all(u8::is_ascii_uppercase) {
            return Err(format!("{s:?} is not a three-letter currency code"));
        }
        Ok(Currency([b[0], b[1], b[2]]))
    }

    pub fn code(&self) -> &str {
        std::str::from_utf8(&self.0).expect("validated ascii")
    }

    /// Digits after the decimal point in the smallest unit, per ISO 4217.
    pub fn exponent(self) -> u32 {
        match &self.0 {
            b"BIF" | b"CLP" | b"DJF" | b"GNF" | b"ISK" | b"JPY" | b"KMF" | b"KRW" | b"PYG"
            | b"RWF" | b"UGX" | b"VND" | b"VUV" | b"XAF" | b"XOF" | b"XPF" => 0,
            b"BHD" | b"IQD" | b"JOD" | b"KWD" | b"LYD" | b"OMR" | b"TND" => 3,
            _ => 2,
        }
    }

    pub fn is_payout_wallet(self) -> bool {
        Currency::PAYOUT_WALLETS.contains(&self)
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}
impl fmt::Debug for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}
impl Serialize for Currency {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.code())
    }
}
impl<'de> Deserialize<'de> for Currency {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Currency::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// An amount tagged with its currency. Adding two currencies is an error, not a sum.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Money {
    pub minor: i64,
    pub currency: Currency,
}

impl Money {
    pub fn new(minor: i64, currency: Currency) -> Money {
        Money { minor, currency }
    }
    pub fn zero(currency: Currency) -> Money {
        Money { minor: 0, currency }
    }
    pub fn checked_add(self, o: Money) -> Result<Money, String> {
        if self.currency != o.currency {
            return Err(format!("can't add {} to {}", o.currency, self.currency));
        }
        let minor = self
            .minor
            .checked_add(o.minor)
            .ok_or_else(|| format!("{} overflowed", self.currency))?;
        Ok(Money {
            minor,
            currency: self.currency,
        })
    }
    pub fn checked_sub(self, o: Money) -> Result<Money, String> {
        self.checked_add(o.checked_neg()?)
    }
    pub fn checked_neg(self) -> Result<Money, String> {
        let minor = self.minor.checked_neg().ok_or("amount overflowed")?;
        Ok(Money {
            minor,
            currency: self.currency,
        })
    }
}

/// Sum amounts that must all be in `cur`.
pub fn sum<I: IntoIterator<Item = Money>>(cur: Currency, it: I) -> Result<Money, String> {
    it.into_iter()
        .try_fold(Money::zero(cur), Money::checked_add)
}

impl fmt::Display for Money {
    /// "-1,234.56 USD", "1,000 JPY", "12.345 KWD"
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let e = self.currency.exponent();
        let sign = if self.minor < 0 { "-" } else { "" };
        let abs = self.minor.unsigned_abs();
        let scale = 10u64.pow(e);
        let whole = (abs / scale).to_string();
        let mut grouped = String::with_capacity(whole.len() + whole.len() / 3);
        for (i, c) in whole.chars().enumerate() {
            if i > 0 && (whole.len() - i).is_multiple_of(3) {
                grouped.push(',');
            }
            grouped.push(c);
        }
        if e == 0 {
            write!(f, "{sign}{grouped} {}", self.currency)
        } else {
            write!(
                f,
                "{sign}{grouped}.{:0w$} {}",
                abs % scale,
                self.currency,
                w = e as usize
            )
        }
    }
}
impl fmt::Debug for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// How a fraction of the smallest unit is resolved. The choice is recorded in every trace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rounding {
    /// Banker's rounding: exact halves go to the even neighbour, so many rows don't drift upward.
    #[default]
    HalfEven,
    /// Exact halves go away from zero.
    HalfUp,
    /// Drop the fraction.
    TowardZero,
}

/// `n / d` rounded. `d` must be positive.
pub fn div_round(n: i128, d: i128, mode: Rounding) -> i128 {
    assert!(d > 0, "divisor must be positive");
    let q = n.div_euclid(d); // floor
    let r = n.rem_euclid(d); // 0 <= r < d
    if r == 0 {
        return q;
    }
    let twice = 2 * r;
    match mode {
        Rounding::TowardZero => q + i128::from(n < 0),
        Rounding::HalfUp => q + i128::from(twice > d || (twice == d && n > 0)),
        Rounding::HalfEven => q + i128::from(twice > d || (twice == d && q.rem_euclid(2) == 1)),
    }
}

fn gcd(mut a: i128, mut b: i128) -> i128 {
    a = a.abs();
    b = b.abs();
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

/// An exact fraction of a smallest unit, kept so rounding is auditable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fraction {
    pub num: i128,
    pub den: i128,
}

impl Fraction {
    pub fn new(num: i128, den: i128) -> Fraction {
        let g = gcd(num, den);
        Fraction {
            num: num / g,
            den: den / g,
        }
    }
    pub fn is_zero(&self) -> bool {
        self.num == 0
    }
}

impl fmt::Display for Fraction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{}/{}", self.num, self.den)
        }
    }
}
impl Serialize for Fraction {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

/// A decimal rate kept as an exact fraction. "1.0832" is 677/625.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rate {
    pub num: i128,
    pub den: i128,
}

impl Rate {
    pub const ONE: Rate = Rate { num: 1, den: 1 };

    /// Up to 12 decimal places, strictly positive.
    pub fn parse(s: &str) -> Result<Rate, String> {
        let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
        let digits = |x: &str| x.bytes().all(|c| c.is_ascii_digit());
        if whole.is_empty()
            || whole.len() > 12
            || frac.len() > 12
            || !digits(whole)
            || !digits(frac)
        {
            return Err(format!(
                "bad rate {s:?}: want a positive decimal with at most 12 places"
            ));
        }
        let num: i128 = format!("{whole}{frac}")
            .parse()
            .map_err(|_| format!("bad rate {s:?}"))?;
        if num == 0 {
            return Err(format!("rate {s:?} is zero"));
        }
        let den = 10i128.pow(frac.len() as u32);
        let g = gcd(num, den);
        Ok(Rate {
            num: num / g,
            den: den / g,
        })
    }

    /// `a / b`, both quoted against the same base currency.
    pub fn cross(a: Rate, b: Rate) -> Rate {
        let (num, den) = (a.num * b.den, a.den * b.num);
        let g = gcd(num, den);
        Rate {
            num: num / g,
            den: den / g,
        }
    }
}

impl fmt::Display for Rate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{}/{}", self.num, self.den)
        }
    }
}

/// One conversion with everything needed to redo it by hand.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Conversion {
    pub from: Money,
    pub to: Money,
    /// Units of `to` per unit of `from`, as an exact fraction.
    pub rate: String,
    pub rate_date: chrono::NaiveDate,
    /// Exact result in `to`'s smallest unit before rounding.
    pub exact: Fraction,
    /// `exact - to`, in `to`'s smallest unit: what rounding dropped (positive) or added (negative).
    pub residual: Fraction,
    pub rounding: Rounding,
}

/// Convert `m` at `rate` (units of `to` per unit of `m.currency`).
pub fn convert(
    m: Money,
    to: Currency,
    rate: Rate,
    rate_date: chrono::NaiveDate,
    mode: Rounding,
) -> Result<Conversion, String> {
    let (ef, et) = (m.currency.exponent(), to.exponent());
    let num = (m.minor as i128)
        .checked_mul(rate.num)
        .and_then(|x| x.checked_mul(10i128.pow(et)))
        .ok_or("conversion overflowed")?;
    let den = rate
        .den
        .checked_mul(10i128.pow(ef))
        .ok_or("conversion overflowed")?;
    let rounded = div_round(num, den, mode);
    let minor = i64::try_from(rounded).map_err(|_| "conversion overflowed")?;
    Ok(Conversion {
        from: m,
        to: Money::new(minor, to),
        rate: rate.to_string(),
        rate_date,
        exact: Fraction::new(num, den),
        residual: Fraction::new(num - rounded * den, den),
        rounding: mode,
    })
}

/// `m * bps / 10_000` rounded, with the residual. 150 bps is 1.5%.
pub fn apply_bps(m: Money, bps: u32, mode: Rounding) -> Result<(Money, Fraction), String> {
    let num = (m.minor as i128) * bps as i128;
    let r = div_round(num, 10_000, mode);
    let minor = i64::try_from(r).map_err(|_| "fee overflowed")?;
    Ok((
        Money::new(minor, m.currency),
        Fraction::new(num - r * 10_000, 10_000),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn c(s: &str) -> Currency {
        Currency::parse(s).unwrap()
    }
    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 1).unwrap()
    }

    #[test]
    fn rounding_modes() {
        use Rounding::*;
        for (n, d, he, hu, tz) in [
            (5, 2, 2, 3, 2),
            (7, 2, 4, 4, 3),
            (-5, 2, -2, -3, -2),
            (-7, 2, -4, -4, -3),
            (10, 3, 3, 3, 3),
            (-10, 3, -3, -3, -3),
            (11, 3, 4, 4, 3),
        ] {
            assert_eq!(div_round(n, d, HalfEven), he, "{n}/{d} half even");
            assert_eq!(div_round(n, d, HalfUp), hu, "{n}/{d} half up");
            assert_eq!(div_round(n, d, TowardZero), tz, "{n}/{d} toward zero");
        }
    }

    #[test]
    fn exponents_follow_iso() {
        assert_eq!(c("USD").exponent(), 2);
        assert_eq!(c("JPY").exponent(), 0);
        assert_eq!(c("KWD").exponent(), 3);
        assert_eq!(Money::new(123456, c("USD")).to_string(), "1,234.56 USD");
        assert_eq!(Money::new(1000, c("JPY")).to_string(), "1,000 JPY");
        assert_eq!(Money::new(-12345, c("KWD")).to_string(), "-12.345 KWD");
        assert!(Currency::parse("usd").is_err());
    }

    #[test]
    fn adding_two_currencies_is_an_error() {
        assert!(
            Money::new(1, Currency::USD)
                .checked_add(Money::new(1, Currency::EUR))
                .is_err()
        );
        assert!(
            Money::new(i64::MAX, Currency::USD)
                .checked_add(Money::new(1, Currency::USD))
                .is_err()
        );
    }

    #[test]
    fn conversion_across_exponents_records_the_residual() {
        // 1,000 JPY at 0.006712 USD per JPY = 671.2 cents -> 671, residual 1/5 of a cent.
        let x = convert(
            Money::new(1000, c("JPY")),
            Currency::USD,
            Rate::parse("0.006712").unwrap(),
            day(),
            Rounding::HalfEven,
        )
        .unwrap();
        assert_eq!(x.to, Money::new(671, Currency::USD));
        assert_eq!(x.residual, Fraction::new(1, 5));
        // 10.00 USD at 0.307 KWD per USD = 3.070 KWD exactly.
        let y = convert(
            Money::new(1000, Currency::USD),
            c("KWD"),
            Rate::parse("0.307").unwrap(),
            day(),
            Rounding::HalfEven,
        )
        .unwrap();
        assert_eq!(y.to, Money::new(3070, c("KWD")));
        assert!(y.residual.is_zero());
    }

    #[test]
    fn credits_round_to_the_cent_not_the_dollar() {
        // A 21.98 credit applied to a 20.00 charge leaves 1.98, never 1.00 or 2.00.
        let credit = Money::new(2198, Currency::USD);
        let charge = Money::new(2000, Currency::USD);
        assert_eq!(
            credit.checked_sub(charge).unwrap(),
            Money::new(198, Currency::USD)
        );
    }

    #[test]
    fn cross_rates_are_exact() {
        let eur = Rate::parse("1.08").unwrap();
        let gbp = Rate::parse("1.35").unwrap();
        assert_eq!(Rate::cross(eur, gbp), Rate { num: 4, den: 5 });
        assert!(Rate::parse("0").is_err());
        assert!(Rate::parse("-1").is_err());
        assert!(Rate::parse("1.0000000000001").is_err());
    }

    #[test]
    fn bps() {
        let (fee, r) =
            apply_bps(Money::new(23000, Currency::USD), 150, Rounding::HalfEven).unwrap();
        assert_eq!(fee, Money::new(345, Currency::USD));
        assert!(r.is_zero());
        let (fee, r) = apply_bps(Money::new(333, Currency::USD), 150, Rounding::HalfEven).unwrap();
        assert_eq!((fee.minor, r), (5, Fraction::new(-1, 200)));
    }
}
