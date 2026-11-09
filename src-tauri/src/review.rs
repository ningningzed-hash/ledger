//! 月度复盘:数字全部由程序算好并排成表格,模型只负责写解读和建议;可以保存到报表页。
use crate::agent::yuan;
use crate::core::{self, Error, Result};
use crate::report;
use chrono::{Datelike, NaiveDate};
use mysql::prelude::Queryable;
use serde::Serialize;
use serde_json::json;

const TOP_CATEGORIES: usize = 8;
const TOP_EXPENSES: usize = 5;

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct CatLine {
    pub name: String,
    pub total: i64,
    pub share_pct: f64,
    pub prev: i64,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct TxnLine {
    pub date: String,
    pub amount: i64,
    pub category: String,
    pub note: String,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct FixedLine {
    pub name: String,
    pub amount: i64,
    pub date: String,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct GoalLine {
    pub name: String,
    pub progress_pct: f64,
    pub status: String,
    pub required_per_month: Option<i64>,
}

/// 复盘用的事实。金额为分
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Facts {
    pub month: String,
    /// 当前月还没过完
    pub partial: bool,
    /// 当前月已过天数占比(%);往月为 100
    pub month_progress_pct: f64,
    pub txn_count: usize,
    pub income: i64,
    pub expense: i64,
    pub net: i64,
    pub savings_rate_pct: Option<f64>,
    pub budget: Option<i64>,
    pub budget_used_pct: Option<f64>,
    pub prev_expense: i64,
    pub expense_change_pct: Option<f64>,
    pub categories: Vec<CatLine>,
    pub largest: Vec<TxnLine>,
    pub fixed_charged: Vec<FixedLine>,
    pub fixed_monthly: i64,
    pub goals: Vec<GoalLine>,
    /// 投资表现;没有任何能算出盈亏的持仓时为空
    pub investment: Option<InvestFacts>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct HoldLine {
    pub name: String,
    pub pnl: i64,
    pub pnl_pct: f64,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct InvestFacts {
    /// 数字取自实时持仓(本月进行中,或那个月末之前没有快照)而不是月末快照
    pub live: bool,
    pub value: i64,
    pub cost: i64,
    pub pnl: i64,
    pub pnl_pct: Option<f64>,
    /// 本月盈亏变化;快照不足时为空
    pub month_pnl: Option<i64>,
    /// 本月盈亏的起算快照日期;不早于月初时说明只能从这天算起
    pub month_since: Option<String>,
    pub month_full: bool,
    /// 按当前收益率选出的最好、最差持仓(快照只记总额,不记单只)
    pub best: Option<HoldLine>,
    pub worst: Option<HoldLine>,
}

fn investment(c: &mut impl Queryable, month: &str, today: &str, partial: bool) -> Result<Option<InvestFacts>> {
    use crate::portfolio;
    let s = portfolio::pnl_summary(c, today)?;
    if s.counted == 0 {
        return Ok(None);
    }
    let end = report::month_end(month)?;
    let snap: Option<(i64, i64)> =
        if partial { None } else { c.exec_first("SELECT value, cost FROM holding_snapshot WHERE date <= ? ORDER BY date DESC LIMIT 1", (&end,))? };
    let live = snap.is_none();
    let (value, cost) = snap.unwrap_or((s.value, s.cost));
    let period = if partial || !live { portfolio::period_pnl(c, &format!("{month}-01"), &end, today, (s.value, s.cost))? } else { None };
    let mut lines: Vec<HoldLine> = portfolio::list_holdings(c)?
        .into_iter()
        .filter_map(|h| Some(HoldLine { pnl: h.pnl?, pnl_pct: h.pnl_pct?, name: h.name }))
        .collect();
    lines.sort_by(|a, b| b.pnl_pct.total_cmp(&a.pnl_pct));
    let best = lines.first().cloned();
    let worst = (lines.len() > 1).then(|| lines.last().cloned()).flatten();
    Ok(Some(InvestFacts {
        live,
        value,
        cost,
        pnl: value - cost,
        pnl_pct: portfolio::pct_of(value - cost, cost),
        month_pnl: period.as_ref().map(|p| p.pnl),
        month_since: period.as_ref().map(|p| p.since.clone()),
        month_full: period.as_ref().is_some_and(|p| p.full),
        best,
        worst,
    }))
}

fn pct(a: i64, b: i64) -> Option<f64> {
    (b != 0).then(|| (a as f64 / b as f64 * 1000.0).round() / 10.0)
}

fn goal_status(s: &str) -> &'static str {
    match s {
        "achieved" => "已达成",
        "overdue" => "已逾期",
        "on_track" => "进度正常",
        "behind" => "落后",
        "open" => "无截止日",
        _ => "数据不足",
    }
}

pub fn facts(c: &mut impl Queryable, month: &str, today: &str) -> Result<Facts> {
    let first = NaiveDate::parse_from_str(&format!("{month}-01"), "%Y-%m-%d").map_err(|_| Error::Invalid(format!("月份格式应为 YYYY-MM:{month}")))?;
    let today_d = NaiveDate::parse_from_str(today, "%Y-%m-%d").map_err(|_| Error::Invalid(format!("日期格式不对:{today}")))?;
    if first > today_d {
        return Err(Error::Invalid("这个月还没开始,没有可复盘的数据".into()));
    }
    let partial = month == &today[..7];
    let days = NaiveDate::parse_from_str(&report::month_end(month)?, "%Y-%m-%d").map(|d| d.day()).unwrap_or(30);
    let month_progress_pct = if partial { (today_d.day() as f64 / days as f64 * 1000.0).round() / 10.0 } else { 100.0 };
    let prev = report::shift_back(month, 1)?;

    let txns = core::list_txns(c, month)?;
    let st = core::budget_status(c, month)?;
    let prev_st = core::budget_status(c, &prev)?;
    let prev_cats = core::category_summary(c, &prev)?;
    let categories = core::category_summary(c, month)?
        .into_iter()
        .take(TOP_CATEGORIES)
        .map(|s| CatLine {
            share_pct: pct(s.total, st.spent).unwrap_or(0.0),
            prev: prev_cats.iter().find(|p| p.category == s.category).map(|p| p.total).unwrap_or(0),
            name: s.category,
            total: s.total,
        })
        .collect();
    let mut expenses: Vec<&core::Txn> = txns.iter().filter(|t| t.kind == "expense").collect();
    expenses.sort_by(|a, b| b.amount.cmp(&a.amount).then(a.date.cmp(&b.date)));
    let largest = expenses
        .iter()
        .take(TOP_EXPENSES)
        .map(|t| TxnLine { date: t.date.clone(), amount: t.amount, category: t.category.clone().unwrap_or_else(|| "未分类".into()), note: t.note.clone() })
        .collect();
    let fixed_charged = c.exec_map(
        "SELECT r.name, t.amount, t.date FROM recurring_run rr
         JOIN recurring r ON r.id = rr.recurring_id JOIN txn t ON t.id = rr.txn_id
         WHERE t.kind = 'expense' AND SUBSTR(t.date,1,7) = ? ORDER BY t.date, r.name",
        (month,),
        |(name, amount, date)| FixedLine { name, amount, date },
    )?;
    let fixed_monthly = crate::recurring::list_recurring(c, today)?.monthly_expense;
    let goals = crate::goals::list_goals(c, today)?
        .into_iter()
        .map(|g| GoalLine { name: g.name, progress_pct: (g.progress * 1000.0).round() / 10.0, status: goal_status(&g.status).into(), required_per_month: g.required_per_month })
        .collect();
    let investment = investment(c, month, today, partial)?;

    Ok(Facts {
        month: month.into(),
        partial,
        month_progress_pct,
        txn_count: txns.len(),
        income: st.income,
        expense: st.spent,
        net: st.income - st.spent,
        savings_rate_pct: (st.income > 0).then(|| pct(st.income - st.spent, st.income)).flatten(),
        budget: st.budget,
        budget_used_pct: st.budget.and_then(|b| pct(st.spent, b)),
        prev_expense: prev_st.spent,
        expense_change_pct: pct(st.spent - prev_st.spent, prev_st.spent),
        categories,
        largest,
        fixed_charged,
        fixed_monthly,
        goals,
        investment,
    })
}

fn money(c: i64) -> String {
    let y = yuan(c.abs());
    let s = format!("{y:.2}");
    let (int, dec) = s.split_once('.').unwrap_or((&s, "00"));
    let mut out = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    format!("{}¥{out}.{dec}", if c < 0 { "-" } else { "" })
}

fn signed(c: i64) -> String {
    if c > 0 { format!("+{}", money(c)) } else { money(c) }
}

fn md_cell(s: &str) -> String {
    s.replace('|', "/").replace('\n', " ")
}

/// 程序生成的数字部分(Markdown)
pub fn facts_markdown(f: &Facts) -> String {
    let (y, m) = f.month.split_once('-').unwrap_or((&f.month, ""));
    let mut o = format!("## {y} 年 {} 月复盘\n\n", m.trim_start_matches('0'));
    if f.partial {
        o += &format!("注:本月还没过完(已过 {}%),以下是截至今天的数据。\n\n", f.month_progress_pct);
    }
    o += "| 项目 | 金额 |\n|---|---|\n";
    o += &format!("| 收入 | {} |\n| 支出 | {} |\n| 结余 | {} |\n", money(f.income), money(f.expense), signed(f.net));
    if let Some(r) = f.savings_rate_pct {
        o += &format!("| 储蓄率 | {r}% |\n");
    }
    match (f.budget, f.budget_used_pct) {
        (Some(b), Some(p)) => o += &format!("| 预算 | {}(已用 {p}%) |\n", money(b)),
        _ => o += "| 预算 | 未设置 |\n",
    }
    o += &format!(
        "| 上月支出 | {}{} |\n\n",
        money(f.prev_expense),
        f.expense_change_pct.map(|p| format!("(本月{}{p}%)", if p >= 0.0 { "+" } else { "" })).unwrap_or_default()
    );
    if !f.categories.is_empty() {
        o += "### 钱花在哪了\n\n| 分类 | 本月 | 占比 | 上月 | 变化 |\n|---|---|---|---|---|\n";
        for cl in &f.categories {
            o += &format!("| {} | {} | {}% | {} | {} |\n", md_cell(&cl.name), money(cl.total), cl.share_pct, money(cl.prev), signed(cl.total - cl.prev));
        }
        o += "\n";
    }
    if !f.largest.is_empty() {
        o += "### 最大的几笔支出\n\n| 日期 | 金额 | 分类 | 备注 |\n|---|---|---|---|\n";
        for t in &f.largest {
            o += &format!("| {} | {} | {} | {} |\n", t.date, money(t.amount), md_cell(&t.category), md_cell(&t.note));
        }
        o += "\n";
    }
    o += &format!("### 订阅与固定支出\n\n每月固定支出合计 {}。", money(f.fixed_monthly));
    if f.fixed_charged.is_empty() {
        o += "本月还没有自动扣费的订阅。\n\n";
    } else {
        let total: i64 = f.fixed_charged.iter().map(|x| x.amount).sum();
        o += &format!("本月已自动记入 {} 笔,共 {}:\n\n", f.fixed_charged.len(), money(total));
        for x in &f.fixed_charged {
            o += &format!("- {} {} {}\n", x.date, md_cell(&x.name), money(x.amount));
        }
        o += "\n";
    }
    if !f.goals.is_empty() {
        o += "### 目标\n\n| 目标 | 进度 | 状态 | 每月还需 |\n|---|---|---|---|\n";
        for g in &f.goals {
            o += &format!("| {} | {}% | {} | {} |\n", md_cell(&g.name), g.progress_pct, g.status, g.required_per_month.map(money).unwrap_or_else(|| "—".into()));
        }
        o += "\n";
    }
    if let Some(i) = &f.investment {
        o += &invest_markdown(i, f.partial);
    }
    o
}

fn pct_text(p: f64) -> String {
    format!("{}{p:.2}%", if p > 0.0 { "+" } else { "" })
}

fn invest_markdown(i: &InvestFacts, partial: bool) -> String {
    let when = if partial { "截至今天" } else if i.live { "当前(这个月末之前还没有记录)" } else { "月末" };
    let mut o = format!("### 投资\n\n只统计填了成本价的持仓,盈亏按原币价格计算、不含汇率变化。\n\n| 项目 | 金额 |\n|---|---|\n");
    o += &format!("| {when}市值 | {} |\n| 成本 | {} |\n", money(i.value), money(i.cost));
    o += &format!("| 累计盈亏 | {}{} |\n", signed(i.pnl), i.pnl_pct.map(|p| format!("({})", pct_text(p))).unwrap_or_default());
    let month = match (i.month_pnl, &i.month_since) {
        (Some(p), Some(since)) if !i.month_full => format!("{}(从 {} 开始统计)", signed(p), &since[5..]),
        (Some(p), _) => signed(p),
        _ => "数据不足".into(),
    };
    o += &format!("| 本月盈亏变化 | {month} |\n\n");
    if let Some(b) = &i.best {
        o += &format!("- 收益率最高:{} {}({})\n", md_cell(&b.name), pct_text(b.pnl_pct), signed(b.pnl));
    }
    if let Some(w) = &i.worst {
        o += &format!("- 收益率最低:{} {}({})\n", md_cell(&w.name), pct_text(w.pnl_pct), signed(w.pnl));
    }
    o + "\n"
}

/// 交给模型的系统提示:只解读,不算数
pub fn interpret_prompt(persona: &str) -> String {
    format!(
        "{}\n\n你在为用户写月度复盘的「解读」部分。用户会给你一份由程序算好的 JSON 数据(金额单位:元)。\n\
         规则:\n- 只能引用数据里已有的数字,不要自己做加减乘除、不要算新的百分比或合计、不要估算。\n\
         - 数据里没有的就不要说;本月没过完(partial=true)时提醒这是截至今天的情况。\n\
         - 用 3~5 个短段落:整体、花在哪(和上月比明显变化的分类)、预算、订阅与固定支出、目标、投资(investment 有值时);没数据的部分跳过。\n\
         - 投资部分只描述盈亏数字,不预测行情,不给买卖建议。\n\
         - 最后给 1~3 条下个月可以执行的具体建议,用列表。\n- 不要输出标题,不要重复表格,直接写正文;总长度不超过 350 字。",
        persona.trim()
    )
}

pub fn facts_for_model(f: &Facts) -> String {
    let y = |c: i64| yuan(c);
    json!({
        "month": f.month, "partial": f.partial, "month_progress_pct": f.month_progress_pct, "txn_count": f.txn_count,
        "income": y(f.income), "expense": y(f.expense), "net": y(f.net), "savings_rate_pct": f.savings_rate_pct,
        "budget": f.budget.map(y), "budget_used_pct": f.budget_used_pct,
        "prev_month_expense": y(f.prev_expense), "expense_change_pct_vs_prev": f.expense_change_pct,
        "categories": f.categories.iter().map(|c| json!({"name": c.name, "total": y(c.total), "share_pct": c.share_pct, "prev_month": y(c.prev), "change": y(c.total - c.prev)})).collect::<Vec<_>>(),
        "largest_expenses": f.largest.iter().map(|t| json!({"date": t.date, "amount": y(t.amount), "category": t.category, "note": t.note})).collect::<Vec<_>>(),
        "subscriptions_charged_this_month": f.fixed_charged.iter().map(|x| json!({"name": x.name, "amount": y(x.amount), "date": x.date})).collect::<Vec<_>>(),
        "fixed_expense_per_month": y(f.fixed_monthly),
        "goals": f.goals.iter().map(|g| json!({"name": g.name, "progress_pct": g.progress_pct, "status": g.status, "required_per_month": g.required_per_month.map(y)})).collect::<Vec<_>>(),
        "investment": f.investment.as_ref().map(|i| json!({
            "note": "只含填了成本价的持仓;盈亏按原币价格计算,不含汇率变化",
            "from_live_holdings": i.live, "value": y(i.value), "cost": y(i.cost), "total_pnl": y(i.pnl), "total_pnl_pct": i.pnl_pct,
            "month_pnl": i.month_pnl.map(y), "month_pnl_counted_from": i.month_since, "month_pnl_covers_whole_month": i.month_full,
            "best_holding": i.best.as_ref().map(|b| json!({"name": b.name, "pnl": y(b.pnl), "pnl_pct": b.pnl_pct})),
            "worst_holding": i.worst.as_ref().map(|b| json!({"name": b.name, "pnl": y(b.pnl), "pnl_pct": b.pnl_pct})),
        })),
    })
    .to_string()
}

/// 完整复盘:数字表格 + 模型解读。commentary 为空表示模型不可用,只给数字
pub fn compose(facts_md: &str, commentary: std::result::Result<String, String>) -> String {
    match commentary {
        Ok(t) => format!("{facts_md}### 助手解读\n\n{}\n", t.trim()),
        Err(e) => format!("{facts_md}注:助手解读暂时不可用({e}),以上数字仍然有效。\n"),
    }
}

// ---------- 保存 ----------

#[derive(Serialize, Debug, PartialEq)]
pub struct Saved {
    pub month: String,
    pub content: String,
    pub created_at: String,
}

pub fn save(c: &mut impl Queryable, month: &str, content: &str) -> Result<()> {
    if content.trim().is_empty() {
        return Err(Error::Invalid("复盘内容为空".into()));
    }
    NaiveDate::parse_from_str(&format!("{month}-01"), "%Y-%m-%d").map_err(|_| Error::Invalid(format!("月份格式应为 YYYY-MM:{month}")))?;
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    c.exec_drop("REPLACE INTO review (month, content, created_at) VALUES (?, ?, ?)", (month, content, now))?;
    Ok(())
}

pub fn list(c: &mut impl Queryable) -> Result<Vec<Saved>> {
    Ok(c.query_map("SELECT month, content, created_at FROM review ORDER BY month DESC", |(month, content, created_at)| Saved { month, content, created_at })?)
}

pub fn delete(c: &mut impl Queryable, month: &str) -> Result<()> {
    c.exec_drop("DELETE FROM review WHERE month = ?", (month,))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdb::TestDb;

    #[test]
    fn facts_are_computed_by_program() {
        let t = TestDb::new();
        let mut c = t.conn();
        let food = core::find_category(&mut c, "餐饮", "expense").unwrap();
        let fun = core::find_category(&mut c, "娱乐", "expense").unwrap();
        let pay = core::find_category(&mut c, "工资", "income").unwrap();
        core::add_txn(&mut c, None, food, "expense", 300_00, "2026-09-10", "").unwrap();
        core::add_txn(&mut c, None, food, "expense", 500_00, "2026-10-05", "聚餐").unwrap();
        core::add_txn(&mut c, None, fun, "expense", 1500_00, "2026-10-12", "演唱会").unwrap();
        core::add_txn(&mut c, None, pay, "income", 10000_00, "2026-10-10", "").unwrap();
        core::set_budget(&mut c, "2026-10", None, 4000_00).unwrap();

        let f = facts(&mut c, "2026-10", "2026-10-15").unwrap();
        assert!(f.partial);
        assert_eq!(f.month_progress_pct, 48.4);
        assert_eq!((f.income, f.expense, f.net, f.savings_rate_pct), (10000_00, 2000_00, 8000_00, Some(80.0)));
        assert_eq!((f.budget_used_pct, f.prev_expense, f.expense_change_pct), (Some(50.0), 300_00, Some(566.7)));
        assert_eq!(f.categories[0], CatLine { name: "娱乐".into(), total: 1500_00, share_pct: 75.0, prev: 0 });
        assert_eq!(f.categories[1].prev, 300_00);
        assert_eq!(f.largest[0].note, "演唱会");

        let md = facts_markdown(&f);
        assert!(md.contains("## 2026 年 10 月复盘") && md.contains("截至今天"), "{md}");
        assert!(md.contains("| 支出 | ¥2,000.00 |") && md.contains("| 餐饮 | ¥500.00 | 25% | ¥300.00 | +¥200.00 |"), "{md}");
        assert!(facts(&mut c, "2026-11", "2026-10-15").is_err(), "未来月份不能复盘");
        let full = compose(&md, Err("连接失败".into()));
        assert!(full.contains("暂时不可用"));
        assert!(facts_for_model(&f).contains("\"expense\":2000.0"));
    }

    fn holding(c: &mut crate::db::Conn, name: &str, secid: &str, yuan: i64, cost_price: f64, price: f64) {
        let acc: Option<i64> = c.query_first("SELECT id FROM account WHERE kind='investment' LIMIT 1").unwrap();
        let acc = acc.unwrap_or_else(|| core::add_account(c, "券商", "investment", "CNY", 1_00, "2026-09-01").unwrap());
        c.exec_drop(
            "INSERT INTO holding (account_id, name, asset_class, amount, updated_on, shares, secid, cost_price, price) VALUES (?, ?, 'other', ?, '2026-11-01', 10000, ?, ?, ?)",
            (acc, name, yuan * 100, secid, cost_price, price),
        )
        .unwrap();
    }

    fn snap(c: &mut crate::db::Conn, date: &str, value: i64, cost: i64) {
        c.exec_drop("INSERT INTO holding_snapshot (date, value, cost) VALUES (?,?,?)", (date, value * 100, cost * 100)).unwrap();
    }

    #[test]
    fn investment_section_only_with_costs() {
        let t = TestDb::new();
        let mut c = t.conn();
        let f = facts(&mut c, "2026-10", "2026-11-02").unwrap();
        assert_eq!(f.investment, None);
        assert!(!facts_markdown(&f).contains("### 投资"), "没有成本数据时省略");
        assert!(facts_for_model(&f).contains("\"investment\":null"));
    }

    #[test]
    fn investment_for_a_finished_month_uses_snapshots() {
        let t = TestDb::new();
        let mut c = t.conn();
        holding(&mut c, "VOO", "107.VOO", 39600, 500.0, 550.0); // +10%
        holding(&mut c, "茅台", "1.600519", 9000, 1000.0, 900.0); // -10%
        snap(&mut c, "2026-09-30", 33000, 30000); // +3000
        snap(&mut c, "2026-10-31", 34500, 30000); // +4500
        let i = facts(&mut c, "2026-10", "2026-11-02").unwrap().investment.unwrap();
        assert_eq!((i.live, i.value, i.pnl, i.month_pnl, i.month_full), (false, 34500_00, 4500_00, Some(1500_00), true));
        assert_eq!((i.best.as_ref().unwrap().name.as_str(), i.worst.as_ref().unwrap().name.as_str()), ("VOO", "茅台"));
        let md = facts_markdown(&facts(&mut c, "2026-10", "2026-11-02").unwrap());
        assert!(md.contains("| 月末市值 | ¥34,500.00 |") && md.contains("| 本月盈亏变化 | +¥1,500.00 |"), "{md}");
        assert!(md.contains("收益率最高:VOO +10.00%") && md.contains("收益率最低:茅台 -10.00%"), "{md}");
    }

    #[test]
    fn investment_for_the_current_month_is_live() {
        let t = TestDb::new();
        let mut c = t.conn();
        holding(&mut c, "VOO", "107.VOO", 39600, 500.0, 550.0); // 当前 +3600
        snap(&mut c, "2026-10-31", 39000, 36000); // +3000
        let f = facts(&mut c, "2026-11", "2026-11-15").unwrap();
        let i = f.investment.as_ref().unwrap();
        assert_eq!((i.live, i.value, i.month_pnl), (true, 39600_00, Some(600_00)));
        assert!(i.worst.is_none(), "只有一只时不重复列出");
        assert!(facts_markdown(&f).contains("| 截至今天市值 |"));
    }

    #[test]
    fn investment_without_snapshots_says_not_enough_data() {
        let t = TestDb::new();
        let mut c = t.conn();
        holding(&mut c, "VOO", "107.VOO", 39600, 500.0, 550.0);
        let f = facts(&mut c, "2026-09", "2026-11-02").unwrap();
        let i = f.investment.as_ref().unwrap();
        assert_eq!((i.live, i.value, i.month_pnl), (true, 39600_00, None));
        assert!(facts_markdown(&f).contains("| 本月盈亏变化 | 数据不足 |"));

        snap(&mut c, "2026-11-02", 39600, 36000);
        let f = facts(&mut c, "2026-11", "2026-11-15").unwrap();
        assert!(facts_markdown(&f).contains("从 11-02 开始统计"), "快照晚于月初时注明起算日期");
    }

    #[test]
    fn save_list_replace_delete() {
        let t = TestDb::new();
        let mut c = t.conn();
        save(&mut c, "2026-09", "九月").unwrap();
        save(&mut c, "2026-10", "十月 v1").unwrap();
        save(&mut c, "2026-10", "十月 v2").unwrap();
        let l = list(&mut c).unwrap();
        assert_eq!(l.iter().map(|x| (x.month.as_str(), x.content.as_str())).collect::<Vec<_>>(), [("2026-10", "十月 v2"), ("2026-09", "九月")]);
        delete(&mut c, "2026-09").unwrap();
        assert_eq!(list(&mut c).unwrap().len(), 1);
        assert!(save(&mut c, "2026-13", "x").is_err());
    }
}

#[cfg(test)]
mod live {
    use super::*;
    use crate::testdb::TestDb;

    /// 真实模型写解读:cargo test live_review -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_review() {
        let t = TestDb::new();
        let mut c = t.conn();
        let food = core::find_category(&mut c, "餐饮", "expense").unwrap();
        let fun = core::find_category(&mut c, "娱乐", "expense").unwrap();
        core::add_txn(&mut c, None, food, "expense", 1300_00, "2026-09-10", "").unwrap();
        core::add_txn(&mut c, None, food, "expense", 2100_00, "2026-10-05", "聚餐").unwrap();
        core::add_txn(&mut c, None, fun, "expense", 1500_00, "2026-10-12", "演唱会").unwrap();
        let pay = core::find_category(&mut c, "工资", "income").unwrap();
        core::add_txn(&mut c, None, pay, "income", 12000_00, "2026-10-10", "").unwrap();
        core::set_budget(&mut c, "2026-10", None, 4000_00).unwrap();
        let f = facts(&mut c, "2026-10", "2026-10-30").unwrap();
        let s = core::get_settings(&mut c).unwrap();
        let text = crate::agent::complete(&s, &interpret_prompt(&s.persona), &facts_for_model(&f)).unwrap();
        println!("{}", compose(&facts_markdown(&f), Ok(text)));
    }
}
