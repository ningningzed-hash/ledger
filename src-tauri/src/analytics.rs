//! 助手的只读分析工具:区间查账、分组统计、收支/净资产趋势、时期对比、异常支出。
//!
//! 约定:统计一律在这里用整数"分"算好,出口再转成"元";模型只负责解读,不做加减。
//! 所有筛选值都走参数化查询,不拼接 SQL;每个工具的返回体都有大小上限,超出时带 `truncated`。
use crate::agent::{num, text, tool, yuan};
use crate::core::{self, Error, Result};
use crate::report;
use chrono::{Datelike, Duration, Months, NaiveDate};
use mysql::prelude::Queryable;
use mysql::Value as Sql;
use serde_json::{json, Value};
use std::collections::BTreeMap;

const DEFAULT_DAYS: i64 = 90;
const MAX_SPAN_DAYS: i64 = 3660;
const DEFAULT_ROWS: u64 = 30;
const MAX_ROWS: u64 = 100;
const MAX_GROUPS: usize = 60;
const DEFAULT_TREND_MONTHS: u64 = 6;
const MAX_TREND_MONTHS: u64 = 36;
const DIFF_ROWS: usize = 15;
const OUTLIER_ROWS: usize = 10;
const OUTLIER_MIN_SAMPLES: usize = 5;

#[cfg(test)]
pub const TOOL_NAMES: [&str; 6] = [
    "query_transactions",
    "aggregate_transactions",
    "cash_flow_trend",
    "net_worth_trend",
    "compare_periods",
    "find_outliers",
];

// ---------- 工具定义 ----------

pub fn tool_defs() -> Vec<Value> {
    let from = json!({"type":"string","description":"起始日期 YYYY-MM-DD(含);省略则取结束日期往前 90 天"});
    let to = json!({"type":"string","description":"结束日期 YYYY-MM-DD(含);省略则为今天"});
    let months = json!({"type":"integer","description":"回看月数,默认 6,最多 36"});
    let filters = |extra: Value| {
        let mut p = json!({
            "from": from, "to": to,
            "category": {"type":"string","description":"分类名;“未分类”表示没有分类的账目"},
            "account": {"type":"string","description":"账户名称(与 list_accounts 一致)"},
            "keyword": {"type":"string","description":"关键词,同时匹配备注和分类名"},
            "min_amount": {"type":"number","description":"金额下限,单位元"},
            "max_amount": {"type":"number","description":"金额上限,单位元"}
        });
        for (k, v) in extra.as_object().unwrap() {
            p[k] = v.clone();
        }
        p
    };
    vec![
        tool(
            "query_transactions",
            "按条件查账目明细(跨月、按日期区间、分类、账户、关键词、金额范围),可按日期或金额排序、分页。返回命中的总笔数和收入/支出总额(不受分页影响)。适用:找某类/某关键词的账、最大的几笔支出。只看单个月的最近账目用 list_transactions。",
            filters(json!({
                "kind": {"type":"string","enum":["expense","income"],"description":"省略则收入和支出都查"},
                "sort": {"type":"string","enum":["date_desc","date_asc","amount_desc","amount_asc"],"description":"默认 date_desc"},
                "limit": {"type":"integer","description":"最多返回条数,默认 30,最多 100"},
                "offset": {"type":"integer","description":"跳过前面多少条,用于翻页,默认 0"}
            })),
            &[],
        ),
        tool(
            "aggregate_transactions",
            "在与 query_transactions 相同的筛选条件下分组统计,每组给出合计、笔数、均值、中位数;按月分组时附带环比。适用:各月/各周/各分类/各账户花了多少、占比、变化。一次只统计一种类型(默认支出)。",
            filters(json!({
                "group_by": {"type":"string","enum":["month","week","category","account"],"description":"分组方式;week 以周一为一周起点"},
                "kind": {"type":"string","enum":["expense","income"],"description":"默认 expense"}
            })),
            &["group_by"],
        ),
        tool(
            "cash_flow_trend",
            "最近 N 个月每月的收入、支出、结余、储蓄率,以及月均和支出最高/最低的月份(均值不含尚未过完的当前月)。适用:判断开销/收入是否在变化、储蓄情况。",
            json!({"months": months}),
            &[],
        ),
        tool(
            "net_worth_trend",
            "最近 N 个月月底的资产、负债、净资产,以及区间内的变化量和变化幅度。适用:净资产在涨还是在跌。",
            json!({"months": months}),
            &[],
        ),
        tool(
            "compare_periods",
            "对比两个日期区间(A 期与 B 期)的收入、支出、结余,并逐分类给出支出差额(B 相对 A),按差额绝对值降序。适用:这个月比上个月多花了什么、今年和去年同期对比。",
            json!({
                "a_from": {"type":"string","description":"A 期起始日期 YYYY-MM-DD"},
                "a_to": {"type":"string","description":"A 期结束日期 YYYY-MM-DD"},
                "b_from": {"type":"string","description":"B 期起始日期 YYYY-MM-DD"},
                "b_to": {"type":"string","description":"B 期结束日期 YYYY-MM-DD"}
            }),
            &["a_from", "a_to", "b_from", "b_to"],
        ),
        tool(
            "find_outliers",
            "找出区间内明显高于所在分类常规水平(中位数)的单笔支出。适用:有没有异常大的支出。这是启发式判断,表述时说“明显高于常规”,不要下定论。",
            json!({
                "from": from, "to": to,
                "factor": {"type":"number","description":"至少是该分类中位数的多少倍,默认 3"},
                "min_amount": {"type":"number","description":"金额下限,单位元,默认 100"}
            }),
            &[],
        ),
    ]
}

/// 不是分析工具时返回 None,由调用方继续匹配其它工具
pub fn run(c: &mut impl Queryable, name: &str, args: &Value, today: &str) -> Option<Result<Value>> {
    let today = match NaiveDate::parse_from_str(today, "%Y-%m-%d") {
        Ok(d) => d,
        Err(_) => return Some(Err(Error::Invalid(format!("今天的日期无效:{today}")))),
    };
    Some(match name {
        "query_transactions" => query_transactions(c, args, today),
        "aggregate_transactions" => aggregate_transactions(c, args, today),
        "cash_flow_trend" => cash_flow_trend(c, args, today),
        "net_worth_trend" => net_worth_trend(c, args, today),
        "compare_periods" => compare_periods(c, args),
        "find_outliers" => find_outliers(c, args, today),
        _ => return None,
    })
}

// ---------- 参数解析 ----------

fn fmt(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn parse_date(label: &str, s: &str) -> Result<NaiveDate> {
    let bad = || Error::Invalid(format!("{label} 应为存在的日期,格式 YYYY-MM-DD,收到:{s}"));
    if s.len() != 10 {
        return Err(bad());
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| bad())
}

fn check_span(from: NaiveDate, to: NaiveDate) -> Result<()> {
    if from > to {
        return Err(Error::Invalid(format!("起始日期 {} 晚于结束日期 {}", fmt(from), fmt(to))));
    }
    if (to - from).num_days() > MAX_SPAN_DAYS {
        return Err(Error::Invalid("统计区间最长 10 年,请缩小范围".into()));
    }
    Ok(())
}

/// 省略时:结束日期取今天,起始日期取结束日期往前 90 天(含两端)
fn parse_range(from: &str, to: &str, today: NaiveDate) -> Result<(NaiveDate, NaiveDate)> {
    let to_d = if to.is_empty() { today } else { parse_date("to", to)? };
    let from_d = if from.is_empty() { to_d - Duration::days(DEFAULT_DAYS - 1) } else { parse_date("from", from)? };
    check_span(from_d, to_d)?;
    Ok((from_d, to_d))
}

fn opt_num(args: &Value, key: &str) -> Result<Option<f64>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(_) => num(args, key)
            .filter(|v| v.is_finite())
            .map(Some)
            .ok_or_else(|| Error::Invalid(format!("{key} 必须是数字"))),
    }
}

/// 非负整数;模型有时会把数字写成字符串或带小数
fn opt_uint(args: &Value, key: &str) -> Result<Option<u64>> {
    match opt_num(args, key)? {
        None => Ok(None),
        Some(v) if v >= 0.0 => Ok(Some(v as u64)),
        Some(_) => Err(Error::Invalid(format!("{key} 不能是负数"))),
    }
}

fn opt_cents(args: &Value, key: &str) -> Result<Option<i64>> {
    match opt_num(args, key)? {
        None => Ok(None),
        Some(v) if (0.0..=1e12).contains(&v) => Ok(Some((v * 100.0).round() as i64)),
        Some(_) => Err(Error::Invalid(format!("{key} 必须是不小于 0 的金额(单位:元)"))),
    }
}

fn trend_months(args: &Value) -> Result<u32> {
    let n = opt_uint(args, "months")?.unwrap_or(DEFAULT_TREND_MONTHS);
    if n == 0 || n > MAX_TREND_MONTHS {
        return Err(Error::Invalid(format!("months 需在 1 到 {MAX_TREND_MONTHS} 之间")));
    }
    Ok(n as u32)
}

// ---------- 筛选 ----------

enum Cat {
    Any,
    Uncategorized,
    Id(i64),
}

struct Filter {
    from: NaiveDate,
    to: NaiveDate,
    kind: Option<String>,
    cat: Cat,
    account_id: Option<i64>,
    keyword: Option<String>,
    min: Option<i64>,
    max: Option<i64>,
}

const FROM_TXN: &str = "FROM txn t LEFT JOIN category c ON c.id=t.category_id LEFT JOIN account a ON a.id=t.account_id";

fn like_pattern(k: &str) -> String {
    let mut out = String::from("%");
    for ch in k.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('%');
    out
}

impl Filter {
    fn parse(c: &mut impl Queryable, args: &Value, today: NaiveDate) -> Result<Filter> {
        let (from, to) = parse_range(text(args, "from"), text(args, "to"), today)?;
        let kind = match text(args, "kind") {
            "" => None,
            k @ ("expense" | "income") => Some(k.to_string()),
            k => return Err(Error::Invalid(format!("kind 只能是 expense 或 income,收到:{k}"))),
        };

        let cat_name = text(args, "category");
        let cat = if cat_name.is_empty() {
            Cat::Any
        } else if cat_name == "未分类" {
            Cat::Uncategorized
        } else {
            let kinds: Vec<&str> = match &kind {
                Some(k) => vec![k.as_str()],
                None => vec!["expense", "income"],
            };
            let mut found = None;
            for k in kinds {
                if let Some(id) = core::find_category(c, cat_name, k)? {
                    found = Some(id);
                    break;
                }
            }
            match found {
                Some(id) => Cat::Id(id),
                None => {
                    let names: Vec<String> = core::list_categories(c)?
                        .into_iter()
                        .filter(|x| kind.as_deref().is_none_or(|k| x.kind == k))
                        .map(|x| x.name)
                        .collect();
                    return Err(Error::Invalid(format!("没有分类“{cat_name}”,可选分类:{}、未分类", names.join("、"))));
                }
            }
        };

        let acc_name = text(args, "account");
        let account_id = if acc_name.is_empty() {
            None
        } else {
            match core::find_account(c, acc_name)? {
                Some(a) => Some(a.id),
                None => {
                    let names: Vec<String> = core::list_accounts(c)?.into_iter().map(|a| a.name).collect();
                    return Err(Error::Invalid(format!("找不到账户“{acc_name}”,现有账户:{}", names.join("、"))));
                }
            }
        };

        let (min, max) = (opt_cents(args, "min_amount")?, opt_cents(args, "max_amount")?);
        if let (Some(lo), Some(hi)) = (min, max) {
            if lo > hi {
                return Err(Error::Invalid("min_amount 不能大于 max_amount".into()));
            }
        }
        let keyword = Some(text(args, "keyword")).filter(|k| !k.is_empty()).map(String::from);
        Ok(Filter { from, to, kind, cat, account_id, keyword, min, max })
    }

    /// 带占位符的 WHERE 子句(含 "WHERE")与按顺序的参数
    fn where_sql(&self) -> (String, Vec<Sql>) {
        let mut w = vec!["t.date BETWEEN ? AND ?".to_string()];
        let mut p: Vec<Sql> = vec![fmt(self.from).into(), fmt(self.to).into()];
        if let Some(k) = &self.kind {
            w.push("t.kind=?".into());
            p.push(k.clone().into());
        }
        match self.cat {
            Cat::Any => {}
            Cat::Uncategorized => w.push("t.category_id IS NULL".into()),
            Cat::Id(id) => {
                w.push("t.category_id=?".into());
                p.push(id.into());
            }
        }
        if let Some(id) = self.account_id {
            w.push("t.account_id=?".into());
            p.push(id.into());
        }
        if let Some(k) = &self.keyword {
            w.push("(t.note LIKE ? OR c.name LIKE ?)".into());
            p.push(like_pattern(k).into());
            p.push(like_pattern(k).into());
        }
        if let Some(v) = self.min {
            w.push("t.amount>=?".into());
            p.push(v.into());
        }
        if let Some(v) = self.max {
            w.push("t.amount<=?".into());
            p.push(v.into());
        }
        (format!("WHERE {}", w.join(" AND ")), p)
    }
}

// ---------- 统计小工具 ----------

/// 整数相除,四舍五入(金额均为非负)
fn div_round(n: i64, d: i64) -> i64 {
    (n * 2 + d) / (2 * d)
}

struct Stats {
    total: i64,
    count: i64,
    avg: Option<i64>,
    median: Option<i64>,
}

fn stats(amounts: &[i64]) -> Stats {
    let count = amounts.len() as i64;
    let total: i64 = amounts.iter().sum();
    if amounts.is_empty() {
        return Stats { total, count, avg: None, median: None };
    }
    let mut v = amounts.to_vec();
    v.sort_unstable();
    let n = v.len();
    let median = if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2] + 1) / 2 };
    Stats { total, count, avg: Some(div_round(total, count)), median: Some(median) }
}

fn stats_json(s: &Stats) -> Value {
    json!({"total": yuan(s.total), "count": s.count, "avg": s.avg.map(yuan), "median": s.median.map(yuan)})
}

/// 变化百分比,保留一位小数;基数不大于 0 时没有意义,返回 None
fn pct(change: i64, base: i64) -> Option<f64> {
    (base > 0).then(|| ((change as f64) / (base as f64) * 1000.0).round() / 10.0)
}

fn month_list(from: NaiveDate, to: NaiveDate) -> Vec<String> {
    let mut d = from.with_day(1).unwrap_or(from);
    let mut out = Vec::new();
    while d <= to {
        out.push(d.format("%Y-%m").to_string());
        match d.checked_add_months(Months::new(1)) {
            Some(n) => d = n,
            None => break,
        }
    }
    out
}

fn prev_month(m: &str) -> Option<String> {
    let d = NaiveDate::parse_from_str(&format!("{m}-01"), "%Y-%m-%d").ok()?;
    d.checked_sub_months(Months::new(1)).map(|p| p.format("%Y-%m").to_string())
}

// ---------- query_transactions ----------

fn query_transactions(c: &mut impl Queryable, args: &Value, today: NaiveDate) -> Result<Value> {
    let f = Filter::parse(c, args, today)?;
    let limit = opt_uint(args, "limit")?.unwrap_or(DEFAULT_ROWS).clamp(1, MAX_ROWS);
    let offset = opt_uint(args, "offset")?.unwrap_or(0);
    let order = match text(args, "sort") {
        "" | "date_desc" => "t.date DESC, t.id DESC",
        "date_asc" => "t.date ASC, t.id ASC",
        "amount_desc" => "t.amount DESC, t.id DESC",
        "amount_asc" => "t.amount ASC, t.id ASC",
        s => return Err(Error::Invalid(format!("sort 只能是 date_desc/date_asc/amount_desc/amount_asc,收到:{s}"))),
    };
    let (where_sql, params) = f.where_sql();

    let (count, expense, income): (i64, i64, i64) = c
        .exec_first(
            format!(
                "SELECT COUNT(*),
                        CAST(COALESCE(SUM(CASE WHEN t.kind='expense' THEN t.amount END),0) AS SIGNED),
                        CAST(COALESCE(SUM(CASE WHEN t.kind='income' THEN t.amount END),0) AS SIGNED)
                 {FROM_TXN} {where_sql}"
            ),
            params.clone(),
        )?
        .unwrap_or((0, 0, 0));

    let mut page_params = params;
    page_params.push((limit as i64).into());
    page_params.push((offset as i64).into());
    let rows: Vec<Value> = c.exec_map(
        format!(
            "SELECT t.date, t.kind, t.amount, c.name, a.name, t.note {FROM_TXN} {where_sql}
             ORDER BY {order} LIMIT ? OFFSET ?"
        ),
        page_params,
        |(date, kind, amount, category, account, note): (String, String, i64, Option<String>, Option<String>, String)| {
            json!({"date": date, "kind": kind, "amount": yuan(amount), "category": category, "account": account, "note": note})
        },
    )?;

    Ok(json!({
        "from": fmt(f.from), "to": fmt(f.to),
        "total_count": count,
        "total_expense": yuan(expense),
        "total_income": yuan(income),
        "offset": offset,
        "returned": rows.len(),
        "truncated": (offset as i64) + (rows.len() as i64) < count,
        "transactions": rows,
    }))
}

// ---------- aggregate_transactions ----------

#[derive(Clone, Copy, PartialEq)]
enum GroupBy {
    Month,
    Week,
    Category,
    Account,
}

fn aggregate_transactions(c: &mut impl Queryable, args: &Value, today: NaiveDate) -> Result<Value> {
    let group_by = match text(args, "group_by") {
        "month" => GroupBy::Month,
        "week" => GroupBy::Week,
        "category" => GroupBy::Category,
        "account" => GroupBy::Account,
        "" => return Err(Error::Invalid("缺少 group_by(month/week/category/account)".into())),
        g => return Err(Error::Invalid(format!("group_by 只能是 month/week/category/account,收到:{g}"))),
    };
    let mut f = Filter::parse(c, args, today)?;
    // 收入和支出混在一起统计没有意义,默认只看支出
    let kind = f.kind.get_or_insert_with(|| "expense".to_string()).clone();
    let (where_sql, params) = f.where_sql();

    let rows: Vec<(String, i64, String, String)> = c.exec(
        format!("SELECT t.date, t.amount, COALESCE(c.name,'未分类'), COALESCE(a.name,'无账户') {FROM_TXN} {where_sql}"),
        params,
    )?;

    let mut groups: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    let mut all = Vec::with_capacity(rows.len());
    for (date, amount, cat, acct) in rows {
        let key = match group_by {
            GroupBy::Month => date.get(..7).unwrap_or("").to_string(),
            GroupBy::Week => match NaiveDate::parse_from_str(&date, "%Y-%m-%d") {
                Ok(d) => fmt(d - Duration::days(d.weekday().num_days_from_monday() as i64)),
                Err(_) => continue,
            },
            GroupBy::Category => cat,
            GroupBy::Account => acct,
        };
        groups.entry(key).or_default().push(amount);
        all.push(amount);
    }
    if group_by == GroupBy::Month {
        for m in month_list(f.from, f.to) {
            groups.entry(m).or_default(); // 中间没有账目的月份补成空组
        }
    }

    let mut list: Vec<(String, Stats)> = groups.into_iter().map(|(k, v)| (k, stats(&v))).collect();
    let timeline = matches!(group_by, GroupBy::Month | GroupBy::Week);
    if !timeline {
        list.sort_by(|a, b| b.1.total.cmp(&a.1.total).then_with(|| a.0.cmp(&b.0)));
    }

    // 环比必须在截断之前算,否则被截掉的上月会让第一组的环比丢失
    let month_totals: BTreeMap<String, i64> = list.iter().map(|(k, s)| (k.clone(), s.total)).collect();
    let first_month = list.first().map(|(k, _)| k.clone());
    let to_json = |(key, s): &(String, Stats)| {
        let mut g = stats_json(s);
        g["group"] = json!(key);
        if group_by == GroupBy::Month {
            // 区间内的第一个月没有可比的上月
            let prev = if first_month.as_deref() == Some(key.as_str()) { None } else { prev_month(key).and_then(|p| month_totals.get(&p).copied()) };
            g["change"] = json!(prev.map(|p| yuan(s.total - p)));
            g["change_pct"] = json!(prev.and_then(|p| pct(s.total - p, p)));
            // 区间首尾如果不是整月,环比不可靠
            let ends_partial = (key.as_str() == &fmt(f.from)[..7] && f.from.day() != 1)
                || (key.as_str() == &fmt(f.to)[..7] && f.to.succ_opt().is_some_and(|n| n.day() != 1));
            if ends_partial {
                g["partial"] = json!(true);
            }
        }
        g
    };
    let total_groups = list.len();
    let shown: Vec<Value> = if total_groups > MAX_GROUPS {
        let slice = if timeline { &list[total_groups - MAX_GROUPS..] } else { &list[..MAX_GROUPS] };
        slice.iter().map(to_json).collect()
    } else {
        list.iter().map(to_json).collect()
    };

    Ok(json!({
        "from": fmt(f.from), "to": fmt(f.to), "kind": kind,
        "group_by": text(args, "group_by"),
        "overall": stats_json(&stats(&all)),
        "total_groups": total_groups,
        "truncated": total_groups > MAX_GROUPS,
        "groups": shown,
    }))
}

// ---------- 趋势 ----------

fn savings_rate(income: i64, expense: i64) -> Option<f64> {
    pct(income - expense, income)
}

fn cash_flow_trend(c: &mut impl Queryable, args: &Value, today: NaiveDate) -> Result<Value> {
    let months = trend_months(args)?;
    let through = today.format("%Y-%m").to_string();
    let r = report::report(c, &through, months)?;
    if r.months.is_empty() {
        return Ok(json!({"months": [], "note": "还没有可以统计的数据"}));
    }

    let rows: Vec<Value> = r
        .months
        .iter()
        .map(|m| {
            json!({
                "month": m.month, "income": yuan(m.income), "expense": yuan(m.expense),
                "net": yuan(m.income - m.expense),
                "savings_rate_pct": savings_rate(m.income, m.expense),
                // 当前月还没过完,收支偏少,不代表真实水平
                "partial": m.month == through,
            })
        })
        .collect();

    // 均值与最高/最低只看已经过完的月份(只有一个月时才退而用当前月)
    let full: Vec<&report::MonthRow> = if r.months.len() > 1 { r.months.iter().filter(|m| m.month != through).collect() } else { r.months.iter().collect() };
    let n = full.len() as i64;
    let (inc, exp): (i64, i64) = full.iter().fold((0, 0), |a, m| (a.0 + m.income, a.1 + m.expense));
    let highest = full.iter().copied().reduce(|a, b| if b.expense > a.expense { b } else { a });
    let lowest = full.iter().copied().reduce(|a, b| if b.expense < a.expense { b } else { a });
    let month_pair = |m: Option<&report::MonthRow>| m.map(|m| json!({"month": m.month, "expense": yuan(m.expense)}));

    Ok(json!({
        "from": r.from, "to": r.to,
        "months": rows,
        "summary": {
            "months_in_average": n,
            "avg_income": yuan(div_round(inc, n)),
            "avg_expense": yuan(div_round(exp, n)),
            "avg_net": yuan(div_round(inc, n) - div_round(exp, n)),
            "savings_rate_pct": savings_rate(inc, exp),
            "highest_expense_month": month_pair(highest),
            "lowest_expense_month": month_pair(lowest),
        },
    }))
}

fn net_worth_trend(c: &mut impl Queryable, args: &Value, today: NaiveDate) -> Result<Value> {
    let months = trend_months(args)?;
    let through = today.format("%Y-%m").to_string();
    let r = report::report(c, &through, months)?;
    let (Some(first), Some(last)) = (r.net_worth.first(), r.net_worth.last()) else {
        return Ok(json!({"months": [], "note": "还没有可以统计的数据"}));
    };
    let change = last.net - first.net;
    let points: Vec<Value> = r
        .net_worth
        .iter()
        .map(|p| json!({"month": p.month, "assets": yuan(p.assets), "liabilities": yuan(p.liabilities), "net": yuan(p.net)}))
        .collect();
    Ok(json!({
        "from": r.from, "to": r.to,
        "months": points,
        "change": yuan(change),
        "change_pct": pct(change, first.net),
    }))
}

// ---------- compare_periods ----------

fn required_date(args: &Value, key: &str) -> Result<NaiveDate> {
    match text(args, key) {
        "" => Err(Error::Invalid(format!("缺少 {key}(YYYY-MM-DD)"))),
        s => parse_date(key, s),
    }
}

struct Period {
    from: NaiveDate,
    to: NaiveDate,
    income: i64,
    expense: i64,
    by_category: BTreeMap<String, i64>,
}

fn load_period(c: &mut impl Queryable, from: NaiveDate, to: NaiveDate) -> Result<Period> {
    check_span(from, to)?;
    let (f, t) = (fmt(from), fmt(to));
    let kinds: Vec<(String, i64)> = c.exec(
        "SELECT kind, CAST(SUM(amount) AS SIGNED) FROM txn WHERE date BETWEEN ? AND ? GROUP BY kind",
        (&f, &t),
    )?;
    let sum = |k: &str| kinds.iter().find(|(x, _)| x == k).map_or(0, |(_, v)| *v);
    let cats: Vec<(String, i64)> = c.exec(
        "SELECT COALESCE(c.name,'未分类'), CAST(SUM(t.amount) AS SIGNED)
         FROM txn t LEFT JOIN category c ON c.id=t.category_id
         WHERE t.kind='expense' AND t.date BETWEEN ? AND ? GROUP BY COALESCE(c.name,'未分类')",
        (&f, &t),
    )?;
    Ok(Period { from, to, income: sum("income"), expense: sum("expense"), by_category: cats.into_iter().collect() })
}

fn period_json(p: &Period) -> Value {
    let days = (p.to - p.from).num_days() + 1;
    json!({
        "from": fmt(p.from), "to": fmt(p.to), "days": days,
        "income": yuan(p.income), "expense": yuan(p.expense), "net": yuan(p.income - p.expense),
        "avg_daily_expense": yuan(div_round(p.expense, days)),
    })
}

fn compare_periods(c: &mut impl Queryable, args: &Value) -> Result<Value> {
    let a = load_period(c, required_date(args, "a_from")?, required_date(args, "a_to")?)?;
    let b = load_period(c, required_date(args, "b_from")?, required_date(args, "b_to")?)?;

    let mut names: Vec<&String> = a.by_category.keys().chain(b.by_category.keys()).collect();
    names.sort();
    names.dedup();
    let mut diffs: Vec<(String, i64, i64)> = names
        .into_iter()
        .map(|n| (n.clone(), a.by_category.get(n).copied().unwrap_or(0), b.by_category.get(n).copied().unwrap_or(0)))
        .collect();
    diffs.sort_by(|x, y| (y.2 - y.1).abs().cmp(&(x.2 - x.1).abs()).then_with(|| x.0.cmp(&y.0)));

    let row = |(name, av, bv): &(String, i64, i64)| {
        json!({"category": name, "a": yuan(*av), "b": yuan(*bv), "change": yuan(bv - av), "change_pct": pct(bv - av, *av)})
    };
    let mut rows: Vec<Value> = diffs.iter().take(DIFF_ROWS).map(row).collect();
    if diffs.len() > DIFF_ROWS {
        let (ra, rb) = diffs[DIFF_ROWS..].iter().fold((0, 0), |s, d| (s.0 + d.1, s.1 + d.2));
        rows.push(row(&("其他分类".to_string(), ra, rb)));
    }

    Ok(json!({
        "period_a": period_json(&a),
        "period_b": period_json(&b),
        "expense_change": yuan(b.expense - a.expense),
        "expense_change_pct": pct(b.expense - a.expense, a.expense),
        "income_change": yuan(b.income - a.income),
        "categories": rows,
    }))
}

// ---------- find_outliers ----------

fn find_outliers(c: &mut impl Queryable, args: &Value, today: NaiveDate) -> Result<Value> {
    let (from, to) = parse_range(text(args, "from"), text(args, "to"), today)?;
    let factor = opt_num(args, "factor")?.unwrap_or(3.0);
    if factor < 1.0 {
        return Err(Error::Invalid("factor 不能小于 1".into()));
    }
    let min_amount = opt_cents(args, "min_amount")?.unwrap_or(100_00);

    let rows: Vec<(String, i64, String, String)> = c.exec(
        "SELECT t.date, t.amount, COALESCE(c.name,'未分类'), t.note
         FROM txn t LEFT JOIN category c ON c.id=t.category_id
         WHERE t.kind='expense' AND t.date BETWEEN ? AND ?",
        (fmt(from), fmt(to)),
    )?;
    let mut by_cat: BTreeMap<String, Vec<(String, i64, String)>> = BTreeMap::new();
    for (date, amount, cat, note) in rows {
        by_cat.entry(cat).or_default().push((date, amount, note));
    }

    let mut found: Vec<(f64, Value, i64)> = Vec::new();
    for (cat, items) in &by_cat {
        if items.len() < OUTLIER_MIN_SAMPLES {
            continue;
        }
        let amounts: Vec<i64> = items.iter().map(|i| i.1).collect();
        let Some(median) = stats(&amounts).median.filter(|m| *m > 0) else { continue };
        let threshold = (factor * median as f64).max(min_amount as f64);
        for (date, amount, note) in items {
            if (*amount as f64) >= threshold {
                let multiple = *amount as f64 / median as f64;
                found.push((
                    multiple,
                    json!({
                        "date": date, "amount": yuan(*amount), "category": cat, "note": note,
                        "category_median": yuan(median), "multiple": (multiple * 10.0).round() / 10.0,
                    }),
                    *amount,
                ));
            }
        }
    }
    found.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| b.2.cmp(&a.2)));
    let total_found = found.len();
    Ok(json!({
        "from": fmt(from), "to": fmt(to),
        "factor": factor, "min_amount": yuan(min_amount), "min_samples_per_category": OUTLIER_MIN_SAMPLES,
        "total_found": total_found,
        "outliers": found.into_iter().take(OUTLIER_ROWS).map(|f| f.1).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Conn;
    use crate::testdb::TestDb;

    const TODAY: &str = "2026-10-27"; // 周二

    fn ask(c: &mut Conn, name: &str, args: Value) -> Value {
        run(c, name, &args, TODAY).expect("是分析工具").unwrap_or_else(|e| panic!("{name} 失败:{e}"))
    }

    fn err(c: &mut Conn, name: &str, args: Value) -> String {
        run(c, name, &args, TODAY).expect("是分析工具").expect_err("应当报错").to_string()
    }

    fn spend(c: &mut Conn, cat: Option<&str>, yuan_amt: i64, date: &str, note: &str) {
        let id = cat.map(|n| core::find_category(c, n, "expense").unwrap().expect("分类存在"));
        core::add_txn(c, None, id, "expense", yuan_amt * 100, date, note).unwrap();
    }

    fn earn(c: &mut Conn, yuan_amt: i64, date: &str) {
        let id = core::find_category(c, "工资", "income").unwrap();
        core::add_txn(c, None, id, "income", yuan_amt * 100, date, "").unwrap();
    }

    fn f(v: &Value) -> Option<f64> {
        v.as_f64()
    }

    fn txn_count(c: &mut Conn) -> i64 {
        c.query_first("SELECT COUNT(*) FROM txn").unwrap().unwrap()
    }

    // ----- 小工具 -----

    #[test]
    fn median_and_average_round_correctly() {
        let s = stats(&[1000, 2000, 3000, 10000]);
        assert_eq!((s.total, s.count, s.avg, s.median), (16000, 4, Some(4000), Some(2500)));
        let s = stats(&[100, 300, 200]);
        assert_eq!((s.avg, s.median), (Some(200), Some(200)));
        assert_eq!(stats(&[1, 2]).median, Some(2)); // 1.5 分四舍五入
        assert_eq!(stats(&[]).avg, None);
        assert_eq!(pct(200, 1000), Some(20.0));
        assert_eq!(pct(5, 0), None);
        assert_eq!(pct(-5, -10), None);
    }

    // ----- query_transactions -----

    #[test]
    fn query_spans_months_and_matches_keyword_in_note_or_category() {
        let t = TestDb::new();
        let mut c = t.conn();
        spend(&mut c, Some("交通"), 30, "2026-08-10", "滴滴");
        spend(&mut c, Some("餐饮"), 20, "2026-09-10", "滴滴外卖");
        spend(&mut c, Some("交通"), 99, "2026-10-10", "地铁"); // 分类名“交通”不含“滴滴”
        spend(&mut c, Some("餐饮"), 50, "2026-07-31", "滴滴"); // 区间之外
        let r = ask(&mut c, "query_transactions", json!({"from":"2026-08-01","to":"2026-10-31","keyword":"滴滴"}));
        assert_eq!(r["total_count"], 2);
        assert_eq!(f(&r["total_expense"]), Some(50.0));
        assert_eq!(r["transactions"].as_array().unwrap().len(), 2);
        assert_eq!(r["transactions"][0]["date"], "2026-09-10"); // 默认日期倒序

        let r = ask(&mut c, "query_transactions", json!({"from":"2026-08-01","to":"2026-10-31","keyword":"交通"}));
        assert_eq!(r["total_count"], 2, "关键词也匹配分类名");
    }

    #[test]
    fn top_n_by_amount_keeps_full_totals() {
        let t = TestDb::new();
        let mut c = t.conn();
        for (i, a) in [5, 80, 30, 200, 15, 60, 7].iter().enumerate() {
            spend(&mut c, Some("购物"), *a, &format!("2026-10-{:02}", i + 1), "");
        }
        let r = ask(&mut c, "query_transactions", json!({"kind":"expense","sort":"amount_desc","limit":5}));
        let amounts: Vec<f64> = r["transactions"].as_array().unwrap().iter().map(|t| t["amount"].as_f64().unwrap()).collect();
        assert_eq!(amounts, [200.0, 80.0, 60.0, 30.0, 15.0]);
        assert_eq!((r["total_count"].as_i64(), f(&r["total_expense"])), (Some(7), Some(397.0)));
        assert_eq!(r["truncated"], true);
    }

    #[test]
    fn limit_is_capped_and_offset_pages_through() {
        let t = TestDb::new();
        let mut c = t.conn();
        for i in 0..250 {
            spend(&mut c, None, 1, "2026-10-01", &format!("n{i}"));
        }
        let r = ask(&mut c, "query_transactions", json!({"limit":500}));
        assert_eq!((r["total_count"].as_i64(), r["returned"].as_i64(), r["truncated"].as_bool()), (Some(250), Some(100), Some(true)));
        let r = ask(&mut c, "query_transactions", json!({"limit":100,"offset":200}));
        assert_eq!((r["returned"].as_i64(), r["truncated"].as_bool()), (Some(50), Some(false)));
        let r = ask(&mut c, "query_transactions", json!({"limit":"2"})); // 数字写成字符串
        assert_eq!(r["returned"], 2);
    }

    #[test]
    fn like_wildcards_are_literal_and_injection_is_inert() {
        let t = TestDb::new();
        let mut c = t.conn();
        spend(&mut c, None, 10, "2026-10-01", "满100%减10");
        spend(&mut c, None, 10, "2026-10-02", "满100元减10");
        spend(&mut c, None, 10, "2026-10-03", "abc_def");
        spend(&mut c, None, 10, "2026-10-04", "abcxdef");
        assert_eq!(ask(&mut c, "query_transactions", json!({"keyword":"100%"}))["total_count"], 1);
        assert_eq!(ask(&mut c, "query_transactions", json!({"keyword":"abc_def"}))["total_count"], 1);
        let r = ask(&mut c, "query_transactions", json!({"keyword":"'; DROP TABLE txn; --"}));
        assert_eq!(r["total_count"], 0);
        assert_eq!(txn_count(&mut c), 4);
    }

    #[test]
    fn default_range_is_last_90_days_and_reported() {
        let t = TestDb::new();
        let mut c = t.conn();
        spend(&mut c, None, 1, "2026-07-29", "区间外");
        spend(&mut c, None, 2, "2026-07-30", "区间内");
        let r = ask(&mut c, "query_transactions", json!({}));
        assert_eq!((r["from"].as_str(), r["to"].as_str(), r["total_count"].as_i64()), (Some("2026-07-30"), Some("2026-10-27"), Some(1)));
    }

    #[test]
    fn filters_by_account_category_and_amount_range() {
        let t = TestDb::new();
        let mut c = t.conn();
        let acc = core::add_account(&mut c, "招行卡", "deposit", "CNY", 10_000_00, "2026-10-01").unwrap();
        let food = core::find_category(&mut c, "餐饮", "expense").unwrap();
        core::add_txn(&mut c, Some(acc), food, "expense", 50_00, "2026-10-05", "").unwrap();
        spend(&mut c, Some("餐饮"), 500, "2026-10-06", "");
        spend(&mut c, None, 70, "2026-10-07", "");
        assert_eq!(ask(&mut c, "query_transactions", json!({"account":"招行卡"}))["total_count"], 1);
        assert_eq!(ask(&mut c, "query_transactions", json!({"category":"餐饮","min_amount":100}))["total_count"], 1);
        assert_eq!(ask(&mut c, "query_transactions", json!({"category":"未分类"}))["total_count"], 1);
        assert_eq!(ask(&mut c, "query_transactions", json!({"min_amount":60,"max_amount":100}))["total_count"], 1);
    }

    #[test]
    fn bad_parameters_return_actionable_errors() {
        let t = TestDb::new();
        let mut c = t.conn();
        let e = err(&mut c, "query_transactions", json!({"category":"不存在"}));
        assert!(e.contains("可选分类") && e.contains("餐饮"), "{e}");
        assert!(err(&mut c, "query_transactions", json!({"account":"没有的"})).contains("现有账户"));
        assert!(err(&mut c, "query_transactions", json!({"to":"2026-02-30"})).contains("2026-02-30"));
        assert!(err(&mut c, "query_transactions", json!({"from":"2026-1-5"})).contains("YYYY-MM-DD"));
        assert!(err(&mut c, "query_transactions", json!({"from":"2026-10-10","to":"2026-10-01"})).contains("晚于"));
        assert!(err(&mut c, "query_transactions", json!({"from":"2010-01-01","to":"2026-01-01"})).contains("10 年"));
        assert!(err(&mut c, "query_transactions", json!({"min_amount":500,"max_amount":100})).contains("min_amount"));
        assert!(err(&mut c, "query_transactions", json!({"kind":"转账"})).contains("kind"));
        assert!(err(&mut c, "query_transactions", json!({"limit":"很多"})).contains("limit"));
        assert!(err(&mut c, "query_transactions", json!({"sort":"random"})).contains("sort"));
    }

    // ----- aggregate_transactions -----

    #[test]
    fn aggregate_reports_total_count_average_and_median_per_group() {
        let t = TestDb::new();
        let mut c = t.conn();
        for a in [10, 20, 30, 100] {
            spend(&mut c, Some("餐饮"), a, "2026-10-05", "");
        }
        spend(&mut c, Some("交通"), 500, "2026-10-06", "");
        let r = ask(&mut c, "aggregate_transactions", json!({"group_by":"category"}));
        let g = &r["groups"];
        assert_eq!(g[0]["group"], "交通"); // 按合计降序
        assert_eq!(g[1]["group"], "餐饮");
        assert_eq!((f(&g[1]["total"]), g[1]["count"].as_i64(), f(&g[1]["avg"]), f(&g[1]["median"])), (Some(160.0), Some(4), Some(40.0), Some(25.0)));
        assert_eq!((f(&r["overall"]["total"]), r["overall"]["count"].as_i64()), (Some(660.0), Some(5)));
        assert_eq!(r["kind"], "expense", "默认只统计支出");
    }

    #[test]
    fn monthly_change_compares_with_the_previous_calendar_month() {
        let t = TestDb::new();
        let mut c = t.conn();
        spend(&mut c, None, 1000, "2026-09-10", "");
        spend(&mut c, None, 1200, "2026-10-10", "");
        let r = ask(&mut c, "aggregate_transactions", json!({"group_by":"month","from":"2026-09-01","to":"2026-10-31"}));
        let g = r["groups"].as_array().unwrap();
        assert_eq!(g.len(), 2);
        assert_eq!(g[0]["change"], Value::Null, "区间内第一个月没有可比的上月");
        assert_eq!((f(&g[1]["change"]), f(&g[1]["change_pct"])), (Some(200.0), Some(20.0)));
        assert!(g[1].get("partial").is_none());
    }

    #[test]
    fn missing_months_become_zero_groups_and_zero_base_has_no_percentage() {
        let t = TestDb::new();
        let mut c = t.conn();
        spend(&mut c, None, 300, "2026-08-10", "");
        spend(&mut c, None, 400, "2026-10-10", "");
        let r = ask(&mut c, "aggregate_transactions", json!({"group_by":"month","from":"2026-08-01","to":"2026-10-31"}));
        let g = r["groups"].as_array().unwrap();
        let months: Vec<&str> = g.iter().map(|x| x["group"].as_str().unwrap()).collect();
        assert_eq!(months, ["2026-08", "2026-09", "2026-10"]);
        assert_eq!((f(&g[1]["total"]), g[1]["count"].as_i64(), g[1]["avg"].clone()), (Some(0.0), Some(0), Value::Null));
        assert_eq!((f(&g[2]["change"]), g[2]["change_pct"].clone()), (Some(400.0), Value::Null));
    }

    #[test]
    fn partial_edge_months_are_flagged() {
        let t = TestDb::new();
        let mut c = t.conn();
        spend(&mut c, None, 10, "2026-09-20", "");
        let r = ask(&mut c, "aggregate_transactions", json!({"group_by":"month","from":"2026-09-15","to":"2026-10-27"}));
        let g = r["groups"].as_array().unwrap();
        assert_eq!(g[0]["partial"], true);
        assert_eq!(g[1]["partial"], true);
    }

    #[test]
    fn weeks_start_on_monday() {
        let t = TestDb::new();
        let mut c = t.conn();
        spend(&mut c, None, 100, "2026-10-25", ""); // 周日
        spend(&mut c, None, 200, "2026-10-26", ""); // 周一
        spend(&mut c, None, 50, "2026-10-27", "");
        let r = ask(&mut c, "aggregate_transactions", json!({"group_by":"week","from":"2026-10-19","to":"2026-10-27"}));
        let g = r["groups"].as_array().unwrap();
        assert_eq!((g[0]["group"].as_str(), f(&g[0]["total"])), (Some("2026-10-19"), Some(100.0)));
        assert_eq!((g[1]["group"].as_str(), f(&g[1]["total"])), (Some("2026-10-26"), Some(250.0)));
    }

    #[test]
    fn uncategorized_and_no_account_are_named_groups() {
        let t = TestDb::new();
        let mut c = t.conn();
        spend(&mut c, None, 15, "2026-10-05", "");
        let r = ask(&mut c, "aggregate_transactions", json!({"group_by":"category"}));
        assert_eq!(r["groups"][0]["group"], "未分类");
        let r = ask(&mut c, "aggregate_transactions", json!({"group_by":"account"}));
        assert_eq!(r["groups"][0]["group"], "无账户");
    }

    #[test]
    fn groups_are_capped_keeping_the_most_recent() {
        let t = TestDb::new();
        let mut c = t.conn();
        let start = NaiveDate::from_ymd_opt(2025, 1, 6).unwrap(); // 周一
        for i in 0..65 {
            spend(&mut c, None, 1, &fmt(start + Duration::days(7 * i)), "");
        }
        let last = fmt(start + Duration::days(7 * 64));
        let r = ask(&mut c, "aggregate_transactions", json!({"group_by":"week","from":"2025-01-01","to":"2026-10-27"}));
        let g = r["groups"].as_array().unwrap();
        assert_eq!((g.len(), r["total_groups"].as_i64(), r["truncated"].as_bool()), (60, Some(65), Some(true)));
        assert_eq!(g[59]["group"].as_str(), Some(last.as_str()));
    }

    #[test]
    fn aggregate_requires_a_valid_group_by_and_can_target_income() {
        let t = TestDb::new();
        let mut c = t.conn();
        earn(&mut c, 8000, "2026-10-01");
        assert!(err(&mut c, "aggregate_transactions", json!({})).contains("group_by"));
        assert!(err(&mut c, "aggregate_transactions", json!({"group_by":"year"})).contains("group_by"));
        let r = ask(&mut c, "aggregate_transactions", json!({"group_by":"category","kind":"income"}));
        assert_eq!((r["groups"][0]["group"].as_str(), f(&r["groups"][0]["total"])), (Some("工资"), Some(8000.0)));
    }

    // ----- 趋势 -----

    #[test]
    fn cash_flow_has_savings_rate_and_skips_the_unfinished_month_in_averages() {
        let t = TestDb::new();
        let mut c = t.conn();
        earn(&mut c, 10_000, "2026-09-01");
        spend(&mut c, None, 7_500, "2026-09-15", "");
        spend(&mut c, None, 300, "2026-10-05", ""); // 当前月:只有支出
        let r = ask(&mut c, "cash_flow_trend", json!({"months":3}));
        let m = r["months"].as_array().unwrap();
        assert_eq!(m.len(), 2, "最早的数据在 9 月,不补空月份");
        assert_eq!(r["from"], "2026-09");
        assert_eq!((f(&m[0]["net"]), f(&m[0]["savings_rate_pct"])), (Some(2500.0), Some(25.0)));
        assert_eq!((f(&m[1]["net"]), m[1]["savings_rate_pct"].clone(), m[1]["partial"].as_bool()), (Some(-300.0), Value::Null, Some(true)));
        let s = &r["summary"];
        assert_eq!((s["months_in_average"].as_i64(), f(&s["avg_expense"]), f(&s["savings_rate_pct"])), (Some(1), Some(7500.0), Some(25.0)));
        assert_eq!(s["highest_expense_month"]["month"], "2026-09");
    }

    #[test]
    fn trend_months_are_validated_and_empty_db_is_not_an_error() {
        let t = TestDb::new();
        let mut c = t.conn();
        assert!(err(&mut c, "cash_flow_trend", json!({"months":100})).contains("36"));
        assert!(err(&mut c, "net_worth_trend", json!({"months":0})).contains("36"));
        assert_eq!(ask(&mut c, "cash_flow_trend", json!({}))["months"], json!([]));
        assert_eq!(ask(&mut c, "net_worth_trend", json!({}))["months"], json!([]));
    }

    #[test]
    fn net_worth_trend_ends_at_the_overview_figure_and_reports_change() {
        let t = TestDb::new();
        let mut c = t.conn();
        let id = core::add_account(&mut c, "房产", "property", "CNY", 1_000_000_00, "2026-08-01").unwrap();
        core::set_valuation(&mut c, id, "2026-10-05", 1_060_000_00).unwrap();
        let r = ask(&mut c, "net_worth_trend", json!({"months":3}));
        let m = r["months"].as_array().unwrap();
        assert_eq!(m.len(), 3);
        let now = core::net_worth(&mut c).unwrap();
        assert_eq!(f(&m[2]["net"]), Some(yuan(now.net)));
        assert_eq!((f(&r["change"]), f(&r["change_pct"])), (Some(60_000.0), Some(6.0)));
    }

    #[test]
    fn net_worth_percentage_is_null_when_the_start_is_not_positive() {
        let t = TestDb::new();
        let mut c = t.conn();
        core::add_account(&mut c, "信用卡", "liability", "CNY", 5_000_00, "2026-08-01").unwrap();
        let r = ask(&mut c, "net_worth_trend", json!({"months":3}));
        assert_eq!(r["change_pct"], Value::Null);
        assert_eq!(f(&r["change"]), Some(0.0));
    }

    // ----- compare_periods -----

    #[test]
    fn compare_periods_diffs_categories_and_handles_one_sided_ones() {
        let t = TestDb::new();
        let mut c = t.conn();
        spend(&mut c, Some("餐饮"), 800, "2026-09-10", "");
        spend(&mut c, Some("餐饮"), 1100, "2026-10-10", "");
        spend(&mut c, Some("交通"), 50, "2026-10-11", "");
        earn(&mut c, 9000, "2026-10-01");
        let r = ask(&mut c, "compare_periods", json!({"a_from":"2026-09-01","a_to":"2026-09-30","b_from":"2026-10-01","b_to":"2026-10-31"}));
        assert_eq!((r["period_a"]["days"].as_i64(), r["period_b"]["days"].as_i64()), (Some(30), Some(31)));
        assert_eq!(f(&r["period_b"]["avg_daily_expense"]), Some(37.1)); // 1150/31 = 37.09
        assert_eq!((f(&r["expense_change"]), f(&r["income_change"])), (Some(350.0), Some(9000.0)));
        let cats = r["categories"].as_array().unwrap();
        assert_eq!(cats[0]["category"], "餐饮");
        assert_eq!((f(&cats[0]["a"]), f(&cats[0]["b"]), f(&cats[0]["change"]), f(&cats[0]["change_pct"])), (Some(800.0), Some(1100.0), Some(300.0), Some(37.5)));
        assert_eq!((cats[1]["category"].as_str(), f(&cats[1]["a"]), f(&cats[1]["change"]), cats[1]["change_pct"].clone()), (Some("交通"), Some(0.0), Some(50.0), Value::Null));
    }

    #[test]
    fn compare_periods_folds_the_tail_into_other_categories() {
        let t = TestDb::new();
        let mut c = t.conn();
        for i in 0..20 {
            let name = format!("分类{i}");
            c.exec_drop("INSERT INTO category (name, kind) VALUES (?, 'expense')", (&name,)).unwrap();
            spend(&mut c, Some(&name), 10 + i, "2026-10-10", "");
        }
        let r = ask(&mut c, "compare_periods", json!({"a_from":"2026-09-01","a_to":"2026-09-30","b_from":"2026-10-01","b_to":"2026-10-31"}));
        let cats = r["categories"].as_array().unwrap();
        assert_eq!(cats.len(), DIFF_ROWS + 1);
        assert_eq!(cats[DIFF_ROWS]["category"], "其他分类");
        let shown: f64 = cats.iter().map(|x| x["b"].as_f64().unwrap()).sum();
        assert_eq!(shown, f(&r["period_b"]["expense"]).unwrap(), "合并后总额不丢");
    }

    #[test]
    fn compare_periods_rejects_bad_periods() {
        let t = TestDb::new();
        let mut c = t.conn();
        let e = err(&mut c, "compare_periods", json!({"a_from":"2026-09-30","a_to":"2026-09-01","b_from":"2026-10-01","b_to":"2026-10-31"}));
        assert!(e.contains("晚于"), "{e}");
        assert!(err(&mut c, "compare_periods", json!({"a_from":"2026-09-01"})).contains("a_to"));
    }

    // ----- find_outliers -----

    #[test]
    fn outlier_is_reported_with_multiple_and_category_median() {
        let t = TestDb::new();
        let mut c = t.conn();
        for (i, a) in [30, 32, 34, 36, 40, 40, 44, 46, 48].iter().enumerate() {
            spend(&mut c, Some("餐饮"), *a, &format!("2026-10-{:02}", i + 1), "");
        }
        spend(&mut c, Some("餐饮"), 400, "2026-10-20", "聚餐");
        let r = ask(&mut c, "find_outliers", json!({}));
        let o = r["outliers"].as_array().unwrap();
        assert_eq!(o.len(), 1);
        assert_eq!((o[0]["note"].as_str(), f(&o[0]["amount"]), f(&o[0]["category_median"]), f(&o[0]["multiple"])), (Some("聚餐"), Some(400.0), Some(40.0), Some(10.0)));
    }

    #[test]
    fn small_samples_and_steady_rent_are_not_outliers() {
        let t = TestDb::new();
        let mut c = t.conn();
        for d in ["2026-09-01", "2026-10-01", "2026-10-02"] {
            spend(&mut c, Some("餐饮"), if d == "2026-10-02" { 400 } else { 30 }, d, "");
        }
        for m in 1..=6 {
            spend(&mut c, Some("居住"), 3000, &format!("2026-{m:02}-05"), "房租");
        }
        let r = ask(&mut c, "find_outliers", json!({"from":"2026-01-01"}));
        assert_eq!(r["total_found"], 0, "餐饮只有 3 笔不判定,房租恒定");
        assert_eq!(r["outliers"], json!([]));
        assert!(err(&mut c, "find_outliers", json!({"factor":0.5})).contains("factor"));
    }

    // ----- 只读 -----

    #[test]
    fn analytics_tools_are_registered_and_never_write() {
        let t = TestDb::new();
        let mut c = t.conn();
        spend(&mut c, Some("餐饮"), 10, "2026-10-05", "x");
        let defs = tool_defs();
        let names: Vec<&str> = defs.iter().map(|d| d["function"]["name"].as_str().unwrap()).collect();
        assert_eq!(names, TOOL_NAMES);
        assert!(names.iter().all(|n| !n.contains("sql") && !n.starts_with("add_") && !n.starts_with("delete_")));

        let before = txn_count(&mut c);
        let args = json!({"group_by":"month","a_from":"2026-09-01","a_to":"2026-09-30","b_from":"2026-10-01","b_to":"2026-10-31"});
        for n in names {
            ask(&mut c, n, args.clone());
        }
        assert_eq!(txn_count(&mut c), before);
        assert!(run(&mut c, "add_transaction", &json!({}), TODAY).is_none());
    }
}
