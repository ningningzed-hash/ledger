//! 目标:把一个或几个账户的余额与目标金额比较,进度随账户余额自动更新。
//!
//! - 存钱:关联存款、现金或理财账户(不含房产和负债),余额合计达到目标金额即完成。进度 = 当前余额 ÷ 目标金额。
//! - 还债:关联负债账户,欠款降到目标金额(通常是 0)即完成。进度按"建立目标时的欠款"算起,
//!   也就是已经还掉了多少。关联的账户变了,起点会重新记录。
//!
//! 有截止日期的目标还会算出"每月需要多少"和"按近几个月的实际速度预计何时达成",
//! 并据此给出状态(进度正常 / 稍落后 / 已逾期 …)。
use crate::core::{self, Account, Error, Result};
use crate::db::Conn;
use crate::report;
use chrono::{Datelike, NaiveDate};
use mysql::prelude::Queryable;
use mysql::TxOpts;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const MAX_NAME_CHARS: usize = 60;
/// 金额上限(分),只是防止输入荒谬的数字
const MAX_AMOUNT: i64 = 100_000_000_000_000;
/// 估算速度时最多回看几个月
const PACE_WINDOW_MONTHS: i64 = 3;
/// 按当前速度超过这么多个月才能达成,就不再给出预计月份
const MAX_PROJECTION_MONTHS: i64 = 1200;

#[derive(Deserialize, Debug, Clone)]
pub struct GoalInput {
    pub name: String,
    /// save | payoff
    pub kind: String,
    /// 分。存钱:要攒到的金额;还债:欠款降到多少算还清(可以是 0)
    pub target: i64,
    /// YYYY-MM-DD,可不填
    pub deadline: Option<String>,
    pub account_ids: Vec<i64>,
}

#[derive(Serialize, Debug)]
pub struct GoalAccount {
    pub id: i64,
    pub name: String,
}

#[derive(Serialize, Debug)]
pub struct GoalView {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub target: i64,
    pub deadline: Option<String>,
    pub created_on: String,
    pub accounts: Vec<GoalAccount>,
    /// 关联账户当前余额合计(分)
    pub current: i64,
    /// 还差多少(分),已达成则为 0
    pub remaining: i64,
    /// 0..=1
    pub progress: f64,
    pub achieved: bool,
    /// 距截止还有几个整月(截止日在本月为 0);没有截止日或已逾期时为空
    pub months_left: Option<i64>,
    /// 为了按时达成,今后每个月需要增加(存钱)或减少(还债)多少(分)
    pub required_per_month: Option<i64>,
    /// 近几个月的实际月均变化(分),数据不足时为空;为负表示在往反方向走
    pub pace_per_month: Option<i64>,
    /// 按当前速度预计达成的月份 YYYY-MM
    pub projected_month: Option<String>,
    /// achieved | overdue | on_track | behind | unknown | open
    pub status: String,
}

struct GoalRow {
    id: i64,
    name: String,
    kind: String,
    target: i64,
    start_amount: i64,
    deadline: Option<String>,
    created_on: String,
}

type GoalTuple = (i64, String, String, i64, i64, Option<String>, String);

fn parse_date(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| Error::Invalid(format!("日期格式应为 YYYY-MM-DD,收到:{s}")))
}

struct Checked {
    name: String,
    kind: String,
    target: i64,
    deadline: Option<String>,
    ids: Vec<i64>,
    accounts: Vec<Account>,
}

/// 校验输入。不写任何数据。
fn validate(c: &mut impl Queryable, input: &GoalInput) -> Result<Checked> {
    let name = input.name.trim().to_string();
    if name.is_empty() {
        return Err(Error::Invalid("请填写目标名称".into()));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(Error::Invalid(format!("目标名称不能超过 {MAX_NAME_CHARS} 个字")));
    }
    let kind = input.kind.as_str();
    if kind != "save" && kind != "payoff" {
        return Err(Error::Invalid(format!("目标类型只能是 save 或 payoff,收到:{kind}")));
    }
    let min_target = if kind == "save" { 1 } else { 0 };
    if input.target < min_target || input.target > MAX_AMOUNT {
        return Err(Error::Invalid(if kind == "save" { "目标金额必须大于 0" } else { "还清线不能为负数" }.into()));
    }
    let deadline = match input.deadline.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(d) => {
            parse_date(d)?;
            Some(d.to_string())
        }
    };

    let mut seen = HashSet::new();
    let ids: Vec<i64> = input.account_ids.iter().copied().filter(|i| seen.insert(*i)).collect();
    if ids.is_empty() {
        return Err(Error::Invalid("请至少关联一个账户".into()));
    }
    let all = core::list_accounts(c)?;
    let mut accounts = Vec::new();
    for id in &ids {
        let a = all.iter().find(|a| a.id == *id).ok_or_else(|| Error::Invalid("关联的账户不存在".into()))?;
        match (kind, a.kind.as_str()) {
            ("save", "liability") => return Err(Error::Invalid(format!("“{}”是负债账户,存钱目标只能关联存款、现金或理财账户", a.name))),
            // 房产的估值是手动更新的,也不能随时取用,不适合作为"攒钱"的对象
            ("save", "property") => return Err(Error::Invalid(format!("“{}”是房产账户,存钱目标请关联存款、现金或理财账户", a.name))),
            ("payoff", k) if k != "liability" => return Err(Error::Invalid(format!("“{}”不是负债账户,还债目标只能关联负债账户", a.name))),
            _ => {}
        }
        accounts.push(Account { id: a.id, name: a.name.clone(), kind: a.kind.clone(), currency: a.currency.clone(), balance: a.balance });
    }
    Ok(Checked { name, kind: kind.into(), target: input.target, deadline, ids, accounts })
}

fn check_future(deadline: &Option<String>, today: &str) -> Result<()> {
    match deadline {
        Some(d) if d.as_str() < today => Err(Error::Invalid("截止日期不能早于今天".into())),
        _ => Ok(()),
    }
}

fn link_accounts(tx: &mut impl Queryable, goal_id: i64, ids: &[i64]) -> Result<()> {
    tx.exec_batch("INSERT INTO goal_account (goal_id, account_id) VALUES (?,?)", ids.iter().map(|a| (goal_id, *a)))?;
    Ok(())
}

pub fn add_goal(c: &mut Conn, input: &GoalInput, today: &str) -> Result<i64> {
    parse_date(today)?;
    let v = validate(c, input)?;
    check_future(&v.deadline, today)?;
    let start = if v.kind == "payoff" { v.accounts.iter().map(|a| a.balance).sum() } else { 0 };

    let mut tx = c.start_transaction(TxOpts::default())?;
    tx.exec_drop(
        "INSERT INTO goal (name, kind, target, start_amount, deadline, created_on) VALUES (?,?,?,?,?,?)",
        (&v.name, &v.kind, v.target, start, &v.deadline, today),
    )?;
    let id = core::last_id(&mut tx)?;
    link_accounts(&mut tx, id, &v.ids)?;
    tx.commit()?;
    Ok(id)
}

pub fn update_goal(c: &mut Conn, id: i64, input: &GoalInput, today: &str) -> Result<()> {
    parse_date(today)?;
    let old: Option<GoalTuple> = c.exec_first(
        "SELECT id, name, kind, target, start_amount, deadline, created_on FROM goal WHERE id=?",
        (id,),
    )?;
    let old = old.ok_or_else(|| Error::Invalid("目标不存在".into()))?;
    let old = GoalRow { id: old.0, name: old.1, kind: old.2, target: old.3, start_amount: old.4, deadline: old.5, created_on: old.6 };
    let old_ids: HashSet<i64> = c
        .exec_map("SELECT account_id FROM goal_account WHERE goal_id=?", (id,), |a: i64| a)?
        .into_iter()
        .collect();

    let v = validate(c, input)?;
    // 只有改了截止日期才要求它不早于今天,这样已逾期的目标仍可以改名字、金额或关联账户
    if v.deadline != old.deadline {
        check_future(&v.deadline, today)?;
    }
    let same_basis = old.kind == v.kind && old_ids == v.ids.iter().copied().collect::<HashSet<_>>();
    let start = match (v.kind.as_str(), same_basis) {
        ("payoff", true) => old.start_amount,
        ("payoff", false) => v.accounts.iter().map(|a| a.balance).sum(), // 换了账户,重新以现在的欠款为起点
        _ => 0,
    };

    let mut tx = c.start_transaction(TxOpts::default())?;
    tx.exec_drop(
        "UPDATE goal SET name=?, kind=?, target=?, start_amount=?, deadline=? WHERE id=?",
        (&v.name, &v.kind, v.target, start, &v.deadline, id),
    )?;
    tx.exec_drop("DELETE FROM goal_account WHERE goal_id=?", (id,))?;
    link_accounts(&mut tx, id, &v.ids)?;
    tx.commit()?;
    Ok(())
}

pub fn delete_goal(c: &mut Conn, id: i64) -> Result<()> {
    c.exec_drop("DELETE FROM goal WHERE id=?", (id,))?; // 关联行随之级联删除,账户本身不受影响
    if c.affected_rows() == 0 {
        return Err(Error::Invalid("目标不存在".into()));
    }
    Ok(())
}

/// 近几个月关联账户的月均变化(分)。存钱:余额增加为正;还债:欠款减少为正。
/// 回看的起点是 n 个月之前那个月的月底,终点是现在,所以实际跨度是 n 个月加上本月已过去的部分。
/// 账户最早的估值就在本月时没有历史可比,返回空。
fn pace(c: &mut impl Queryable, ids: &[i64], kind: &str, current: i64, today: &NaiveDate) -> Result<Option<i64>> {
    if ids.is_empty() {
        return Ok(None);
    }
    let through = today.format("%Y-%m").to_string();
    let marks = vec!["?"; ids.len()].join(",");
    let earliest: Option<Option<String>> =
        c.exec_first(format!("SELECT MIN(SUBSTR(date,1,7)) FROM valuation WHERE account_id IN ({marks})"), ids.to_vec())?;
    let Some(Some(earliest)) = earliest else { return Ok(None) };
    let n = report::month_diff(&earliest, &through)?.clamp(0, PACE_WINDOW_MONTHS);
    if n == 0 {
        return Ok(None);
    }
    let base_month = report::shift_back(&through, n as u32)?;
    let base: i64 = core::accounts_as_of(c, &report::month_end(&base_month)?)?
        .iter()
        .filter(|a| ids.contains(&a.id))
        .map(|a| a.balance)
        .sum();
    let days_in_month: f64 = report::month_end(&through)?[8..].parse().unwrap_or(30.0);
    let elapsed = n as f64 + today.day() as f64 / days_in_month;
    let delta = if kind == "payoff" { base - current } else { current - base };
    Ok(Some((delta as f64 / elapsed).round() as i64))
}

pub fn list_goals(c: &mut impl Queryable, today: &str) -> Result<Vec<GoalView>> {
    let today_d = parse_date(today)?;
    let through = today_d.format("%Y-%m").to_string();

    let rows: Vec<GoalRow> = c.query_map(
        "SELECT id, name, kind, target, start_amount, deadline, created_on FROM goal ORDER BY id",
        |t: GoalTuple| GoalRow { id: t.0, name: t.1, kind: t.2, target: t.3, start_amount: t.4, deadline: t.5, created_on: t.6 },
    )?;
    let links: Vec<(i64, i64)> = c.query("SELECT goal_id, account_id FROM goal_account")?;
    let accounts = core::list_accounts(c)?;
    let by_id: HashMap<i64, &Account> = accounts.iter().map(|a| (a.id, a)).collect();

    let mut out = Vec::with_capacity(rows.len());
    for g in rows {
        let mut linked: Vec<&Account> = links.iter().filter(|(gid, _)| *gid == g.id).filter_map(|(_, aid)| by_id.get(aid).copied()).collect();
        linked.sort_by_key(|a| a.id);
        let ids: Vec<i64> = linked.iter().map(|a| a.id).collect();
        let current: i64 = linked.iter().map(|a| a.balance).sum();
        let payoff = g.kind == "payoff";

        let achieved = if payoff { current <= g.target } else { current >= g.target };
        let remaining = if payoff { (current - g.target).max(0) } else { (g.target - current).max(0) };
        let progress = if achieved {
            1.0
        } else if payoff {
            let span = g.start_amount - g.target;
            if span <= 0 { 1.0 } else { ((g.start_amount - current) as f64 / span as f64).clamp(0.0, 1.0) }
        } else {
            (current as f64 / g.target as f64).clamp(0.0, 1.0)
        };

        let overdue = !achieved && g.deadline.as_deref().is_some_and(|d| d < today);
        let months_left = match (&g.deadline, achieved, overdue) {
            (Some(d), false, false) => Some(report::month_diff(&through, &d[..7])?.max(0)),
            _ => None,
        };
        let required = months_left.map(|m| (remaining + m.max(1) - 1) / m.max(1));

        let pace = pace(c, &ids, &g.kind, current, &today_d)?;
        let projected = match pace {
            Some(p) if !achieved && p > 0 => {
                let n = (remaining + p - 1) / p;
                if n <= MAX_PROJECTION_MONTHS { Some(report::shift_forward(&through, n as u32)?) } else { None }
            }
            _ => None,
        };

        let status = if achieved {
            "achieved"
        } else if overdue {
            "overdue"
        } else if g.deadline.is_none() {
            "open"
        } else {
            match (pace, required) {
                (Some(p), Some(r)) if p >= r => "on_track",
                (Some(_), Some(_)) => "behind",
                _ => "unknown",
            }
        };

        out.push(GoalView {
            id: g.id,
            name: g.name,
            kind: g.kind,
            target: g.target,
            deadline: g.deadline,
            created_on: g.created_on,
            accounts: linked.iter().map(|a| GoalAccount { id: a.id, name: a.name.clone() }).collect(),
            current,
            remaining,
            progress,
            achieved,
            months_left,
            required_per_month: required,
            pace_per_month: pace,
            projected_month: projected,
            status: status.into(),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdb::TestDb;

    const TODAY: &str = "2026-10-28";

    fn input(name: &str, kind: &str, target: i64, deadline: Option<&str>, ids: &[i64]) -> GoalInput {
        GoalInput { name: name.into(), kind: kind.into(), target, deadline: deadline.map(Into::into), account_ids: ids.to_vec() }
    }

    fn goal_rows(c: &mut Conn) -> (i64, i64) {
        (
            c.query_first("SELECT COUNT(*) FROM goal").unwrap().unwrap(),
            c.query_first("SELECT COUNT(*) FROM goal_account").unwrap().unwrap(),
        )
    }

    fn only(c: &mut Conn) -> GoalView {
        let mut v = list_goals(c, TODAY).unwrap();
        assert_eq!(v.len(), 1);
        v.remove(0)
    }

    #[test]
    fn save_goal_adds_up_several_accounts_and_follows_them() {
        let t = TestDb::new();
        let mut c = t.conn();
        let a = core::add_account(&mut c, "招行", "deposit", "CNY", 80_000_00, "2026-10-01").unwrap();
        let b = core::add_account(&mut c, "货基", "investment", "CNY", 20_000_00, "2026-10-01").unwrap();
        core::add_account(&mut c, "无关账户", "cash", "CNY", 999_00, "2026-10-01").unwrap();
        add_goal(&mut c, &input("攒 20 万", "save", 200_000_00, None, &[a, b]), TODAY).unwrap();

        let g = only(&mut c);
        assert_eq!((g.current, g.remaining, g.achieved, g.status.as_str()), (100_000_00, 100_000_00, false, "open"));
        assert!((g.progress - 0.5).abs() < 1e-9);
        assert_eq!(g.accounts.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["招行", "货基"]);

        // 工资进账 → 进度自动前进,不用手动改目标
        core::add_txn(&mut c, Some(a), None, "income", 20_000_00, TODAY, "").unwrap();
        let g = only(&mut c);
        assert_eq!((g.current, g.remaining), (120_000_00, 80_000_00));

        // 超额也算达成,进度封顶 100%
        core::set_valuation(&mut c, a, TODAY, 500_000_00).unwrap();
        let g = only(&mut c);
        assert!(g.achieved && g.progress == 1.0 && g.remaining == 0 && g.status == "achieved");
    }

    #[test]
    fn payoff_goal_measures_how_much_has_been_paid_off() {
        let t = TestDb::new();
        let mut c = t.conn();
        let card = core::add_account(&mut c, "信用卡", "liability", "CNY", 50_000_00, "2026-10-01").unwrap();
        add_goal(&mut c, &input("还清信用卡", "payoff", 0, None, &[card]), TODAY).unwrap();
        assert_eq!(only(&mut c).progress, 0.0);

        core::set_valuation(&mut c, card, TODAY, 30_000_00).unwrap(); // 还了 2 万
        let g = only(&mut c);
        assert_eq!((g.current, g.remaining, g.achieved), (30_000_00, 30_000_00, false));
        assert!((g.progress - 0.4).abs() < 1e-9);

        core::set_valuation(&mut c, card, TODAY, 70_000_00).unwrap(); // 反而欠得更多:进度不会为负
        assert_eq!(only(&mut c).progress, 0.0);

        core::set_valuation(&mut c, card, TODAY, 0).unwrap();
        let g = only(&mut c);
        assert!(g.achieved && g.progress == 1.0 && g.status == "achieved");
    }

    #[test]
    fn payoff_goal_with_a_remaining_line_above_zero() {
        let t = TestDb::new();
        let mut c = t.conn();
        let loan = core::add_account(&mut c, "房贷", "liability", "CNY", 100_0000_00, "2026-10-01").unwrap();
        add_goal(&mut c, &input("房贷降到 60 万", "payoff", 60_0000_00, None, &[loan]), TODAY).unwrap();
        core::set_valuation(&mut c, loan, TODAY, 80_0000_00).unwrap(); // 100→80,目标 100→60:完成一半
        let g = only(&mut c);
        assert!((g.progress - 0.5).abs() < 1e-9, "{}", g.progress);
        assert_eq!(g.remaining, 20_0000_00);
    }

    #[test]
    fn invalid_input_is_rejected_and_nothing_is_written() {
        let t = TestDb::new();
        let mut c = t.conn();
        let dep = core::add_account(&mut c, "招行", "deposit", "CNY", 1_000_00, "2026-10-01").unwrap();
        let card = core::add_account(&mut c, "信用卡", "liability", "CNY", 500_00, "2026-10-01").unwrap();
        let house = core::add_account(&mut c, "住宅", "property", "CNY", 100_0000_00, "2026-10-01").unwrap();
        let long = "长".repeat(61);
        let bad = [
            input("  ", "save", 100, None, &[dep]),                          // 没有名称
            input(&long, "save", 100, None, &[dep]),                         // 名称太长
            input("a", "invest", 100, None, &[dep]),                         // 类型不对
            input("a", "save", 0, None, &[dep]),                             // 存钱目标必须大于 0
            input("a", "save", -5, None, &[dep]),
            input("a", "payoff", -1, None, &[card]),                         // 还清线不能为负
            input("a", "save", i64::MAX, None, &[dep]),                      // 荒谬的大数
            input("a", "save", 100, None, &[]),                              // 没有关联账户
            input("a", "save", 100, None, &[999]),                           // 账户不存在
            input("a", "save", 100, None, &[card]),                          // 存钱不能关联负债
            input("a", "save", 100, None, &[house]),                         // 存钱不能关联房产
            input("a", "payoff", 0, None, &[dep]),                           // 还债只能关联负债
            input("a", "payoff", 0, None, &[house]),
            input("a", "save", 100, Some("2026-13-40"), &[dep]),             // 日期不合法
            input("a", "save", 100, Some("2026-10-27"), &[dep]),             // 截止日早于今天
        ];
        for b in &bad {
            assert!(add_goal(&mut c, b, TODAY).is_err(), "应拒绝 {b:?}");
        }
        assert_eq!(goal_rows(&mut c), (0, 0));
        assert!(add_goal(&mut c, &input("a", "save", 100, Some(TODAY), &[dep]), TODAY).is_ok(), "截止日就是今天可以");
        // 重复的账户只算一次
        add_goal(&mut c, &input("b", "save", 100, Some(""), &[dep, dep]), TODAY).unwrap();
        assert_eq!(goal_rows(&mut c), (2, 2));
        assert!(list_goals(&mut c, TODAY).unwrap()[1].deadline.is_none(), "空字符串当作没有截止日");
    }

    #[test]
    fn deadline_status_needs_history_to_judge_pace() {
        let t = TestDb::new();
        let mut c = t.conn();
        // 7 月中有 1 万,10 月 20 日涨到 1.9 万:近三个月月均约增加 2,306 元
        let a = core::add_account(&mut c, "招行", "deposit", "CNY", 10_000_00, "2026-07-15").unwrap();
        core::set_valuation(&mut c, a, "2026-10-20", 19_000_00).unwrap();
        let expect_pace = (9_000_00f64 / (3.0 + 28.0 / 31.0)).round() as i64;

        add_goal(&mut c, &input("紧", "save", 30_000_00, Some("2027-02-28"), &[a]), TODAY).unwrap(); // 还剩 4 个月,每月需 2,750
        add_goal(&mut c, &input("松", "save", 30_000_00, Some("2027-06-30"), &[a]), TODAY).unwrap(); // 还剩 8 个月,每月需 1,375
        add_goal(&mut c, &input("没期限", "save", 30_000_00, None, &[a]), TODAY).unwrap();
        let gs = list_goals(&mut c, TODAY).unwrap();

        let (tight, loose, open) = (&gs[0], &gs[1], &gs[2]);
        assert_eq!(tight.pace_per_month, Some(expect_pace));
        assert_eq!((tight.months_left, tight.required_per_month), (Some(4), Some(2_750_00)));
        assert_eq!(tight.status, "behind");
        assert_eq!((loose.months_left, loose.required_per_month), (Some(8), Some(1_375_00)));
        assert_eq!(loose.status, "on_track");
        assert_eq!(open.status, "open");
        // 还差 11,000 元,每月约 2,306 元 → 5 个月后(2027-03)
        assert_eq!(tight.projected_month.as_deref(), Some("2027-03"));
        assert_eq!(open.required_per_month, None);
    }

    #[test]
    fn pace_is_unknown_without_history_and_stalled_goals_have_no_projection() {
        let t = TestDb::new();
        let mut c = t.conn();
        // 账户本月才建立:没有更早的数据可比
        let fresh = core::add_account(&mut c, "新卡", "deposit", "CNY", 1_000_00, "2026-10-10").unwrap();
        add_goal(&mut c, &input("a", "save", 9_000_00, Some("2027-10-01"), &[fresh]), TODAY).unwrap();
        let g = only(&mut c);
        assert_eq!((g.pace_per_month, g.projected_month.clone(), g.status.as_str()), (None, None, "unknown"));

        // 余额在往下走:速度为负,不给预计月份,且判为落后
        let old = core::add_account(&mut c, "老卡", "deposit", "CNY", 10_000_00, "2026-07-15").unwrap();
        core::set_valuation(&mut c, old, "2026-10-20", 4_000_00).unwrap();
        add_goal(&mut c, &input("b", "save", 20_000_00, Some("2027-10-01"), &[old]), TODAY).unwrap();
        let g = list_goals(&mut c, TODAY).unwrap().remove(1);
        assert!(g.pace_per_month.unwrap() < 0);
        assert_eq!((g.projected_month, g.status.as_str()), (None, "behind"));
    }

    #[test]
    fn payoff_pace_counts_debt_reduction() {
        let t = TestDb::new();
        let mut c = t.conn();
        let loan = core::add_account(&mut c, "消费贷", "liability", "CNY", 30_000_00, "2026-07-15").unwrap();
        core::set_valuation(&mut c, loan, "2026-10-20", 21_000_00).unwrap(); // 三个月还了 9,000
        add_goal(&mut c, &input("还清", "payoff", 0, Some("2027-06-30"), &[loan]), TODAY).unwrap();
        let g = only(&mut c);
        let expect = (9_000_00f64 / (3.0 + 28.0 / 31.0)).round() as i64;
        assert_eq!(g.pace_per_month, Some(expect));
        assert_eq!(g.remaining, 21_000_00);
        assert_eq!((g.months_left, g.required_per_month), (Some(8), Some(2_625_00)));
        assert_eq!(g.status, "behind", "每月需还 2,625 元,实际约 2,306 元");
    }

    #[test]
    fn overdue_goals_are_flagged_but_can_still_be_edited() {
        let t = TestDb::new();
        let mut c = t.conn();
        let a = core::add_account(&mut c, "招行", "deposit", "CNY", 1_000_00, "2026-10-01").unwrap();
        let id = add_goal(&mut c, &input("旅行基金", "save", 5_000_00, Some("2026-12-31"), &[a]), TODAY).unwrap();

        let later = "2027-01-15";
        let g = list_goals(&mut c, later).unwrap().remove(0);
        assert_eq!((g.status.as_str(), g.months_left, g.required_per_month), ("overdue", None, None));

        // 没动截止日期:允许继续改其他字段
        update_goal(&mut c, id, &input("旅行基金(改)", "save", 6_000_00, Some("2026-12-31"), &[a]), later).unwrap();
        // 改成一个已经过去的新日期:不行;改成未来的:可以
        assert!(update_goal(&mut c, id, &input("x", "save", 6_000_00, Some("2026-11-01"), &[a]), later).is_err());
        update_goal(&mut c, id, &input("旅行基金(改)", "save", 6_000_00, Some("2027-03-31"), &[a]), later).unwrap();
        // 账户 10 月起就有记录、三个月来余额没动:速度是 0(不是"未知"),赶不上期限
        let g = list_goals(&mut c, later).unwrap().remove(0);
        assert_eq!((g.pace_per_month, g.status.as_str()), (Some(0), "behind"));

        // 达成了的目标,过了截止日也不算逾期
        core::set_valuation(&mut c, a, "2027-01-10", 9_000_00).unwrap();
        update_goal(&mut c, id, &input("旅行基金(改)", "save", 6_000_00, Some("2027-03-31"), &[a]), later).unwrap();
        assert_eq!(list_goals(&mut c, "2027-05-01").unwrap()[0].status, "achieved");
    }

    #[test]
    fn update_keeps_payoff_start_unless_the_accounts_change() {
        let t = TestDb::new();
        let mut c = t.conn();
        let card = core::add_account(&mut c, "信用卡", "liability", "CNY", 50_000_00, "2026-10-01").unwrap();
        let other = core::add_account(&mut c, "花呗", "liability", "CNY", 8_000_00, "2026-10-01").unwrap();
        let id = add_goal(&mut c, &input("还清", "payoff", 0, None, &[card]), TODAY).unwrap();
        core::set_valuation(&mut c, card, TODAY, 30_000_00).unwrap();

        // 只改名称:起点仍是 5 万,进度还是 40%
        update_goal(&mut c, id, &input("还清信用卡", "payoff", 0, None, &[card]), TODAY).unwrap();
        assert!((only(&mut c).progress - 0.4).abs() < 1e-9);

        // 加上花呗:起点重新以现在的欠款(3 万 + 8 千)为准,进度从 0 重新开始
        update_goal(&mut c, id, &input("还清所有", "payoff", 0, None, &[card, other]), TODAY).unwrap();
        let g = only(&mut c);
        assert_eq!((g.current, g.progress), (38_000_00, 0.0));
        core::set_valuation(&mut c, other, TODAY, 0).unwrap();
        assert!((only(&mut c).progress - 8.0 / 38.0).abs() < 1e-9);

        assert!(update_goal(&mut c, 999, &input("x", "save", 1, None, &[card]), TODAY).is_err(), "不存在的目标");
        // 校验失败不应改动原目标
        assert!(update_goal(&mut c, id, &input("", "payoff", 0, None, &[card]), TODAY).is_err());
        assert_eq!(only(&mut c).name, "还清所有");
    }

    #[test]
    fn deleting_a_goal_leaves_accounts_alone() {
        let t = TestDb::new();
        let mut c = t.conn();
        let a = core::add_account(&mut c, "招行", "deposit", "CNY", 1_000_00, "2026-10-01").unwrap();
        let g1 = add_goal(&mut c, &input("一", "save", 5_000_00, None, &[a]), TODAY).unwrap();
        add_goal(&mut c, &input("二", "save", 9_000_00, None, &[a]), TODAY).unwrap();
        delete_goal(&mut c, g1).unwrap();
        assert_eq!(goal_rows(&mut c), (1, 1));
        assert_eq!(core::list_accounts(&mut c).unwrap().len(), 1);
        assert!(delete_goal(&mut c, g1).is_err(), "再删一次应报告不存在");
    }

    #[test]
    fn archived_accounts_drop_out_of_the_total() {
        let t = TestDb::new();
        let mut c = t.conn();
        let a = core::add_account(&mut c, "旧卡", "deposit", "CNY", 3_000_00, "2026-10-01").unwrap();
        let b = core::add_account(&mut c, "新卡", "deposit", "CNY", 1_000_00, "2026-10-01").unwrap();
        add_goal(&mut c, &input("攒", "save", 10_000_00, None, &[a, b]), TODAY).unwrap();
        c.exec_drop("UPDATE account SET archived=1 WHERE id=?", (a,)).unwrap();
        let g = only(&mut c);
        assert_eq!((g.current, g.accounts.len()), (1_000_00, 1));
    }

    /// 对真实数据库的只读冒烟检查:cargo test live_goals_real -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_goals_real_database() {
        let cfg = crate::db::load_config(&crate::testdb::app_data_dir()).expect("需要先运行 scripts/setup-mysql.sh");
        let mut c = crate::db::connect(&cfg).expect("连接真实数据库失败").get_conn().unwrap();
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let goals = list_goals(&mut c, &today).unwrap();
        println!("真实库里有 {} 个目标", goals.len());
        for g in &goals {
            println!("  {} [{}] {}/{} {:.0}% {}", g.name, g.kind, g.current, g.target, g.progress * 100.0, g.status);
        }
    }
}
