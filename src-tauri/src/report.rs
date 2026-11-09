//! 报表:按月收支、净资产历史、区间内分类支出。
//!
//! 区间由"截止月份 + 回看月数"确定,起点不会早于最早的一条数据(不画数据出现之前的空月份)。
//! 截止月份(当前月)的净资产取"当前值",与总览页完全一致;更早的月份取该月最后一天的值。
use crate::core::{self, CategorySum, Error, Result};
use chrono::{Months, NaiveDate};
use mysql::prelude::Queryable;
use serde::Serialize;
use std::collections::HashMap;

/// 最多回看 20 年,避免一次请求逐月算上千次
const MAX_MONTHS: u32 = 240;

#[derive(Serialize, Debug, PartialEq)]
pub struct MonthRow {
    pub month: String,
    pub income: i64,
    pub expense: i64,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct NetPoint {
    pub month: String,
    pub assets: i64,
    pub liabilities: i64,
    pub net: i64,
}

#[derive(Serialize)]
pub struct Report {
    pub from: String,
    pub to: String,
    pub months: Vec<MonthRow>,
    pub net_worth: Vec<NetPoint>,
    /// 区间内各分类支出合计,按金额降序
    pub categories: Vec<CategorySum>,
}

fn first_day(m: &str) -> Result<NaiveDate> {
    let bad = || Error::Invalid(format!("月份格式应为 YYYY-MM,收到:{m}"));
    if m.len() != 7 {
        return Err(bad());
    }
    NaiveDate::parse_from_str(&format!("{m}-01"), "%Y-%m-%d").map_err(|_| bad())
}

fn fmt_month(d: NaiveDate) -> String {
    d.format("%Y-%m").to_string()
}

/// 往前推 n 个月
pub fn shift_back(m: &str, n: u32) -> Result<String> {
    let d = first_day(m)?.checked_sub_months(Months::new(n)).ok_or_else(|| Error::Invalid("月份超出范围".into()))?;
    Ok(fmt_month(d))
}

/// 往后推 n 个月
pub fn shift_forward(m: &str, n: u32) -> Result<String> {
    let d = first_day(m)?.checked_add_months(Months::new(n)).ok_or_else(|| Error::Invalid("月份超出范围".into()))?;
    Ok(fmt_month(d))
}

/// b 比 a 晚几个月(b 早于 a 时为负数)
pub fn month_diff(a: &str, b: &str) -> Result<i64> {
    let (x, y) = (first_day(a)?, first_day(b)?);
    use chrono::Datelike;
    Ok((y.year() as i64 - x.year() as i64) * 12 + (y.month() as i64 - x.month() as i64))
}

/// 该月最后一天,形如 "2024-02-29"
pub fn month_end(m: &str) -> Result<String> {
    let next = first_day(m)?.checked_add_months(Months::new(1)).ok_or_else(|| Error::Invalid("月份超出范围".into()))?;
    Ok(next.pred_opt().ok_or_else(|| Error::Invalid("月份超出范围".into()))?.format("%Y-%m-%d").to_string())
}

/// from..=to 的每个月份
fn months_between(from: &str, to: &str) -> Result<Vec<String>> {
    let (mut d, end) = (first_day(from)?, first_day(to)?);
    let mut out = Vec::new();
    while d <= end {
        out.push(fmt_month(d));
        d = d.checked_add_months(Months::new(1)).ok_or_else(|| Error::Invalid("月份超出范围".into()))?;
    }
    Ok(out)
}

/// 最早一条估值或账目所在的月份
fn earliest_month(c: &mut impl Queryable) -> Result<Option<String>> {
    let first: Option<Option<String>> = c.query_first(
        "SELECT MIN(m) FROM (
            SELECT MIN(SUBSTR(date,1,7)) AS m FROM valuation
            UNION ALL SELECT MIN(SUBSTR(date,1,7)) FROM txn) x",
    )?;
    Ok(first.flatten())
}

/// `through` 为截止月份(通常是当前月);`months` 为回看月数,0 表示从最早的数据开始。
pub fn report(c: &mut impl Queryable, through: &str, months: u32) -> Result<Report> {
    first_day(through)?;
    if months > MAX_MONTHS {
        return Err(Error::Invalid(format!("回看月数不能超过 {MAX_MONTHS}")));
    }
    let empty = |from: &str| Report { from: from.into(), to: through.into(), months: vec![], net_worth: vec![], categories: vec![] };

    let Some(earliest) = earliest_month(c)? else { return Ok(empty(through)) };
    let wanted = if months == 0 { earliest.clone() } else { shift_back(through, months - 1)? };
    let from = if wanted < earliest { earliest } else { wanted }; // YYYY-MM 的字典序即时间序
    if from.as_str() > through {
        return Ok(empty(&from));
    }
    let list = months_between(&from, through)?;

    let mut sums: HashMap<(String, String), i64> = HashMap::new();
    let rows: Vec<(String, String, i64)> = c.exec(
        "SELECT SUBSTR(date,1,7), kind, CAST(SUM(amount) AS SIGNED) FROM txn
         WHERE SUBSTR(date,1,7) BETWEEN ? AND ? GROUP BY SUBSTR(date,1,7), kind",
        (&from, through),
    )?;
    for (m, kind, total) in rows {
        sums.insert((m, kind), total);
    }
    let months_rows = list
        .iter()
        .map(|m| MonthRow {
            income: sums.get(&(m.clone(), "income".into())).copied().unwrap_or(0),
            expense: sums.get(&(m.clone(), "expense".into())).copied().unwrap_or(0),
            month: m.clone(),
        })
        .collect();

    let mut net_worth = Vec::with_capacity(list.len());
    for m in &list {
        let as_of = if m == through { core::NO_LIMIT.to_string() } else { month_end(m)? };
        let n = core::net_worth_as_of(c, &as_of)?;
        net_worth.push(NetPoint { month: m.clone(), assets: n.assets, liabilities: n.liabilities, net: n.net });
    }

    Ok(Report {
        categories: core::category_summary_range(c, &from, through)?,
        from,
        to: through.into(),
        months: months_rows,
        net_worth,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdb::TestDb;

    #[test]
    fn month_arithmetic_handles_year_and_leap_boundaries() {
        assert_eq!(shift_back("2026-01", 1).unwrap(), "2025-12");
        assert_eq!(shift_back("2026-10", 0).unwrap(), "2026-10");
        assert_eq!(shift_back("2026-03", 14).unwrap(), "2025-01");
        assert_eq!(month_end("2024-02").unwrap(), "2024-02-29");
        assert_eq!(month_end("2026-02").unwrap(), "2026-02-28");
        assert_eq!(month_end("2026-12").unwrap(), "2026-12-31");
        assert_eq!(months_between("2025-11", "2026-02").unwrap(), ["2025-11", "2025-12", "2026-01", "2026-02"]);
        assert!(first_day("2026-13").is_err() && first_day("2026-1").is_err() && first_day("abc").is_err());
    }

    #[test]
    fn empty_database_gives_empty_report() {
        let t = TestDb::new();
        let r = report(&mut t.conn(), "2026-10", 6).unwrap();
        assert!(r.months.is_empty() && r.net_worth.is_empty() && r.categories.is_empty());
        assert_eq!((r.from.as_str(), r.to.as_str()), ("2026-10", "2026-10"));
    }

    #[test]
    fn rejects_bad_arguments() {
        let t = TestDb::new();
        let mut c = t.conn();
        assert!(report(&mut c, "2026-13", 6).is_err());
        assert!(report(&mut c, "2026-10", 241).is_err());
        assert!(report(&mut c, "2026-10", 240).is_ok());
    }

    #[test]
    fn net_worth_history_follows_valuations_and_entries_month_by_month() {
        let t = TestDb::new();
        let mut c = t.conn();
        let a = core::add_account(&mut c, "招行", "deposit", "CNY", 1000_00, "2026-08-01").unwrap();
        core::add_txn(&mut c, Some(a), None, "expense", 100_00, "2026-09-10", "").unwrap();
        core::set_valuation(&mut c, a, "2026-10-05", 2000_00).unwrap();
        core::add_account(&mut c, "住宅", "property", "CNY", 1_000_000_00, "2026-10-01").unwrap();

        let r = report(&mut c, "2026-10", 0).unwrap();
        let nets: Vec<(&str, i64)> = r.net_worth.iter().map(|p| (p.month.as_str(), p.net)).collect();
        assert_eq!(
            nets,
            [("2026-08", 1000_00), ("2026-09", 900_00), ("2026-10", 2000_00 + 1_000_000_00)],
            "八月只有招行;九月扣掉那笔支出;住宅在建档之前不计入"
        );
        // 最后一个点与总览页的"当前净资产"完全一致
        assert_eq!(r.net_worth.last().unwrap().net, core::net_worth(&mut c).unwrap().net);
    }

    #[test]
    fn liabilities_are_subtracted_in_history() {
        let t = TestDb::new();
        let mut c = t.conn();
        core::add_account(&mut c, "招行", "deposit", "CNY", 1000_00, "2026-09-01").unwrap();
        let card = core::add_account(&mut c, "信用卡", "liability", "CNY", 200_00, "2026-09-01").unwrap();
        core::add_txn(&mut c, Some(card), None, "expense", 50_00, "2026-09-15", "").unwrap();
        let r = report(&mut c, "2026-10", 0).unwrap();
        let sep = &r.net_worth[0];
        assert_eq!((sep.assets, sep.liabilities, sep.net), (1000_00, 250_00, 750_00));
    }

    #[test]
    fn range_is_clamped_to_first_data_and_gaps_are_zero_filled() {
        let t = TestDb::new();
        let mut c = t.conn();
        core::add_account(&mut c, "招行", "deposit", "CNY", 500_00, "2026-08-20").unwrap();
        core::add_txn(&mut c, None, None, "expense", 30_00, "2026-08-21", "").unwrap();
        core::add_txn(&mut c, None, None, "income", 900_00, "2026-10-02", "").unwrap();

        let r = report(&mut c, "2026-10", 12).unwrap();
        assert_eq!(r.from, "2026-08", "只有三个月的数据,就只画三个月");
        assert_eq!(
            r.months,
            [
                MonthRow { month: "2026-08".into(), income: 0, expense: 30_00 },
                MonthRow { month: "2026-09".into(), income: 0, expense: 0 },
                MonthRow { month: "2026-10".into(), income: 900_00, expense: 0 },
            ]
        );
        // 回看更短时从该月起
        let r2 = report(&mut c, "2026-10", 2).unwrap();
        assert_eq!((r2.from.as_str(), r2.months.len(), r2.net_worth.len()), ("2026-09", 2, 2));
        // 回看 1 个月只有当月
        assert_eq!(report(&mut c, "2026-10", 1).unwrap().months.len(), 1);
    }

    #[test]
    fn categories_cover_the_whole_range_only() {
        let t = TestDb::new();
        let mut c = t.conn();
        let food = core::find_category(&mut c, "餐饮", "expense").unwrap();
        let traffic = core::find_category(&mut c, "交通", "expense").unwrap();
        core::add_txn(&mut c, None, food, "expense", 100_00, "2026-07-30", "区间外").unwrap();
        core::add_txn(&mut c, None, food, "expense", 30_00, "2026-09-02", "").unwrap();
        core::add_txn(&mut c, None, food, "expense", 20_00, "2026-10-02", "").unwrap();
        core::add_txn(&mut c, None, traffic, "expense", 80_00, "2026-10-03", "").unwrap();
        core::add_txn(&mut c, None, None, "expense", 5_00, "2026-10-04", "").unwrap();
        core::add_txn(&mut c, None, food, "income", 999_00, "2026-10-05", "收入不计入").unwrap();

        let r = report(&mut c, "2026-10", 2).unwrap(); // 2026-09 ~ 2026-10
        let cats: Vec<(&str, i64, i64)> = r.categories.iter().map(|x| (x.category.as_str(), x.total, x.count)).collect();
        assert_eq!(cats, [("交通", 80_00, 1), ("餐饮", 50_00, 2), ("未分类", 5_00, 1)]);
    }

    #[test]
    fn data_dated_after_the_through_month_does_not_extend_the_range() {
        let t = TestDb::new();
        let mut c = t.conn();
        core::add_account(&mut c, "招行", "deposit", "CNY", 100_00, "2026-10-01").unwrap();
        core::add_txn(&mut c, None, None, "expense", 7_00, "2027-03-01", "未来").unwrap();
        let r = report(&mut c, "2026-10", 6).unwrap();
        assert_eq!(r.months.len(), 1);
        assert_eq!(r.months[0].expense, 0);
    }

    #[test]
    fn data_that_only_starts_after_the_through_month_is_empty_not_an_error() {
        let t = TestDb::new();
        let mut c = t.conn();
        core::add_account(&mut c, "招行", "deposit", "CNY", 100_00, "2027-01-01").unwrap();
        let r = report(&mut c, "2026-10", 6).unwrap();
        assert!(r.months.is_empty() && r.net_worth.is_empty());
    }

    /// 对真实数据库的冒烟检查:cargo test live_report -- --ignored --nocapture
    /// 只读,不写任何数据;用来确认真实库上报表能正常算出来、且与总览页一致
    #[test]
    #[ignore]
    fn live_report_against_real_database() {
        let cfg = crate::db::load_config(&crate::testdb::app_data_dir()).expect("需要先运行 scripts/setup-mysql.sh");
        let pool = crate::db::connect(&cfg).expect("连接真实数据库失败");
        let mut c = pool.get_conn().unwrap();
        let through = chrono::Local::now().format("%Y-%m").to_string();
        let r = report(&mut c, &through, 0).unwrap();
        println!("区间 {} ~ {},{} 个月,{} 个分类", r.from, r.to, r.months.len(), r.categories.len());
        if let Some(last) = r.net_worth.last() {
            assert_eq!(last.net, core::net_worth(&mut c).unwrap().net, "报表最后一个点必须等于当前净资产");
            println!("当前净资产(分):{}", last.net);
        }
    }
}
