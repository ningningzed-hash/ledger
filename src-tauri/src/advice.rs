//! 组合建议:占比与集中度、目标配比、前置条件、再平衡、利息视角、候选产品。
//!
//! 全部由程序按确定性规则计算;大模型只负责解读结果,不生成任何数字或产品。
//! 金额为整数"分"(Tauri 命令直接返回);助手工具会按 `MONEY_KEYS` 转成"元"。
//! 只陈述事实和算术,不预测行情、不给买卖时点。
use crate::core::{self, Error, Result};
use crate::goals;
use crate::portfolio::{self, class_label, Holding, InterestOverview, Product, RiskProfile};
use crate::report;
use chrono::NaiveDate;
use mysql::prelude::Queryable;
use serde::Serialize;
use serde_json::{json, Value};

pub const DISCLAIMER: &str = "以上仅为一般性参考,不构成投资建议。";

/// 集中度门槛(%)
const CONCENTRATION_PCT: i64 = 70;
/// 持仓超过多少天没更新就提示可能过期
const STALE_DAYS: i64 = 90;
/// 持仓合计与投资账户余额的覆盖率容差(%)
const COVERAGE_LOW: f64 = 95.0;
const COVERAGE_HIGH: f64 = 105.0;
/// 差额绝对值小于这个百分点的大类视为基本合适
const BALANCED_PP: f64 = 5.0;
/// 应急金目标月数;闲置现金按保留这么多个月支出计算
const EMERGENCY_MONTHS: f64 = 3.0;
const IDLE_KEEP_MONTHS: i64 = 6;
/// 年利率达到这个值(基点)的负债视为高息
const HIGH_RATE_BPS: i64 = 800;
/// 这么多个月内到期的存钱目标,所需资金按现金类预留
const SHORT_GOAL_MONTHS: i64 = 36;
/// 估算月均收支最多看最近几个已过完的月份
const FLOW_MONTHS: u32 = 3;

/// 助手工具返回前要从"分"转成"元"的字段名
const MONEY_KEYS: [&str; 18] = [
    "amount", "total", "balance", "liquid", "liabilities", "investment_balance", "gap", "current_amount", "target_amount", "diff_amount",
    "buy_total", "avg_monthly_expense", "avg_monthly_surplus", "idle_cash", "yearly_interest", "extra_yearly_interest",
    "reference_yearly_interest", "reserved_for_short_term_goals",
];
/// 概览里的金额字段名较多,单独列出
const MONEY_KEYS_2: [&str; 3] = ["monthly_income", "monthly_cost", "monthly_interest"];

// ---------- 数据结构 ----------

#[derive(Serialize, Debug, Clone)]
pub struct Notice {
    /// high_interest_debt | emergency_fund | concentration | single_market | coverage | stale | no_profile | no_holdings | no_account | thin_surplus
    pub code: String,
    /// priority 排在最前的优先事项 | warn | info
    pub level: String,
    pub message: String,
}

fn notice(code: &str, level: &str, message: String) -> Notice {
    Notice { code: code.into(), level: level.into(), message }
}

#[derive(Serialize, Debug)]
pub struct ClassShare {
    pub asset_class: String,
    pub label: String,
    pub amount: i64,
    /// 占持仓合计的百分比,保留一位小数
    pub share_pct: f64,
}

#[derive(Serialize, Debug)]
pub struct StaleHolding {
    pub id: i64,
    pub name: String,
    pub days: i64,
}

#[derive(Serialize, Debug)]
pub struct Portfolio {
    pub holdings: Vec<Holding>,
    pub total: i64,
    pub shares: Vec<ClassShare>,
    pub has_investment_account: bool,
    pub investment_balance: i64,
    /// 持仓合计 ÷ 投资账户余额合计(%);没有投资账户或余额不为正时为空
    pub coverage_pct: Option<f64>,
    /// 覆盖率超出 95%~105% 时的差额(账户余额 − 持仓合计)
    pub gap: Option<i64>,
    pub stale: Vec<StaleHolding>,
    pub notices: Vec<Notice>,
}

#[derive(Serialize, Debug)]
pub struct HighDebt {
    pub name: String,
    pub balance: i64,
    pub rate_bps: i64,
    pub yearly_interest: i64,
}

#[derive(Serialize, Debug)]
pub struct Readiness {
    /// 现金类 + 存款类账户余额合计
    pub liquid: i64,
    pub avg_monthly_expense: Option<i64>,
    pub avg_monthly_surplus: Option<i64>,
    pub emergency_months: Option<f64>,
    pub liabilities: i64,
    pub high_interest_debts: Vec<HighDebt>,
    /// 用户自述:没有登记年利率的负债里有高息的
    pub high_debt_declared: bool,
}

#[derive(Serialize, Debug, PartialEq, Clone, Copy)]
pub struct TargetPct {
    pub us_equity_index: i64,
    pub other_equity: i64,
    pub bond: i64,
    pub gold: i64,
    pub cash: i64,
}

#[derive(Serialize, Debug)]
pub struct Candidates {
    pub products: Vec<Product>,
    /// 清单里没有这一类时的说明
    pub hint: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct RebalanceRow {
    pub asset_class: String,
    pub label: String,
    pub current_amount: i64,
    pub current_pct: f64,
    pub target_amount: i64,
    pub target_pct: f64,
    /// 目标 − 当前,百分点
    pub diff_pct: f64,
    /// 目标 − 当前,分;基本合适时为空
    pub diff_amount: Option<i64>,
    /// add 增配 | reduce 减配 | balanced 基本合适
    pub action: String,
    /// 只有需要增配的大类才有
    pub candidates: Option<Candidates>,
}

#[derive(Serialize, Debug)]
pub struct Rebalance {
    pub rows: Vec<RebalanceRow>,
    /// 需要增配的金额合计
    pub buy_total: i64,
    /// 全部用新增资金补足,按近几个月平均结余大约需要几个月
    pub months_to_fill: Option<i64>,
    /// 为 36 个月内到期的存钱目标预留(计入现金类)
    pub reserved_for_short_term_goals: i64,
}

#[derive(Serialize, Debug)]
pub struct DebtCost {
    pub name: String,
    pub balance: i64,
    pub rate_bps: i64,
    /// 还清可每年少付的利息
    pub yearly_interest: i64,
    pub high: bool,
}

#[derive(Serialize, Debug)]
pub struct InterestView {
    pub overview: InterestOverview,
    /// 现金类 + 存款类余额 − 6 个月支出;没有支出数据时为空
    pub idle_cash: Option<i64>,
    /// 这些账户的加权平均年利率(基点);有账户没登记利率时为空
    pub weighted_rate_bps: Option<f64>,
    pub reference_rate_bps: Option<i64>,
    pub reference_yearly_interest: Option<i64>,
    /// 按参考利率比现有加权平均利率每年多出的利息
    pub extra_yearly_interest: Option<i64>,
    pub hint: Option<String>,
    pub debts: Vec<DebtCost>,
}

#[derive(Serialize, Debug)]
pub struct Advice {
    pub disclaimer: &'static str,
    pub profile: Option<RiskProfile>,
    pub profile_missing: bool,
    pub readiness: Readiness,
    pub notices: Vec<Notice>,
    pub target: Option<TargetPct>,
    pub rebalance: Option<Rebalance>,
    pub interest: InterestView,
    pub portfolio: Portfolio,
}

// ---------- 小工具 ----------

fn pct1(num: i64, den: i64) -> f64 {
    if den <= 0 {
        0.0
    } else {
        ((num as f64) / (den as f64) * 1000.0).round() / 10.0
    }
}

fn yuan_text(cents: i64) -> String {
    let neg = cents < 0;
    let a = cents.abs();
    let (y, f) = (a / 100, a % 100);
    let mut s = y.to_string();
    let mut grouped = String::new();
    for (i, ch) in s.drain(..).rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let int: String = grouped.chars().rev().collect();
    format!("{}¥{int}.{f:02}", if neg { "-" } else { "" })
}

fn parse_day(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

// ---------- 持仓占比 ----------

pub fn portfolio_view(c: &mut impl Queryable, today: &str) -> Result<Portfolio> {
    let holdings = portfolio::list_holdings(c)?;
    let total: i64 = holdings.iter().map(|h| h.amount).sum();
    let shares: Vec<ClassShare> = portfolio::CLASSES
        .iter()
        .map(|(key, label)| {
            let amount: i64 = holdings.iter().filter(|h| h.asset_class == *key).map(|h| h.amount).sum();
            ClassShare { asset_class: key.to_string(), label: label.to_string(), amount, share_pct: pct1(amount, total) }
        })
        .collect();

    let accounts = core::list_accounts(c)?;
    let inv: Vec<&core::Account> = accounts.iter().filter(|a| a.kind == "investment").collect();
    let investment_balance: i64 = inv.iter().map(|a| a.balance).sum();
    let coverage_pct = (!inv.is_empty() && investment_balance > 0).then(|| pct1(total, investment_balance));

    let mut notices = Vec::new();
    if inv.is_empty() {
        notices.push(notice("no_account", "info", "还没有投资类账户:先在资产里添加投资账户,再登记持仓。".into()));
    } else if holdings.is_empty() {
        notices.push(notice("no_holdings", "info", "还没有登记持仓:登记后才能看到各大类占比和建议。".into()));
    }

    // 集中度:任一大类 ≥ 70%;股票全在美股宽基
    if total > 0 {
        if let Some(top) = shares.iter().max_by_key(|s| s.amount) {
            if top.amount * 100 >= total * CONCENTRATION_PCT {
                notices.push(notice("concentration", "warn", format!("集中度偏高:{}占持仓的 {:.1}%。", top.label, top.share_pct)));
            }
        }
        let amount_of = |k: &str| shares.iter().find(|s| s.asset_class == k).map_or(0, |s| s.amount);
        if amount_of("us_equity_index") > 0 && amount_of("other_equity") == 0 {
            notices.push(notice("single_market", "info", "股票全部集中在美股宽基,没有其他市场的股票。".into()));
        }
    }

    // 对账:持仓合计与投资账户余额
    let mut gap = None;
    if let Some(cov) = coverage_pct.filter(|_| !holdings.is_empty()) {
        if !(COVERAGE_LOW..=COVERAGE_HIGH).contains(&cov) {
            let diff = investment_balance - total;
            gap = Some(diff);
            let msg = if diff > 0 {
                format!("持仓合计 {} 只占投资账户余额 {} 的 {:.0}%,还有 {} 没有登记。", yuan_text(total), yuan_text(investment_balance), cov, yuan_text(diff))
            } else {
                format!("持仓合计 {} 超过投资账户余额 {},差额 {},请核对金额。", yuan_text(total), yuan_text(investment_balance), yuan_text(-diff))
            };
            notices.push(notice("coverage", "warn", msg));
        }
    }

    // 新鲜度
    let now = parse_day(today);
    let mut stale = Vec::new();
    for h in &holdings {
        if let (Some(t), Some(u)) = (now, parse_day(&h.updated_on)) {
            let days = (t - u).num_days();
            if days > STALE_DAYS {
                stale.push(StaleHolding { id: h.id, name: h.name.clone(), days });
            }
        }
    }
    if !stale.is_empty() {
        let names: Vec<String> = stale.iter().map(|s| s.name.clone()).collect();
        notices.push(notice("stale", "info", format!("有 {} 条持仓超过 {STALE_DAYS} 天没有更新,金额可能已过期:{}。", stale.len(), names.join("、"))));
    }

    Ok(Portfolio { holdings, total, shares, has_investment_account: !inv.is_empty(), investment_balance, coverage_pct, gap, stale, notices })
}

// ---------- 目标配比 ----------

/// 风险等级为底,再依次施加期限上限和最大回撤上限;削减的股票转入债券。
/// 估算最大回撤 = 股票×50% + 黄金×30% + 债券×10%(现金 0)。系数是粗略的经验值。
pub fn target_percent(level: &str, max_drawdown: i64, horizon_years: i64) -> TargetPct {
    let (mut eq, mut bond, gold, cash) = match level {
        "conservative" => (30, 50, 10, 10),
        "growth" => (70, 20, 7, 3),
        _ => (50, 35, 10, 5),
    };
    let cap = if horizon_years < 3 {
        20
    } else if horizon_years < 5 {
        40
    } else {
        100
    };
    if eq > cap {
        bond += eq - cap;
        eq = cap;
    }
    // 估算回撤的 10 倍:5×股票 + 3×黄金 + 1×债券,与 10×最大回撤比较,避免浮点
    while eq > 0 && 5 * eq + 3 * gold + bond > 10 * max_drawdown {
        let step = eq.min(5);
        eq -= step;
        bond += step;
    }
    let us = eq * 3 / 5; // 股票内部:美股宽基 60%、非美股票 40%
    TargetPct { us_equity_index: us, other_equity: eq - us, bond, gold, cash }
}

/// 把目标百分比换成金额(分)。`reserved` 先整体划入现金类,其余按比例分配;合计恒等于 `total`。
fn target_amounts(total: i64, reserved: i64, t: &TargetPct) -> [i64; 5] {
    let rest = total - reserved;
    let part = |p: i64| (rest * p * 2 + 100) / 200;
    let (us, other, bond, gold) = (part(t.us_equity_index), part(t.other_equity), part(t.bond), part(t.gold));
    [us, other, bond, gold, total - us - other - bond - gold]
}

const MODELLED: [&str; 5] = ["us_equity_index", "other_equity", "bond", "gold", "cash"];

// ---------- 前置条件 ----------

/// 最近至多 3 个已过完月份的月均 (收入, 支出);没有数据时为空
fn monthly_flow(c: &mut impl Queryable, today: &str) -> Result<Option<(i64, i64)>> {
    let through = report::shift_back(&today[..7], 1)?;
    let r = report::report(c, &through, FLOW_MONTHS)?;
    if r.months.is_empty() {
        return Ok(None);
    }
    let n = r.months.len() as i64;
    let (inc, exp) = r.months.iter().fold((0, 0), |a, m| (a.0 + m.income, a.1 + m.expense));
    Ok(Some((inc / n, exp / n)))
}

fn readiness(c: &mut impl Queryable, today: &str, profile: Option<&RiskProfile>) -> Result<(Readiness, Vec<Notice>)> {
    let accounts = core::list_accounts(c)?;
    let rates = portfolio::account_rates(c)?;
    let liquid: i64 = accounts.iter().filter(|a| a.kind == "cash" || a.kind == "deposit").map(|a| a.balance).sum();
    let liabilities: i64 = accounts.iter().filter(|a| a.kind == "liability").map(|a| a.balance).sum();
    let flow = monthly_flow(c, today)?;
    let avg_expense = flow.map(|f| f.1);
    let surplus = flow.map(|f| f.0 - f.1);
    let emergency_months = avg_expense.filter(|e| *e > 0).map(|e| liquid as f64 / e as f64);

    let mut high = Vec::new();
    for a in accounts.iter().filter(|a| a.kind == "liability" && a.balance > 0) {
        if let Some(bps) = rates.get(&a.id).copied().filter(|b| *b >= HIGH_RATE_BPS) {
            high.push(HighDebt { name: a.name.clone(), balance: a.balance, rate_bps: bps, yearly_interest: a.balance * bps / 10_000 });
        }
    }
    let has_unrated_debt = accounts.iter().any(|a| a.kind == "liability" && a.balance > 0 && !rates.contains_key(&a.id));
    let declared = profile.is_some_and(|p| p.high_debt) && has_unrated_debt;

    // 优先级:高息负债 → 应急金
    let mut notices = Vec::new();
    if !high.is_empty() {
        let list: Vec<String> = high.iter().map(|d| format!("{}(年利率 {:.2}%,每年约 {})", d.name, d.rate_bps as f64 / 100.0, yuan_text(d.yearly_interest))).collect();
        notices.push(notice("high_interest_debt", "priority", format!("优先还债:{}。负债合计 {}。", list.join("、"), yuan_text(liabilities))));
    } else if declared {
        notices.push(notice("high_interest_debt", "priority", format!("优先还债:你标注了有高息负债。负债合计 {};登记各负债的年利率后可以自动判断。", yuan_text(liabilities))));
    }
    match emergency_months {
        Some(m) if m < EMERGENCY_MONTHS => notices.push(notice(
            "emergency_fund", "priority",
            format!("先补应急金:现金类和存款类账户合计 {},只够 {m:.1} 个月的支出,建议至少 {EMERGENCY_MONTHS:.0} 个月。", yuan_text(liquid)),
        )),
        None => notices.push(notice("emergency_fund", "info", "应急金月数数据不足:还没有已过完月份的支出记录。".into())),
        _ => {}
    }

    let r = Readiness {
        liquid,
        avg_monthly_expense: avg_expense,
        avg_monthly_surplus: surplus,
        emergency_months: emergency_months.map(|m| (m * 10.0).round() / 10.0),
        liabilities,
        high_interest_debts: high,
        high_debt_declared: declared,
    };
    Ok((r, notices))
}

/// 36 个月内到期、还没达成的存钱目标"还差金额"合计
fn short_term_goal_need(c: &mut impl Queryable, today: &str) -> Result<i64> {
    Ok(goals::list_goals(c, today)?
        .iter()
        .filter(|g| g.kind == "save" && !g.achieved && g.months_left.is_some_and(|m| m <= SHORT_GOAL_MONTHS))
        .map(|g| g.remaining)
        .sum())
}

// ---------- 利息视角 ----------

fn interest_view(c: &mut impl Queryable, r: &Readiness, reference_bps: Option<i64>) -> Result<InterestView> {
    let overview = portfolio::interest_overview(c)?;
    let mut debts = Vec::new();
    for a in overview.accounts.iter().filter(|a| a.kind == "liability" && a.balance > 0) {
        if let Some(bps) = a.rate_bps {
            debts.push(DebtCost { name: a.name.clone(), balance: a.balance, rate_bps: bps, yearly_interest: a.balance * bps / 10_000, high: bps >= HIGH_RATE_BPS });
        }
    }
    debts.sort_by(|x, y| y.rate_bps.cmp(&x.rate_bps));

    // 闲置现金:保留 6 个月支出后多出来的部分
    let idle = r.avg_monthly_expense.map(|e| (r.liquid - IDLE_KEEP_MONTHS * e).max(0));
    let liquid_accts: Vec<_> = overview.accounts.iter().filter(|a| (a.kind == "cash" || a.kind == "deposit") && a.balance > 0).collect();
    let all_rated = !liquid_accts.is_empty() && liquid_accts.iter().all(|a| a.rate_bps.is_some());
    let weighted = all_rated.then(|| {
        let bal: i64 = liquid_accts.iter().map(|a| a.balance).sum();
        let sum: f64 = liquid_accts.iter().map(|a| a.balance as f64 * a.rate_bps.unwrap_or(0) as f64).sum();
        sum / bal as f64
    });

    let mut hint = None;
    let (mut ref_interest, mut extra) = (None, None);
    if let (Some(idle), Some(refr)) = (idle.filter(|i| *i > 0), reference_bps) {
        ref_interest = Some(idle * refr / 10_000);
        match weighted {
            Some(w) => extra = Some(((idle as f64) * (refr as f64 - w) / 10_000.0).round().max(0.0) as i64),
            None => hint = Some("现金类/存款类账户没有全部登记年利率,登记后可以和参考利率对比。".into()),
        }
    }
    Ok(InterestView {
        overview, idle_cash: idle, weighted_rate_bps: weighted.map(|w| (w * 100.0).round() / 100.0),
        reference_rate_bps: reference_bps, reference_yearly_interest: ref_interest, extra_yearly_interest: extra, hint, debts,
    })
}

// ---------- 总入口 ----------

pub fn advice(c: &mut impl Queryable, today: &str, reference_rate_bps: Option<i64>) -> Result<Advice> {
    if let Some(b) = reference_rate_bps {
        if !(0..=10_000).contains(&b) {
            return Err(Error::Invalid("参考利率需在 0% 到 100% 之间".into()));
        }
    }
    let profile = portfolio::get_risk_profile(c)?;
    let pf = portfolio_view(c, today)?;
    let (ready, mut notices) = readiness(c, today, profile.as_ref())?;
    let interest = interest_view(c, &ready, reference_rate_bps)?;

    let modelled: i64 = pf.shares.iter().filter(|s| s.asset_class != "other").map(|s| s.amount).sum();
    let mut target = None;
    let mut rebalance = None;
    match &profile {
        None => notices.push(notice("no_profile", "info", "先完成风险偏好问卷,才能给出目标配比。".into())),
        Some(p) => {
            let t = target_percent(&p.level, p.max_drawdown, p.horizon_years);
            target = Some(t);
            if modelled > 0 {
                rebalance = Some(rebalance_rows(c, today, &pf, modelled, &t, ready.avg_monthly_surplus, &mut notices)?);
            }
        }
    }
    notices.extend(pf.notices.iter().cloned());
    // priority 在前,其余保持原有顺序
    notices.sort_by_key(|n| match n.level.as_str() {
        "priority" => 0,
        "warn" => 1,
        _ => 2,
    });

    Ok(Advice { disclaimer: DISCLAIMER, profile_missing: profile.is_none(), profile, readiness: ready, notices, target, rebalance, interest, portfolio: pf })
}

fn rebalance_rows(
    c: &mut impl Queryable, today: &str, pf: &Portfolio, modelled: i64, t: &TargetPct, surplus: Option<i64>, notices: &mut Vec<Notice>,
) -> Result<Rebalance> {
    let reserved = short_term_goal_need(c, today)?.min(modelled);
    let amounts = target_amounts(modelled, reserved, t);
    let products = portfolio::list_products(c)?;
    let mut rows = Vec::new();
    let mut buy_total = 0;
    for (i, key) in MODELLED.iter().enumerate() {
        let current = pf.shares.iter().find(|s| s.asset_class == *key).map_or(0, |s| s.amount);
        let target = amounts[i];
        let (cur_pct, tgt_pct) = (pct1(current, modelled), pct1(target, modelled));
        let diff_pct = ((tgt_pct - cur_pct) * 10.0).round() / 10.0;
        let action = if diff_pct.abs() < BALANCED_PP { "balanced" } else if diff_pct > 0.0 { "add" } else { "reduce" };
        let diff = target - current;
        if action == "add" {
            buy_total += diff;
        }
        let candidates = (action == "add").then(|| {
            let ps: Vec<Product> = products.iter().filter(|p| p.asset_class == *key).cloned().collect();
            let hint = ps.is_empty().then(|| "清单里还没有这一类,可以到产品清单里添加".to_string());
            Candidates { products: ps, hint }
        });
        rows.push(RebalanceRow {
            asset_class: key.to_string(), label: class_label(key).to_string(),
            current_amount: current, current_pct: cur_pct, target_amount: target, target_pct: tgt_pct, diff_pct,
            diff_amount: (action != "balanced").then_some(diff), action: action.into(), candidates,
        });
    }
    let months_to_fill = match surplus {
        Some(s) if s > 0 && buy_total > 0 => Some((buy_total + s - 1) / s),
        Some(s) if s <= 0 && buy_total > 0 => {
            notices.push(notice("thin_surplus", "info", "近几个月平均结余不为正,无法靠新增资金补足:先改善结余。".into()));
            None
        }
        _ => None,
    };
    Ok(Rebalance { rows, buy_total, months_to_fill, reserved_for_short_term_goals: reserved })
}

// ---------- 助手工具 ----------

pub const TOOL_PORTFOLIO: &str = "get_portfolio";
pub const TOOL_ADVICE: &str = "get_portfolio_advice";

pub fn tool_defs() -> Vec<Value> {
    use crate::agent::tool;
    vec![
        tool(
            TOOL_PORTFOLIO,
            "读取投资持仓:各条持仓、各资产大类占比、集中度提示、与投资账户余额的对账。适用:用户问现在持有什么、占比如何、是否太集中。",
            json!({}),
            &[],
        ),
        tool(
            TOOL_ADVICE,
            "读取组合建议:风险问卷、应急金/负债/结余等前置条件、目标配比、当前与目标的差额和再平衡金额、需增配大类的候选产品、利息视角。所有数字由程序算好,你只负责解读;产品只能从返回的 candidates 里挑,不得补充清单之外的代码、费率或收益数据;问卷没填(profile_missing)时提示用户先填问卷。不要预测行情,不要给买卖时点。回答末尾要带上 disclaimer。",
            json!({"reference_rate": {"type":"number","description":"参考年利率,%,用于对比闲置现金放更高利率能多赚多少;用户没提就不要填"}}),
            &[],
        ),
    ]
}

/// 递归地把工具结果里的金额字段从"分"转成"元"
fn to_yuan(v: &mut Value) {
    match v {
        Value::Object(m) => {
            for (k, val) in m.iter_mut() {
                if (MONEY_KEYS.contains(&k.as_str()) || MONEY_KEYS_2.contains(&k.as_str())) && val.is_number() {
                    *val = json!(val.as_f64().unwrap_or(0.0) / 100.0);
                } else {
                    to_yuan(val);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(to_yuan),
        _ => {}
    }
}

pub fn run(c: &mut impl Queryable, name: &str, args: &Value, today: &str) -> Option<Result<Value>> {
    let result = match name {
        TOOL_PORTFOLIO => portfolio_view(c, today).and_then(|p| serde_json::to_value(p).map_err(|e| Error::Invalid(e.to_string()))),
        TOOL_ADVICE => (|| {
            let reference = match args.get("reference_rate") {
                None | Some(Value::Null) => None,
                Some(_) => {
                    let v = crate::agent::num(args, "reference_rate").filter(|v| v.is_finite() && (0.0..=100.0).contains(v));
                    Some((v.ok_or_else(|| Error::Invalid("reference_rate 必须是 0 到 100 之间的数字(单位 %)".into()))? * 100.0).round() as i64)
                }
            };
            let a = advice(c, today, reference)?;
            serde_json::to_value(a).map_err(|e| Error::Invalid(e.to_string()))
        })(),
        _ => return None,
    };
    Some(result.map(|mut v| {
        to_yuan(&mut v);
        v
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Conn;
    use crate::portfolio::{HoldingInput, ProductInput};
    use crate::testdb::TestDb;

    const TODAY: &str = "2026-10-27";

    fn account(c: &mut Conn, name: &str, kind: &str, yuan: i64) -> i64 {
        core::add_account(c, name, kind, "CNY", yuan * 100, "2026-01-01").unwrap()
    }

    fn hold(c: &mut Conn, acc: i64, name: &str, class: &str, yuan: i64) {
        portfolio::add_holding(c, &HoldingInput { account_id: acc, name: name.into(), code: None, asset_class: class.into(), amount: yuan * 100, ..Default::default() }, TODAY).unwrap();
    }

    fn profile(level: &str, dd: i64, years: i64) -> RiskProfile {
        RiskProfile { level: level.into(), max_drawdown: dd, horizon_years: years, high_debt: false }
    }

    fn set_profile(c: &mut Conn, p: &RiskProfile) {
        portfolio::save_risk_profile(c, p).unwrap();
    }

    /// 给最近三个已过完的月份(7、8、9 月)各记一笔支出,月均支出 = yuan
    fn monthly_spend(c: &mut Conn, yuan: i64) {
        for m in ["2026-07-10", "2026-08-10", "2026-09-10"] {
            core::add_txn(c, None, None, "expense", yuan * 100, m, "").unwrap();
        }
    }

    fn row<'a>(a: &'a Advice, class: &str) -> &'a RebalanceRow {
        a.rebalance.as_ref().expect("有再平衡").rows.iter().find(|r| r.asset_class == class).unwrap()
    }

    fn codes(a: &Advice) -> Vec<&str> {
        a.notices.iter().map(|n| n.code.as_str()).collect()
    }

    // ----- 目标配比(纯函数) -----

    #[test]
    fn targets_follow_the_documented_rule_table() {
        let t = |l, d, y| target_percent(l, d, y);
        // 宽松:不触发任何上限
        assert_eq!(t("balanced", 40, 10), TargetPct { us_equity_index: 30, other_equity: 20, bond: 35, gold: 10, cash: 5 });
        // 回撤 30%:估算 31.5% 超限,股票挪 5 个点到债券
        assert_eq!(t("balanced", 30, 10), TargetPct { us_equity_index: 27, other_equity: 18, bond: 40, gold: 10, cash: 5 });
        assert_eq!(t("conservative", 40, 10), TargetPct { us_equity_index: 18, other_equity: 12, bond: 50, gold: 10, cash: 10 });
        assert_eq!(t("growth", 40, 10), TargetPct { us_equity_index: 42, other_equity: 28, bond: 20, gold: 7, cash: 3 });
    }

    #[test]
    fn horizon_caps_equity_and_moves_the_rest_to_bonds() {
        let short = target_percent("growth", 60, 2);
        assert_eq!((short.us_equity_index + short.other_equity, short.bond), (20, 70));
        let mid = target_percent("growth", 60, 4);
        assert_eq!(mid.us_equity_index + mid.other_equity, 40);
        let edge = target_percent("growth", 60, 3);
        assert_eq!(edge.us_equity_index + edge.other_equity, 40, "3 年属于 3~5 年档");
        let five = target_percent("growth", 60, 5);
        assert_eq!(five.us_equity_index + five.other_equity, 70, "5 年及以上不限");
    }

    #[test]
    fn very_low_drawdown_drives_equity_to_zero_without_going_negative() {
        let t = target_percent("growth", 5, 20);
        assert_eq!((t.us_equity_index, t.other_equity), (0, 0));
        for (l, d, y) in [("growth", 5, 1), ("balanced", 5, 50), ("conservative", 5, 2), ("growth", 60, 50), ("balanced", 12, 3)] {
            let t = target_percent(l, d, y);
            let all = [t.us_equity_index, t.other_equity, t.bond, t.gold, t.cash];
            assert!(all.iter().all(|v| *v >= 0), "{l} {d} {y}: {t:?}");
            assert_eq!(all.iter().sum::<i64>(), 100, "{l} {d} {y}: {t:?}");
        }
    }

    // ----- 占比与集中度 -----

    #[test]
    fn all_us_index_shows_full_share_and_both_concentration_notices() {
        let t = TestDb::new();
        let mut c = t.conn();
        let inv = account(&mut c, "投资", "investment", 100_000);
        hold(&mut c, inv, "标普", "us_equity_index", 60_000);
        hold(&mut c, inv, "纳指", "us_equity_index", 40_000);
        let p = portfolio_view(&mut c, TODAY).unwrap();
        assert_eq!(p.total, 100_000_00);
        let us = p.shares.iter().find(|s| s.asset_class == "us_equity_index").unwrap();
        assert_eq!(us.share_pct, 100.0);
        assert!(p.shares.iter().filter(|s| s.asset_class != "us_equity_index").all(|s| s.share_pct == 0.0));
        let codes: Vec<&str> = p.notices.iter().map(|n| n.code.as_str()).collect();
        assert!(codes.contains(&"concentration") && codes.contains(&"single_market"), "{codes:?}");
    }

    #[test]
    fn shares_for_a_mixed_portfolio_and_the_69_9_percent_boundary() {
        let t = TestDb::new();
        let mut c = t.conn();
        let inv = account(&mut c, "投资", "investment", 100_000);
        hold(&mut c, inv, "美股", "us_equity_index", 60_000);
        hold(&mut c, inv, "债", "bond", 30_000);
        hold(&mut c, inv, "货基", "cash", 10_000);
        let p = portfolio_view(&mut c, TODAY).unwrap();
        let pct: Vec<f64> = ["us_equity_index", "bond", "cash"].iter().map(|k| p.shares.iter().find(|s| s.asset_class == *k).unwrap().share_pct).collect();
        assert_eq!(pct, [60.0, 30.0, 10.0]);
        assert!(!p.notices.iter().any(|n| n.code == "concentration"));

        let t2 = TestDb::new();
        let mut c2 = t2.conn();
        let inv = account(&mut c2, "投资", "investment", 1000);
        hold(&mut c2, inv, "美股", "us_equity_index", 699);
        hold(&mut c2, inv, "债", "bond", 301);
        let p = portfolio_view(&mut c2, TODAY).unwrap();
        assert!(!p.notices.iter().any(|n| n.code == "concentration"), "69.9% 不触发");

        let t3 = TestDb::new();
        let mut c3 = t3.conn();
        let inv = account(&mut c3, "投资", "investment", 1000);
        hold(&mut c3, inv, "美股", "us_equity_index", 700);
        hold(&mut c3, inv, "债", "bond", 300);
        let p = portfolio_view(&mut c3, TODAY).unwrap();
        assert!(p.notices.iter().any(|n| n.code == "concentration"), "刚好 70.0% 触发");
    }

    #[test]
    fn no_equity_means_no_single_market_notice_and_empty_states_guide() {
        let t = TestDb::new();
        let mut c = t.conn();
        let p = portfolio_view(&mut c, TODAY).unwrap();
        assert_eq!((p.total, p.coverage_pct), (0, None));
        assert!(p.notices.iter().any(|n| n.code == "no_account"));
        let inv = account(&mut c, "投资", "investment", 100);
        let p = portfolio_view(&mut c, TODAY).unwrap();
        assert!(p.notices.iter().any(|n| n.code == "no_holdings"));
        hold(&mut c, inv, "债", "bond", 50);
        hold(&mut c, inv, "现金", "cash", 50);
        let p = portfolio_view(&mut c, TODAY).unwrap();
        assert!(!p.notices.iter().any(|n| n.code == "single_market"));
    }

    #[test]
    fn coverage_flags_a_gap_outside_95_to_105_percent() {
        let t = TestDb::new();
        let mut c = t.conn();
        // 登记了持仓的账户余额跟随持仓;差额只来自没登记持仓的投资账户
        let inv = account(&mut c, "投资", "investment", 100_000);
        account(&mut c, "另一个券商", "investment", 20_000);
        hold(&mut c, inv, "债", "bond", 80_000);
        let p = portfolio_view(&mut c, TODAY).unwrap();
        assert_eq!((p.investment_balance, p.coverage_pct, p.gap), (100_000_00, Some(80.0), Some(20_000_00)));
        assert!(p.notices.iter().any(|n| n.code == "coverage" && n.message.contains("20,000.00")));
        let small = account(&mut c, "小账户", "investment", 1);
        hold(&mut c, small, "零头", "bond", 1);
        c.exec_drop("UPDATE account SET archived=1 WHERE name='另一个券商'", ()).unwrap();
        let p = portfolio_view(&mut c, TODAY).unwrap();
        assert_eq!((p.coverage_pct, p.gap), (Some(100.0), None), "都登记了持仓就不会有差额");
    }

    #[test]
    fn stale_holdings_are_listed_after_90_days() {
        let t = TestDb::new();
        let mut c = t.conn();
        let inv = account(&mut c, "投资", "investment", 100);
        hold(&mut c, inv, "新的", "bond", 50);
        c.exec_drop("INSERT INTO holding (account_id, name, asset_class, amount, updated_on) VALUES (?, '老的', 'gold', 5000, '2026-06-29')", (inv,)).unwrap();
        let p = portfolio_view(&mut c, TODAY).unwrap();
        assert_eq!(p.stale.len(), 1);
        assert_eq!((p.stale[0].name.as_str(), p.stale[0].days), ("老的", 120));
        assert!(p.notices.iter().any(|n| n.code == "stale" && n.message.contains("老的")));
    }

    // ----- 建议 -----

    #[test]
    fn only_us_index_rebalance_matches_the_documented_example() {
        let t = TestDb::new();
        let mut c = t.conn();
        let inv = account(&mut c, "投资", "investment", 100_000);
        hold(&mut c, inv, "标普", "us_equity_index", 100_000);
        set_profile(&mut c, &profile("balanced", 30, 10));
        let a = advice(&mut c, TODAY, None).unwrap();
        let rb = a.rebalance.as_ref().unwrap();
        let amt = |k: &str| row(&a, k).diff_amount;
        assert_eq!(amt("us_equity_index"), Some(-73_000_00));
        assert_eq!(amt("other_equity"), Some(18_000_00));
        assert_eq!(amt("bond"), Some(40_000_00));
        assert_eq!(amt("gold"), Some(10_000_00));
        assert_eq!(amt("cash"), Some(5_000_00));
        assert_eq!(row(&a, "us_equity_index").diff_pct, -73.0);
        assert_eq!(row(&a, "us_equity_index").action, "reduce");
        assert_eq!(rb.buy_total, 73_000_00);
        let sum: i64 = rb.rows.iter().map(|r| r.target_amount).sum();
        assert_eq!(sum, 100_000_00, "目标金额合计等于持仓合计");
        assert!(a.disclaimer.contains("不构成投资建议"));
    }

    #[test]
    fn small_gaps_are_balanced_and_get_no_amount_or_candidates() {
        let t = TestDb::new();
        let mut c = t.conn();
        let inv = account(&mut c, "投资", "investment", 100_000);
        hold(&mut c, inv, "美股", "us_equity_index", 28_000);
        hold(&mut c, inv, "非美", "other_equity", 18_000);
        hold(&mut c, inv, "债", "bond", 40_000);
        hold(&mut c, inv, "金", "gold", 9_000);
        hold(&mut c, inv, "现金", "cash", 5_000);
        set_profile(&mut c, &profile("balanced", 30, 10));
        let a = advice(&mut c, TODAY, None).unwrap();
        for r in &a.rebalance.as_ref().unwrap().rows {
            assert_eq!(r.action, "balanced", "{}", r.label);
            assert!(r.diff_amount.is_none() && r.candidates.is_none());
        }
    }

    #[test]
    fn candidates_come_only_from_the_catalog_and_missing_classes_say_so() {
        let t = TestDb::new();
        let mut c = t.conn();
        let inv = account(&mut c, "投资", "investment", 100_000);
        hold(&mut c, inv, "标普", "us_equity_index", 100_000);
        set_profile(&mut c, &profile("balanced", 30, 10));
        for (n, class) in [("债券基金甲", "bond"), ("债券基金乙", "bond"), ("全球股票", "other_equity")] {
            portfolio::add_product(&mut c, &ProductInput { name: n.into(), code: Some("X1".into()), asset_class: class.into(), note: String::new() }).unwrap();
        }
        let a = advice(&mut c, TODAY, None).unwrap();
        let bond = row(&a, "bond").candidates.as_ref().unwrap();
        assert_eq!(bond.products.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["债券基金甲", "债券基金乙"]);
        assert!(bond.hint.is_none());
        let gold = row(&a, "gold").candidates.as_ref().unwrap();
        assert!(gold.products.is_empty());
        assert!(gold.hint.as_deref().unwrap().contains("清单里还没有这一类"));
        assert!(row(&a, "us_equity_index").candidates.is_none(), "减配的大类不列候选");
    }

    #[test]
    fn missing_profile_or_holdings_give_guidance_not_numbers() {
        let t = TestDb::new();
        let mut c = t.conn();
        let inv = account(&mut c, "投资", "investment", 100);
        hold(&mut c, inv, "债", "bond", 100);
        let a = advice(&mut c, TODAY, None).unwrap();
        assert!(a.profile_missing && a.target.is_none() && a.rebalance.is_none());
        assert!(codes(&a).contains(&"no_profile"));

        let t2 = TestDb::new();
        let mut c2 = t2.conn();
        set_profile(&mut c2, &profile("balanced", 30, 10));
        let a = advice(&mut c2, TODAY, None).unwrap();
        assert!(a.target.is_some() && a.rebalance.is_none(), "没有持仓不给再平衡");
    }

    #[test]
    fn holdings_in_other_class_are_listed_but_not_rebalanced() {
        let t = TestDb::new();
        let mut c = t.conn();
        let inv = account(&mut c, "投资", "investment", 100_000);
        hold(&mut c, inv, "收藏品", "other", 50_000);
        hold(&mut c, inv, "标普", "us_equity_index", 50_000);
        set_profile(&mut c, &profile("balanced", 30, 10));
        let a = advice(&mut c, TODAY, None).unwrap();
        assert_eq!(a.portfolio.total, 100_000_00);
        let rb = a.rebalance.as_ref().unwrap();
        assert_eq!(rb.rows.iter().map(|r| r.target_amount).sum::<i64>(), 50_000_00, "只对非“其他”的部分配比");
        // 只有“其他”时没有可配比的部分
        let t2 = TestDb::new();
        let mut c2 = t2.conn();
        let inv = account(&mut c2, "投资", "investment", 100);
        hold(&mut c2, inv, "收藏品", "other", 100);
        set_profile(&mut c2, &profile("balanced", 30, 10));
        assert!(advice(&mut c2, TODAY, None).unwrap().rebalance.is_none());
    }

    // ----- 短期目标预留 -----

    fn save_goal(c: &mut Conn, name: &str, target: i64, deadline: Option<&str>, acc: i64) {
        goals::add_goal(c, &goals::GoalInput { name: name.into(), kind: "save".into(), target: target * 100, deadline: deadline.map(String::from), account_ids: vec![acc] }, TODAY).unwrap();
    }

    #[test]
    fn short_term_goal_money_is_reserved_in_cash() {
        let t = TestDb::new();
        let mut c = t.conn();
        let inv = account(&mut c, "投资", "investment", 100_000);
        let sav = account(&mut c, "首付存款", "deposit", 0);
        hold(&mut c, inv, "标普", "us_equity_index", 100_000);
        set_profile(&mut c, &profile("balanced", 30, 10));
        save_goal(&mut c, "首付", 20_000, Some("2027-10-31"), sav); // 12 个月后,还差 20,000
        let a = advice(&mut c, TODAY, None).unwrap();
        let rb = a.rebalance.as_ref().unwrap();
        assert_eq!(rb.reserved_for_short_term_goals, 20_000_00);
        let cash = row(&a, "cash");
        assert_eq!(cash.target_amount, 20_000_00 + 80_000_00 * 5 / 100, "现金类 = 预留 + 其余 × 5%");
        assert_eq!(rb.rows.iter().map(|r| r.target_amount).sum::<i64>(), 100_000_00);
    }

    #[test]
    fn long_or_open_goals_do_not_reserve_and_reservation_is_capped() {
        let t = TestDb::new();
        let mut c = t.conn();
        let inv = account(&mut c, "投资", "investment", 100_000);
        let sav = account(&mut c, "存款", "deposit", 0);
        hold(&mut c, inv, "标普", "us_equity_index", 100_000);
        set_profile(&mut c, &profile("balanced", 30, 10));
        save_goal(&mut c, "远期", 50_000, Some("2031-01-31"), sav); // 超过 36 个月
        save_goal(&mut c, "没期限", 50_000, None, sav);
        assert_eq!(advice(&mut c, TODAY, None).unwrap().rebalance.unwrap().reserved_for_short_term_goals, 0);
        save_goal(&mut c, "大额近期", 300_000, Some("2027-06-30"), sav);
        let a = advice(&mut c, TODAY, None).unwrap();
        let rb = a.rebalance.unwrap();
        assert_eq!(rb.reserved_for_short_term_goals, 100_000_00, "预留不超过持仓合计");
        assert_eq!(rb.rows.iter().find(|r| r.asset_class == "cash").unwrap().target_amount, 100_000_00);
    }

    // ----- 前置条件 -----

    #[test]
    fn emergency_fund_months_and_priority_notice() {
        let t = TestDb::new();
        let mut c = t.conn();
        account(&mut c, "存款", "deposit", 20_000);
        monthly_spend(&mut c, 10_000);
        let a = advice(&mut c, TODAY, None).unwrap();
        assert_eq!(a.readiness.emergency_months, Some(2.0));
        assert_eq!(a.notices[0].code, "emergency_fund");
        assert_eq!(a.notices[0].level, "priority");

        let t2 = TestDb::new();
        let mut c2 = t2.conn();
        account(&mut c2, "存款", "deposit", 60_000);
        monthly_spend(&mut c2, 10_000);
        let a = advice(&mut c2, TODAY, None).unwrap();
        assert_eq!(a.readiness.emergency_months, Some(6.0));
        assert!(!codes(&a).contains(&"emergency_fund"), "6 个月时不出现优先级提示");
    }

    #[test]
    fn no_expense_history_means_unknown_not_blocked() {
        let t = TestDb::new();
        let mut c = t.conn();
        account(&mut c, "存款", "deposit", 20_000);
        let a = advice(&mut c, TODAY, None).unwrap();
        assert_eq!(a.readiness.emergency_months, None);
        let n = a.notices.iter().find(|n| n.code == "emergency_fund").unwrap();
        assert_eq!((n.level.as_str(), n.message.contains("数据不足")), ("info", true));
    }

    #[test]
    fn high_interest_debt_is_judged_by_registered_rate() {
        let t = TestDb::new();
        let mut c = t.conn();
        let card = account(&mut c, "信用卡", "liability", 200_000);
        portfolio::set_account_rate(&mut c, card, Some(840)).unwrap();
        let a = advice(&mut c, TODAY, None).unwrap();
        assert_eq!(a.readiness.high_interest_debts.len(), 1);
        assert_eq!(a.readiness.high_interest_debts[0].yearly_interest, 16_800_00);
        assert_eq!(a.notices[0].code, "high_interest_debt");
        assert_eq!(a.notices[0].level, "priority");
        assert!(a.notices[0].message.contains("16,800.00"));

        portfolio::set_account_rate(&mut c, card, Some(790)).unwrap(); // 7.9%
        let a = advice(&mut c, TODAY, None).unwrap();
        assert!(a.readiness.high_interest_debts.is_empty() && !codes(&a).contains(&"high_interest_debt"));
    }

    #[test]
    fn declared_high_debt_only_applies_to_debts_without_a_rate() {
        let t = TestDb::new();
        let mut c = t.conn();
        let card = account(&mut c, "消费贷", "liability", 50_000);
        let mut p = profile("balanced", 30, 10);
        p.high_debt = true;
        set_profile(&mut c, &p);
        let a = advice(&mut c, TODAY, None).unwrap();
        assert!(a.readiness.high_debt_declared && codes(&a).contains(&"high_interest_debt"));
        assert!(a.notices.iter().find(|n| n.code == "high_interest_debt").unwrap().message.contains("登记各负债的年利率"));

        // 登记了且低于 8%:忽略自述
        portfolio::set_account_rate(&mut c, card, Some(450)).unwrap();
        let a = advice(&mut c, TODAY, None).unwrap();
        assert!(!a.readiness.high_debt_declared && !codes(&a).contains(&"high_interest_debt"));
    }

    #[test]
    fn priority_notices_come_first_and_debt_before_emergency_fund() {
        let t = TestDb::new();
        let mut c = t.conn();
        let inv = account(&mut c, "投资", "investment", 100_000);
        hold(&mut c, inv, "标普", "us_equity_index", 100_000);
        let card = account(&mut c, "信用卡", "liability", 20_000);
        portfolio::set_account_rate(&mut c, card, Some(1800)).unwrap();
        account(&mut c, "存款", "deposit", 5_000);
        monthly_spend(&mut c, 10_000);
        let a = advice(&mut c, TODAY, None).unwrap();
        assert_eq!(&codes(&a)[..2], ["high_interest_debt", "emergency_fund"]);
        assert!(a.rebalance.is_none() || a.profile_missing, "没填问卷时仍不给目标");
        // 优先级提示之后才是集中度之类
        assert!(a.notices.iter().position(|n| n.code == "concentration").unwrap() > 1);
    }

    #[test]
    fn surplus_drives_months_to_fill_and_a_thin_surplus_is_called_out() {
        let t = TestDb::new();
        let mut c = t.conn();
        let inv = account(&mut c, "投资", "investment", 100_000);
        hold(&mut c, inv, "标普", "us_equity_index", 100_000);
        set_profile(&mut c, &profile("balanced", 30, 10));
        // 7、8、9 月每月收入 20,000、支出 15,000 → 结余 5,000;需增配 73,000
        for m in ["2026-07-10", "2026-08-10", "2026-09-10"] {
            core::add_txn(&mut c, None, None, "income", 20_000_00, m, "").unwrap();
            core::add_txn(&mut c, None, None, "expense", 15_000_00, m, "").unwrap();
        }
        let a = advice(&mut c, TODAY, None).unwrap();
        assert_eq!(a.readiness.avg_monthly_surplus, Some(5_000_00));
        assert_eq!(a.rebalance.as_ref().unwrap().months_to_fill, Some(15), "73,000 ÷ 5,000 向上取整");

        let t2 = TestDb::new();
        let mut c2 = t2.conn();
        let inv = account(&mut c2, "投资", "investment", 100_000);
        hold(&mut c2, inv, "标普", "us_equity_index", 100_000);
        set_profile(&mut c2, &profile("balanced", 30, 10));
        monthly_spend(&mut c2, 8_000); // 只有支出 → 结余为负
        let a = advice(&mut c2, TODAY, None).unwrap();
        assert_eq!(a.rebalance.as_ref().unwrap().months_to_fill, None);
        assert!(codes(&a).contains(&"thin_surplus"));
    }

    // ----- 利息视角 -----

    #[test]
    fn idle_cash_and_the_extra_interest_against_a_reference_rate() {
        let t = TestDb::new();
        let mut c = t.conn();
        let cur = account(&mut c, "活期", "cash", 200_000);
        portfolio::set_account_rate(&mut c, cur, Some(20)).unwrap(); // 0.2%
        monthly_spend(&mut c, 10_000);
        let a = advice(&mut c, TODAY, Some(200)).unwrap(); // 参考 2.0%
        let i = &a.interest;
        assert_eq!(i.idle_cash, Some(140_000_00), "200,000 − 6 × 10,000");
        assert_eq!(i.weighted_rate_bps, Some(20.0));
        assert_eq!(i.reference_yearly_interest, Some(2_800_00));
        assert_eq!(i.extra_yearly_interest, Some(2_520_00));
        assert!(i.hint.is_none());
    }

    #[test]
    fn no_idle_cash_or_unrated_accounts_degrade_gracefully() {
        let t = TestDb::new();
        let mut c = t.conn();
        account(&mut c, "活期", "cash", 50_000);
        monthly_spend(&mut c, 10_000); // 6 个月 = 60,000 > 余额
        let a = advice(&mut c, TODAY, Some(200)).unwrap();
        assert_eq!(a.interest.idle_cash, Some(0));
        assert_eq!((a.interest.extra_yearly_interest, a.interest.reference_yearly_interest), (None, None));

        let t2 = TestDb::new();
        let mut c2 = t2.conn();
        account(&mut c2, "活期", "cash", 200_000); // 没登记利率
        monthly_spend(&mut c2, 10_000);
        let a = advice(&mut c2, TODAY, Some(200)).unwrap();
        assert_eq!(a.interest.reference_yearly_interest, Some(2_800_00));
        assert_eq!(a.interest.extra_yearly_interest, None);
        assert!(a.interest.hint.as_deref().unwrap().contains("登记"));
        // 没给参考利率:什么对比都不出
        let a = advice(&mut c2, TODAY, None).unwrap();
        assert_eq!((a.interest.reference_yearly_interest, a.interest.hint), (None, None));
        assert!(advice(&mut c2, TODAY, Some(20_000)).is_err());
    }

    #[test]
    fn paying_off_a_rated_debt_is_a_certain_yearly_saving() {
        let t = TestDb::new();
        let mut c = t.conn();
        let card = account(&mut c, "信用卡", "liability", 50_000);
        let loan = account(&mut c, "房贷", "liability", 800_000);
        portfolio::set_account_rate(&mut c, card, Some(1800)).unwrap();
        portfolio::set_account_rate(&mut c, loan, Some(350)).unwrap();
        let a = advice(&mut c, TODAY, None).unwrap();
        let d = &a.interest.debts;
        assert_eq!((d[0].name.as_str(), d[0].yearly_interest, d[0].high), ("信用卡", 9_000_00, true));
        assert_eq!((d[1].name.as_str(), d[1].yearly_interest, d[1].high), ("房贷", 28_000_00, false));
    }

    // ----- 助手工具 -----

    #[test]
    fn tools_return_yuan_and_flag_a_missing_profile() {
        let t = TestDb::new();
        let mut c = t.conn();
        let inv = account(&mut c, "投资", "investment", 100_000);
        hold(&mut c, inv, "标普", "us_equity_index", 100_000);
        let v = run(&mut c, TOOL_ADVICE, &json!({}), TODAY).unwrap().unwrap();
        assert_eq!(v["profile_missing"], true);
        assert!(v["target"].is_null() && v["rebalance"].is_null());
        assert_eq!(v["portfolio"]["total"], 100_000.0, "金额已转成元");

        set_profile(&mut c, &profile("balanced", 30, 10));
        let v = run(&mut c, TOOL_ADVICE, &json!({}), TODAY).unwrap().unwrap();
        let rows = v["rebalance"]["rows"].as_array().unwrap();
        let us = rows.iter().find(|r| r["asset_class"] == "us_equity_index").unwrap();
        assert_eq!((us["diff_amount"].as_f64(), us["current_amount"].as_f64()), (Some(-73_000.0), Some(100_000.0)));
        assert_eq!(v["rebalance"]["buy_total"], 73_000.0);
        assert!(v["disclaimer"].as_str().unwrap().contains("不构成投资建议"));

        let p = run(&mut c, TOOL_PORTFOLIO, &json!({}), TODAY).unwrap().unwrap();
        assert_eq!(p["holdings"][0]["amount"], 100_000.0);
        assert!(run(&mut c, "list_accounts", &json!({}), TODAY).is_none());
    }

    #[test]
    fn advice_tool_takes_a_reference_rate_in_percent_and_validates_it() {
        let t = TestDb::new();
        let mut c = t.conn();
        account(&mut c, "活期", "cash", 200_000);
        monthly_spend(&mut c, 10_000);
        let v = run(&mut c, TOOL_ADVICE, &json!({"reference_rate": "2"}), TODAY).unwrap().unwrap();
        assert_eq!(v["interest"]["reference_yearly_interest"], 2_800.0);
        let e = run(&mut c, TOOL_ADVICE, &json!({"reference_rate": 300}), TODAY).unwrap().unwrap_err();
        assert!(e.to_string().contains("reference_rate"));
    }

    #[test]
    fn tools_are_read_only_and_money_keys_are_complete() {
        let t = TestDb::new();
        let mut c = t.conn();
        let inv = account(&mut c, "投资", "investment", 100_000);
        hold(&mut c, inv, "标普", "us_equity_index", 100_000);
        let before: i64 = c.query_first("SELECT COUNT(*) FROM holding").unwrap().unwrap();
        for n in [TOOL_PORTFOLIO, TOOL_ADVICE] {
            run(&mut c, n, &json!({}), TODAY).unwrap().unwrap();
        }
        assert_eq!(c.query_first::<i64, _>("SELECT COUNT(*) FROM holding").unwrap().unwrap(), before);
        let names: Vec<String> = tool_defs().iter().map(|d| d["function"]["name"].as_str().unwrap().to_string()).collect();
        assert!(names.iter().all(|n| n.starts_with("get_")), "{names:?}");

        // 一份有数据的完整结果里,没有任何“分”的痕迹:所有金额字段都被转换过(不会是 ≥ 1e6 且为整百的大数)
        set_profile(&mut c, &profile("balanced", 30, 10));
        let v = run(&mut c, TOOL_ADVICE, &json!({}), TODAY).unwrap().unwrap();
        fn walk(v: &Value, path: &str, bad: &mut Vec<String>) {
            match v {
                Value::Object(m) => m.iter().for_each(|(k, x)| walk(x, &format!("{path}.{k}"), bad)),
                Value::Array(a) => a.iter().enumerate().for_each(|(i, x)| walk(x, &format!("{path}[{i}]"), bad)),
                Value::Number(n) => {
                    let f = n.as_f64().unwrap();
                    let looks_like_cents = f.fract() == 0.0 && f.abs() >= 1_000_000.0 && !path.ends_with(".id") && !path.contains("rate_bps");
                    if looks_like_cents {
                        bad.push(format!("{path} = {f}"));
                    }
                }
                _ => {}
            }
        }
        let mut bad = Vec::new();
        walk(&v, "", &mut bad);
        assert!(bad.is_empty(), "疑似没有转成元的字段:{bad:?}");
    }
}
