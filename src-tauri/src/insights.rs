//! 总览页的洞察卡:纯规则计算,不调用模型。每条附带一个可以直接拿去问助手的问题。
//!
//! - 预算:已超支,或消耗进度比月份进度快 10 个百分点以上
//! - 订阅:3 天内(含今天)要自动扣费的支出订阅
//! - 大额支出:近 7 天里,明显高于该分类近 90 天中位数(≥3 倍且 ≥200 元)的单笔;
//!   分类样本不足 5 笔时,只提醒 ≥2000 元的
use crate::agent::yuan;
use crate::core::{self, Error, Result};
use crate::report;
use chrono::{Datelike, Duration, NaiveDate};
use mysql::prelude::Queryable;
use serde::Serialize;
use std::collections::HashMap;

const PACE_MARGIN_PCT: f64 = 10.0;
const PACE_MIN_USED_PCT: f64 = 30.0;
const SUB_DAYS: i64 = 3;
const RECENT_DAYS: i64 = 7;
const BASELINE_DAYS: i64 = 90;
const MIN_SAMPLES: usize = 5;
const FACTOR: i64 = 3;
const MIN_LARGE: i64 = 200_00;
const ABS_LARGE: i64 = 2000_00;
const MAX_LARGE: usize = 3;

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Insight {
    /// budget_over | budget_pace | sub_due | large_expense
    pub code: String,
    /// bad | warn | info
    pub level: String,
    pub title: String,
    pub detail: String,
    /// 点「问助手」时发出的问题
    pub question: String,
}

fn y(c: i64) -> String {
    format!("¥{:.2}", yuan(c))
}

fn median(v: &mut [i64]) -> i64 {
    v.sort_unstable();
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2 }
}

pub fn insights(c: &mut impl Queryable, today: &str) -> Result<Vec<Insight>> {
    let t = NaiveDate::parse_from_str(today, "%Y-%m-%d").map_err(|_| Error::Invalid(format!("日期格式不对:{today}")))?;
    let month = &today[..7];
    let mut out = vec![];

    // 预算
    let st = core::budget_status(c, month)?;
    if let Some(b) = st.budget.filter(|b| *b > 0) {
        let days = NaiveDate::parse_from_str(&report::month_end(month)?, "%Y-%m-%d").map(|d| d.day()).unwrap_or(30);
        let elapsed = t.day() as f64 / days as f64 * 100.0;
        let used = st.spent as f64 / b as f64 * 100.0;
        if st.spent > b {
            out.push(Insight {
                code: "budget_over".into(), level: "bad".into(),
                title: format!("本月预算已超支 {}", y(st.spent - b)),
                detail: format!("预算 {},已支出 {}。", y(b), y(st.spent)),
                question: "这个月预算超支了,主要超在哪些分类?和上个月比多花在哪?".into(),
            });
        } else if used >= PACE_MIN_USED_PCT && used - elapsed >= PACE_MARGIN_PCT {
            let projected = (st.spent as f64 / (t.day() as f64) * days as f64).round() as i64;
            out.push(Insight {
                code: "budget_pace".into(), level: "warn".into(),
                title: "预算消耗超前".into(),
                detail: format!("已用 {:.0}%({} / {}),本月才过 {:.0}%。照这个速度,月底会花到约 {}。", used, y(st.spent), y(b), elapsed, y(projected)),
                question: "这个月预算为什么花得这么快?哪些分类花得最多?".into(),
            });
        }
    }

    // 订阅临近扣费
    let horizon = t + Duration::days(SUB_DAYS - 1);
    for r in crate::recurring::list_recurring(c, today)?.items {
        let Some(due) = r.next_due.as_deref().and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()) else { continue };
        if r.kind != "expense" || r.status != "active" || due < t || due > horizon {
            continue;
        }
        let when = match (due - t).num_days() { 0 => "今天".to_string(), 1 => "明天".to_string(), n => format!("{n} 天后") };
        out.push(Insight {
            code: "sub_due".into(), level: "info".into(),
            title: format!("「{}」{when}扣费 {}", r.name, y(r.amount)),
            detail: format!("{}{} 自动记账{}。", due.format("%m-%d"), if r.frequency == "yearly" { " 年度订阅" } else { "" },
                r.account.map(|a| format!(",从「{a}」扣")).unwrap_or_default()),
            question: "我接下来一周有哪些固定扣费?每月固定支出一共多少?".into(),
        });
    }

    // 大额支出
    let base_from = (t - Duration::days(BASELINE_DAYS)).format("%Y-%m-%d").to_string();
    let recent_from = t - Duration::days(RECENT_DAYS - 1);
    let rows: Vec<(String, i64, String, String)> = c.exec(
        "SELECT t.date, t.amount, COALESCE(c.name,'未分类'), t.note FROM txn t LEFT JOIN category c ON c.id = t.category_id
         WHERE t.kind = 'expense' AND t.date BETWEEN ? AND ?",
        (&base_from, today),
    )?;
    let mut by_cat: HashMap<&str, Vec<i64>> = HashMap::new();
    for (_, amount, cat, _) in &rows {
        by_cat.entry(cat.as_str()).or_default().push(*amount);
    }
    let medians: HashMap<&str, Option<i64>> = by_cat.into_iter().map(|(k, mut v)| (k, (v.len() >= MIN_SAMPLES).then(|| median(&mut v)))).collect();
    let mut large: Vec<(i64, Insight)> = rows
        .iter()
        .filter(|(d, ..)| NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok_and(|d| d >= recent_from))
        .filter_map(|(date, amount, cat, note)| {
            let med = medians.get(cat.as_str()).copied().flatten();
            let hit = match med {
                Some(m) => *amount >= (m * FACTOR).max(MIN_LARGE),
                None => *amount >= ABS_LARGE,
            };
            hit.then(|| {
                let label = if note.is_empty() { cat.clone() } else { format!("{cat} · {note}") };
                let detail = match med {
                    Some(m) => format!("{} 一笔 {},是「{cat}」近 90 天单笔中位数 {} 的 {:.1} 倍。", &date[5..], y(*amount), y(m), *amount as f64 / m as f64),
                    None => format!("{} 一笔 {}。", &date[5..], y(*amount)),
                };
                (*amount, Insight {
                    code: "large_expense".into(), level: "warn".into(),
                    title: format!("大额支出:{label}"),
                    detail,
                    question: format!("{date} 那笔「{label}」{} 算偏高吗?和我平时在这个分类的花费比怎么样?", y(*amount)),
                })
            })
        })
        .collect();
    large.sort_by(|a, b| b.0.cmp(&a.0));
    out.extend(large.into_iter().take(MAX_LARGE).map(|x| x.1));

    let rank = |l: &str| match l { "bad" => 0, "warn" => 1, _ => 2 };
    out.sort_by_key(|i| rank(&i.level));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdb::TestDb;

    #[test]
    fn budget_pace_subscription_and_large_expense() {
        let t = TestDb::new();
        let mut c = t.conn();
        let food = core::find_category(&mut c, "餐饮", "expense").unwrap();
        let shop = core::find_category(&mut c, "购物", "expense").unwrap();
        core::set_budget(&mut c, "2026-10", None, 3000_00).unwrap();
        for d in ["2026-09-01", "2026-09-08", "2026-09-15", "2026-09-22", "2026-10-01"] {
            core::add_txn(&mut c, None, food, "expense", 40_00, d, "午饭").unwrap();
        }
        core::add_txn(&mut c, None, food, "expense", 600_00, "2026-10-09", "请客").unwrap();
        core::add_txn(&mut c, None, shop, "expense", 1500_00, "2026-10-08", "鞋").unwrap(); // 样本少且不到 2000:不提醒
        let r = insights(&mut c, "2026-10-10").unwrap();
        let codes: Vec<&str> = r.iter().map(|i| i.code.as_str()).collect();
        assert!(codes.contains(&"large_expense"), "{r:?}");
        let big = r.iter().find(|i| i.code == "large_expense").unwrap();
        assert!(big.title.contains("请客") && big.detail.contains("15.0 倍"), "{big:?}");
        assert_eq!(r.iter().filter(|i| i.code == "large_expense").count(), 1);

        crate::recurring::add_recurring(&mut c, &crate::recurring::RecurringInput {
            name: "视频会员".into(), kind: "expense".into(), amount: 25_00, frequency: "monthly".into(),
            start_date: "2026-10-11".into(), end_date: None, category_id: None, account_id: None,
        }, "2026-10-10").unwrap();
        let r = insights(&mut c, "2026-10-10").unwrap();
        let sub = r.iter().find(|i| i.code == "sub_due").unwrap();
        assert!(sub.title.contains("明天扣费 ¥25.00"), "{sub:?}");

        core::add_txn(&mut c, None, food, "expense", 1500_00, "2026-10-10", "").unwrap();
        let r = insights(&mut c, "2026-10-10").unwrap();
        assert_eq!(r[0].code, "budget_over", "超支排最前");
    }

    #[test]
    fn pace_warning() {
        let t = TestDb::new();
        let mut c = t.conn();
        core::set_budget(&mut c, "2026-10", None, 1000_00).unwrap();
        core::add_txn(&mut c, None, None, "expense", 500_00, "2026-10-05", "").unwrap();
        let r = insights(&mut c, "2026-10-10").unwrap();
        let p = r.iter().find(|i| i.code == "budget_pace").unwrap();
        assert!(p.detail.contains("已用 50%") && p.detail.contains("才过 32%") && p.detail.contains("¥1550.00"), "{p:?}");
    }
}
