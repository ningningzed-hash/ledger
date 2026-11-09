//! 订阅:每月、每年固定重复的收支(房租、房贷、工资、会员费……),到期自动记账。
//!
//! 几条保证自动记账"不会悄悄弄坏数据"的规则:
//! - **每一期只入账一次。** (订阅, 日期) 在 `recurring_run` 里是主键,重复触发、多个窗口同时触发都不会重复入账。
//! - **用户删掉的账目不会被补回来。** 入账的账目被删后,这一期的记录仍在(只是 txn_id 变空)。
//! - **暂停和结束期间的空档不补记。** 恢复或延长期限后,只从现在往后记。
//! - **一个订阅出错不影响其他订阅。** 出错的会被报告,修好之后会补上。
//! - **改金额只影响以后。** 已经入账的账目保持当时的金额。
use crate::core::{self, Error, Result};
use crate::db::Conn;
use chrono::{Months, NaiveDate};
use mysql::prelude::Queryable;
use mysql::TxOpts;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const MAX_NAME_CHARS: usize = 60;
const MAX_AMOUNT: i64 = 100_000_000_000_000;
/// 新建时,开始日期太早会一次补记太多账目;超过这个数就拒绝,让用户把日期设近一些
const MAX_BACKFILL_ON_CREATE: usize = 60;
/// 单个订阅一次最多入账多少期(久未打开应用时的补记上限)
const MAX_PER_RUN: usize = 120;

#[derive(Deserialize, Debug, Clone)]
pub struct RecurringInput {
    pub name: String,
    /// expense | income
    pub kind: String,
    /// 分,必须大于 0
    pub amount: i64,
    /// monthly | yearly
    pub frequency: String,
    /// 第一期的日期。这一天及以后按周期自动记账;已经过去的日期会立刻补记
    pub start_date: String,
    pub end_date: Option<String>,
    pub category_id: Option<i64>,
    pub account_id: Option<i64>,
}

/// 编辑:周期(频率与开始日期)和收支类型不可改,要改就删除后重建,已入账的账目不受影响
#[derive(Deserialize, Debug, Clone)]
pub struct RecurringUpdate {
    pub name: String,
    pub amount: i64,
    pub end_date: Option<String>,
    pub category_id: Option<i64>,
    pub account_id: Option<i64>,
    pub paused: bool,
}

#[derive(Serialize, Debug)]
pub struct RecurringView {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub amount: i64,
    pub frequency: String,
    pub start_date: String,
    pub end_date: Option<String>,
    pub paused: bool,
    pub category_id: Option<i64>,
    pub category: Option<String>,
    pub account_id: Option<i64>,
    pub account: Option<String>,
    /// 已经自动入账的期数
    pub posted_count: i64,
    pub last_posted: Option<String>,
    /// 下一期的日期;暂停或已结束时为空
    pub next_due: Option<String>,
    /// active | paused | ended
    pub status: String,
    /// 折算成每月多少(年度订阅除以 12)
    pub monthly_equivalent: i64,
}

#[derive(Serialize, Debug)]
pub struct RecurringList {
    pub items: Vec<RecurringView>,
    /// 生效中的订阅折算后的每月固定支出 / 收入
    pub monthly_expense: i64,
    pub monthly_income: i64,
}

#[derive(Serialize, Debug)]
pub struct RunFailure {
    pub name: String,
    pub reason: String,
}

#[derive(Serialize, Debug, Default)]
pub struct RunSummary {
    pub posted: usize,
    pub failed: Vec<RunFailure>,
}

#[derive(Serialize, Debug)]
pub struct Added {
    pub id: i64,
    pub posted: usize,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Preview {
    /// 按这个设置,保存后会立刻补记几期
    pub due_now: usize,
    pub first_due: Option<String>,
    pub last_due: Option<String>,
    /// 之后的下一期
    pub next_due: Option<String>,
}

// ---------- 日期推算 ----------

fn parse_date(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| Error::Invalid(format!("日期格式应为 YYYY-MM-DD,收到:{s}")))
}

fn fmt(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// 第 i 期的日期。每一期都从开始日期直接推算(而不是上一期往后加),
/// 所以 1 月 31 日的月度订阅是 1/31、2/28、3/31、4/30……,不会越推越靠前
fn nth(freq: &str, start: NaiveDate, i: u32) -> Option<NaiveDate> {
    let months = if freq == "yearly" { i.checked_mul(12)? } else { i };
    start.checked_add_months(Months::new(months))
}

/// 开始日期起、不晚于 until 的各期日期,最多 limit 个
pub fn occurrences(freq: &str, start: NaiveDate, until: NaiveDate, limit: usize) -> Vec<NaiveDate> {
    let mut out = Vec::new();
    for i in 0..(limit as u32).saturating_add(1000) {
        match nth(freq, start, i) {
            Some(d) if d <= until && out.len() < limit => out.push(d),
            _ => break,
        }
    }
    out
}

/// 严格晚于 after 的下一期;超过 end(含)则没有下一期
pub fn next_after(freq: &str, start: NaiveDate, after: NaiveDate, end: Option<NaiveDate>) -> Option<NaiveDate> {
    for i in 0..20_000u32 {
        let d = nth(freq, start, i)?;
        if d > after {
            return if end.is_none_or(|e| d <= e) { Some(d) } else { None };
        }
    }
    None
}

fn due_until(today: NaiveDate, end: Option<NaiveDate>) -> NaiveDate {
    end.map_or(today, |e| e.min(today))
}

/// 保存前预览:按这个设置会立刻补记几期、下一期是哪天(不读写数据库)
pub fn preview(frequency: &str, start_date: &str, end_date: Option<&str>, today: &str) -> Result<Preview> {
    if frequency != "monthly" && frequency != "yearly" {
        return Err(Error::Invalid(format!("频率只能是 monthly 或 yearly,收到:{frequency}")));
    }
    let (start, today_d) = (parse_date(start_date)?, parse_date(today)?);
    let end = match end_date {
        Some(e) if !e.trim().is_empty() => Some(parse_date(e.trim())?),
        _ => None,
    };
    let due = occurrences(frequency, start, due_until(today_d, end), MAX_BACKFILL_ON_CREATE + 1);
    Ok(Preview {
        due_now: due.len(),
        first_due: due.first().map(|d| fmt(*d)),
        last_due: due.last().map(|d| fmt(*d)),
        next_due: next_after(frequency, start, today_d, end).map(fmt),
    })
}

// ---------- 读写 ----------

struct Row {
    id: i64,
    name: String,
    kind: String,
    amount: i64,
    frequency: String,
    start: NaiveDate,
    end: Option<NaiveDate>,
    paused: bool,
    category_id: Option<i64>,
    account_id: Option<i64>,
}

type RowTuple = (i64, String, String, i64, String, String, Option<String>, i64, Option<i64>, Option<i64>);
const ROW_COLS: &str = "id, name, kind, amount, frequency, start_date, end_date, paused, category_id, account_id";

fn to_row(t: RowTuple) -> Result<Row> {
    Ok(Row {
        id: t.0,
        name: t.1,
        kind: t.2,
        amount: t.3,
        frequency: t.4,
        start: parse_date(&t.5)?,
        end: t.6.as_deref().map(parse_date).transpose()?,
        paused: t.7 != 0,
        category_id: t.8,
        account_id: t.9,
    })
}

fn load_rows(c: &mut impl Queryable, id: Option<i64>) -> Result<Vec<Row>> {
    let tuples: Vec<RowTuple> = match id {
        Some(id) => c.exec(format!("SELECT {ROW_COLS} FROM recurring WHERE id=? ORDER BY id"), (id,))?,
        None => c.query(format!("SELECT {ROW_COLS} FROM recurring ORDER BY id"))?,
    };
    tuples.into_iter().map(to_row).collect()
}

struct Checked {
    name: String,
    end: Option<NaiveDate>,
}

/// 校验名称、金额、结束日期,以及分类、账户是否可用于这种收支。不写任何数据。
fn validate_common(
    c: &mut impl Queryable, name: &str, kind: &str, amount: i64, start: NaiveDate,
    end_date: Option<&str>, category_id: Option<i64>, account_id: Option<i64>,
) -> Result<Checked> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(Error::Invalid("请填写订阅名称".into()));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(Error::Invalid(format!("订阅名称不能超过 {MAX_NAME_CHARS} 个字")));
    }
    if amount <= 0 || amount > MAX_AMOUNT {
        return Err(Error::Invalid("金额必须大于 0".into()));
    }
    let end = match end_date.map(str::trim) {
        None | Some("") => None,
        Some(e) => {
            let e = parse_date(e)?;
            if e < start {
                return Err(Error::Invalid("结束日期不能早于开始日期".into()));
            }
            Some(e)
        }
    };
    if let Some(cid) = category_id {
        let cat = core::list_categories(c)?.into_iter().find(|x| x.id == cid).ok_or_else(|| Error::Invalid("分类不存在".into()))?;
        if cat.kind != kind {
            return Err(Error::Invalid(format!("分类“{}”不属于{}", cat.name, if kind == "expense" { "支出" } else { "收入" })));
        }
    }
    if let Some(aid) = account_id {
        let acc = core::list_accounts(c)?.into_iter().find(|x| x.id == aid).ok_or_else(|| Error::Invalid("账户不存在".into()))?;
        if acc.kind == "property" {
            return Err(Error::Invalid(format!("“{}”是房产账户,不能用于记收支", acc.name)));
        }
    }
    Ok(Checked { name, end })
}

/// 处理某一期:先占住 (订阅, 日期) 这个位置,再记账,两步在同一个事务里。
/// 返回 false 表示这一期早已处理过(已入账、已被跳过或被用户删除过),什么都不做。
fn post_occurrence(c: &mut Conn, r: &Row, date: NaiveDate) -> Result<bool> {
    let d = fmt(date);
    let mut tx = c.start_transaction(TxOpts::default())?;
    tx.exec_drop("INSERT IGNORE INTO recurring_run (recurring_id, due_date) VALUES (?,?)", (r.id, &d))?;
    if tx.affected_rows() == 0 {
        return Ok(false); // 事务随 drop 回滚,没有任何改动
    }
    let txn_id = core::add_txn(&mut tx, r.account_id, r.category_id, &r.kind, r.amount, &d, &r.name)?;
    tx.exec_drop("UPDATE recurring_run SET txn_id=? WHERE recurring_id=? AND due_date=?", (txn_id, r.id, &d))?;
    tx.commit()?;
    Ok(true)
}

/// 把某个订阅截至今天所有还没处理的期数记入账目,返回新记了几期
fn post_due(c: &mut Conn, r: &Row, today: NaiveDate) -> Result<usize> {
    if r.paused {
        return Ok(0);
    }
    let mut n = 0;
    for date in occurrences(&r.frequency, r.start, due_until(today, r.end), MAX_PER_RUN) {
        if post_occurrence(c, r, date)? {
            n += 1;
        }
    }
    Ok(n)
}

/// 把 (after, today] 内尚未处理的期数标记为"跳过":它们不会被记账,也不会再被补记
fn skip_gap(c: &mut Conn, r: &Row, after: Option<NaiveDate>, today: NaiveDate) -> Result<()> {
    for date in occurrences(&r.frequency, r.start, today, MAX_PER_RUN * 4) {
        if after.is_some_and(|a| date <= a) {
            continue;
        }
        c.exec_drop("INSERT IGNORE INTO recurring_run (recurring_id, due_date) VALUES (?,?)", (r.id, fmt(date)))?;
    }
    Ok(())
}

pub fn add_recurring(c: &mut Conn, input: &RecurringInput, today: &str) -> Result<Added> {
    let today_d = parse_date(today)?;
    if input.kind != "expense" && input.kind != "income" {
        return Err(Error::Invalid(format!("收支类型只能是 expense 或 income,收到:{}", input.kind)));
    }
    if input.frequency != "monthly" && input.frequency != "yearly" {
        return Err(Error::Invalid(format!("频率只能是 monthly 或 yearly,收到:{}", input.frequency)));
    }
    let start = parse_date(&input.start_date)?;
    let v = validate_common(c, &input.name, &input.kind, input.amount, start, input.end_date.as_deref(), input.category_id, input.account_id)?;
    let backfill = occurrences(&input.frequency, start, due_until(today_d, v.end), MAX_BACKFILL_ON_CREATE + 1).len();
    if backfill > MAX_BACKFILL_ON_CREATE {
        return Err(Error::Invalid(format!("开始日期太早,保存后会一次补记超过 {MAX_BACKFILL_ON_CREATE} 笔账目。请把开始日期设得近一些")));
    }

    c.exec_drop(
        "INSERT INTO recurring (name, kind, amount, frequency, start_date, end_date, paused, category_id, account_id) VALUES (?,?,?,?,?,?,0,?,?)",
        (&v.name, &input.kind, input.amount, &input.frequency, fmt(start), v.end.map(fmt), input.category_id, input.account_id),
    )?;
    let id = core::last_id(c)?;
    let row = load_rows(c, Some(id))?.remove(0);
    let posted = post_due(c, &row, today_d)?;
    Ok(Added { id, posted })
}

pub fn update_recurring(c: &mut Conn, id: i64, upd: &RecurringUpdate, today: &str) -> Result<usize> {
    let today_d = parse_date(today)?;
    let old = load_rows(c, Some(id))?.into_iter().next().ok_or_else(|| Error::Invalid("订阅不存在".into()))?;
    let v = validate_common(c, &upd.name, &old.kind, upd.amount, old.start, upd.end_date.as_deref(), upd.category_id, upd.account_id)?;

    c.exec_drop(
        "UPDATE recurring SET name=?, amount=?, end_date=?, paused=?, category_id=?, account_id=? WHERE id=?",
        (&v.name, upd.amount, v.end.map(fmt), upd.paused as i64, upd.category_id, upd.account_id, id),
    )?;
    let row = load_rows(c, Some(id))?.remove(0);

    // 暂停期间、结束之后错过的期数不补记:恢复或延长期限时先把这些空档标成"跳过"
    if old.paused && !upd.paused {
        skip_gap(c, &row, None, today_d)?;
    } else if let Some(old_end) = old.end {
        if v.end.is_none_or(|e| e > old_end) {
            skip_gap(c, &row, Some(old_end), today_d)?;
        }
    }
    // 之前因为出错(如账户不可用)没记上的期数,修好之后在这里补上
    post_due(c, &row, today_d)
}

pub fn delete_recurring(c: &mut Conn, id: i64) -> Result<()> {
    c.exec_drop("DELETE FROM recurring WHERE id=?", (id,))?; // 历史记录随之级联删除;已入账的账目保留
    if c.affected_rows() == 0 {
        return Err(Error::Invalid("订阅不存在".into()));
    }
    Ok(())
}

/// 把所有生效中的订阅截至今天到期的账目记上。某个订阅出错不影响其他订阅。
pub fn run_due(c: &mut Conn, today: &str) -> Result<RunSummary> {
    let today_d = parse_date(today)?;
    let mut summary = RunSummary::default();
    for row in load_rows(c, None)? {
        match post_due(c, &row, today_d) {
            Ok(n) => summary.posted += n,
            Err(e) => summary.failed.push(RunFailure { name: row.name.clone(), reason: e.to_string() }),
        }
    }
    Ok(summary)
}

pub fn list_recurring(c: &mut impl Queryable, today: &str) -> Result<RecurringList> {
    let today_d = parse_date(today)?;
    type Joined = (RowTuple, Option<String>, Option<String>);
    let rows: Vec<Joined> = c
        .query_map(
            "SELECT r.id, r.name, r.kind, r.amount, r.frequency, r.start_date, r.end_date, r.paused, r.category_id, r.account_id,
                    c.name, a.name
             FROM recurring r
             LEFT JOIN category c ON c.id=r.category_id
             LEFT JOIN account a ON a.id=r.account_id
             ORDER BY r.id",
            |(a, b, cc, d, e, f, g, h, i, j, cat, acc): (i64, String, String, i64, String, String, Option<String>, i64, Option<i64>, Option<i64>, Option<String>, Option<String>)| {
                ((a, b, cc, d, e, f, g, h, i, j), cat, acc)
            },
        )?;
    let stats: HashMap<i64, (i64, Option<String>)> = c
        .query_map(
            "SELECT recurring_id, COUNT(txn_id), MAX(CASE WHEN txn_id IS NOT NULL THEN due_date END) FROM recurring_run GROUP BY recurring_id",
            |(id, n, last): (i64, i64, Option<String>)| (id, (n, last)),
        )?
        .into_iter()
        .collect();

    let (mut monthly_expense, mut monthly_income) = (0, 0);
    let mut items = Vec::with_capacity(rows.len());
    for (tuple, category, account) in rows {
        let r = to_row(tuple)?;
        let next = if r.paused { None } else { next_after(&r.frequency, r.start, today_d, r.end) };
        let status = if r.paused {
            "paused"
        } else if r.end.is_some() && next.is_none() {
            "ended"
        } else {
            "active"
        };
        let monthly_equivalent = if r.frequency == "yearly" { (r.amount as f64 / 12.0).round() as i64 } else { r.amount };
        if status == "active" {
            if r.kind == "expense" { monthly_expense += monthly_equivalent } else { monthly_income += monthly_equivalent }
        }
        let (posted_count, last_posted) = stats.get(&r.id).cloned().unwrap_or((0, None));
        items.push(RecurringView {
            id: r.id,
            name: r.name,
            kind: r.kind,
            amount: r.amount,
            frequency: r.frequency,
            start_date: fmt(r.start),
            end_date: r.end.map(fmt),
            paused: r.paused,
            category_id: r.category_id,
            category,
            account_id: r.account_id,
            account,
            posted_count,
            last_posted,
            next_due: next.map(fmt),
            status: status.into(),
            monthly_equivalent,
        });
    }
    Ok(RecurringList { items, monthly_expense, monthly_income })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdb::TestDb;

    const TODAY: &str = "2026-10-28";
    fn d(s: &str) -> NaiveDate {
        parse_date(s).unwrap()
    }
    fn dates(v: Vec<NaiveDate>) -> Vec<String> {
        v.into_iter().map(fmt).collect()
    }

    fn rent(start: &str, account: Option<i64>, category: Option<i64>) -> RecurringInput {
        RecurringInput {
            name: "房租".into(), kind: "expense".into(), amount: 3_000_00, frequency: "monthly".into(),
            start_date: start.into(), end_date: None, category_id: category, account_id: account,
        }
    }
    fn txn_count(c: &mut Conn) -> i64 {
        c.query_first("SELECT COUNT(*) FROM txn").unwrap().unwrap()
    }
    fn list(c: &mut Conn, today: &str) -> RecurringList {
        list_recurring(c, today).unwrap()
    }
    fn only(c: &mut Conn, today: &str) -> RecurringView {
        let mut l = list(c, today).items;
        assert_eq!(l.len(), 1);
        l.remove(0)
    }
    fn expense_cat(c: &mut Conn, name: &str) -> Option<i64> {
        core::find_category(c, name, "expense").unwrap()
    }

    // ---- 日期推算(纯函数) ----

    #[test]
    fn month_end_days_are_clamped_without_drifting() {
        let got = dates(occurrences("monthly", d("2026-01-31"), d("2026-06-30"), 100));
        assert_eq!(got, ["2026-01-31", "2026-02-28", "2026-03-31", "2026-04-30", "2026-05-31", "2026-06-30"]);
        let leap = dates(occurrences("monthly", d("2024-01-31"), d("2024-03-31"), 100));
        assert_eq!(leap, ["2024-01-31", "2024-02-29", "2024-03-31"]);
    }

    #[test]
    fn yearly_follows_leap_days() {
        let got = dates(occurrences("yearly", d("2024-02-29"), d("2029-03-01"), 100));
        assert_eq!(got, ["2024-02-29", "2025-02-28", "2026-02-28", "2027-02-28", "2028-02-29", "2029-02-28"]);
    }

    #[test]
    fn occurrences_respect_bounds_and_limit() {
        assert!(occurrences("monthly", d("2026-11-05"), d("2026-10-28"), 10).is_empty(), "开始日期在未来");
        assert_eq!(occurrences("monthly", d("2020-01-05"), d("2026-10-28"), 5).len(), 5, "受 limit 限制");
        assert_eq!(dates(occurrences("monthly", d("2026-10-28"), d("2026-10-28"), 5)), ["2026-10-28"], "当天算到期");
    }

    #[test]
    fn next_after_and_end_date() {
        assert_eq!(next_after("monthly", d("2026-08-05"), d("2026-10-28"), None), Some(d("2026-11-05")));
        assert_eq!(next_after("monthly", d("2026-08-05"), d("2026-10-05"), None), Some(d("2026-11-05")), "严格晚于");
        assert_eq!(next_after("monthly", d("2026-12-01"), d("2026-10-28"), None), Some(d("2026-12-01")), "还没开始");
        assert_eq!(next_after("monthly", d("2026-08-05"), d("2026-10-28"), Some(d("2026-10-31"))), None, "已超过结束日期");
        assert_eq!(next_after("monthly", d("2026-08-05"), d("2026-10-28"), Some(d("2026-11-05"))), Some(d("2026-11-05")), "结束日当天仍有效");
        assert_eq!(next_after("yearly", d("2025-03-10"), d("2026-10-28"), None), Some(d("2027-03-10")));
    }

    #[test]
    fn preview_matches_what_saving_would_do() {
        let p = preview("monthly", "2026-08-05", None, TODAY).unwrap();
        assert_eq!(p, Preview { due_now: 3, first_due: Some("2026-08-05".into()), last_due: Some("2026-10-05".into()), next_due: Some("2026-11-05".into()) });
        let future = preview("yearly", "2027-01-01", Some(""), TODAY).unwrap();
        assert_eq!((future.due_now, future.next_due.as_deref()), (0, Some("2027-01-01")));
        assert!(preview("weekly", "2026-08-05", None, TODAY).is_err());
        assert!(preview("monthly", "not-a-date", None, TODAY).is_err());
    }

    // ---- 自动入账 ----

    #[test]
    fn future_start_posts_nothing_until_it_is_due() {
        let t = TestDb::new();
        let mut c = t.conn();
        let r = add_recurring(&mut c, &rent("2026-11-05", None, None), TODAY).unwrap();
        assert_eq!(r.posted, 0);
        assert_eq!(txn_count(&mut c), 0);
        let v = only(&mut c, TODAY);
        assert_eq!((v.status.as_str(), v.next_due.as_deref(), v.posted_count), ("active", Some("2026-11-05"), 0));

        // 到期当天自动记账;再运行一次不会重复
        assert_eq!(run_due(&mut c, "2026-11-05").unwrap().posted, 1);
        assert_eq!(run_due(&mut c, "2026-11-05").unwrap().posted, 0);
        assert_eq!(txn_count(&mut c), 1);
    }

    #[test]
    fn past_start_backfills_each_period_once_with_correct_fields() {
        let t = TestDb::new();
        let mut c = t.conn();
        let acc = core::add_account(&mut c, "招行", "deposit", "CNY", 10_000_00, "2026-08-01").unwrap();
        let cat = expense_cat(&mut c, "居住");
        let r = add_recurring(&mut c, &rent("2026-08-05", Some(acc), cat), TODAY).unwrap();
        assert_eq!(r.posted, 3);

        let txns = core::list_all_txns(&mut c).unwrap();
        assert_eq!(txns.iter().map(|x| x.date.as_str()).collect::<Vec<_>>(), ["2026-08-05", "2026-09-05", "2026-10-05"]);
        assert!(txns.iter().all(|x| x.amount == 3_000_00 && x.kind == "expense" && x.note == "房租" && x.category.as_deref() == Some("居住") && x.account.as_deref() == Some("招行")));
        // 账户余额随之减少:1 万 − 3 期 × 3,000
        assert_eq!(core::list_accounts(&mut c).unwrap()[0].balance, 1_000_00);

        assert_eq!(run_due(&mut c, TODAY).unwrap().posted, 0, "再运行不会重复入账");
        let v = only(&mut c, TODAY);
        assert_eq!((v.posted_count, v.last_posted.as_deref(), v.next_due.as_deref()), (3, Some("2026-10-05"), Some("2026-11-05")));
    }

    #[test]
    fn catches_up_after_the_app_was_closed_for_months() {
        let t = TestDb::new();
        let mut c = t.conn();
        add_recurring(&mut c, &rent("2026-10-05", None, None), TODAY).unwrap();
        assert_eq!(txn_count(&mut c), 1);
        let s = run_due(&mut c, "2027-02-10").unwrap(); // 11 月、12 月、1 月、2 月
        assert_eq!((s.posted, s.failed.len()), (4, 0));
        assert_eq!(txn_count(&mut c), 5);
    }

    #[test]
    fn yearly_and_income_items() {
        let t = TestDb::new();
        let mut c = t.conn();
        let salary = core::find_category(&mut c, "工资", "income").unwrap();
        let mut sal = rent("2026-10-25", None, salary);
        (sal.name, sal.kind, sal.amount) = ("工资".into(), "income".into(), 28_500_00);
        add_recurring(&mut c, &sal, TODAY).unwrap();
        let mut vip = rent("2026-03-10", None, None);
        (vip.name, vip.frequency, vip.amount) = ("视频会员".into(), "yearly".into(), 198_00);
        assert_eq!(add_recurring(&mut c, &vip, TODAY).unwrap().posted, 1, "2026-03-10 这一期");
        let all = core::list_all_txns(&mut c).unwrap();
        assert!(all.iter().any(|x| x.kind == "income" && x.amount == 28_500_00 && x.date == "2026-10-25"));
        assert_eq!(only_named(&mut c, "视频会员").next_due.as_deref(), Some("2027-03-10"));
    }
    fn only_named(c: &mut Conn, name: &str) -> RecurringView {
        list(c, TODAY).items.into_iter().find(|x| x.name == name).unwrap()
    }

    #[test]
    fn deleted_auto_posted_entries_are_not_posted_again() {
        let t = TestDb::new();
        let mut c = t.conn();
        add_recurring(&mut c, &rent("2026-09-05", None, None), TODAY).unwrap(); // 9/5、10/5
        let sep = core::list_all_txns(&mut c).unwrap().into_iter().find(|x| x.date == "2026-09-05").unwrap();
        core::delete_txn(&mut c, sep.id).unwrap();
        assert_eq!(run_due(&mut c, TODAY).unwrap().posted, 0, "用户删掉的那期不再补");
        assert_eq!(txn_count(&mut c), 1);
        assert_eq!(only(&mut c, TODAY).posted_count, 1);
    }

    #[test]
    fn concurrent_runs_never_double_post() {
        let t = TestDb::new();
        add_recurring(&mut t.conn(), &rent("2030-01-05", None, None), TODAY).unwrap();
        let pool = t.pool.clone();
        let handles: Vec<_> = (0..6)
            .map(|_| {
                let pool = pool.clone();
                std::thread::spawn(move || {
                    let mut c = pool.get_conn().unwrap();
                    run_due(&mut c, "2030-06-10").unwrap().posted // 1~6 月共 6 期
                })
            })
            .collect();
        let total: usize = handles.into_iter().map(|h| h.join().unwrap()).sum();
        assert_eq!(total, 6, "每一期恰好被一个线程入账");
        assert_eq!(txn_count(&mut t.conn()), 6);
    }

    // ---- 暂停、结束、编辑 ----

    fn update(c: &mut Conn, id: i64, today: &str, f: impl FnOnce(&mut RecurringUpdate)) -> Result<usize> {
        let v = list(c, today).items.into_iter().find(|x| x.id == id).unwrap();
        let mut u = RecurringUpdate {
            name: v.name, amount: v.amount, end_date: v.end_date, category_id: v.category_id, account_id: v.account_id, paused: v.paused,
        };
        f(&mut u);
        update_recurring(c, id, &u, today)
    }

    #[test]
    fn pausing_stops_posting_and_resuming_does_not_backfill_the_gap() {
        let t = TestDb::new();
        let mut c = t.conn();
        let id = add_recurring(&mut c, &rent("2026-10-05", None, None), TODAY).unwrap().id;
        update(&mut c, id, TODAY, |u| u.paused = true).unwrap();
        let v = only(&mut c, TODAY);
        assert_eq!((v.status.as_str(), v.next_due), ("paused", None));

        assert_eq!(run_due(&mut c, "2027-01-20").unwrap().posted, 0, "暂停期间不入账");
        assert_eq!(txn_count(&mut c), 1);

        // 1 月 20 日恢复:11、12、1 月的空档被跳过,2 月 5 日起继续
        assert_eq!(update(&mut c, id, "2027-01-20", |u| u.paused = false).unwrap(), 0);
        assert_eq!(txn_count(&mut c), 1);
        assert_eq!(run_due(&mut c, "2027-02-05").unwrap().posted, 1);
        assert_eq!(txn_count(&mut c), 2);
        assert_eq!(only(&mut c, "2027-02-05").posted_count, 2);
    }

    #[test]
    fn end_date_stops_posting_and_extending_it_does_not_backfill() {
        let t = TestDb::new();
        let mut c = t.conn();
        let mut input = rent("2026-08-05", None, None);
        input.end_date = Some("2026-09-30".into());
        let id = add_recurring(&mut c, &input, TODAY).unwrap().id; // 只记 8/5、9/5
        assert_eq!(txn_count(&mut c), 2);
        let v = only(&mut c, TODAY);
        assert_eq!((v.status.as_str(), v.next_due), ("ended", None));
        assert_eq!(run_due(&mut c, "2027-05-01").unwrap().posted, 0);

        // 2027-01-10 把结束日期延长到 2027-03-31:10、11、12 月和 1 月 5 日的空档不补,2 月 5 日起继续
        assert_eq!(update(&mut c, id, "2027-01-10", |u| u.end_date = Some("2027-03-31".into())).unwrap(), 0);
        assert_eq!(txn_count(&mut c), 2);
        assert_eq!(run_due(&mut c, "2027-02-05").unwrap().posted, 1);
        assert_eq!(run_due(&mut c, "2027-03-05").unwrap().posted, 1);
        assert_eq!(run_due(&mut c, "2027-04-05").unwrap().posted, 0, "超过新的结束日期");
        assert_eq!(txn_count(&mut c), 4);
    }

    #[test]
    fn changing_the_amount_only_affects_later_periods() {
        let t = TestDb::new();
        let mut c = t.conn();
        let id = add_recurring(&mut c, &rent("2026-09-05", None, None), TODAY).unwrap().id; // 9/5、10/5 各 3,000
        update(&mut c, id, TODAY, |u| u.amount = 3_500_00).unwrap();
        run_due(&mut c, "2026-11-05").unwrap();
        let amounts: Vec<i64> = core::list_all_txns(&mut c).unwrap().iter().map(|x| x.amount).collect();
        assert_eq!(amounts, [3_000_00, 3_000_00, 3_500_00]);
    }

    #[test]
    fn one_failing_item_does_not_block_the_others_and_is_retried_once_fixed() {
        let t = TestDb::new();
        let mut c = t.conn();
        let acc = core::add_account(&mut c, "旧卡", "deposit", "CNY", 5_000_00, "2026-08-01").unwrap();
        let bad = add_recurring(&mut c, &rent("2026-11-05", Some(acc), None), TODAY).unwrap().id;
        let mut other = rent("2026-11-10", None, None);
        other.name = "宽带".into();
        add_recurring(&mut c, &other, TODAY).unwrap();

        c.exec_drop("UPDATE account SET archived=1 WHERE id=?", (acc,)).unwrap(); // 账户被归档,房租记不上
        let s = run_due(&mut c, "2026-11-15").unwrap();
        assert_eq!(s.posted, 1, "宽带照常入账");
        assert_eq!((s.failed.len(), s.failed[0].name.as_str()), (1, "房租"));
        assert!(s.failed[0].reason.contains("账户"), "{}", s.failed[0].reason);
        assert_eq!(txn_count(&mut c), 1);

        // 改成不关联账户:之前没记上的 11 月房租补上(不是被"跳过")
        assert_eq!(update(&mut c, bad, "2026-11-15", |u| u.account_id = None).unwrap(), 1);
        assert_eq!(txn_count(&mut c), 2);
    }

    #[test]
    fn deleting_a_subscription_keeps_what_it_already_posted() {
        let t = TestDb::new();
        let mut c = t.conn();
        let id = add_recurring(&mut c, &rent("2026-09-05", None, None), TODAY).unwrap().id;
        delete_recurring(&mut c, id).unwrap();
        assert_eq!(txn_count(&mut c), 2);
        let runs: i64 = c.query_first("SELECT COUNT(*) FROM recurring_run").unwrap().unwrap();
        assert_eq!(runs, 0);
        assert!(delete_recurring(&mut c, id).is_err());
        assert_eq!(run_due(&mut c, "2027-01-01").unwrap().posted, 0, "删除后不再入账");
    }

    // ---- 校验 ----

    #[test]
    fn invalid_input_is_rejected_and_nothing_is_written() {
        let t = TestDb::new();
        let mut c = t.conn();
        let acc = core::add_account(&mut c, "招行", "deposit", "CNY", 1_000_00, "2026-08-01").unwrap();
        let house = core::add_account(&mut c, "住宅", "property", "CNY", 100_0000_00, "2026-08-01").unwrap();
        let income_cat = core::find_category(&mut c, "工资", "income").unwrap();
        let base = rent("2026-11-05", Some(acc), None);
        let with = |f: &dyn Fn(&mut RecurringInput)| {
            let mut r = base.clone();
            f(&mut r);
            r
        };
        let bad = [
            with(&|r| r.name = "  ".into()),
            with(&|r| r.name = "长".repeat(61)),
            with(&|r| r.kind = "refund".into()),
            with(&|r| r.amount = 0),
            with(&|r| r.amount = -5),
            with(&|r| r.frequency = "weekly".into()),
            with(&|r| r.start_date = "2026-13-01".into()),
            with(&|r| r.end_date = Some("nope".into())),
            with(&|r| r.end_date = Some("2026-10-01".into())),   // 早于开始日期
            with(&|r| r.category_id = Some(999)),
            with(&|r| r.category_id = income_cat),               // 支出订阅不能用收入分类
            with(&|r| r.account_id = Some(999)),
            with(&|r| r.account_id = Some(house)),               // 房产不能记收支
            rent("2020-01-05", None, None),                      // 会一次补记 80 多笔
        ];
        for b in &bad {
            assert!(add_recurring(&mut c, b, TODAY).is_err(), "应拒绝 {b:?}");
        }
        assert_eq!(list(&mut c, TODAY).items.len(), 0);
        assert_eq!(txn_count(&mut c), 0);
        // 刚好 60 笔可以(2021-11-05 ~ 2026-10-05)
        assert_eq!(add_recurring(&mut c, &rent("2021-11-05", None, None), TODAY).unwrap().posted, 60);
    }

    #[test]
    fn update_validates_and_refuses_unknown_ids() {
        let t = TestDb::new();
        let mut c = t.conn();
        let id = add_recurring(&mut c, &rent("2026-11-05", None, None), TODAY).unwrap().id;
        let ok = RecurringUpdate { name: "房租".into(), amount: 1, end_date: None, category_id: None, account_id: None, paused: false };
        assert!(update_recurring(&mut c, 999, &ok, TODAY).is_err());
        assert!(update_recurring(&mut c, id, &RecurringUpdate { amount: 0, ..ok.clone() }, TODAY).is_err());
        assert!(update_recurring(&mut c, id, &RecurringUpdate { end_date: Some("2026-01-01".into()), ..ok.clone() }, TODAY).is_err());
        assert!(update_recurring(&mut c, id, &RecurringUpdate { name: "".into(), ..ok.clone() }, TODAY).is_err());
        assert_eq!(only(&mut c, TODAY).amount, 3_000_00, "校验失败不应改动原订阅");
    }

    // ---- 汇总 ----

    #[test]
    fn monthly_totals_count_only_active_items_and_spread_yearly_ones() {
        let t = TestDb::new();
        let mut c = t.conn();
        add_recurring(&mut c, &rent("2026-11-05", None, None), TODAY).unwrap(); // 每月 3,000
        let mut vip = rent("2026-12-01", None, None);
        (vip.name, vip.frequency, vip.amount) = ("会员".into(), "yearly".into(), 1_200_00);
        add_recurring(&mut c, &vip, TODAY).unwrap(); // 折合每月 100
        let mut sal = rent("2026-11-25", None, None);
        (sal.name, sal.kind, sal.amount) = ("工资".into(), "income".into(), 28_500_00);
        add_recurring(&mut c, &sal, TODAY).unwrap();
        let paused = add_recurring(&mut c, &RecurringInput { name: "停用的".into(), amount: 999_00, ..rent("2026-11-06", None, None) }, TODAY).unwrap().id;
        update(&mut c, paused, TODAY, |u| u.paused = true).unwrap();

        let l = list(&mut c, TODAY);
        assert_eq!((l.monthly_expense, l.monthly_income), (3_100_00, 28_500_00));
        assert_eq!(l.items.len(), 4);
    }

    /// 对真实数据库的只读检查(只列出,不触发入账):cargo test live_recurring_real -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_recurring_real_database() {
        let cfg = crate::db::load_config(&crate::testdb::app_data_dir()).expect("需要先运行 scripts/setup-mysql.sh");
        let mut c = crate::db::connect(&cfg).expect("连接真实数据库失败").get_conn().unwrap();
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let l = list_recurring(&mut c, &today).unwrap();
        println!("真实库里有 {} 个订阅,每月固定支出 {} 分、收入 {} 分", l.items.len(), l.monthly_expense, l.monthly_income);
    }
}
