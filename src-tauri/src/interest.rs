//! 利息计算:存款、理财估算、贷款。全是纯函数,不碰数据库,也不内置任何银行利率。
//!
//! 约定:金额内部用整数"分",中间用 f64 求解,出口四舍五入到分;利率在函数里是小数
//! (3.5% = 0.035),在工具参数里是百分数。每个结果都回显所用的公式和口径假设。
//! 各家银行的天数基准、舍入和靠档规则不同,所以结果只是估算。
use crate::agent::{num, text, tool, yuan};
use crate::core::{Error, Result};
use serde_json::{json, Value};

const MAX_PERIODS: u32 = 600;
const MAX_DAYS: u32 = 36_600;
const MAX_MONEY_YUAN: f64 = 1e10;
/// 超过这个期数不返回完整还款表,只返回汇总与首末期
const SCHEDULE_MAX: u32 = 36;

fn cents(x: f64) -> i64 {
    x.round() as i64
}

// ---------- 存款与理财 ----------

/// 活期:单利,按实际天数。`basis` 为一年的天数基准(360 或 365)。
pub fn demand_interest(principal: i64, rate: f64, days: u32, basis: u32) -> i64 {
    cents(principal as f64 * rate / basis as f64 * days as f64)
}

/// 整存整取/大额存单:单利,到期一次付息。
pub fn fixed_interest(principal: i64, rate: f64, months: u32) -> i64 {
    cents(principal as f64 * rate * months as f64 / 12.0)
}

/// 零存整取:每月存入 `monthly`,共 `months` 个月,利息 = M × 年利率 ÷ 12 × n(n+1) ÷ 2。
pub fn installment_savings_interest(monthly: i64, rate: f64, months: u32) -> i64 {
    let n = months as f64;
    cents(monthly as f64 * rate / 12.0 * n * (n + 1.0) / 2.0)
}

/// 货币基金/理财估算:按日复利再投资。
pub fn money_fund_gain(principal: i64, annual: f64, days: u32) -> i64 {
    cents(principal as f64 * ((1.0 + annual / 365.0).powi(days as i32) - 1.0))
}

// ---------- 贷款 ----------

fn annuity_payment(p: f64, r: f64, n: u32) -> f64 {
    if r == 0.0 {
        p / n as f64
    } else {
        p * r / (1.0 - (1.0 + r).powi(-(n as i32)))
    }
}

/// 还了 k 期之后的剩余本金
fn balance_after(p: f64, r: f64, pay: f64, k: u32) -> f64 {
    if r == 0.0 {
        p - pay * k as f64
    } else {
        let g = (1.0 + r).powi(k as i32);
        p * g - pay * (g - 1.0) / r
    }
}

#[derive(Debug, PartialEq)]
pub struct Period {
    pub payment: i64,
    pub principal: i64,
    pub interest: i64,
    pub balance: i64,
}

#[derive(Debug)]
pub struct EqualPayment {
    pub payment: i64,
    pub total_interest: i64,
    pub first: Period,
    pub last: Period,
    /// 期数不超过 `SCHEDULE_MAX` 时给出完整还款表
    pub schedule: Vec<Period>,
}

pub fn equal_payment(p: i64, rate: f64, n: u32) -> EqualPayment {
    let (pf, r) = (p as f64, rate / 12.0);
    let pay = annuity_payment(pf, r, n);
    let pay_c = cents(pay);
    let row = |k: u32| {
        // 第 k 期(从 1 起):期初余额 → 利息、本金、期末余额
        let before = balance_after(pf, r, pay, k - 1);
        let interest = before * r;
        let after = if k == n { 0.0 } else { (before - (pay - interest)).max(0.0) };
        Period { payment: pay_c, principal: cents(before - after), interest: cents(interest), balance: cents(after) }
    };
    EqualPayment {
        payment: pay_c,
        total_interest: pay_c * n as i64 - p,
        first: row(1),
        last: row(n),
        schedule: if n <= SCHEDULE_MAX { (1..=n).map(row).collect() } else { Vec::new() },
    }
}

#[derive(Debug)]
pub struct EqualPrincipal {
    pub monthly_principal: i64,
    pub first_payment: i64,
    pub last_payment: i64,
    pub step: i64,
    pub total_interest: i64,
}

pub fn equal_principal(p: i64, rate: f64, n: u32) -> EqualPrincipal {
    let (pf, r, nf) = (p as f64, rate / 12.0, n as f64);
    let unit = pf / nf;
    EqualPrincipal {
        monthly_principal: cents(unit),
        first_payment: cents(unit + pf * r),
        last_payment: cents(unit + unit * r),
        step: cents(unit * r),
        total_interest: cents(pf * r * (nf + 1.0) / 2.0),
    }
}

#[derive(Debug)]
pub struct Irr {
    pub monthly: f64,
    pub nominal_annual: f64,
    pub effective_annual: f64,
}

/// 本金 `p`,分 `n` 期,每期还 `pay`(分):解 P = A·(1 − (1+i)^−n) ÷ i 得月内部收益率。
pub fn irr(p: i64, n: u32, pay: i64) -> Result<Irr> {
    let (pf, a) = (p as f64, pay as f64);
    let total = a * n as f64;
    if total < pf {
        return Err(Error::Invalid("每期还款合计小于本金,算不出利率:请检查还款额".into()));
    }
    let monthly = if (total - pf).abs() < 0.5 {
        0.0
    } else {
        let f = |i: f64| a * (1.0 - (1.0 + i).powi(-(n as i32))) / i - pf;
        let (mut lo, mut hi) = (1e-12, 10.0);
        for _ in 0..200 {
            let mid = (lo + hi) / 2.0;
            if f(mid) > 0.0 {
                lo = mid;
            } else {
                hi = mid;
            }
            if hi - lo < 1e-13 {
                break;
            }
        }
        (lo + hi) / 2.0
    };
    Ok(Irr { monthly, nominal_annual: monthly * 12.0, effective_annual: (1.0 + monthly).powi(12) - 1.0 })
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PrepayMode {
    /// 月供不变,缩短期限
    Shorten,
    /// 期限不变,减少月供
    Reduce,
}

#[derive(Debug)]
pub struct Prepayment {
    pub balance_before: i64,
    pub balance_after: i64,
    pub original_total_interest: i64,
    pub new_total_interest: i64,
    pub saved_interest: i64,
    pub new_payment: i64,
    pub remaining_months: u32,
    pub original_remaining_months: u32,
}

/// 逐月推演,直到还清;返回(月数, 利息合计)。月供不足以覆盖利息时返回 None。
fn simulate(mut bal: f64, r: f64, pay: f64) -> Option<(u32, f64)> {
    let (mut months, mut interest) = (0u32, 0.0);
    while bal > 0.5 {
        let i = bal * r;
        if pay <= i && r > 0.0 {
            return None;
        }
        months += 1;
        interest += i;
        if bal + i <= pay + 1e-6 {
            break;
        }
        bal = bal + i - pay;
        if months > 5000 {
            return None;
        }
    }
    Some((months, interest))
}

/// 等额本息贷款,在第 `k` 期还款后一次性提前还本金 `x`。
pub fn prepayment(p: i64, rate: f64, n: u32, k: u32, x: i64, mode: PrepayMode) -> Result<Prepayment> {
    if k >= n {
        return Err(Error::Invalid(format!("提前还款发生在第几期之后:应在 0 到 {} 之间", n - 1)));
    }
    let (pf, r) = (p as f64, rate / 12.0);
    let pay = annuity_payment(pf, r, n);
    let before = balance_after(pf, r, pay, k).max(0.0);
    if (x as f64) > before + 0.5 {
        return Err(Error::Invalid(format!("提前还款额不能超过当时的剩余本金 ¥{:.2}", before / 100.0)));
    }
    let after = (before - x as f64).max(0.0);
    let paid_interest = pay * k as f64 - (pf - before);
    let original = pay * n as f64 - pf;
    let remaining = n - k;
    let (new_pay, new_months, rest_interest) = match mode {
        PrepayMode::Shorten => {
            let (m, i) = simulate(after, r, pay).ok_or_else(|| Error::Invalid("月供不足以覆盖利息".into()))?;
            (pay, m, i)
        }
        PrepayMode::Reduce if after <= 0.5 => (0.0, 0, 0.0),
        PrepayMode::Reduce => {
            let np = annuity_payment(after, r, remaining);
            (np, remaining, np * remaining as f64 - after)
        }
    };
    let new_total = paid_interest + rest_interest;
    Ok(Prepayment {
        balance_before: cents(before),
        balance_after: (cents(before) - x).max(0),
        original_total_interest: cents(original),
        new_total_interest: cents(new_total),
        saved_interest: cents(original) - cents(new_total),
        new_payment: cents(new_pay),
        remaining_months: new_months,
        original_remaining_months: remaining,
    })
}

// ---------- 助手工具 ----------

pub const TOOL_NAME: &str = "calculate_interest";

pub fn tool_defs() -> Vec<Value> {
    vec![tool(
        TOOL_NAME,
        "计算银行存款、理财和贷款的利息。金额单位元,利率单位 % (3.5 表示 3.5%)。type 取值:demand_deposit 活期;fixed_deposit 整存整取定期;cd 大额存单;installment_savings 零存整取;money_fund_estimate 货币基金/理财收益估算;loan_equal_payment 等额本息;loan_equal_principal 等额本金;effective_rate 分期的实际年化;prepayment 提前还款节省的利息。凡是涉及利息、月供、总利息的数字都必须用本工具算,结果是估算,以银行实际结算为准。",
        json!({
            "type": {"type":"string","enum":["demand_deposit","fixed_deposit","cd","installment_savings","money_fund_estimate","loan_equal_payment","loan_equal_principal","effective_rate","prepayment"]},
            "principal": {"type":"number","description":"本金/贷款额,元(零存整取不用)"},
            "monthly_amount": {"type":"number","description":"零存整取每月存入额,元"},
            "rate": {"type":"number","description":"年利率,%,如 3.5。effective_rate 不需要"},
            "days": {"type":"integer","description":"活期、货币基金的天数"},
            "basis": {"type":"integer","enum":[360,365],"description":"活期的天数基准,默认 360"},
            "months": {"type":"integer","description":"定期、大额存单、零存整取的月数;贷款的期数(月)"},
            "years": {"type":"number","description":"贷款年数,与 months 二选一"},
            "payout": {"type":"string","enum":["maturity","monthly","yearly"],"description":"大额存单的付息方式,默认 maturity 到期一次付息"},
            "installments": {"type":"integer","description":"effective_rate:分期期数"},
            "fee_rate": {"type":"number","description":"effective_rate:每期手续费率,%"},
            "payment": {"type":"number","description":"effective_rate:每期还款额,元(与 fee_rate 二选一)"},
            "prepay_after": {"type":"integer","description":"prepayment:在第几期还款之后提前还,0 表示还款开始前"},
            "prepay_amount": {"type":"number","description":"prepayment:提前还的本金,元"},
            "prepay_mode": {"type":"string","enum":["shorten","reduce"],"description":"shorten 缩短期限(月供不变),reduce 减少月供(期限不变)"}
        }),
        &["type"],
    )]
}

/// 不是利息工具时返回 None
pub fn run(name: &str, args: &Value) -> Option<Result<Value>> {
    (name == TOOL_NAME).then(|| calculate(args))
}

fn field(args: &Value, key: &str, hint: &str) -> Result<f64> {
    match args.get(key) {
        None | Some(Value::Null) => Err(Error::Invalid(format!("缺少 {key}({hint})"))),
        Some(_) => num(args, key).filter(|v| v.is_finite()).ok_or_else(|| Error::Invalid(format!("{key} 必须是数字({hint})"))),
    }
}

fn money(args: &Value, key: &str) -> Result<i64> {
    let v = field(args, key, "金额,单位元")?;
    if v <= 0.0 || v > MAX_MONEY_YUAN {
        return Err(Error::Invalid(format!("{key} 必须是大于 0 的金额(单位元)")));
    }
    Ok(cents(v * 100.0))
}

fn rate(args: &Value) -> Result<f64> {
    let v = field(args, "rate", "年利率,单位 %,如 3.5")?;
    if !(0.0..=100.0).contains(&v) {
        return Err(Error::Invalid("rate 必须在 0 到 100 之间(单位 %)".into()));
    }
    Ok(v / 100.0)
}

fn whole(args: &Value, key: &str, min: u32, max: u32, hint: &str) -> Result<u32> {
    let v = field(args, key, hint)?;
    if v.fract() != 0.0 || v < min as f64 || v > max as f64 {
        return Err(Error::Invalid(format!("{key} 必须是 {min} 到 {max} 之间的整数")));
    }
    Ok(v as u32)
}

/// 贷款期数:months 或 years
fn loan_periods(args: &Value) -> Result<u32> {
    if args.get("months").is_some_and(|v| !v.is_null()) {
        return whole(args, "months", 1, MAX_PERIODS, "贷款期数,月");
    }
    if args.get("years").is_some_and(|v| !v.is_null()) {
        let y = field(args, "years", "贷款年数")?;
        let m = y * 12.0;
        if m.fract() != 0.0 || m < 1.0 || m > MAX_PERIODS as f64 {
            return Err(Error::Invalid(format!("years 折算后的期数必须是 1 到 {MAX_PERIODS} 之间的整数月")));
        }
        return Ok(m as u32);
    }
    Err(Error::Invalid("缺少 months 或 years(贷款期限)".into()))
}

fn period_json(p: &Period) -> Value {
    json!({"payment": yuan(p.payment), "principal": yuan(p.principal), "interest": yuan(p.interest), "balance": yuan(p.balance)})
}

fn pct2(x: f64) -> f64 {
    (x * 10000.0).round() / 100.0
}

fn out(kind: &str, result: Value, formula: &str, assumptions: &[&str]) -> Value {
    json!({"type": kind, "result": result, "formula": formula, "assumptions": assumptions, "note": "估算结果,以银行实际结算为准"})
}

pub fn calculate(args: &Value) -> Result<Value> {
    let kind = text(args, "type");
    match kind {
        "demand_deposit" => {
            let (p, r) = (money(args, "principal")?, rate(args)?);
            let days = whole(args, "days", 0, MAX_DAYS, "天数")?;
            let basis = match args.get("basis").and_then(Value::as_u64) {
                None | Some(360) => 360,
                Some(365) => 365,
                Some(_) => return Err(Error::Invalid("basis 只能是 360 或 365".into())),
            };
            let i = demand_interest(p, r, days, basis);
            Ok(out(kind, json!({"interest": yuan(i), "total": yuan(p + i), "days": days}),
                "利息 = 本金 × 年利率 ÷ 天数基准 × 实际天数",
                &[&format!("天数基准 {basis}"), "单利,未考虑利率调整与结息日复利"]))
        }
        "fixed_deposit" | "cd" => {
            let (p, r) = (money(args, "principal")?, rate(args)?);
            let months = whole(args, "months", 1, MAX_PERIODS, "存期,月")?;
            let i = fixed_interest(p, r, months);
            let mut result = json!({"interest": yuan(i), "maturity_amount": yuan(p + i), "months": months});
            let payout = match text(args, "payout") {
                "" => "maturity",
                x => x,
            };
            if kind == "cd" && payout != "maturity" {
                let every = match payout {
                    "monthly" => 1,
                    "yearly" => 12,
                    _ => return Err(Error::Invalid("payout 只能是 maturity、monthly 或 yearly".into())),
                };
                let count = months.div_ceil(every);
                let per = cents(p as f64 * r * every as f64 / 12.0);
                // 最后一期可能不满一个周期:用总额减去前面各期,保证合计与到期一次付息相同
                let last = i - per * (count as i64 - 1);
                result["payout_each"] = json!(yuan(per));
                result["payout_count"] = json!(count);
                result["payout_last"] = json!(yuan(last));
            }
            Ok(out(kind, result, "利息 = 本金 × 年利率 × 月数 ÷ 12", &["单利", "按存入时的约定利率计算,提前支取、靠档等规则未考虑"]))
        }
        "installment_savings" => {
            let (m, r) = (money(args, "monthly_amount")?, rate(args)?);
            let months = whole(args, "months", 1, MAX_PERIODS, "存期,月")?;
            let i = installment_savings_interest(m, r, months);
            Ok(out(kind, json!({"interest": yuan(i), "maturity_amount": yuan(m * months as i64 + i), "deposited": yuan(m * months as i64)}),
                "利息 = 每月存入额 × 年利率 ÷ 12 × n(n+1) ÷ 2", &["每月固定日存入,不漏存", "单利"]))
        }
        "money_fund_estimate" => {
            let (p, r) = (money(args, "principal")?, rate(args)?);
            let days = whole(args, "days", 0, MAX_DAYS, "持有天数")?;
            let g = money_fund_gain(p, r, days);
            Ok(out(kind, json!({"estimated_gain": yuan(g), "estimated_total": yuan(p + g), "days": days}),
                "收益 = 本金 × ((1 + 年化 ÷ 365)^天数 − 1)",
                &["按日复利再投资", "年化收益率由你输入,不代表未来收益", "估算,非承诺"]))
        }
        "loan_equal_payment" => {
            let (p, r) = (money(args, "principal")?, rate(args)?);
            let n = loan_periods(args)?;
            let e = equal_payment(p, r, n);
            let mut result = json!({
                "periods": n, "monthly_payment": yuan(e.payment), "total_interest": yuan(e.total_interest),
                "total_paid": yuan(e.payment * n as i64), "first_period": period_json(&e.first), "last_period": period_json(&e.last),
            });
            if !e.schedule.is_empty() {
                result["schedule"] = json!(e.schedule.iter().enumerate().map(|(i, p)| {
                    let mut v = period_json(p);
                    v["period"] = json!(i + 1);
                    v
                }).collect::<Vec<_>>());
            }
            Ok(out(kind, result, "月供 = P·r ÷ (1 − (1+r)^−n),r = 年利率 ÷ 12;总利息 = 月供 × n − P",
                &["每月等额还款", "月供四舍五入到分,总利息按取整后的月供计算", "完整还款表仅在期数不超过 36 时返回"]))
        }
        "loan_equal_principal" => {
            let (p, r) = (money(args, "principal")?, rate(args)?);
            let n = loan_periods(args)?;
            let e = equal_principal(p, r, n);
            Ok(out(kind, json!({
                "periods": n, "monthly_principal": yuan(e.monthly_principal), "first_payment": yuan(e.first_payment),
                "last_payment": yuan(e.last_payment), "monthly_decrease": yuan(e.step), "total_interest": yuan(e.total_interest),
                "total_paid": yuan(p + e.total_interest),
            }), "每月还本 = P ÷ n,当月利息 = 剩余本金 × r;总利息 = P × r × (n+1) ÷ 2", &["每月还本金额固定,月供逐月递减"]))
        }
        "effective_rate" => {
            let p = money(args, "principal")?;
            let n = whole(args, "installments", 1, MAX_PERIODS, "分期期数")?;
            let pay = if args.get("payment").is_some_and(|v| !v.is_null()) {
                money(args, "payment")?
            } else if args.get("fee_rate").is_some_and(|v| !v.is_null()) {
                let f = field(args, "fee_rate", "每期手续费率,单位 %")?;
                if !(0.0..=100.0).contains(&f) {
                    return Err(Error::Invalid("fee_rate 必须在 0 到 100 之间(单位 %)".into()));
                }
                cents(p as f64 / n as f64 + p as f64 * f / 100.0)
            } else {
                return Err(Error::Invalid("缺少 payment(每期还款额)或 fee_rate(每期手续费率)".into()));
            };
            let i = irr(p, n, pay)?;
            Ok(out(kind, json!({
                "payment_each": yuan(pay), "total_paid": yuan(pay * n as i64), "total_cost": yuan(pay * n as i64 - p),
                "monthly_rate_pct": (i.monthly * 1e6).round() / 1e4, "nominal_annual_pct": pct2(i.nominal_annual), "effective_annual_pct": pct2(i.effective_annual),
            }), "解 P = A·(1 − (1+i)^−n) ÷ i 得月内部收益率 i;名义年化 = i × 12,实际年化 = (1+i)^12 − 1",
                &["每期等额还款,期末还款", "手续费折算后的实际成本通常远高于标称费率"]))
        }
        "prepayment" => {
            let (p, r) = (money(args, "principal")?, rate(args)?);
            let n = loan_periods(args)?;
            let k = whole(args, "prepay_after", 0, MAX_PERIODS, "在第几期还款之后提前还")?;
            let x = if num(args, "prepay_amount") == Some(0.0) { 0 } else { money(args, "prepay_amount")? };
            let mode = match text(args, "prepay_mode") {
                "" | "shorten" => PrepayMode::Shorten,
                "reduce" => PrepayMode::Reduce,
                _ => return Err(Error::Invalid("prepay_mode 只能是 shorten 或 reduce".into())),
            };
            let e = prepayment(p, r, n, k, x, mode)?;
            Ok(out(kind, json!({
                "balance_before": yuan(e.balance_before), "balance_after": yuan(e.balance_after),
                "original_total_interest": yuan(e.original_total_interest), "new_total_interest": yuan(e.new_total_interest),
                "saved_interest": yuan(e.saved_interest), "new_monthly_payment": yuan(e.new_payment),
                "original_remaining_months": e.original_remaining_months, "new_remaining_months": e.remaining_months,
                "mode": if mode == PrepayMode::Shorten { "shorten" } else { "reduce" },
            }), "节省的利息 = 原总利息 − 新总利息(含已付部分)",
                &["原贷款为等额本息", "提前还款当期不收违约金", "shorten 月供不变、缩短期限;reduce 期限不变、重算月供"]))
        }
        "" => Err(Error::Invalid("缺少 type".into())),
        other => Err(Error::Invalid(format!("未知的 type:{other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const Y: i64 = 100; // 1 元 = 100 分

    fn call(v: Value) -> Value {
        calculate(&v).unwrap_or_else(|e| panic!("计算失败:{e}"))
    }

    fn err(v: Value) -> String {
        calculate(&v).expect_err("应当报错").to_string()
    }

    fn res(v: &Value, key: &str) -> f64 {
        v["result"][key].as_f64().unwrap_or_else(|| panic!("缺少 {key}:{v}"))
    }

    #[test]
    fn demand_deposit_uses_the_chosen_day_basis() {
        assert_eq!(demand_interest(100_000 * Y, 0.002, 90, 360), 50_00);
        assert_eq!(demand_interest(100_000 * Y, 0.002, 90, 365), 49_32);
        assert_eq!(demand_interest(100_000 * Y, 0.002, 0, 360), 0);
        let v = call(json!({"type":"demand_deposit","principal":100000,"rate":0.2,"days":90}));
        assert_eq!(res(&v, "interest"), 50.0);
        assert!(v["assumptions"][0].as_str().unwrap().contains("360"));
        let v = call(json!({"type":"demand_deposit","principal":100000,"rate":0.2,"days":90,"basis":365}));
        assert_eq!(res(&v, "interest"), 49.32);
    }

    #[test]
    fn fixed_deposit_and_cd_are_simple_interest() {
        assert_eq!(fixed_interest(100_000 * Y, 0.03, 12), 3_000 * Y);
        assert_eq!(fixed_interest(100_000 * Y, 0.02, 6), 1_000 * Y);
        let v = call(json!({"type":"fixed_deposit","principal":100000,"rate":3,"months":12}));
        assert_eq!((res(&v, "interest"), res(&v, "maturity_amount")), (3000.0, 103_000.0));

        let v = call(json!({"type":"cd","principal":200000,"rate":3.6,"months":12,"payout":"monthly"}));
        assert_eq!((res(&v, "payout_each"), v["result"]["payout_count"].as_i64(), res(&v, "interest")), (600.0, Some(12), 7200.0));
        // 付息合计与到期一次付息相同,即使最后一期不满一个周期
        let v = call(json!({"type":"cd","principal":100000,"rate":3,"months":18,"payout":"yearly"}));
        let total = res(&v, "payout_each") * (v["result"]["payout_count"].as_f64().unwrap() - 1.0) + res(&v, "payout_last");
        assert!((total - res(&v, "interest")).abs() < 0.005, "{total}");
        assert!(err(json!({"type":"cd","principal":1,"rate":3,"months":12,"payout":"weekly"})).contains("payout"));
    }

    #[test]
    fn installment_savings_matches_the_textbook_formula() {
        assert_eq!(installment_savings_interest(1_000 * Y, 0.015, 12), 97_50);
        let v = call(json!({"type":"installment_savings","monthly_amount":1000,"rate":1.5,"months":12}));
        assert_eq!((res(&v, "interest"), res(&v, "maturity_amount")), (97.5, 12_097.5));
    }

    #[test]
    fn money_fund_compounds_daily_and_is_labelled_an_estimate() {
        let g = money_fund_gain(100_000 * Y, 0.025, 365);
        assert!((253_100..=253_200).contains(&g), "{g}");
        assert_eq!(money_fund_gain(100_000 * Y, 0.0, 365), 0);
        let v = call(json!({"type":"money_fund_estimate","principal":100000,"rate":2.5,"days":365}));
        assert!(v["assumptions"].to_string().contains("非承诺"));
    }

    #[test]
    fn equal_payment_mortgage_matches_known_figures() {
        let e = equal_payment(1_000_000 * Y, 0.035, 360);
        assert_eq!(e.payment, 4490_45);
        assert_eq!(e.total_interest, 616_562 * Y);
        assert_eq!((e.first.interest, e.first.principal), (2916_67, 1573_78));
        assert_eq!(e.last.balance, 0);
        assert!(e.last.interest < 20 * Y, "最后一期利息很少:{}", e.last.interest);
        assert!(e.schedule.is_empty(), "超过 36 期不返回完整还款表");
    }

    #[test]
    fn equal_payment_edge_cases() {
        let z = equal_payment(12_000 * Y, 0.0, 12);
        assert_eq!((z.payment, z.total_interest), (1_000 * Y, 0));
        // 短期限给出完整还款表,本金之和等于贷款额(舍入误差在 1 分/期以内)
        let s = equal_payment(10_000 * Y, 0.06, 12);
        assert_eq!(s.schedule.len(), 12);
        let principal: i64 = s.schedule.iter().map(|p| p.principal).sum();
        assert!((principal - 10_000 * Y).abs() <= 12, "{principal}");
        assert_eq!(s.schedule[11].balance, 0);
        let v = call(json!({"type":"loan_equal_payment","principal":1000000,"rate":3.5,"years":30}));
        assert_eq!((res(&v, "monthly_payment"), res(&v, "total_interest")), (4490.45, 616_562.0));
        assert!(v["result"].get("schedule").is_none());
        assert!(err(json!({"type":"loan_equal_payment","principal":1000,"rate":3,"months":0})).contains("months"));
        assert!(err(json!({"type":"loan_equal_payment","principal":1000,"rate":3,"months":601})).contains("months"));
        assert!(err(json!({"type":"loan_equal_payment","principal":1000,"rate":3})).contains("months 或 years"));
    }

    #[test]
    fn equal_principal_mortgage_matches_known_figures_and_beats_equal_payment() {
        let e = equal_principal(1_000_000 * Y, 0.035, 360);
        assert_eq!(e.first_payment, 5694_44);
        assert_eq!(e.last_payment, 2785_88);
        assert_eq!(e.step, 8_10);
        assert_eq!(e.total_interest, 526_458_33);
        assert!(e.total_interest < equal_payment(1_000_000 * Y, 0.035, 360).total_interest);
    }

    #[test]
    fn effective_rate_reveals_the_real_cost_of_fee_installments() {
        let v = call(json!({"type":"effective_rate","principal":10000,"installments":12,"fee_rate":0.6}));
        assert_eq!(res(&v, "payment_each"), 893.33);
        assert!((res(&v, "nominal_annual_pct") - 13.0).abs() <= 0.1, "{v}");
        assert!((res(&v, "effective_annual_pct") - 13.8).abs() <= 0.1, "{v}");
        // 与等额本息互验:用月供反解利率应得回原利率
        let e = equal_payment(100_000 * Y, 0.12, 24);
        let back = irr(100_000 * Y, 24, e.payment).unwrap();
        assert!((back.nominal_annual - 0.12).abs() < 1e-4, "{back:?}");
        // 无息分期
        let z = call(json!({"type":"effective_rate","principal":1200,"installments":12,"payment":100}));
        assert_eq!((res(&z, "nominal_annual_pct"), res(&z, "effective_annual_pct")), (0.0, 0.0));
        // 还款不足本金
        assert!(err(json!({"type":"effective_rate","principal":1200,"installments":12,"payment":99})).contains("小于本金"));
        assert!(err(json!({"type":"effective_rate","principal":1200,"installments":12})).contains("payment"));
    }

    #[test]
    fn prepayment_shorten_keeps_the_payment_and_cuts_the_term() {
        let e = prepayment(1_000_000 * Y, 0.035, 360, 60, 200_000 * Y, PrepayMode::Shorten).unwrap();
        assert_eq!(e.new_payment, 4490_45);
        assert!(e.remaining_months < 300 && e.remaining_months > 200, "{}", e.remaining_months);
        assert!(e.saved_interest > 0);
        assert_eq!(e.balance_before - e.balance_after, 200_000 * Y);
        assert_eq!(e.new_total_interest, e.original_total_interest - e.saved_interest);
    }

    #[test]
    fn prepayment_reduce_keeps_the_term_and_lowers_the_payment() {
        let e = prepayment(1_000_000 * Y, 0.035, 360, 60, 200_000 * Y, PrepayMode::Reduce).unwrap();
        assert_eq!(e.remaining_months, 300);
        assert!(e.new_payment < 4490_45);
        assert!(e.saved_interest > 0);
        // 缩短期限省的利息比减少月供省得多
        let s = prepayment(1_000_000 * Y, 0.035, 360, 60, 200_000 * Y, PrepayMode::Shorten).unwrap();
        assert!(s.saved_interest > e.saved_interest);
    }

    #[test]
    fn prepayment_boundaries() {
        for mode in [PrepayMode::Shorten, PrepayMode::Reduce] {
            let e = prepayment(500_000 * Y, 0.04, 240, 12, 0, mode).unwrap();
            assert_eq!(e.saved_interest, 0, "{mode:?}");
            assert_eq!(e.remaining_months, 228);
        }
        let full = prepayment(500_000 * Y, 0.04, 240, 12, 10_000_000 * Y, PrepayMode::Shorten);
        assert!(full.unwrap_err().to_string().contains("剩余本金"));
        // 恰好还清
        let bal = prepayment(500_000 * Y, 0.04, 240, 12, 0, PrepayMode::Shorten).unwrap().balance_before;
        let all = prepayment(500_000 * Y, 0.04, 240, 12, bal, PrepayMode::Reduce).unwrap();
        assert_eq!((all.balance_after, all.remaining_months, all.new_payment), (0, 0, 0));
        assert!(prepayment(500_000 * Y, 0.04, 240, 240, 1, PrepayMode::Shorten).is_err());
        let v = call(json!({"type":"prepayment","principal":1000000,"rate":3.5,"months":360,"prepay_after":60,"prepay_amount":0}));
        assert_eq!(res(&v, "saved_interest"), 0.0);
    }

    #[test]
    fn inputs_are_validated_with_actionable_errors() {
        assert!(err(json!({"type":"fixed_deposit","principal":1000,"months":12})).contains("rate"));
        assert!(err(json!({"type":"fixed_deposit","principal":1000,"rate":120,"months":12})).contains("rate"));
        assert!(err(json!({"type":"fixed_deposit","principal":-5,"rate":3,"months":12})).contains("principal"));
        assert!(err(json!({"type":"fixed_deposit","principal":"很多","rate":3,"months":12})).contains("principal"));
        assert!(err(json!({"type":"demand_deposit","principal":1,"rate":1,"days":5,"basis":300})).contains("basis"));
        assert!(err(json!({"type":"nope"})).contains("未知"));
        assert!(err(json!({})).contains("type"));
        // 数字写成字符串也能用
        let v = call(json!({"type":"fixed_deposit","principal":"100000","rate":"3","months":"12"}));
        assert_eq!(res(&v, "interest"), 3000.0);
    }

    #[test]
    fn tool_is_registered_and_dispatches() {
        let defs = tool_defs();
        assert_eq!(defs[0]["function"]["name"], TOOL_NAME);
        assert!(run("list_accounts", &json!({})).is_none());
        let v = run(TOOL_NAME, &json!({"type":"fixed_deposit","principal":100000,"rate":3,"months":12})).unwrap().unwrap();
        assert_eq!(v["result"]["interest"], 3000.0);
    }
}
