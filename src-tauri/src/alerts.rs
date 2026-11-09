//! 涨跌过大提醒:一个开关加一个阈值,对自选和场内持仓统一生效。
//!
//! 应用运行期间由后台线程每小时调一次 [`run_once`]:开关关闭或没有提醒对象时不联网;
//! 否则一次取回所有报价,当日涨跌幅绝对值达到阈值的发系统通知。每只证券每个行情日最多提醒一次,
//! 已提醒的记在 `alert_sent`(只是运行状态,不进备份)。
use crate::core::{Error, Result};
use crate::market::Quote;
use chrono::{Duration, NaiveDate, NaiveDateTime};
use mysql::prelude::Queryable;
use serde::Serialize;
use std::collections::HashSet;

pub const DEFAULT_PCT: f64 = 5.0;
const MAX_PCT: f64 = 50.0;
/// `alert_sent` 只留最近几天,够判断"今天提醒过没有"就行
const KEEP_DAYS: i64 = 7;

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct AlertSettings {
    pub enabled: bool,
    /// 阈值,百分数:5 表示当日涨跌 ±5%
    pub pct: f64,
    /// 后台发通知时发现系统通知没有授权
    pub notify_denied: bool,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Hit {
    pub secid: String,
    pub name: String,
    pub day: String,
    pub price: f64,
    pub change_pct: f64,
}

fn setting(c: &mut impl Queryable, key: &str) -> Result<Option<String>> {
    Ok(c.exec_first("SELECT value FROM setting WHERE name=?", (key,))?)
}

fn put(c: &mut impl Queryable, key: &str, value: &str) -> Result<()> {
    c.exec_drop("INSERT INTO setting (name, value) VALUES (?,?) AS new ON DUPLICATE KEY UPDATE value=new.value", (key, value))?;
    Ok(())
}

pub fn get_settings(c: &mut impl Queryable) -> Result<AlertSettings> {
    Ok(AlertSettings {
        enabled: setting(c, "alert_enabled")?.as_deref() == Some("1"),
        pct: setting(c, "alert_pct")?.and_then(|v| v.parse().ok()).filter(|p: &f64| valid_pct(*p)).unwrap_or(DEFAULT_PCT),
        notify_denied: setting(c, "notify_denied")?.as_deref() == Some("1"),
    })
}

fn valid_pct(p: f64) -> bool {
    p.is_finite() && p > 0.0 && p <= MAX_PCT
}

pub fn save_settings(c: &mut impl Queryable, enabled: bool, pct: f64) -> Result<AlertSettings> {
    if !valid_pct(pct) {
        return Err(Error::Invalid(format!("阈值要大于 0、不超过 {MAX_PCT}(单位 %)")));
    }
    put(c, "alert_enabled", if enabled { "1" } else { "0" })?;
    put(c, "alert_pct", &((pct * 100.0).round() / 100.0).to_string())?;
    get_settings(c)
}

pub fn set_notify_denied(c: &mut impl Queryable, denied: bool) -> Result<()> {
    put(c, "notify_denied", if denied { "1" } else { "0" })
}

/// 提醒对象:自选 ∪ 按份额估值的场内持仓(场外基金只有每日净值,不参与)
pub fn targets(c: &mut impl Queryable) -> Result<Vec<String>> {
    Ok(c.query(
        "SELECT secid FROM watchlist
         UNION SELECT secid FROM holding WHERE secid IS NOT NULL AND shares IS NOT NULL AND secid NOT LIKE 'OF.%'
         ORDER BY secid",
    )?)
}

/// 行情日:报价时间(北京时间)的日期。美股一个交易日跨北京时间午夜(21:30 至次日 05:00 左右),
/// 往前推 12 小时后再取日期,同一个交易日就落在同一天
pub fn quote_day(secid: &str, time: &str) -> Option<String> {
    let t = NaiveDateTime::parse_from_str(time, "%Y-%m-%d %H:%M").ok()?;
    let us = matches!(secid.split_once('.').map(|(m, _)| m), Some("105" | "106" | "107"));
    let t = if us { t - Duration::hours(12) } else { t };
    Some(t.format("%Y-%m-%d").to_string())
}

/// 选出要提醒的证券。sent 是已经提醒过的 (secid, 行情日)
pub fn evaluate(quotes: &[Quote], pct: f64, sent: &HashSet<(String, String)>) -> Vec<Hit> {
    let mut seen = HashSet::new();
    quotes
        .iter()
        .filter_map(|q| {
            // 停牌、未开盘时没有现价,不判断
            let (price, chg) = (q.price?, q.change_pct?);
            let day = quote_day(&q.secid, q.time.as_deref()?)?;
            let fresh = chg.abs() >= pct && !sent.contains(&(q.secid.clone(), day.clone())) && seen.insert(q.secid.clone());
            fresh.then(|| Hit { secid: q.secid.clone(), name: q.name.clone(), day, price, change_pct: chg })
        })
        .collect()
}

/// 跑一轮检查,返回这轮要通知的证券(已记入 `alert_sent`)。fetch 是取报价的函数,测试时可以替换
pub fn run_once(c: &mut impl Queryable, today: &str, fetch: impl FnOnce(&[String]) -> Result<Vec<Quote>>) -> Result<Vec<Hit>> {
    let s = get_settings(c)?;
    if !s.enabled {
        return Ok(vec![]);
    }
    let ids = targets(c)?;
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let quotes = fetch(&ids)?;
    if let Some(cut) = NaiveDate::parse_from_str(today, "%Y-%m-%d").ok().map(|d| d - Duration::days(KEEP_DAYS)) {
        c.exec_drop("DELETE FROM alert_sent WHERE quote_day < ?", (cut.format("%Y-%m-%d").to_string(),))?;
    }
    let sent: HashSet<(String, String)> = c.query::<(String, String), _>("SELECT secid, quote_day FROM alert_sent")?.into_iter().collect();
    let hits = evaluate(&quotes, s.pct, &sent);
    for h in &hits {
        c.exec_drop("INSERT IGNORE INTO alert_sent (secid, quote_day) VALUES (?, ?)", (&h.secid, &h.day))?;
    }
    Ok(hits)
}

/// 通知的标题和正文
pub fn notice_text(h: &Hit) -> (String, String) {
    let dir = if h.change_pct > 0.0 { "大涨" } else { "大跌" };
    let price = format!("{:.3}", h.price);
    let price = price.trim_end_matches('0').trim_end_matches('.');
    (format!("{} 今日{dir}", h.name), format!("当日涨跌 {:+.2}%,现价 {price}", h.change_pct))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdb::TestDb;

    fn q(secid: &str, price: Option<f64>, pct: Option<f64>, time: &str) -> Quote {
        Quote {
            secid: secid.into(), code: secid.split('.').nth(1).unwrap_or(secid).into(), name: format!("名称{secid}"),
            price, change: None, change_pct: pct, open: None, high: None, low: None, prev_close: None,
            time: Some(time.into()).filter(|t: &String| !t.is_empty()),
        }
    }

    #[test]
    fn settings_default_validate_and_save() {
        let t = TestDb::new();
        let mut c = t.conn();
        assert_eq!(get_settings(&mut c).unwrap(), AlertSettings { enabled: false, pct: 5.0, notify_denied: false });
        for bad in [0.0, -1.0, 50.01, f64::NAN] {
            assert!(save_settings(&mut c, true, bad).unwrap_err().to_string().contains("阈值"), "{bad}");
        }
        assert!(!get_settings(&mut c).unwrap().enabled, "非法阈值不改原设置");
        let s = save_settings(&mut c, true, 3.0).unwrap();
        assert_eq!((s.enabled, s.pct), (true, 3.0));
        set_notify_denied(&mut c, true).unwrap();
        assert!(get_settings(&mut c).unwrap().notify_denied);
    }

    #[test]
    fn targets_merge_watchlist_and_exchange_holdings() {
        let t = TestDb::new();
        let mut c = t.conn();
        c.query_drop("INSERT INTO watchlist (secid, code, name) VALUES ('105.QQQM','QQQM','QQQM'), ('1.000001','000001','上证指数')").unwrap();
        let acc = crate::core::add_account(&mut c, "券商", "investment", "CNY", 1_00, "2026-11-01").unwrap();
        for (name, secid, shares) in [("QQQM", Some("105.QQQM"), Some(10_0000)), ("VOO", Some("107.VOO"), Some(10_0000)), ("基金", Some("OF.110020"), Some(1_0000)), ("手动", None, None)] {
            c.exec_drop(
                "INSERT INTO holding (account_id, name, asset_class, amount, updated_on, shares, secid) VALUES (?, ?, 'other', 100, '2026-11-01', ?, ?)",
                (acc, name, shares, secid),
            )
            .unwrap();
        }
        assert_eq!(targets(&mut c).unwrap(), ["1.000001", "105.QQQM", "107.VOO"], "去重,排除场外基金和手动持仓");
    }

    #[test]
    fn evaluates_moves_once_per_quote_day() {
        let quotes = [
            q("1.600519", Some(1500.0), Some(5.2), "2026-11-02 15:00"),  // 涨超阈值
            q("0.000001", Some(10.0), Some(-6.0), "2026-11-02 15:00"),   // 跌超阈值
            q("1.510300", Some(4.0), Some(4.99), "2026-11-02 15:00"),    // 未达阈值
            q("1.600000", None, Some(-9.0), "2026-11-02 15:00"),         // 停牌,没有现价
            q("1.600001", Some(1.0), Some(9.0), ""),                     // 没有行情时间
            q("1.600519", Some(1500.0), Some(5.2), "2026-11-02 15:00"),  // 重复的报价
        ];
        let hits = evaluate(&quotes, 5.0, &HashSet::new());
        assert_eq!(hits.iter().map(|h| h.secid.as_str()).collect::<Vec<_>>(), ["1.600519", "0.000001"]);
        assert_eq!(hits[0].day, "2026-11-02");

        let sent = HashSet::from([("1.600519".to_string(), "2026-11-02".to_string())]);
        assert_eq!(evaluate(&quotes[..1], 5.0, &sent), vec![], "同一行情日不再提醒");
        let next = [q("1.600519", Some(1600.0), Some(6.0), "2026-11-03 10:00")];
        assert_eq!(evaluate(&next, 5.0, &sent).len(), 1, "下一个行情日再提醒");
    }

    #[test]
    fn us_trading_day_spans_beijing_midnight() {
        assert_eq!(quote_day("105.QQQM", "2026-11-02 22:30").as_deref(), Some("2026-11-02"));
        assert_eq!(quote_day("105.QQQM", "2026-11-03 04:59").as_deref(), Some("2026-11-02"), "北京时间过了午夜还是同一个交易日");
        assert_eq!(quote_day("1.600519", "2026-11-03 04:59").as_deref(), Some("2026-11-03"));
        let sent = HashSet::from([("105.QQQM".to_string(), "2026-11-02".to_string())]);
        assert!(evaluate(&[q("105.QQQM", Some(200.0), Some(-6.0), "2026-11-03 01:00")], 5.0, &sent).is_empty());
    }

    #[test]
    fn run_once_skips_network_when_off_and_records_hits() {
        let t = TestDb::new();
        let mut c = t.conn();
        c.query_drop("INSERT INTO watchlist (secid, code, name) VALUES ('105.QQQM','QQQM','QQQM')").unwrap();
        let never = |_: &[String]| -> Result<Vec<Quote>> { panic!("关闭时不应联网") };
        assert!(run_once(&mut c, "2026-11-02", never).unwrap().is_empty());

        save_settings(&mut c, true, 5.0).unwrap();
        let fail = |_: &[String]| -> Result<Vec<Quote>> { Err(Error::Net("行情暂时不可用".into())) };
        assert!(run_once(&mut c, "2026-11-02", fail).is_err(), "取不到报价时这轮跳过,由调用方记日志");

        let fetch = |ids: &[String]| -> Result<Vec<Quote>> {
            assert_eq!(ids, ["105.QQQM"]);
            Ok(vec![q("105.QQQM", Some(200.0), Some(-5.6), "2026-11-02 23:00")])
        };
        let hits = run_once(&mut c, "2026-11-02", fetch).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(run_once(&mut c, "2026-11-03", fetch).unwrap().is_empty(), "同一行情日第二轮不再提醒");

        c.query_drop("INSERT INTO alert_sent VALUES ('1.600519', '2026-10-01')").unwrap();
        run_once(&mut c, "2026-11-03", fetch).unwrap();
        let n: Option<i64> = c.query_first("SELECT COUNT(*) FROM alert_sent WHERE quote_day < '2026-10-27'").unwrap();
        assert_eq!(n, Some(0), "清掉一周前的记录");

        c.query_drop("DELETE FROM watchlist").unwrap();
        assert!(run_once(&mut c, "2026-11-03", never).unwrap().is_empty(), "没有提醒对象时不联网");
    }

    #[test]
    fn notice_text_reads_naturally() {
        let h = Hit { secid: "105.QQQM".into(), name: "QQQM".into(), day: "2026-11-02".into(), price: 201.5, change_pct: -5.613 };
        assert_eq!(notice_text(&h), ("QQQM 今日大跌".to_string(), "当日涨跌 -5.61%,现价 201.5".to_string()));
    }
}
