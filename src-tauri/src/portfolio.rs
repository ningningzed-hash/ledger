//! 组合建议所需的数据:账户年利率、持仓、候选产品清单、风险问卷。
//!
//! 约定:金额为整数"分",年利率为基点(3.5% = 350)。持仓可以只记金额(手动更新),也可以记份额 +
//! 行情来源,由 `market` 按最新价自动算出金额。候选清单只是参考,不记渠道,也不存费率等会过期的数据。
//! 目标配比与建议的计算在 `advice.rs`。
//!
//! 按份额估值的持仓还可以记成本价(证券交易币种的原币价格)。盈亏只看股价本身的涨跌,
//! 用最近一次估值的原币现价和人民币金额推算,不另存汇率,见 [`pnl_of`]。每天留一份
//! 盈亏快照(`holding_snapshot`),用来算今年以来、某个月的盈亏。
//!
//! 登记了持仓的投资账户,余额跟随持仓合计:持仓增、改、删或估值刷新后由 [`holdings_changed`]
//! 写入当天的估值。没登记持仓的投资账户仍按手动估值。
use crate::core::{self, Error, Result};
use mysql::prelude::Queryable;
use mysql::TxOpts;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 资产大类(键, 中文名)。`other` 不参与目标配比,只在占比里单列。
pub const CLASSES: [(&str, &str); 6] = [
    ("us_equity_index", "美股宽基"),
    ("other_equity", "非美股票"),
    ("bond", "债券"),
    ("gold", "黄金"),
    ("cash", "现金类"),
    ("other", "其他"),
];

pub fn class_label(key: &str) -> &'static str {
    CLASSES.iter().find(|(k, _)| *k == key).map_or("其他", |(_, l)| l)
}

fn check_class(key: &str) -> Result<()> {
    if CLASSES.iter().any(|(k, _)| *k == key) {
        Ok(())
    } else {
        let names: Vec<&str> = CLASSES.iter().map(|(_, l)| *l).collect();
        Err(Error::Invalid(format!("资产大类不在范围内:{key}(可选:{})", names.join("、"))))
    }
}

const MAX_AMOUNT: i64 = 100_000_000_000_000; // 1 万亿元(分)

/// 名称/代码:去掉首尾空白,按字符数限制长度
fn clean(label: &str, s: &str, max: usize, required: bool) -> Result<Option<String>> {
    let t = s.trim();
    if t.is_empty() {
        return if required { Err(Error::Invalid(format!("{label}不能为空"))) } else { Ok(None) };
    }
    if t.chars().count() > max {
        return Err(Error::Invalid(format!("{label}最长 {max} 个字")));
    }
    Ok(Some(t.to_string()))
}

// ---------- 账户年利率 ----------

/// 房产不登记利率;其余四类可以
const RATE_KINDS: [&str; 4] = ["cash", "deposit", "investment", "liability"];

fn account_kind(c: &mut impl Queryable, id: i64) -> Result<String> {
    c.exec_first("SELECT kind FROM account WHERE id=? AND archived=0", (id,))?
        .ok_or_else(|| Error::Invalid("找不到这个账户".into()))
}

/// `rate_bps` 为空表示清除
pub fn set_account_rate(c: &mut impl Queryable, account_id: i64, rate_bps: Option<i64>) -> Result<()> {
    let kind = account_kind(c, account_id)?;
    if !RATE_KINDS.contains(&kind.as_str()) {
        return Err(Error::Invalid("这类账户不支持登记年利率(房产不适用)".into()));
    }
    match rate_bps {
        None => c.exec_drop("DELETE FROM account_rate WHERE account_id=?", (account_id,))?,
        Some(b) if !(0..=10_000).contains(&b) => {
            return Err(Error::Invalid("年利率需在 0% 到 100% 之间".into()));
        }
        Some(b) => c.exec_drop(
            "INSERT INTO account_rate (account_id, rate_bps) VALUES (?,?) AS new ON DUPLICATE KEY UPDATE rate_bps=new.rate_bps",
            (account_id, b),
        )?,
    }
    Ok(())
}

pub fn account_rates(c: &mut impl Queryable) -> Result<HashMap<i64, i64>> {
    Ok(c.query_map("SELECT account_id, rate_bps FROM account_rate", |(a, r): (i64, i64)| (a, r))?.into_iter().collect())
}

/// 按年利率估算的每月利息(分)= 余额 × 年利率 ÷ 12,四舍五入
pub fn monthly_interest(balance: i64, rate_bps: i64) -> i64 {
    (balance * rate_bps * 2 + 120_000) / 240_000
}

#[derive(Serialize, Debug, PartialEq)]
pub struct AccountInterest {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub balance: i64,
    pub rate_bps: Option<i64>,
    /// 存款类为收入,负债类为支出;未登记利率时为空
    pub monthly_interest: Option<i64>,
}

#[derive(Serialize, Debug)]
pub struct InterestOverview {
    pub accounts: Vec<AccountInterest>,
    /// 存款、现金、投资类账户的每月预计利息合计
    pub monthly_income: i64,
    /// 负债类账户的每月预计利息合计
    pub monthly_cost: i64,
    pub unregistered: Vec<String>,
}

/// 账户利息概览:按当前余额的粗略估算,实际以银行结算为准
pub fn interest_overview(c: &mut impl Queryable) -> Result<InterestOverview> {
    let rates = account_rates(c)?;
    let (mut income, mut cost) = (0, 0);
    let (mut accounts, mut unregistered) = (Vec::new(), Vec::new());
    for a in core::list_accounts(c)? {
        if !RATE_KINDS.contains(&a.kind.as_str()) {
            continue;
        }
        let rate = rates.get(&a.id).copied();
        let monthly = rate.map(|r| monthly_interest(a.balance, r));
        match (monthly, a.kind.as_str()) {
            (Some(m), "liability") => cost += m,
            (Some(m), _) => income += m,
            (None, _) => unregistered.push(a.name.clone()),
        }
        accounts.push(AccountInterest { id: a.id, name: a.name, kind: a.kind, balance: a.balance, rate_bps: rate, monthly_interest: monthly });
    }
    Ok(InterestOverview { accounts, monthly_income: income, monthly_cost: cost, unregistered })
}

// ---------- 持仓 ----------

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct HoldingInput {
    pub account_id: i64,
    pub name: String,
    pub code: Option<String>,
    pub asset_class: String,
    /// 分,大于 0。自动估值的持仓由 [`crate::market::value_holding`] 按份额和最新价算出
    pub amount: i64,
    /// 份额(股数),最多 4 位小数;为空表示手动记金额
    #[serde(default)]
    pub shares: Option<f64>,
    /// 行情来源:exchange 场内(股票、ETF)| fund 场外基金。只在填了份额时有用
    #[serde(default)]
    pub source: Option<String>,
    /// 查到的行情 secid,由后端填写,不接受前端传入
    #[serde(skip_deserializing)]
    pub secid: Option<String>,
    /// 成本价,证券交易币种的原币价格(VOO 填美元)。只有按份额估值的持仓可以填
    #[serde(default)]
    pub cost_price: Option<f64>,
    /// 查到的原币现价,由后端填写
    #[serde(skip_deserializing)]
    pub price: Option<f64>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Holding {
    pub id: i64,
    pub account_id: i64,
    pub account: String,
    pub name: String,
    pub code: Option<String>,
    pub asset_class: String,
    pub amount: i64,
    pub updated_on: String,
    pub shares: Option<f64>,
    /// 有值表示金额按行情自动更新
    pub secid: Option<String>,
    /// 成本价与最近一次估值的现价(原币)
    pub cost_price: Option<f64>,
    pub price: Option<f64>,
    /// 原币币种:CNY | HKD | USD;手动金额的持仓为空
    pub currency: Option<String>,
    /// 以下三项只在能算出盈亏时有值:成本(分,折合人民币)、浮动盈亏(分)、收益率(百分数,两位小数)
    pub cost: Option<i64>,
    pub pnl: Option<i64>,
    pub pnl_pct: Option<f64>,
}

/// 份额在库里存为 份额 × 10000 的整数
pub const SHARE_SCALE: f64 = 10_000.0;
const MAX_SHARES: f64 = 1e11;

struct Checked {
    name: String,
    code: Option<String>,
    shares: Option<i64>,
    cost_price: Option<f64>,
    price: Option<f64>,
}

const MAX_PRICE: f64 = 1e9;

/// 份额 → 库里的整数;不合法时报错
pub fn share_units(shares: f64) -> Result<i64> {
    let units = (shares * SHARE_SCALE).round();
    if !shares.is_finite() || units < 1.0 || shares > MAX_SHARES {
        return Err(Error::Invalid("份额必须大于 0(最多 4 位小数)".into()));
    }
    Ok(units as i64)
}

fn check_holding(c: &mut impl Queryable, input: &HoldingInput) -> Result<Checked> {
    let name = clean("名称", &input.name, 100, true)?.unwrap_or_default();
    let code = clean("代码", input.code.as_deref().unwrap_or(""), 32, false)?;
    check_class(&input.asset_class)?;
    if input.amount <= 0 || input.amount > MAX_AMOUNT {
        return Err(Error::Invalid("金额必须大于 0".into()));
    }
    if account_kind(c, input.account_id)? != "investment" {
        return Err(Error::Invalid("持仓只能登记在投资类账户下".into()));
    }
    let shares = match (input.shares, &input.secid) {
        (None, _) => None,
        (Some(s), Some(_)) => Some(share_units(s)?),
        (Some(_), None) => return Err(Error::Invalid("按份额自动估值需要先查到行情".into())),
    };
    let cost_price = match input.cost_price {
        None => None,
        Some(_) if shares.is_none() => return Err(Error::Invalid("手动填金额的持仓不能填成本价".into())),
        Some(p) if p.is_finite() && p > 0.0 && p < MAX_PRICE => Some(p),
        Some(_) => return Err(Error::Invalid("成本价必须大于 0".into())),
    };
    let price = shares.and(input.price).filter(|p| p.is_finite() && *p > 0.0);
    Ok(Checked { name, code, shares, cost_price, price })
}

pub fn add_holding(c: &mut impl Queryable, input: &HoldingInput, today: &str) -> Result<i64> {
    let ck = check_holding(c, input)?;
    c.exec_drop(
        "INSERT INTO holding (account_id, name, code, asset_class, amount, updated_on, shares, secid, cost_price, price) VALUES (?,?,?,?,?,?,?,?,?,?)",
        (input.account_id, &ck.name, &ck.code, &input.asset_class, input.amount, today, ck.shares, ck.shares.and(input.secid.as_ref()), ck.cost_price, ck.price),
    )?;
    let id = c.query_first("SELECT LAST_INSERT_ID()")?.unwrap_or(0);
    holdings_changed(c, today)?;
    Ok(id)
}

pub fn update_holding(c: &mut impl Queryable, id: i64, input: &HoldingInput, today: &str) -> Result<()> {
    let exists: Option<i64> = c.exec_first("SELECT id FROM holding WHERE id=?", (id,))?;
    if exists.is_none() {
        return Err(Error::Invalid("找不到这条持仓".into()));
    }
    let ck = check_holding(c, input)?;
    c.exec_drop(
        "UPDATE holding SET account_id=?, name=?, code=?, asset_class=?, amount=?, updated_on=?, shares=?, secid=?, cost_price=?, price=? WHERE id=?",
        (input.account_id, &ck.name, &ck.code, &input.asset_class, input.amount, today, ck.shares, ck.shares.and(input.secid.as_ref()), ck.cost_price, ck.price, id),
    )?;
    holdings_changed(c, today)
}

/// 自动估值的持仓:(id, 名称, secid, 份额 × 10000)
pub fn auto_holdings(c: &mut impl Queryable) -> Result<Vec<(i64, String, String, i64)>> {
    Ok(c.query("SELECT id, name, secid, shares FROM holding WHERE secid IS NOT NULL AND shares IS NOT NULL ORDER BY id")?)
}

/// 按最新价写入金额(分)和原币现价
pub fn set_holding_amount(c: &mut impl Queryable, id: i64, amount: i64, price: f64, today: &str) -> Result<()> {
    if amount <= 0 || amount > MAX_AMOUNT {
        return Err(Error::Invalid("算出的金额不在合理范围内".into()));
    }
    let price = Some(price).filter(|p| p.is_finite() && *p > 0.0);
    c.exec_drop("UPDATE holding SET amount=?, price=COALESCE(?, price), updated_on=? WHERE id=?", (amount, price, today, id))?;
    Ok(())
}

/// 有份额但还没有原币现价的持仓(升级后还没刷新过):有的话估值刷新不受每小时一次的限制
pub fn missing_prices(c: &mut impl Queryable) -> Result<bool> {
    let one: Option<i64> = c.query_first("SELECT 1 FROM holding WHERE secid IS NOT NULL AND shares IS NOT NULL AND price IS NULL LIMIT 1")?;
    Ok(one.is_some())
}

pub fn delete_holding(c: &mut impl Queryable, id: i64, today: &str) -> Result<()> {
    c.exec_drop("DELETE FROM holding WHERE id=?", (id,))?;
    holdings_changed(c, today)
}

/// 持仓有变化之后:同步投资账户余额,并更新当天的盈亏快照
pub fn holdings_changed(c: &mut impl Queryable, today: &str) -> Result<()> {
    sync_account_balances(c, today)?;
    snapshot_today(c, today)
}

/// 登记了持仓的投资账户,余额改成持仓合计(写当天的估值);余额已经相等的不写。
/// 账户里最后一条持仓被删掉时不动余额,由用户自己决定。返回改动了的账户数
pub fn sync_account_balances(c: &mut impl Queryable, today: &str) -> Result<usize> {
    let sums: Vec<(i64, i64)> = c.query(
        "SELECT h.account_id, CAST(SUM(h.amount) AS SIGNED) FROM holding h JOIN account a ON a.id = h.account_id
         WHERE a.kind = 'investment' AND a.archived = 0 GROUP BY h.account_id",
    )?;
    if sums.is_empty() {
        return Ok(0);
    }
    let balances: HashMap<i64, i64> = core::list_accounts(c)?.into_iter().map(|a| (a.id, a.balance)).collect();
    let mut changed = 0;
    for (id, total) in sums {
        if balances.get(&id) != Some(&total) {
            core::set_valuation(c, id, today, total)?;
            changed += 1;
        }
    }
    Ok(changed)
}

/// (成本(分), 盈亏(分), 收益率 %)。amount 是按 price 估出的人民币金额,所以
/// 盈亏 = amount × (1 − 成本价 / 现价),等于 (现价 − 成本价) × 份额 × 当时的汇率
pub fn pnl_of(amount: i64, cost_price: Option<f64>, price: Option<f64>) -> Option<(i64, i64, f64)> {
    let (cp, p) = (cost_price.filter(|v| *v > 0.0)?, price.filter(|v| *v > 0.0)?);
    let pnl = (amount as f64 * (1.0 - cp / p)).round() as i64;
    let pct = ((p / cp - 1.0) * 10_000.0).round() / 100.0;
    Some((amount - pnl, pnl, pct))
}

pub fn list_holdings(c: &mut impl Queryable) -> Result<Vec<Holding>> {
    Ok(c.query_map(
        "SELECT h.id, h.account_id, a.name, h.name, h.code, h.asset_class, h.amount, h.updated_on, h.shares, h.secid, h.cost_price, h.price
         FROM holding h JOIN account a ON a.id=h.account_id ORDER BY h.id",
        |row: mysql::Row| {
            let (id, account_id, account, name, code, asset_class, amount, updated_on, shares, secid, cost_price, price): (
                i64, i64, String, String, Option<String>, String, i64, String, Option<i64>, Option<String>, Option<f64>, Option<f64>,
            ) = mysql::from_row(row);
            let calc = pnl_of(amount, cost_price, price);
            Holding {
                id, account_id, account, name, code, asset_class, amount, updated_on,
                shares: shares.map(|u| u as f64 / SHARE_SCALE),
                currency: secid.as_deref().map(|s| crate::market::currency_of(s).0.to_string()),
                secid, cost_price, price,
                cost: calc.map(|x| x.0),
                pnl: calc.map(|x| x.1),
                pnl_pct: calc.map(|x| x.2),
            }
        },
    )?)
}

// ---------- 盈亏汇总与快照 ----------

#[derive(Serialize, Debug, PartialEq, Clone)]
pub struct PeriodPnl {
    /// 区间盈亏(分)
    pub pnl: i64,
    /// 起算快照的日期
    pub since: String,
    /// 起算快照早于区间开始(数据覆盖整个区间);为假表示快照不够早,只能从 since 开始算
    pub full: bool,
}

#[derive(Serialize, Debug, PartialEq, Clone)]
pub struct PnlSummary {
    /// 能算出盈亏的持仓:总成本、总市值、总盈亏(分),整体收益率(百分数)
    pub cost: i64,
    pub value: i64,
    pub pnl: i64,
    pub pnl_pct: Option<f64>,
    pub counted: usize,
    /// 没算进来的持仓数(手动金额、没填成本价、还没有现价)
    pub excluded: usize,
    /// 今年以来盈亏;没有快照时为空
    pub ytd: Option<PeriodPnl>,
}

/// (总市值, 总成本):只算能算出盈亏的持仓
fn totals(list: &[Holding]) -> (i64, i64) {
    list.iter().filter_map(|h| Some((h.amount, h.cost?))).fold((0, 0), |(v, c), (a, b)| (v + a, c + b))
}

pub fn pct_of(pnl: i64, cost: i64) -> Option<f64> {
    (cost > 0).then(|| (pnl as f64 / cost as f64 * 10_000.0).round() / 100.0)
}

/// 写入(覆盖)当天的快照
pub fn snapshot_today(c: &mut impl Queryable, today: &str) -> Result<()> {
    let (value, cost) = totals(&list_holdings(c)?);
    c.exec_drop(
        "INSERT INTO holding_snapshot (date, value, cost) VALUES (?, ?, ?) AS new ON DUPLICATE KEY UPDATE value=new.value, cost=new.cost",
        (today, value, cost),
    )?;
    Ok(())
}

/// 区间 [start, end] 的盈亏。end 在今天及以后时用实时数据 live,否则用 end 当天及之前最后一份快照。
/// 起点是 start 之前最后一份快照;没有就用区间内最早的一份。
pub fn period_pnl(c: &mut impl Queryable, start: &str, end: &str, today: &str, live: (i64, i64)) -> Result<Option<PeriodPnl>> {
    let (ev, ec) = if end >= today {
        live
    } else {
        match c.exec_first::<(i64, i64), _, _>("SELECT value, cost FROM holding_snapshot WHERE date <= ? ORDER BY date DESC LIMIT 1", (end,))? {
            Some(x) => x,
            None => return Ok(None),
        }
    };
    let before: Option<(String, i64, i64)> = c.exec_first("SELECT date, value, cost FROM holding_snapshot WHERE date < ? ORDER BY date DESC LIMIT 1", (start,))?;
    let base = match before {
        Some(b) => Some((b, true)),
        None => c
            .exec_first::<(String, i64, i64), _, _>("SELECT date, value, cost FROM holding_snapshot WHERE date >= ? AND date <= ? ORDER BY date LIMIT 1", (start, end.min(today)))?
            .map(|b| (b, false)),
    };
    Ok(base.map(|((since, bv, bc), full)| PeriodPnl { pnl: (ev - ec) - (bv - bc), since, full }))
}

pub fn pnl_summary(c: &mut impl Queryable, today: &str) -> Result<PnlSummary> {
    let list = list_holdings(c)?;
    let counted = list.iter().filter(|h| h.cost.is_some()).count();
    let (value, cost) = totals(&list);
    let year_start = format!("{}-01-01", &today[..4]);
    let ytd = period_pnl(c, &year_start, today, today, (value, cost))?;
    Ok(PnlSummary { cost, value, pnl: value - cost, pnl_pct: pct_of(value - cost, cost), counted, excluded: list.len() - counted, ytd })
}

// ---------- 候选产品清单 ----------

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProductInput {
    pub name: String,
    pub code: Option<String>,
    pub asset_class: String,
    pub note: String,
}

#[derive(Serialize, Debug, PartialEq, Clone)]
pub struct Product {
    pub id: i64,
    pub name: String,
    pub code: Option<String>,
    pub asset_class: String,
    pub note: String,
    pub builtin: bool,
}

struct CheckedProduct {
    name: String,
    code: Option<String>,
    note: String,
}

fn check_product(c: &mut impl Queryable, input: &ProductInput, self_id: Option<i64>) -> Result<CheckedProduct> {
    let name = clean("名称", &input.name, 100, true)?.unwrap_or_default();
    let code = clean("代码", input.code.as_deref().unwrap_or(""), 32, false)?;
    let note = clean("说明", &input.note, 500, false)?.unwrap_or_default();
    check_class(&input.asset_class)?;
    let dup: Option<i64> = c.exec_first("SELECT id FROM product WHERE name=?", (&name,))?;
    if dup.is_some_and(|d| Some(d) != self_id) {
        return Err(Error::Invalid(format!("清单里已经有“{name}”")));
    }
    Ok(CheckedProduct { name, code, note })
}

pub fn add_product(c: &mut impl Queryable, input: &ProductInput) -> Result<i64> {
    let ck = check_product(c, input, None)?;
    c.exec_drop(
        "INSERT INTO product (name, code, asset_class, note, builtin) VALUES (?,?,?,?,0)",
        (&ck.name, &ck.code, &input.asset_class, &ck.note),
    )?;
    Ok(c.query_first("SELECT LAST_INSERT_ID()")?.unwrap_or(0))
}

pub fn update_product(c: &mut impl Queryable, id: i64, input: &ProductInput) -> Result<()> {
    let exists: Option<i64> = c.exec_first("SELECT id FROM product WHERE id=?", (id,))?;
    if exists.is_none() {
        return Err(Error::Invalid("找不到这个产品".into()));
    }
    let ck = check_product(c, input, Some(id))?;
    c.exec_drop(
        "UPDATE product SET name=?, code=?, asset_class=?, note=? WHERE id=?",
        (&ck.name, &ck.code, &input.asset_class, &ck.note, id),
    )?;
    Ok(())
}

pub fn delete_product(c: &mut impl Queryable, id: i64) -> Result<()> {
    c.exec_drop("DELETE FROM product WHERE id=?", (id,))?;
    Ok(())
}

pub fn list_products(c: &mut impl Queryable) -> Result<Vec<Product>> {
    Ok(c.query_map(
        "SELECT id, name, code, asset_class, note, builtin FROM product ORDER BY id",
        |(id, name, code, asset_class, note, builtin): (i64, String, Option<String>, String, String, i64)| Product {
            id, name, code, asset_class, note, builtin: builtin != 0,
        },
    )?)
}

const SEED_NOTE: &str = "示例,仅供参考,以发行方披露为准。";

/// 首版内置示例:(名称, 代码, 大类, 简介)。不含费率、规模、收益率等会过期的数据。
const SEEDS: [(&str, &str, &str, &str); 8] = [
    ("Vanguard 标普500 ETF", "VOO", "us_equity_index", "跟踪标普 500 指数,覆盖美国大型股。"),
    ("Vanguard 美国整体股市 ETF", "VTI", "us_equity_index", "跟踪美国整体股票市场,含大中小盘。"),
    ("Vanguard 美国以外全球股票 ETF", "VXUS", "other_equity", "跟踪美国以外的发达与新兴市场股票。"),
    ("Vanguard 美国整体债券市场 ETF", "BND", "bond", "跟踪美国投资级债券整体市场。"),
    ("SPDR 黄金 ETF", "GLD", "gold", "以实物黄金为依托的黄金 ETF。"),
    ("iShares 黄金信托 ETF", "IAU", "gold", "以实物黄金为依托的黄金 ETF。"),
    ("iShares 0-3 个月美国国债 ETF", "SGOV", "cash", "持有极短期美国国债,作现金类的替代。"),
    ("货币市场基金 / 银行现金管理类产品", "", "cash", "用于存放应急金与短期资金,可按自己的习惯选择同类产品。"),
];

/// 第二批内置示例:在沪深交易所、港交所上市的 A 股、港股、日本、韩国及国内债券、黄金、货币 ETF。
/// 代码可直接用于持仓的"场内"自动估值。美股上市的 EWJ/EWY 是日韩股票在美股市场的同类选择
const SEEDS_ASIA: [(&str, &str, &str, &str); 14] = [
    ("华泰柏瑞沪深300ETF", "510300", "other_equity", "跟踪沪深 300 指数,覆盖 A 股大盘股,在上交所上市。"),
    ("南方中证500ETF", "510500", "other_equity", "跟踪中证 500 指数,覆盖 A 股中盘股,在上交所上市。"),
    ("华夏恒生ETF", "159920", "other_equity", "跟踪恒生指数,覆盖港股大型股,在深交所上市。"),
    ("盈富基金", "02800", "other_equity", "跟踪恒生指数,在港交所上市,以港币交易。"),
    ("华夏恒生科技ETF", "513180", "other_equity", "跟踪恒生科技指数,覆盖在港上市的科技公司,在上交所上市。"),
    ("华夏日经225ETF", "513520", "other_equity", "跟踪日经 225 指数,覆盖日本大型股,在上交所上市。"),
    ("南方日本东证指数ETF", "513800", "other_equity", "跟踪东证指数,覆盖东京证券交易所的大部分股票,在上交所上市。"),
    ("iShares MSCI 日本 ETF", "EWJ", "other_equity", "跟踪 MSCI 日本指数,在美股市场上市,以美元交易。"),
    ("iShares MSCI 韩国 ETF", "EWY", "other_equity", "跟踪 MSCI 韩国指数,覆盖韩国大中盘股,在美股市场上市,以美元交易。"),
    ("博时标普500ETF", "513500", "us_equity_index", "跟踪标普 500 指数,在上交所上市,以人民币交易。"),
    ("国泰上证5年期国债ETF", "511010", "bond", "持有中期国债,在上交所上市。"),
    ("国泰上证10年期国债ETF", "511260", "bond", "持有 10 年期左右的国债,在上交所上市。"),
    ("华安黄金ETF", "518880", "gold", "以上海黄金交易所的实物黄金为依托,在上交所上市。"),
    ("银华日利ETF", "511880", "cash", "场内货币基金,可作现金类的替代,在上交所上市。"),
];

/// 每批示例各写入一次,用各自的 setting 标记,此后用户删除不会再被补回。
/// 已经在用的账本(第一批已写过)启动时会补上后来新增的批次;与已有产品重名的跳过。
const SEED_BATCHES: [(&str, &[(&str, &str, &str, &str)]); 2] = [("product_seeded", &SEEDS), ("product_seeded_asia", &SEEDS_ASIA)];

/// 返回是否写入了示例
pub fn seed_products(c: &mut impl Queryable) -> Result<bool> {
    let mut wrote = false;
    for (flag, seeds) in SEED_BATCHES {
        let done: Option<String> = c.exec_first("SELECT value FROM setting WHERE name=?", (flag,))?;
        if done.is_some() {
            continue;
        }
        for (name, code, class, intro) in seeds {
            let code = Some(*code).filter(|c| !c.is_empty());
            c.exec_drop(
                "INSERT IGNORE INTO product (name, code, asset_class, note, builtin) VALUES (?,?,?,?,1)",
                (name, code, class, format!("{intro}{SEED_NOTE}")),
            )?;
        }
        c.exec_drop("INSERT INTO setting (name, value) VALUES (?,'1')", (flag,))?;
        wrote = true;
    }
    Ok(wrote)
}

// ---------- 风险问卷 ----------

pub const RISK_LEVELS: [&str; 3] = ["conservative", "balanced", "growth"];

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RiskProfile {
    /// conservative 保守 | balanced 稳健 | growth 进取
    pub level: String,
    /// 能接受的最大回撤,百分比整数 5..=60
    pub max_drawdown: i64,
    /// 投资期限,年 1..=50
    pub horizon_years: i64,
    /// 没有登记年利率的负债里,是否有年利率较高(约 8% 以上)的
    pub high_debt: bool,
}

const RISK_KEYS: [&str; 4] = ["risk_level", "risk_max_drawdown", "risk_horizon_years", "risk_high_debt"];

pub fn save_risk_profile(c: &mut mysql::PooledConn, p: &RiskProfile) -> Result<()> {
    if !RISK_LEVELS.contains(&p.level.as_str()) {
        return Err(Error::Invalid("风险等级只能是 保守、稳健、进取 之一".into()));
    }
    if !(5..=60).contains(&p.max_drawdown) {
        return Err(Error::Invalid("能接受的最大回撤需在 5% 到 60% 之间".into()));
    }
    if !(1..=50).contains(&p.horizon_years) {
        return Err(Error::Invalid("投资期限需在 1 到 50 年之间".into()));
    }
    let values = [p.level.clone(), p.max_drawdown.to_string(), p.horizon_years.to_string(), (p.high_debt as i32).to_string()];
    let mut tx = c.start_transaction(TxOpts::default())?;
    for (k, v) in RISK_KEYS.iter().zip(values) {
        tx.exec_drop("INSERT INTO setting (name, value) VALUES (?,?) AS new ON DUPLICATE KEY UPDATE value=new.value", (k, v))?;
    }
    tx.commit()?;
    Ok(())
}

/// 四项没有全部填写(或值不合法)时返回 None,不用缺省值补全
pub fn get_risk_profile(c: &mut impl Queryable) -> Result<Option<RiskProfile>> {
    let mut vals = Vec::new();
    for k in RISK_KEYS {
        let v: Option<String> = c.exec_first("SELECT value FROM setting WHERE name=?", (k,))?;
        match v {
            Some(v) => vals.push(v),
            None => return Ok(None),
        }
    }
    let (Ok(max_drawdown), Ok(horizon_years)) = (vals[1].parse::<i64>(), vals[2].parse::<i64>()) else { return Ok(None) };
    let p = RiskProfile { level: vals[0].clone(), max_drawdown, horizon_years, high_debt: vals[3] == "1" };
    let ok = RISK_LEVELS.contains(&p.level.as_str()) && (5..=60).contains(&p.max_drawdown) && (1..=50).contains(&p.horizon_years);
    Ok(ok.then_some(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup;
    use crate::db::Conn;
    use crate::testdb::TestDb;
    use serde_json::Value;

    const TODAY: &str = "2026-10-27";

    fn account(c: &mut Conn, name: &str, kind: &str, yuan: i64) -> i64 {
        core::add_account(c, name, kind, "CNY", yuan * 100, "2026-10-01").unwrap()
    }

    fn input(account_id: i64, name: &str, class: &str, yuan: i64) -> HoldingInput {
        HoldingInput { account_id, name: name.into(), code: None, asset_class: class.into(), amount: yuan * 100, ..Default::default() }
    }

    fn msg<T: std::fmt::Debug>(r: Result<T>) -> String {
        r.expect_err("应当失败").to_string()
    }

    fn count(c: &mut Conn, table: &str) -> i64 {
        c.query_first(format!("SELECT COUNT(*) FROM `{table}`")).unwrap().unwrap()
    }

    // ----- 账户年利率 -----

    #[test]
    fn monthly_interest_rounds_to_the_cent() {
        assert_eq!(monthly_interest(100_000_00, 365), 304_17);
        assert_eq!(monthly_interest(200_000_00, 840), 1400_00);
        assert_eq!(monthly_interest(0, 500), 0);
        assert_eq!(monthly_interest(100_000_00, 0), 0);
    }

    #[test]
    fn rate_can_be_set_changed_and_cleared() {
        let t = TestDb::new();
        let mut c = t.conn();
        let a = account(&mut c, "招行储蓄卡", "deposit", 1000);
        set_account_rate(&mut c, a, Some(350)).unwrap();
        assert_eq!(account_rates(&mut c).unwrap()[&a], 350);
        set_account_rate(&mut c, a, Some(0)).unwrap(); // 0% 是合法的
        assert_eq!(account_rates(&mut c).unwrap()[&a], 0);
        set_account_rate(&mut c, a, None).unwrap();
        assert!(account_rates(&mut c).unwrap().is_empty());
    }

    #[test]
    fn rate_rejects_out_of_range_property_and_unknown_accounts() {
        let t = TestDb::new();
        let mut c = t.conn();
        let a = account(&mut c, "存款", "deposit", 1000);
        set_account_rate(&mut c, a, Some(350)).unwrap();
        assert!(msg(set_account_rate(&mut c, a, Some(12_000))).contains("0% 到 100%"));
        assert!(msg(set_account_rate(&mut c, a, Some(-1))).contains("0% 到 100%"));
        assert_eq!(account_rates(&mut c).unwrap()[&a], 350, "失败不改变已有利率");
        let house = account(&mut c, "房子", "property", 1_000_000);
        assert!(msg(set_account_rate(&mut c, house, Some(300))).contains("房产"));
        assert!(msg(set_account_rate(&mut c, 9999, Some(300))).contains("找不到"));
    }

    #[test]
    fn interest_overview_splits_income_cost_and_lists_unregistered() {
        let t = TestDb::new();
        let mut c = t.conn();
        let dep = account(&mut c, "定期", "deposit", 100_000);
        let card = account(&mut c, "信用卡", "liability", 200_000);
        account(&mut c, "活期", "cash", 5_000); // 未登记
        account(&mut c, "房子", "property", 1_000_000); // 不适用
        set_account_rate(&mut c, dep, Some(365)).unwrap();
        set_account_rate(&mut c, card, Some(840)).unwrap();
        let o = interest_overview(&mut c).unwrap();
        assert_eq!((o.monthly_income, o.monthly_cost), (304_17, 1400_00));
        assert_eq!(o.unregistered, ["活期"]);
        assert_eq!(o.accounts.len(), 3, "房产不在概览里");
        assert_eq!(o.accounts.iter().find(|a| a.name == "活期").unwrap().monthly_interest, None);
    }

    #[test]
    fn rate_is_removed_with_its_account() {
        let t = TestDb::new();
        let mut c = t.conn();
        let a = account(&mut c, "存款", "deposit", 1000);
        set_account_rate(&mut c, a, Some(300)).unwrap();
        c.exec_drop("DELETE FROM account WHERE id=?", (a,)).unwrap();
        assert_eq!(count(&mut c, "account_rate"), 0);
    }

    // ----- 持仓 -----

    #[test]
    fn holding_can_be_added_updated_and_deleted() {
        let t = TestDb::new();
        let mut c = t.conn();
        let inv = account(&mut c, "招行投资", "investment", 100_000);
        let id = add_holding(&mut c, &HoldingInput { code: Some(" VOO ".into()), ..input(inv, " 标普500指数基金 ", "us_equity_index", 50_000) }, TODAY).unwrap();
        let h = list_holdings(&mut c).unwrap();
        assert_eq!(h.len(), 1);
        assert_eq!((h[0].name.as_str(), h[0].code.as_deref(), h[0].amount, h[0].updated_on.as_str(), h[0].account.as_str()), ("标普500指数基金", Some("VOO"), 50_000_00, TODAY, "招行投资"));

        update_holding(&mut c, id, &input(inv, "标普500指数基金", "us_equity_index", 60_000), "2026-11-01").unwrap();
        let h = list_holdings(&mut c).unwrap();
        assert_eq!((h[0].amount, h[0].updated_on.as_str(), h[0].code.clone()), (60_000_00, "2026-11-01", None));
        assert!(msg(update_holding(&mut c, 9999, &input(inv, "x", "bond", 1), TODAY)).contains("找不到"));

        delete_holding(&mut c, id, "2026-11-01").unwrap();
        assert!(list_holdings(&mut c).unwrap().is_empty());
    }

    #[test]
    fn holding_validation_rejects_bad_input_without_writing() {
        let t = TestDb::new();
        let mut c = t.conn();
        let inv = account(&mut c, "投资", "investment", 100_000);
        let dep = account(&mut c, "存款", "deposit", 1000);
        let debt = account(&mut c, "房贷", "liability", 1000);
        assert!(msg(add_holding(&mut c, &input(dep, "x", "bond", 100), TODAY)).contains("投资类账户"));
        assert!(msg(add_holding(&mut c, &input(debt, "x", "bond", 100), TODAY)).contains("投资类账户"));
        assert!(msg(add_holding(&mut c, &input(9999, "x", "bond", 100), TODAY)).contains("找不到"));
        assert!(msg(add_holding(&mut c, &input(inv, "x", "bond", 0), TODAY)).contains("大于 0"));
        assert!(msg(add_holding(&mut c, &input(inv, "x", "bond", -5), TODAY)).contains("大于 0"));
        assert!(msg(add_holding(&mut c, &input(inv, "  ", "bond", 100), TODAY)).contains("名称"));
        assert!(msg(add_holding(&mut c, &input(inv, &"长".repeat(101), "bond", 100), TODAY)).contains("100"));
        assert!(msg(add_holding(&mut c, &HoldingInput { code: Some("C".repeat(33)), ..input(inv, "x", "bond", 100) }, TODAY)).contains("代码"));
        assert!(msg(add_holding(&mut c, &input(inv, "x", "股票", 100), TODAY)).contains("资产大类"));
        assert_eq!(count(&mut c, "holding"), 0);
    }

    #[test]
    fn old_holding_tables_get_the_share_columns() {
        let t = TestDb::new();
        let mut c = t.conn();
        let inv = account(&mut c, "投资", "investment", 1000);
        add_holding(&mut c, &input(inv, "旧持仓", "bond", 100), TODAY).unwrap();
        c.query_drop("ALTER TABLE holding DROP COLUMN shares, DROP COLUMN secid, DROP COLUMN cost_price, DROP COLUMN price").unwrap();
        c.query_drop("DROP TABLE holding_snapshot, alert_sent").unwrap();
        crate::db::migrate(&mut c).unwrap();
        crate::db::migrate(&mut c).unwrap(); // 再跑一次不报错
        let h = list_holdings(&mut c).unwrap();
        assert_eq!((h[0].name.as_str(), h[0].shares, h[0].secid.clone()), ("旧持仓", None, None));
        for col in ["cost_price", "price"] {
            let has: Option<i64> = c.exec_first("SELECT 1 FROM information_schema.columns WHERE table_schema=DATABASE() AND table_name='holding' AND column_name=?", (col,)).unwrap();
            assert!(has.is_some(), "补上了 {col}");
        }
        assert_eq!(count(&mut c, "holding_snapshot"), 0);
        assert_eq!(count(&mut c, "alert_sent"), 0);
    }

    /// 按份额估值的持仓(跳过联网,直接给出 secid、现价和人民币金额)
    fn auto(account_id: i64, name: &str, secid: &str, shares: f64, yuan: f64, price: f64, cost_price: Option<f64>) -> HoldingInput {
        HoldingInput {
            account_id, name: name.into(), code: Some(name.into()), asset_class: "us_equity_index".into(),
            amount: (yuan * 100.0).round() as i64, shares: Some(shares), secid: Some(secid.into()), price: Some(price), cost_price,
            ..Default::default()
        }
    }

    fn snap(c: &mut Conn, date: &str, value_yuan: i64, cost_yuan: i64) {
        c.exec_drop("INSERT INTO holding_snapshot (date, value, cost) VALUES (?,?,?)", (date, value_yuan * 100, cost_yuan * 100)).unwrap();
    }

    #[test]
    fn cost_price_is_validated_and_only_for_share_holdings() {
        let t = TestDb::new();
        let mut c = t.conn();
        let inv = account(&mut c, "券商", "investment", 1000);
        let id = add_holding(&mut c, &auto(inv, "VOO", "107.VOO", 10.0, 39600.0, 550.0, Some(500.0)), TODAY).unwrap();
        let h = &list_holdings(&mut c).unwrap()[0];
        assert_eq!((h.cost_price, h.price, h.currency.as_deref()), (Some(500.0), Some(550.0), Some("USD")));

        add_holding(&mut c, &auto(inv, "无成本", "1.510300", 100.0, 400.0, 4.0, None), TODAY).unwrap();
        assert_eq!(list_holdings(&mut c).unwrap()[1].cost_price, None, "成本价可以不填");

        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(msg(add_holding(&mut c, &auto(inv, "x", "1.510300", 1.0, 4.0, 4.0, Some(bad)), TODAY)).contains("成本价必须大于 0"), "{bad}");
        }
        let manual = HoldingInput { cost_price: Some(10.0), ..input(inv, "手动", "bond", 100) };
        assert!(msg(add_holding(&mut c, &manual, TODAY)).contains("手动填金额"));
        assert_eq!(count(&mut c, "holding"), 2, "被拒的都没写入");

        update_holding(&mut c, id, &auto(inv, "VOO", "107.VOO", 10.0, 39600.0, 550.0, None), TODAY).unwrap();
        assert_eq!(list_holdings(&mut c).unwrap()[0].cost_price, None, "编辑时清空成本价");
    }

    #[test]
    fn pnl_uses_native_prices_and_ignores_fx_moves() {
        // VOO 10 股,成本 500 美元,现价 550 美元,汇率 7.2:金额 39600 元
        assert_eq!(pnl_of(39600_00, Some(500.0), Some(550.0)), Some((36000_00, 3600_00, 10.0)));
        // 600519 100 股,成本 1200,现价 1080
        assert_eq!(pnl_of(108000_00, Some(1200.0), Some(1080.0)), Some((120000_00, -12000_00, -10.0)));
        // 汇率涨了但美元价格没变:不算盈亏
        assert_eq!(pnl_of(36500_00, Some(500.0), Some(500.0)), Some((36500_00, 0, 0.0)));
        assert_eq!(pnl_of(100_00, None, Some(1.0)), None, "没填成本价");
        assert_eq!(pnl_of(100_00, Some(1.0), None), None, "还没有现价");
    }

    #[test]
    fn summary_counts_only_holdings_with_pnl() {
        let t = TestDb::new();
        let mut c = t.conn();
        let inv = account(&mut c, "券商", "investment", 1000);
        let empty = pnl_summary(&mut c, TODAY).unwrap();
        assert_eq!((empty.counted, empty.excluded, empty.pnl_pct), (0, 0, None));

        add_holding(&mut c, &auto(inv, "VOO", "107.VOO", 10.0, 39600.0, 550.0, Some(500.0)), TODAY).unwrap();
        add_holding(&mut c, &auto(inv, "茅台", "1.600519", 100.0, 108000.0, 1080.0, Some(1200.0)), TODAY).unwrap();
        add_holding(&mut c, &input(inv, "手动", "bond", 5000), TODAY).unwrap();
        add_holding(&mut c, &HoldingInput { price: None, ..auto(inv, "没现价", "1.510300", 1.0, 4.0, 4.0, Some(3.0)) }, TODAY).unwrap();
        let s = pnl_summary(&mut c, TODAY).unwrap();
        assert_eq!((s.counted, s.excluded), (2, 2));
        assert_eq!((s.cost, s.value, s.pnl), (156000_00, 147600_00, -8400_00));
        assert_eq!(s.pnl_pct, Some(-5.38));
        let h = list_holdings(&mut c).unwrap();
        assert_eq!((h[2].pnl, h[3].pnl), (None, None));
    }

    #[test]
    fn snapshots_overwrite_per_day_and_track_period_pnl() {
        let t = TestDb::new();
        let mut c = t.conn();
        let inv = account(&mut c, "券商", "investment", 1000);
        let today = "2026-11-02";
        assert_eq!(pnl_summary(&mut c, today).unwrap().ytd, None, "还没有快照");

        // 同一天多次变动只留最后一份
        let id = add_holding(&mut c, &auto(inv, "VOO", "107.VOO", 10.0, 39600.0, 550.0, Some(500.0)), today).unwrap();
        update_holding(&mut c, id, &auto(inv, "VOO", "107.VOO", 10.0, 39960.0, 555.0, Some(500.0)), today).unwrap();
        let rows: Vec<(String, i64, i64)> = c.query("SELECT date, value, cost FROM holding_snapshot").unwrap();
        assert_eq!(rows, [(today.to_string(), 39960_00, 36000_00)]);

        // 快照不足一年:从最早的快照算起
        c.query_drop("DELETE FROM holding_snapshot").unwrap();
        snap(&mut c, "2026-10-01", 40000, 36000); // 累计 +4000
        let ytd = pnl_summary(&mut c, today).unwrap().ytd.unwrap();
        assert_eq!(ytd, PeriodPnl { pnl: -40_00, since: "2026-10-01".into(), full: false }, "当前累计 +3960");

        // 有去年的快照:以去年最后一份为起点
        snap(&mut c, "2025-12-31", 39000, 36000); // +3000
        snap(&mut c, "2025-06-30", 30000, 36000);
        let ytd = pnl_summary(&mut c, today).unwrap().ytd.unwrap();
        assert_eq!(ytd, PeriodPnl { pnl: 960_00, since: "2025-12-31".into(), full: true });

        // 过去的月份:用月末及之前的最后一份快照
        let oct = period_pnl(&mut c, "2026-10-01", "2026-10-31", today, (0, 0)).unwrap().unwrap();
        assert_eq!(oct, PeriodPnl { pnl: 1000_00, since: "2025-12-31".into(), full: true });
        assert_eq!(period_pnl(&mut c, "2025-01-01", "2025-01-31", today, (0, 0)).unwrap(), None, "那时还没有快照");
    }

    #[test]
    fn adding_to_a_position_is_not_profit() {
        let t = TestDb::new();
        let mut c = t.conn();
        let inv = account(&mut c, "券商", "investment", 1000);
        let id = add_holding(&mut c, &auto(inv, "VOO", "107.VOO", 10.0, 39600.0, 550.0, Some(500.0)), "2026-11-01").unwrap();
        let before = pnl_summary(&mut c, "2026-11-01").unwrap().pnl;
        // 第二天按 550 加仓 10 股:平均成本 525
        update_holding(&mut c, id, &auto(inv, "VOO", "107.VOO", 20.0, 79200.0, 550.0, Some(525.0)), "2026-11-02").unwrap();
        let after = pnl_summary(&mut c, "2026-11-02").unwrap();
        assert_eq!((before, after.pnl), (3600_00, 3600_00));
        assert_eq!(after.ytd.unwrap().pnl, 0, "从 11-01 的快照到现在,盈亏没有变化");
    }

    #[test]
    fn account_balance_follows_its_holdings() {
        let t = TestDb::new();
        let mut c = t.conn();
        let voo = account(&mut c, "VOO指数", "investment", 4740);
        let other = account(&mut c, "ETF", "investment", 756);
        let bank = account(&mut c, "招行", "cash", 1000);
        let bal = |c: &mut Conn, id: i64| core::list_accounts(c).unwrap().into_iter().find(|a| a.id == id).unwrap().balance;

        let h = add_holding(&mut c, &auto(voo, "VOO", "107.VOO", 1.0, 4649.44, 699.49, Some(713.0)), "2026-11-02").unwrap();
        assert_eq!(bal(&mut c, voo), 4649_44, "余额改成持仓合计");
        assert_eq!((bal(&mut c, other), bal(&mut c, bank)), (756_00, 1000_00), "没登记持仓的账户、非投资账户不动");

        add_holding(&mut c, &HoldingInput { asset_class: "cash".into(), ..input(voo, "账户里的现金", "cash", 100) }, "2026-11-02").unwrap();
        assert_eq!(bal(&mut c, voo), 4749_44, "现金也登记成持仓,就算进余额");

        // 估值刷新后跟着变
        set_holding_amount(&mut c, h, 4700_00, 707.0, "2026-11-03").unwrap();
        holdings_changed(&mut c, "2026-11-03").unwrap();
        assert_eq!(bal(&mut c, voo), 4800_00);
        assert_eq!(sync_account_balances(&mut c, "2026-11-03").unwrap(), 0, "余额已相等时不重复写");

        // 持仓挪到另一个账户:两个账户都按各自的持仓重算
        update_holding(&mut c, h, &auto(other, "VOO", "107.VOO", 1.0, 4700.0, 707.0, Some(713.0)), "2026-11-03").unwrap();
        assert_eq!((bal(&mut c, voo), bal(&mut c, other)), (100_00, 4700_00));

        // 删掉账户里最后一条持仓:余额保持原样
        let cash: i64 = c.query_first("SELECT id FROM holding WHERE name='账户里的现金'").unwrap().unwrap();
        delete_holding(&mut c, cash, "2026-11-03").unwrap();
        assert_eq!(bal(&mut c, voo), 100_00);
    }

    #[test]
    fn holdings_are_removed_with_their_account_only() {
        let t = TestDb::new();
        let mut c = t.conn();
        let a = account(&mut c, "投资 A", "investment", 1000);
        let b = account(&mut c, "投资 B", "investment", 1000);
        add_holding(&mut c, &input(a, "甲", "bond", 100), TODAY).unwrap();
        add_holding(&mut c, &input(b, "乙", "gold", 100), TODAY).unwrap();
        c.exec_drop("DELETE FROM account WHERE id=?", (a,)).unwrap();
        let left = list_holdings(&mut c).unwrap();
        assert_eq!(left.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(), ["乙"]);
    }

    // ----- 候选清单 -----

    #[test]
    fn product_crud_and_duplicate_names() {
        let t = TestDb::new();
        let mut c = t.conn();
        let p = ProductInput { name: "某某债券指数基金".into(), code: Some("123456".into()), asset_class: "bond".into(), note: "".into() };
        let id = add_product(&mut c, &p).unwrap();
        assert!(msg(add_product(&mut c, &p)).contains("已经有"));
        assert!(msg(add_product(&mut c, &ProductInput { asset_class: "股票".into(), name: "别的".into(), ..p.clone() })).contains("资产大类"));
        assert!(msg(add_product(&mut c, &ProductInput { name: " ".into(), ..p.clone() })).contains("名称"));

        // 改成自己的名字不算重复;改成别人的名字才算
        update_product(&mut c, id, &ProductInput { note: "改过".into(), ..p.clone() }).unwrap();
        let other = add_product(&mut c, &ProductInput { name: "另一个".into(), ..p.clone() }).unwrap();
        assert!(msg(update_product(&mut c, other, &p)).contains("已经有"));
        let list = list_products(&mut c).unwrap();
        assert_eq!((list[0].note.as_str(), list[0].builtin), ("改过", false));
        delete_product(&mut c, id).unwrap();
        assert_eq!(list_products(&mut c).unwrap().len(), 1);
    }

    #[test]
    fn seeds_cover_every_class_once_and_never_come_back() {
        let t = TestDb::new();
        let mut c = t.conn();
        assert!(list_products(&mut c).unwrap().is_empty(), "migrate 本身不写示例");
        assert!(seed_products(&mut c).unwrap());
        let list = list_products(&mut c).unwrap();
        for class in ["us_equity_index", "other_equity", "bond", "gold", "cash"] {
            assert!(list.iter().any(|p| p.asset_class == class), "缺少 {class}");
        }
        assert!(list.iter().all(|p| p.builtin && p.note.contains("仅供参考,以发行方披露为准")));

        // 重启(再次调用)不重复写入
        assert!(!seed_products(&mut c).unwrap());
        assert_eq!(list_products(&mut c).unwrap().len(), list.len());

        // 删光后重启,保持为空
        for p in &list {
            delete_product(&mut c, p.id).unwrap();
        }
        assert!(!seed_products(&mut c).unwrap());
        assert!(list_products(&mut c).unwrap().is_empty());
    }

    #[test]
    fn existing_ledgers_get_the_later_batch_once() {
        let t = TestDb::new();
        let mut c = t.conn();
        // 模拟只写过第一批的旧账本,且用户删掉了其中一个示例
        c.exec_drop("INSERT INTO setting (name, value) VALUES ('product_seeded','1')", ()).unwrap();
        add_product(&mut c, &ProductInput { name: "华夏日经225ETF".into(), code: Some("513520".into()), asset_class: "other_equity".into(), note: "自己写的".into() }).unwrap();
        assert!(seed_products(&mut c).unwrap());
        let list = list_products(&mut c).unwrap();
        assert!(!list.iter().any(|p| p.code.as_deref() == Some("VOO")), "第一批不会被补回");
        assert!(list.iter().any(|p| p.code.as_deref() == Some("510300")));
        assert_eq!(list.iter().find(|p| p.name == "华夏日经225ETF").unwrap().note, "自己写的", "重名的不覆盖");
        assert!(!seed_products(&mut c).unwrap());
    }

    #[test]
    fn seeds_carry_no_perishable_data_nor_channel_or_region_claims() {
        let t = TestDb::new();
        let mut c = t.conn();
        seed_products(&mut c).unwrap();
        for p in list_products(&mut c).unwrap() {
            let text = format!("{}{}{}", p.name, p.code.clone().unwrap_or_default(), p.note);
            for bad in ["费率", "规模", "收益率", "%", "地区", "券商", "能否购买"] {
                assert!(!text.contains(bad), "{} 含有 {bad}", p.name);
            }
        }
    }

    // ----- 风险问卷 -----

    fn profile() -> RiskProfile {
        RiskProfile { level: "balanced".into(), max_drawdown: 30, horizon_years: 10, high_debt: false }
    }

    #[test]
    fn risk_profile_roundtrip_and_incomplete_states() {
        let t = TestDb::new();
        let mut c = t.conn();
        assert_eq!(get_risk_profile(&mut c).unwrap(), None, "从未填写");
        save_risk_profile(&mut c, &profile()).unwrap();
        assert_eq!(get_risk_profile(&mut c).unwrap(), Some(profile()));
        save_risk_profile(&mut c, &RiskProfile { high_debt: true, level: "growth".into(), ..profile() }).unwrap();
        assert_eq!(get_risk_profile(&mut c).unwrap().map(|p| (p.level, p.high_debt)), Some(("growth".into(), true)));

        // 只剩部分键:视为未完成
        c.exec_drop("DELETE FROM setting WHERE name='risk_horizon_years'", ()).unwrap();
        assert_eq!(get_risk_profile(&mut c).unwrap(), None);
    }

    #[test]
    fn risk_profile_rejects_out_of_range_and_keeps_old_answers() {
        let t = TestDb::new();
        let mut c = t.conn();
        save_risk_profile(&mut c, &profile()).unwrap();
        for bad in [
            RiskProfile { max_drawdown: 80, ..profile() },
            RiskProfile { max_drawdown: 4, ..profile() },
            RiskProfile { horizon_years: 0, ..profile() },
            RiskProfile { horizon_years: 51, ..profile() },
            RiskProfile { level: "激进".into(), ..profile() },
        ] {
            assert!(save_risk_profile(&mut c, &bad).is_err(), "{bad:?}");
        }
        assert_eq!(get_risk_profile(&mut c).unwrap(), Some(profile()));
        // 库里被写进脏值也不会被当作有效问卷
        c.exec_drop("UPDATE setting SET value='abc' WHERE name='risk_max_drawdown'", ()).unwrap();
        assert_eq!(get_risk_profile(&mut c).unwrap(), None);
    }

    #[test]
    fn saving_assistant_settings_and_the_questionnaire_do_not_touch_each_other() {
        let t = TestDb::new();
        let mut c = t.conn();
        save_risk_profile(&mut c, &profile()).unwrap();
        let mut s = core::get_settings(&mut c).unwrap();
        s.persona = "新的人设".into();
        core::save_settings(&mut c, &s).unwrap();
        assert_eq!(get_risk_profile(&mut c).unwrap(), Some(profile()));

        save_risk_profile(&mut c, &RiskProfile { level: "growth".into(), ..profile() }).unwrap();
        let s = core::get_settings(&mut c).unwrap();
        assert_eq!((s.persona.as_str(), s.llm_model.as_str()), ("新的人设", core::DEFAULT_MODEL));
    }

    // ----- 备份 -----

    fn fill(c: &mut Conn) {
        let inv = account(c, "招行投资", "investment", 100_000);
        let dep = account(c, "存款", "deposit", 5_000);
        set_account_rate(c, dep, Some(350)).unwrap();
        add_holding(c, &input(inv, "标普500", "us_equity_index", 80_000), TODAY).unwrap();
        seed_products(c).unwrap();
        add_product(c, &ProductInput { name: "自己加的".into(), code: None, asset_class: "bond".into(), note: "x".into() }).unwrap();
        save_risk_profile(c, &profile()).unwrap();
    }

    fn tables(c: &mut Conn) -> Value {
        backup::dump(c).unwrap()["tables"].clone()
    }

    #[test]
    fn backup_roundtrip_keeps_holdings_rates_products_and_questionnaire() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, file) = (tmp.path().join("backups"), tmp.path().join("b.json"));
        let (a, b) = (TestDb::new(), TestDb::new());
        let (mut ca, mut cb) = (a.conn(), b.conn());
        fill(&mut ca);
        backup::write_dump(&mut ca, &file).unwrap();

        // 另一边先有别的数据,恢复后应被完全替换
        let other = account(&mut cb, "别的投资", "investment", 1);
        add_holding(&mut cb, &input(other, "别的", "gold", 1), TODAY).unwrap();
        seed_products(&mut cb).unwrap();
        backup::restore(&mut cb, &dir, &file).unwrap();

        assert_eq!(tables(&mut cb), tables(&mut ca), "恢复后逐表一致");
        assert_eq!(list_holdings(&mut cb).unwrap().len(), 1);
        assert_eq!(account_rates(&mut cb).unwrap().values().copied().collect::<Vec<_>>(), [350]);
        assert_eq!(get_risk_profile(&mut cb).unwrap(), Some(profile()));
        assert!(list_products(&mut cb).unwrap().iter().any(|p| p.name == "自己加的"));
        assert!(!seed_products(&mut cb).unwrap(), "标记随备份恢复,不会再补示例");
    }

    #[test]
    fn restoring_an_old_backup_without_the_new_tables_works_and_reseeds() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, file) = (tmp.path().join("backups"), tmp.path().join("old.json"));
        let (a, b) = (TestDb::new(), TestDb::new());
        let (mut ca, mut cb) = (a.conn(), b.conn());
        fill(&mut ca);
        backup::write_dump(&mut ca, &file).unwrap();

        // 模拟早于本功能导出的备份:去掉三张新表和问卷、示例标记
        let mut doc: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        for t in ["account_rate", "holding", "product"] {
            doc["tables"].as_object_mut().unwrap().remove(t);
        }
        doc["tables"]["setting"].as_array_mut().unwrap().retain(|r| {
            let n = r["name"].as_str().unwrap_or("");
            !n.starts_with("risk_") && !n.starts_with("product_seeded")
        });
        std::fs::write(&file, serde_json::to_vec(&doc).unwrap()).unwrap();

        let other = account(&mut cb, "别的投资", "investment", 1);
        add_holding(&mut cb, &input(other, "别的", "gold", 1), TODAY).unwrap();
        backup::restore(&mut cb, &dir, &file).unwrap();

        assert!(list_holdings(&mut cb).unwrap().is_empty());
        assert!(account_rates(&mut cb).unwrap().is_empty());
        assert_eq!(get_risk_profile(&mut cb).unwrap(), None);
        assert!(list_products(&mut cb).unwrap().is_empty());
        // 与应用里的恢复命令一致:恢复后补示例
        assert!(seed_products(&mut cb).unwrap());
        assert!(!list_products(&mut cb).unwrap().is_empty());
    }
}
