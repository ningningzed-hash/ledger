//! 行情:自选列表、实时报价与历史走势。
//!
//! 数据取自东方财富的公开行情接口,不需要账号。实时报价接口经常被限流断连,
//! 所以报价按 东方财富 → 东方财富备用站 → 新浪 的顺序轮流尝试(见 [`fetch_quotes`])。证券用 secid("市场.代码")标识:
//! 1 = 上交所、0 = 深交所/北交所、116 = 港股、105/106/107 = 美股(纳斯达克/纽交所/美交所)。
//! 用户只输入代码(600519、00700、AAPL,也可写 sh000001、hk700、us.BRK.B),
//! 由 [`candidates`] 推断可能的 secid,再用报价接口确认哪一个真实存在。
//!
//! 价格只在内存里流转,数据库只存自选列表本身。登记了份额的持仓用这里的价格自动更新金额
//! (见 [`refresh_holdings`]);场外基金的净值取自天天基金的公开接口。
use crate::core::{Error, Result};
use crate::db::Conn;
use crate::portfolio::{self, HoldingInput, SHARE_SCALE};
use chrono::{Months, NaiveDate, TimeZone};
use mysql::prelude::Queryable;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

/// 东方财富报价接口:主站经常直接断开连接,备用站(延时行情站,实测个股同样是实时价)成功率更高
const QUOTE_URLS: [&str; 2] = ["https://push2.eastmoney.com/api/qt/ulist.np/get", "https://push2delay.eastmoney.com/api/qt/ulist.np/get"];
/// 新浪报价接口:东方财富都失败时的兜底,需要带 Referer,返回 GBK 编码的 JS 文本
const SINA_URL: &str = "https://hq.sinajs.cn/list=";
const SINA_REFERER: &str = "https://finance.sina.com.cn";
/// 腾讯报价接口:只用来查美股在哪个交易所(新浪不给交易所)
const TENCENT_URL: &str = "https://qt.gtimg.cn/q=";
const KLINE_URL: &str = "https://push2his.eastmoney.com/api/qt/stock/kline/get";
/// f2 现价、f3 涨跌幅、f4 涨跌额、f12 代码、f13 市场、f14 名称、f15 最高、f16 最低、f17 今开、f18 昨收、f124 行情时间
const QUOTE_FIELDS: &str = "f2,f3,f4,f12,f13,f14,f15,f16,f17,f18,f124";
const MAX_WATCH: usize = 50;

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct WatchItem {
    pub id: i64,
    pub secid: String,
    pub code: String,
    pub name: String,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Quote {
    pub secid: String,
    pub code: String,
    pub name: String,
    /// 停牌、未开盘等没有数据时为空
    pub price: Option<f64>,
    pub change: Option<f64>,
    /// 百分数:2.81 表示 +2.81%
    pub change_pct: Option<f64>,
    pub open: Option<f64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub prev_close: Option<f64>,
    /// 行情时间(本地时区)YYYY-MM-DD HH:MM
    pub time: Option<String>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Point {
    /// 日线为 YYYY-MM-DD,分钟线为 YYYY-MM-DD HH:MM
    pub t: String,
    pub price: f64,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Series {
    pub secid: String,
    pub range: String,
    /// 分钟线(当日、5日)为 true
    pub intraday: bool,
    pub points: Vec<Point>,
    /// 区间开始前最后一个价格(当日走势即昨收),用来算区间涨跌;取不到时为空
    pub base: Option<f64>,
}

// ---------- 代码解析 ----------

fn pad_hk(digits: &str) -> String {
    format!("{digits:0>5}")
}

fn us(sym: &str) -> Vec<String> {
    // 接口里 BRK.B 这类带点的美股写作 BRK_B
    let s = sym.replace(['.', '-'], "_");
    ["105", "106", "107"].iter().map(|m| format!("{m}.{s}")).collect()
}

/// 6 位 A 股代码:先按号段猜交易所,另一个交易所作为备选(000001 是平安银行,sh000001 才是上证指数)
fn a_share(d: &str) -> Vec<String> {
    let sh_first = matches!(d.as_bytes()[0], b'5' | b'6' | b'9');
    let (a, b) = if sh_first { ("1", "0") } else { ("0", "1") };
    vec![format!("{a}.{d}"), format!("{b}.{d}")]
}

fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn us_symbol(s: &str) -> bool {
    !s.is_empty() && s.len() <= 10 && s.as_bytes()[0].is_ascii_alphabetic() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

/// 把用户输入的代码换成可能的 secid,按可能性从高到低排列
pub fn candidates(input: &str) -> Result<Vec<String>> {
    let s: String = input.trim().to_ascii_uppercase().chars().filter(|c| !c.is_whitespace()).collect();
    let bad = || Error::Invalid(format!("认不出“{}”。请输入股票代码,例如 600519、00700、AAPL;指数可写 sh000001", input.trim()));
    // 已经是 secid:105.AAPL
    if let Some((m, code)) = s.split_once('.') {
        if all_digits(m) && !code.is_empty() {
            return Ok(vec![format!("{m}.{code}")]);
        }
    }
    let strip = |p: &str| s.strip_prefix(p).map(|r| r.trim_start_matches(['.', ':']));
    if let Some(d) = strip("SH").filter(|d| d.len() == 6 && all_digits(d)) {
        return Ok(vec![format!("1.{d}")]);
    }
    if let Some(d) = strip("SZ").or_else(|| strip("BJ")).filter(|d| d.len() == 6 && all_digits(d)) {
        return Ok(vec![format!("0.{d}")]);
    }
    if let Some(d) = strip("HK").filter(|d| (1..=5).contains(&d.len()) && all_digits(d)) {
        return Ok(vec![format!("116.{}", pad_hk(d))]);
    }
    if let Some(sym) = strip("US").filter(|d| us_symbol(d)) {
        return Ok(us(sym));
    }
    if s.len() == 6 && all_digits(&s) {
        return Ok(a_share(&s));
    }
    if (1..=5).contains(&s.len()) && all_digits(&s) {
        return Ok(vec![format!("116.{}", pad_hk(&s))]);
    }
    if us_symbol(&s) {
        return Ok(us(&s));
    }
    Err(bad())
}

// ---------- 接口数据解析 ----------

fn num(v: &Value) -> Option<f64> {
    v.as_f64().filter(|n| n.is_finite())
}

fn local_time(ts: i64) -> Option<String> {
    chrono::Local.timestamp_opt(ts, 0).single().map(|t| t.format("%Y-%m-%d %H:%M").to_string())
}

/// 报价接口返回 {"data":{"diff":[...]}};查不到任何一只时 data 为 null
pub fn parse_quotes(v: &Value) -> Vec<Quote> {
    let Some(list) = v["data"]["diff"].as_array() else { return vec![] };
    list.iter()
        .filter_map(|q| {
            let code = q["f12"].as_str()?.to_string();
            let market = q["f13"].as_i64()?;
            let name = q["f14"].as_str().filter(|n| !n.is_empty() && *n != "-")?.to_string();
            Some(Quote {
                secid: format!("{market}.{code}"),
                code,
                name,
                price: num(&q["f2"]),
                change: num(&q["f4"]),
                change_pct: num(&q["f3"]),
                open: num(&q["f17"]),
                high: num(&q["f15"]),
                low: num(&q["f16"]),
                prev_close: num(&q["f18"]),
                time: q["f124"].as_i64().filter(|t| *t > 0).and_then(local_time),
            })
        })
        .collect()
}

/// K 线接口返回 {"data":{"klines":["2026-10-30,1392.05", ...]}}(字段:时间,收盘价)
pub fn parse_klines(v: &Value) -> Vec<Point> {
    let Some(lines) = v["data"]["klines"].as_array() else { return vec![] };
    lines
        .iter()
        .filter_map(|l| {
            let (t, p) = l.as_str()?.split_once(',')?;
            let price: f64 = p.split(',').next()?.parse().ok()?;
            price.is_finite().then(|| Point { t: t.to_string(), price })
        })
        .collect()
}

// ---------- 时间范围 ----------

/// (K 线周期, 取多少根)。分钟线多取几天,再按日期裁出当日 / 最近 5 个交易日
fn range_params(range: &str) -> Result<(u32, u32)> {
    Ok(match range {
        "1d" | "5d" => (5, 480), // 5 分钟线;美股一天 78 根,480 根够 6 个交易日
        "1m" => (101, 40),
        "3m" => (101, 100),
        "1y" => (101, 300),
        "5y" => (102, 300), // 周线
        _ => return Err(Error::Invalid(format!("不支持的时间范围:{range}"))),
    })
}

fn day_of(t: &str) -> &str {
    &t[..t.len().min(10)]
}

/// 从取回的数据里裁出所选范围,同时返回范围开始前的最后一个价格
pub fn trim_range(points: Vec<Point>, range: &str) -> (Vec<Point>, Option<f64>) {
    let Some(last) = points.last() else { return (points, None) };
    let start = match range {
        "1d" | "5d" => {
            let want = if range == "1d" { 1 } else { 5 };
            let mut days: Vec<&str> = points.iter().map(|p| day_of(&p.t)).collect();
            days.dedup();
            days[days.len().saturating_sub(want)].to_string()
        }
        _ => {
            let months = match range { "1m" => 1, "3m" => 3, "1y" => 12, _ => 60 };
            let end = NaiveDate::parse_from_str(day_of(&last.t), "%Y-%m-%d").ok();
            match end.and_then(|d| d.checked_sub_months(Months::new(months))) {
                // 不含起点那天:一个月前那天的收盘作为基准
                Some(d) => (d + chrono::Days::new(1)).format("%Y-%m-%d").to_string(),
                None => return (points, None),
            }
        }
    };
    let cut = points.iter().position(|p| day_of(&p.t) >= start.as_str()).unwrap_or(points.len());
    let base = cut.checked_sub(1).map(|i| points[i].price);
    (points[cut..].to_vec(), base)
}

// ---------- 网络 ----------

fn http() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).build().into()
}

/// 连接被中途断开(行情服务偶尔不发 TLS close_notify 就关连接)、超时、5xx 都是暂时性的,值得重试;
/// 4xx 说明请求本身有问题,重试也没用
fn transient(e: &ureq::Error) -> bool {
    !matches!(e, ureq::Error::StatusCode(400..=499))
}

fn get_once(url: &str, query: &[(&str, &str)]) -> std::result::Result<Value, ureq::Error> {
    let mut req = http().get(url);
    for (k, v) in query {
        req = req.query(*k, *v);
    }
    req.call()?.body_mut().read_json::<Value>()
}

/// 请求一次行情接口;暂时性错误最多重试 2 次,间隔逐次加长
fn get_json(url: &str, query: &[(&str, &str)]) -> Result<Value> {
    const BACKOFF_MS: [u64; 2] = [300, 1000];
    let mut attempt = 0;
    loop {
        match get_once(url, query) {
            Ok(v) => return Ok(v),
            Err(e) if attempt < BACKOFF_MS.len() && transient(&e) => {
                std::thread::sleep(Duration::from_millis(BACKOFF_MS[attempt]));
                attempt += 1;
            }
            Err(ureq::Error::StatusCode(code)) => return Err(Error::Net(format!("行情服务返回错误 {code},请稍后再试"))),
            Err(e) => return Err(Error::Net(format!("暂时无法获取行情(已重试 {attempt} 次),请稍后再试:{e}"))),
        }
    }
}

/// 一次取多只的实时报价,结果按传入顺序排列;查不到的直接略过。
///
/// 依次尝试东方财富主站、备用站和新浪,任何一个成功就用它的结果;一轮都失败就稍等再来一轮。
/// 新浪认不出的代码(见 [`sina_codes`])不会交给新浪,全部认不出时跳过新浪。
/// 识别用户输入的美股时分不清交易所,先问腾讯它在哪个交易所。
pub fn fetch_quotes(secids: &[String]) -> Result<Vec<Quote>> {
    const ROUNDS: usize = 2;
    if secids.is_empty() {
        return Ok(vec![]);
    }
    let joined = secids.join(",");
    let mut sina: Option<Vec<(String, String)>> = None;
    let mut errors: Vec<String> = Vec::new();
    for round in 0..ROUNDS {
        if round > 0 {
            std::thread::sleep(Duration::from_millis(800));
        }
        errors.clear();
        for url in QUOTE_URLS {
            match get_once(url, &[("fltt", "2"), ("fields", QUOTE_FIELDS), ("secids", &joined)]) {
                Ok(v) => return Ok(in_order(parse_quotes(&v), secids)),
                // 4xx 是请求本身有问题,换站点也没用
                Err(ureq::Error::StatusCode(code @ 400..=499)) => {
                    return Err(Error::Net(format!("行情服务返回错误 {code},请稍后再试")));
                }
                Err(e) => errors.push(format!("东方财富:{e}")),
            }
        }
        // 东方财富都失败了才去确定新浪代码,大多数时候用不着多请求腾讯
        let sina = sina.get_or_insert_with(|| sina_codes(secids, us_exchange));
        if !sina.is_empty() {
            match fetch_sina(sina) {
                Ok(q) => return Ok(in_order(q, secids)),
                Err(e) => errors.push(format!("新浪:{e}")),
            }
        }
    }
    Err(Error::Net(format!("暂时无法获取行情(东方财富、新浪各试了 {ROUNDS} 次),请稍后再试:{}", errors.join(";"))))
}

fn in_order(mut got: Vec<Quote>, secids: &[String]) -> Vec<Quote> {
    got.sort_by_key(|q| secids.iter().position(|s| s.eq_ignore_ascii_case(&q.secid)).unwrap_or(usize::MAX));
    got
}

// ---------- 新浪兜底 ----------

/// secid → 新浪代码,返回 (新浪代码, secid)。认不出的略过:
/// - 美股在东方财富分 105/106/107 三个市场,新浪只有一个 gb_ 代码。同一代码传了多个市场
///   (识别用户输入时)就用 us_market(代码) 查它实际在哪个市场,只保留那一个;查不到就不用新浪,
///   免得把纽交所的股票错记成纳斯达克
/// - 0 市场同时包含深交所和北交所,按号段区分
pub fn sina_codes(secids: &[String], us_market: impl Fn(&str) -> Option<&'static str>) -> Vec<(String, String)> {
    let us_markets = |code: &str| secids.iter().filter(|s| matches!(s.split_once('.'), Some(("105" | "106" | "107", c)) if c.eq_ignore_ascii_case(code))).count();
    let mut asked: HashMap<String, Option<&'static str>> = HashMap::new();
    let mut us_ok = |m: &str, code: &str| us_markets(code) == 1 || *asked.entry(code.to_ascii_uppercase()).or_insert_with(|| us_market(code)) == Some(m);
    secids
        .iter()
        .filter_map(|secid| {
            let (m, code) = secid.split_once('.')?;
            let sina = match m {
                "1" => format!("sh{code}"),
                "0" if code.starts_with('4') || code.starts_with('8') || code.starts_with("92") => format!("bj{code}"),
                "0" => format!("sz{code}"),
                "116" => format!("rt_hk{code}"),
                "105" | "106" | "107" if us_ok(m, code) => format!("gb_{}", code.to_ascii_lowercase().replace('_', "$")),
                "133" if code.len() == 6 => format!("fx_s{}", code.to_ascii_lowercase()),
                _ => return None,
            };
            Some((sina, secid.clone()))
        })
        .collect()
}

/// 问腾讯美股代码在哪个交易所,返回东方财富的市场号;查不到或网络失败为空
fn us_exchange(code: &str) -> Option<&'static str> {
    let bytes = http().get(format!("{TENCENT_URL}us{}", code.replace('_', "."))).call().ok()?.body_mut().read_to_vec().ok()?;
    tencent_us_market(&encoding_rs::GBK.decode(&bytes).0)
}

/// 腾讯返回 `v_usAAPL="200~苹果~AAPL.OQ~340.10~...";`,第 3 段的后缀是交易所:OQ 纳斯达克、N 纽交所、AM 美交所
pub fn tencent_us_market(text: &str) -> Option<&'static str> {
    let (_, suffix) = text.split('~').nth(2)?.rsplit_once('.')?;
    match suffix {
        "OQ" => Some("105"),
        "N" => Some("106"),
        "AM" => Some("107"),
        _ => None,
    }
}

fn fetch_sina(codes: &[(String, String)]) -> std::result::Result<Vec<Quote>, ureq::Error> {
    let list: Vec<&str> = codes.iter().map(|(c, _)| c.as_str()).collect();
    let bytes = http().get(format!("{SINA_URL}{}", list.join(","))).header("Referer", SINA_REFERER).call()?.body_mut().read_to_vec()?;
    Ok(parse_sina(&encoding_rs::GBK.decode(&bytes).0, codes))
}

/// 解析新浪返回的 `var hq_str_sh600519="贵州茅台,1410.000,...";`,每行一只;查不到的代码引号里是空的。
/// codes 是 [`sina_codes`] 的结果,用来把新浪代码换回 secid
pub fn parse_sina(text: &str, codes: &[(String, String)]) -> Vec<Quote> {
    text.lines()
        .filter_map(|line| {
            let (code, rest) = line.trim().strip_prefix("var hq_str_")?.split_once("=\"")?;
            let secid = &codes.iter().find(|(c, _)| c == code)?.1;
            let f: Vec<&str> = rest.trim_end_matches(['"', ';']).split(',').collect();
            sina_quote(code, secid, &f)
        })
        .collect()
}

fn sina_quote(sina: &str, secid: &str, f: &[&str]) -> Option<Quote> {
    // 价格类字段:0 表示停牌、未开盘等没有数据;涨跌类字段可以是 0 或负数
    let pos = |i: usize| f.get(i)?.trim().parse::<f64>().ok().filter(|n| n.is_finite() && *n > 0.0);
    let val = |i: usize| f.get(i)?.trim().parse::<f64>().ok().filter(|n| n.is_finite());
    let hm = |t: &str| t.get(..5).map(str::to_string);
    // (名称, 现价, 涨跌额, 涨跌幅, 今开, 最高, 最低, 昨收, 时间)
    let (name, price, change, pct, open, high, low, prev, time) = if sina.starts_with("rt_hk") {
        let time = Some(format!("{} {}", f.get(17)?.replace('/', "-"), hm(f.get(18)?)?));
        (f.get(1)?, pos(6), val(7), val(8), pos(2), pos(4), pos(5), pos(3), time)
    } else if sina.starts_with("gb_") {
        (f.first()?, pos(1), val(4), val(2), pos(5), pos(6), pos(7), pos(26), f.get(3).and_then(|t| t.get(..16)).map(str::to_string))
    } else if sina.starts_with("fx_") {
        let time = Some(format!("{} {}", f.get(17)?, hm(f.first()?)?));
        (f.get(9)?, pos(8), val(11), val(10), pos(5), pos(6), pos(7), pos(3), time)
    } else {
        // 沪深北 A 股与指数:没给涨跌,用现价和昨收算
        let (price, prev) = (pos(3), pos(2));
        let change = price.zip(prev).map(|(p, b)| p - b);
        let pct = change.zip(prev).map(|(c, b)| c / b * 100.0);
        let time = Some(format!("{} {}", f.get(30)?, hm(f.get(31)?)?));
        (f.first()?, price, change, pct, pos(1), pos(4), pos(5), prev, time)
    };
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    // 没有现价时涨跌没有意义
    let (change, pct) = if price.is_some() { (change, pct) } else { (None, None) };
    Some(Quote {
        secid: secid.to_string(),
        code: secid.split_once('.').map_or(secid, |(_, c)| c).to_string(),
        name: name.to_string(),
        price,
        change,
        change_pct: pct,
        open,
        high,
        low,
        prev_close: prev,
        time,
    })
}

/// 用户输入 → 真实存在的那只证券(带名称)
pub fn resolve(input: &str) -> Result<Quote> {
    let cands = candidates(input)?;
    fetch_quotes(&cands)?
        .into_iter()
        .next()
        .ok_or_else(|| Error::Invalid(format!("没有找到代码“{}”对应的股票或基金", input.trim())))
}

pub fn fetch_series(secid: &str, range: &str) -> Result<Series> {
    let (klt, lmt) = range_params(range)?;
    let v = get_json(KLINE_URL, &[
        ("secid", secid),
        ("klt", &klt.to_string()),
        ("lmt", &lmt.to_string()),
        ("fqt", "1"), // 前复权,分红送转不会在图上形成断崖
        ("end", "20500101"),
        ("fields1", "f1,f2,f3"),
        ("fields2", "f51,f53"),
    ])?;
    let (points, base) = trim_range(parse_klines(&v), range);
    Ok(Series { secid: secid.into(), range: range.into(), intraday: klt < 100, points, base })
}

// ---------- 持仓估值 ----------

/// 场外基金的 secid 前缀:行情接口里没有这个市场,单独用基金接口取最新净值
pub const FUND_PREFIX: &str = "OF.";
const FUND_URL: &str = "https://fundsuggest.eastmoney.com/FundSearch/api/FundSearchAPI.ashx";
/// 港股、美股报价按离岸人民币汇率换算
const HKD_CNH: &str = "133.HKDCNH";
const USD_CNH: &str = "133.USDCNH";
/// 自动刷新的最短间隔(秒);手动刷新不受限制
const REFRESH_EVERY_SECS: i64 = 3600;

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Price {
    pub secid: String,
    pub name: String,
    /// 原币价格(场外基金为单位净值)
    pub price: f64,
    /// CNY | HKD | USD
    pub currency: String,
    /// 换算成人民币的价格
    pub price_cny: f64,
    /// 行情时间;场外基金为净值日期
    pub time: Option<String>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Failed {
    pub name: String,
    pub reason: String,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct RefreshSummary {
    /// 距上次自动刷新不到一小时,这次没有联网
    pub skipped: bool,
    pub updated: usize,
    pub failed: Vec<Failed>,
}

/// (币种, 汇率 secid);人民币计价时汇率为空
pub fn currency_of(secid: &str) -> (&'static str, Option<&'static str>) {
    match secid.split_once('.').map(|(m, _)| m) {
        Some("116") => ("HKD", Some(HKD_CNH)),
        Some("105" | "106" | "107") => ("USD", Some(USD_CNH)),
        _ => ("CNY", None),
    }
}

/// 现价;停牌或还没开盘时用昨收
fn last_price(q: &Quote) -> Option<f64> {
    q.price.or(q.prev_close).filter(|p| *p > 0.0)
}

/// 基金搜索接口返回 {"Datas":[{"CODE":"110020","NAME":"…","CATEGORY":700,"FundBaseInfo":{"DWJZ":1.6526,"FSRQ":"2026-10-30"}}]}
pub fn parse_fund(v: &Value, code: &str) -> Option<Price> {
    let f = v["Datas"].as_array()?.iter().find(|d| d["CODE"] == code && d["CATEGORY"] == 700)?;
    let nav = num(&f["FundBaseInfo"]["DWJZ"]).filter(|n| *n > 0.0)?;
    Some(Price {
        secid: format!("{FUND_PREFIX}{code}"),
        name: f["NAME"].as_str()?.to_string(),
        price: nav,
        currency: "CNY".into(),
        price_cny: nav,
        time: f["FundBaseInfo"]["FSRQ"].as_str().map(str::to_string),
    })
}

fn fetch_fund(code: &str) -> Result<Price> {
    let v = get_json(FUND_URL, &[("m", "1"), ("key", code)])?;
    parse_fund(&v, code).ok_or_else(|| Error::Invalid(format!("没有找到场外基金“{code}”的净值")))
}

fn fund_code(input: &str) -> Result<String> {
    let s = input.trim();
    if s.len() == 6 && all_digits(s) {
        Ok(s.to_string())
    } else {
        Err(Error::Invalid(format!("场外基金代码是 6 位数字,“{s}”不对")))
    }
}

/// 把报价换成人民币价格;fx 是汇率 secid → 汇率
fn to_price(q: &Quote, fx: &HashMap<String, f64>) -> std::result::Result<Price, String> {
    let price = last_price(q).ok_or("暂时没有价格")?;
    let (currency, pair) = currency_of(&q.secid);
    let rate = match pair {
        None => 1.0,
        Some(p) => *fx.get(p).ok_or("暂时取不到汇率")?,
    };
    Ok(Price { secid: q.secid.clone(), name: q.name.clone(), price, currency: currency.into(), price_cny: price * rate, time: q.time.clone() })
}

/// 一次取多只的报价和所需汇率,返回人民币价格;取不到的给出原因
fn fetch_exchange(secids: &[String]) -> HashMap<String, std::result::Result<Price, String>> {
    let mut ask: Vec<String> = secids.to_vec();
    for s in secids {
        if let (_, Some(p)) = currency_of(s) {
            if !ask.iter().any(|a| a == p) {
                ask.push(p.to_string());
            }
        }
    }
    let quotes = match fetch_quotes(&ask) {
        Ok(q) => q,
        Err(e) => return secids.iter().map(|s| (s.clone(), Err(e.to_string()))).collect(),
    };
    let fx: HashMap<String, f64> = quotes.iter().filter(|q| q.secid.starts_with("133.")).filter_map(|q| Some((q.secid.clone(), last_price(q)?))).collect();
    secids
        .iter()
        .map(|s| {
            let r = quotes.iter().find(|q| q.secid.eq_ignore_ascii_case(s)).ok_or_else(|| "行情里查不到这个代码".to_string()).and_then(|q| to_price(q, &fx));
            (s.clone(), r)
        })
        .collect()
}

/// 按用户输入的代码查最新价。source:exchange 场内(股票、ETF,默认)| fund 场外基金
pub fn lookup_price(code: &str, source: &str) -> Result<Price> {
    match source {
        "fund" => fetch_fund(&fund_code(code)?),
        "exchange" => {
            let q = resolve(code)?;
            let mut fx = HashMap::new();
            if let (_, Some(pair)) = currency_of(&q.secid) {
                fx.extend(fetch_quotes(&[pair.to_string()])?.iter().filter_map(|x| Some((x.secid.clone(), last_price(x)?))));
            }
            to_price(&q, &fx).map_err(|e| Error::Net(format!("“{}”{e},请稍后再试", q.name)))
        }
        _ => Err(Error::Invalid(format!("不支持的行情来源:{source}"))),
    }
}

/// 份额 × 人民币价格 → 金额(分)
pub fn amount_of(share_units: i64, price_cny: f64) -> i64 {
    (share_units as f64 / SHARE_SCALE * price_cny * 100.0).round() as i64
}

/// 保存持仓前调用:填了份额就联网查最新价,算出金额并记下 secid;没填份额就是手动金额
pub fn value_holding(mut input: HoldingInput) -> Result<HoldingInput> {
    input.secid = None;
    let Some(shares) = input.shares else { return Ok(input) };
    let units = portfolio::share_units(shares)?;
    let code = input.code.as_deref().map(str::trim).filter(|c| !c.is_empty()).ok_or_else(|| Error::Invalid("按份额自动估值需要填写代码".into()))?;
    let p = lookup_price(code, input.source.as_deref().unwrap_or("exchange"))?;
    input.amount = amount_of(units, p.price_cny);
    input.price = Some(p.price);
    input.secid = Some(p.secid);
    Ok(input)
}

/// 把查到的价格写进持仓,返回 (更新条数, 失败列表)
pub fn apply_prices(
    c: &mut impl Queryable, list: &[(i64, String, String, i64)], prices: &HashMap<String, std::result::Result<Price, String>>, today: &str,
) -> Result<(usize, Vec<Failed>)> {
    let (mut updated, mut failed) = (0, Vec::new());
    for (id, name, secid, units) in list {
        let res = prices.get(secid).cloned().unwrap_or_else(|| Err("没有取到价格".into()));
        match res.and_then(|p| portfolio::set_holding_amount(c, *id, amount_of(*units, p.price_cny), p.price, today).map_err(|e| e.to_string())) {
            Ok(()) => updated += 1,
            Err(reason) => failed.push(Failed { name: name.clone(), reason }),
        }
    }
    Ok((updated, failed))
}

/// 按最新价更新所有自动估值的持仓。force 为假时,距上次成功刷新不到一小时就跳过
pub fn refresh_holdings(c: &mut Conn, today: &str, now: i64, force: bool) -> Result<RefreshSummary> {
    let last: Option<String> = c.exec_first("SELECT value FROM setting WHERE name='holding_prices_at'", ())?;
    // 升级后还没有原币现价的持仓要尽快补上,否则看不到盈亏
    let recent = last.and_then(|t| t.parse::<i64>().ok()).is_some_and(|t| (0..REFRESH_EVERY_SECS).contains(&(now - t)));
    if !force && recent && !portfolio::missing_prices(c)? {
        // 不联网,但账户余额仍按现有持仓核对一次(余额相等时不写);有改动时让前端重新加载
        let synced = portfolio::sync_account_balances(c, today)?;
        if synced > 0 {
            portfolio::snapshot_today(c, today)?;
        }
        return Ok(RefreshSummary { skipped: true, updated: synced, failed: vec![] });
    }
    let list = portfolio::auto_holdings(c)?;
    let mut secids: Vec<String> = list.iter().map(|h| h.2.clone()).collect();
    secids.sort();
    secids.dedup();
    let (funds, exchange): (Vec<String>, Vec<String>) = secids.into_iter().partition(|s| s.starts_with(FUND_PREFIX));
    let mut prices = if exchange.is_empty() { HashMap::new() } else { fetch_exchange(&exchange) };
    for f in funds {
        let p = fetch_fund(&f[FUND_PREFIX.len()..]).map_err(|e| e.to_string());
        prices.insert(f, p);
    }
    let (updated, failed) = apply_prices(c, &list, &prices, today)?;
    if updated > 0 {
        portfolio::holdings_changed(c, today)?;
    }
    if updated > 0 || list.is_empty() {
        c.exec_drop("INSERT INTO setting (name, value) VALUES ('holding_prices_at', ?) AS new ON DUPLICATE KEY UPDATE value=new.value", (now.to_string(),))?;
    }
    Ok(RefreshSummary { skipped: false, updated, failed })
}

// ---------- 自选列表 ----------

pub fn list_watch(c: &mut Conn) -> Result<Vec<WatchItem>> {
    Ok(c.query_map("SELECT id, secid, code, name FROM watchlist ORDER BY id", |(id, secid, code, name)| WatchItem { id, secid, code, name })?)
}

/// 加入自选。q 是 [`resolve`] 查到的证券
pub fn add_watch(c: &mut Conn, q: &Quote) -> Result<WatchItem> {
    let list = list_watch(c)?;
    if let Some(w) = list.iter().find(|w| w.secid == q.secid) {
        return Err(Error::Invalid(format!("“{}”已经在自选里了", w.name)));
    }
    if list.len() >= MAX_WATCH {
        return Err(Error::Invalid(format!("自选最多 {MAX_WATCH} 只,请先移除一些")));
    }
    c.exec_drop("INSERT INTO watchlist (secid, code, name) VALUES (?, ?, ?)", (&q.secid, &q.code, &q.name))?;
    Ok(WatchItem { id: c.last_insert_id() as i64, secid: q.secid.clone(), code: q.code.clone(), name: q.name.clone() })
}

pub fn remove_watch(c: &mut Conn, id: i64) -> Result<()> {
    c.exec_drop("DELETE FROM watchlist WHERE id = ?", (id,))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdb::TestDb;
    use serde_json::json;

    #[test]
    fn codes_map_to_markets() {
        let c = |s: &str| candidates(s).unwrap();
        assert_eq!(c("600519"), ["1.600519", "0.600519"]);
        assert_eq!(c(" 000001 "), ["0.000001", "1.000001"], "裸 000001 先当平安银行");
        assert_eq!(c("sh000001"), ["1.000001"], "写明 sh 就是上证指数");
        assert_eq!(c("SZ.399001"), ["0.399001"]);
        assert_eq!(c("510300"), ["1.510300", "0.510300"], "5 开头的场内基金在上交所");
        assert_eq!(c("159915"), ["0.159915", "1.159915"]);
        assert_eq!(c("bj430047"), ["0.430047"]);
        assert_eq!(c("700"), ["116.00700"]);
        assert_eq!(c("hk9988"), ["116.09988"]);
        assert_eq!(c("aapl"), ["105.AAPL", "106.AAPL", "107.AAPL"]);
        assert_eq!(c("BRK.B"), ["105.BRK_B", "106.BRK_B", "107.BRK_B"]);
        assert_eq!(c("us.TSLA"), ["105.TSLA", "106.TSLA", "107.TSLA"]);
        assert_eq!(c("105.AAPL"), ["105.AAPL"], "已经是 secid 就原样用");
    }

    #[test]
    fn unrecognised_codes_are_rejected() {
        for s in ["", "   ", "茅台", "1234567", "12-34", "$AAPL"] {
            assert!(candidates(s).is_err(), "{s:?}");
        }
    }

    #[test]
    fn quotes_parse_and_tolerate_missing_values() {
        let v = json!({"data": {"diff": [
            {"f2": 1392.05, "f3": 2.81, "f4": 38.05, "f12": "600519", "f13": 1, "f14": "贵州茅台", "f15": 1393.37, "f16": 1352.98, "f17": 1363.0, "f18": 1354.0, "f124": 1793347921},
            {"f2": "-", "f3": "-", "f4": "-", "f12": "000002", "f13": 0, "f14": "停牌股", "f15": "-", "f16": "-", "f17": "-", "f18": 8.5, "f124": 0},
            {"f2": "-", "f12": "999999", "f13": 0, "f14": "-"}
        ]}});
        let q = parse_quotes(&v);
        assert_eq!(q.len(), 2, "没有名称的条目视为不存在");
        assert_eq!((q[0].secid.as_str(), q[0].name.as_str(), q[0].price, q[0].change_pct), ("1.600519", "贵州茅台", Some(1392.05), Some(2.81)));
        assert!(q[0].time.is_some());
        assert_eq!((q[1].price, q[1].prev_close, q[1].time.as_deref()), (None, Some(8.5), None));
        assert!(parse_quotes(&json!({"data": null})).is_empty());
    }

    #[test]
    fn klines_parse() {
        let v = json!({"data": {"klines": ["2026-10-29,1354.00", "2026-10-30 09:35,1360.5", "bad", "2026-10-30,abc"]}});
        assert_eq!(parse_klines(&v), vec![
            Point { t: "2026-10-29".into(), price: 1354.0 },
            Point { t: "2026-10-30 09:35".into(), price: 1360.5 },
        ]);
        assert!(parse_klines(&json!({"data": null})).is_empty());
    }

    fn pts(items: &[(&str, f64)]) -> Vec<Point> {
        items.iter().map(|(t, p)| Point { t: (*t).into(), price: *p }).collect()
    }

    #[test]
    fn intraday_keeps_last_day_with_previous_close_as_base() {
        let p = pts(&[("2026-10-28 15:00", 9.0), ("2026-10-29 14:55", 9.8), ("2026-10-29 15:00", 10.0), ("2026-10-30 09:35", 10.2), ("2026-10-30 09:40", 10.4)]);
        let (d1, base) = trim_range(p.clone(), "1d");
        assert_eq!(d1.iter().map(|x| x.price).collect::<Vec<_>>(), [10.2, 10.4]);
        assert_eq!(base, Some(10.0), "昨收");
        let (d5, base) = trim_range(p, "5d");
        assert_eq!(d5.len(), 5, "不足 5 天就全给");
        assert_eq!(base, None);
    }

    #[test]
    fn calendar_ranges_cut_by_date() {
        let p = pts(&[("2026-08-28", 1.0), ("2026-09-29", 2.0), ("2026-09-30", 3.0), ("2026-10-15", 4.0), ("2026-10-30", 5.0)]);
        let (m1, base) = trim_range(p.clone(), "1m");
        assert_eq!(m1.iter().map(|x| x.t.as_str()).collect::<Vec<_>>(), ["2026-10-15", "2026-10-30"]);
        assert_eq!(base, Some(3.0), "一个月前那天(9-30)的收盘是基准");
        let (m3, base) = trim_range(p.clone(), "3m");
        assert_eq!((m3.len(), base), (5, None));
        assert_eq!(trim_range(vec![], "1y"), (vec![], None));
        assert!(range_params("2y").is_err());
    }

    fn quote(secid: &str, name: &str) -> Quote {
        let (_, code) = secid.split_once('.').unwrap();
        Quote { secid: secid.into(), code: code.into(), name: name.into(), price: None, change: None, change_pct: None, open: None, high: None, low: None, prev_close: None, time: None }
    }

    #[test]
    fn watchlist_add_list_remove() {
        let t = TestDb::new();
        let mut c = t.conn();
        let a = add_watch(&mut c, &quote("1.600519", "贵州茅台")).unwrap();
        add_watch(&mut c, &quote("105.AAPL", "苹果")).unwrap();
        let err = add_watch(&mut c, &quote("1.600519", "贵州茅台")).unwrap_err().to_string();
        assert!(err.contains("已经在自选"), "{err}");
        assert_eq!(list_watch(&mut c).unwrap().iter().map(|w| w.code.as_str()).collect::<Vec<_>>(), ["600519", "AAPL"]);
        remove_watch(&mut c, a.id).unwrap();
        assert_eq!(list_watch(&mut c).unwrap().len(), 1);
    }

    #[test]
    fn foreign_quotes_convert_to_cny_and_fall_back_to_previous_close() {
        let fx = HashMap::from([(USD_CNH.to_string(), 7.0), (HKD_CNH.to_string(), 0.9)]);
        let q = |secid: &str, price: Option<f64>, prev: Option<f64>| Quote { price, prev_close: prev, ..quote(secid, "x") };
        let us = to_price(&q("107.EWY", Some(100.0), None), &fx).unwrap();
        assert_eq!((us.currency.as_str(), us.price_cny), ("USD", 700.0));
        let hk = to_price(&q("116.02800", None, Some(20.0)), &fx).unwrap();
        assert_eq!((hk.currency.as_str(), hk.price, hk.price_cny), ("HKD", 20.0, 18.0), "停牌用昨收");
        assert_eq!(to_price(&q("1.510300", Some(4.0), None), &HashMap::new()).unwrap().price_cny, 4.0, "人民币不需要汇率");
        assert!(to_price(&q("105.AAPL", Some(1.0), None), &HashMap::new()).is_err(), "缺汇率不能瞎算");
        assert!(to_price(&q("1.600000", None, None), &fx).is_err());
    }

    #[test]
    fn fund_nav_parses_only_the_exact_fund() {
        let v = json!({"Datas": [
            {"CODE": "110020", "NAME": "某股票", "CATEGORY": 100, "FundBaseInfo": null},
            {"CODE": "110020", "NAME": "易方达沪深300ETF联接A", "CATEGORY": 700, "FundBaseInfo": {"DWJZ": 1.6526, "FSRQ": "2026-10-30"}}
        ]});
        let p = parse_fund(&v, "110020").unwrap();
        assert_eq!((p.secid.as_str(), p.name.as_str(), p.price_cny, p.time.as_deref()), ("OF.110020", "易方达沪深300ETF联接A", 1.6526, Some("2026-10-30")));
        assert!(parse_fund(&v, "110021").is_none());
        assert!(parse_fund(&json!({"Datas": []}), "110020").is_none());
        assert!(fund_code("11002").is_err() && fund_code("AAPL").is_err());
    }

    #[test]
    fn amounts_round_to_the_cent() {
        assert_eq!(amount_of(1000 * 10_000, 4.065), 4065_00);
        assert_eq!(amount_of(12_345, 1.6526), 2_04, "1.2345 份 × 1.6526 = 2.04");
    }

    fn priced(secid: &str, cny: f64) -> std::result::Result<Price, String> {
        Ok(Price { secid: secid.into(), name: "x".into(), price: cny, currency: "CNY".into(), price_cny: cny, time: None })
    }

    #[test]
    fn refresh_updates_auto_holdings_and_reports_failures() {
        use crate::core;
        let t = TestDb::new();
        let mut c = t.conn();
        let inv = core::add_account(&mut c, "券商", "investment", "CNY", 100_000_00, "2026-10-01").unwrap();
        let add = |c: &mut Conn, name: &str, secid: Option<&str>, shares: Option<f64>| {
            let input = HoldingInput { account_id: inv, name: name.into(), code: Some("x".into()), asset_class: "other_equity".into(), amount: 1_00, shares, source: None, secid: secid.map(str::to_string), ..Default::default() };
            portfolio::add_holding(c, &input, "2026-10-01").unwrap()
        };
        let a = add(&mut c, "沪深300", Some("1.510300"), Some(1000.0));
        add(&mut c, "基金", Some("OF.110020"), Some(100.5));
        add(&mut c, "手动", None, None);
        let list = portfolio::auto_holdings(&mut c).unwrap();
        assert_eq!(list.len(), 2, "手动金额的不参与");

        let prices = HashMap::from([("1.510300".to_string(), priced("1.510300", 4.065)), ("OF.110020".to_string(), Err("没有找到".to_string()))]);
        let (n, failed) = apply_prices(&mut c, &list, &prices, "2026-10-31").unwrap();
        assert_eq!((n, failed), (1, vec![Failed { name: "基金".into(), reason: "没有找到".into() }]));
        let h = portfolio::list_holdings(&mut c).unwrap();
        let by = |n: &str| h.iter().find(|x| x.name == n).unwrap();
        assert_eq!((by("沪深300").amount, by("沪深300").updated_on.as_str(), by("沪深300").shares), (4065_00, "2026-10-31", Some(1000.0)));
        assert_eq!((by("基金").amount, by("基金").updated_on.as_str()), (1_00, "2026-10-01"), "失败的保持原样");
        assert_eq!((by("沪深300").price, by("基金").price), (Some(4.065), None), "成功的同时记下原币现价");
        assert!(portfolio::missing_prices(&mut c).unwrap(), "基金还没有现价");

        // 改成手动金额后清掉份额与行情来源
        let manual = HoldingInput { account_id: inv, name: "沪深300".into(), code: Some("510300".into()), asset_class: "other_equity".into(), amount: 5_00, ..Default::default() };
        portfolio::update_holding(&mut c, a, &manual, "2026-10-31").unwrap();
        assert_eq!(portfolio::auto_holdings(&mut c).unwrap().len(), 1);
    }

    #[test]
    fn holding_with_shares_needs_a_resolved_price_and_sane_shares() {
        use crate::core;
        let t = TestDb::new();
        let mut c = t.conn();
        let inv = core::add_account(&mut c, "券商", "investment", "CNY", 1_00, "2026-10-01").unwrap();
        let base = HoldingInput { account_id: inv, name: "x".into(), asset_class: "bond".into(), amount: 1_00, shares: Some(10.0), ..Default::default() };
        assert!(portfolio::add_holding(&mut c, &base, "2026-10-31").unwrap_err().to_string().contains("行情"));
        for bad in [0.0, -1.0, 0.00001, f64::NAN, 1e12] {
            let input = HoldingInput { shares: Some(bad), secid: Some("1.511010".into()), ..base.clone() };
            assert!(portfolio::add_holding(&mut c, &input, "2026-10-31").unwrap_err().to_string().contains("份额"), "{bad}");
        }
        assert!(value_holding(HoldingInput { code: None, ..base.clone() }).unwrap_err().to_string().contains("代码"));
        // 不填份额:不联网,原样当手动金额
        let manual = value_holding(HoldingInput { shares: None, secid: Some("1.1".into()), ..base }).unwrap();
        assert_eq!((manual.amount, manual.secid), (1_00, None));
    }

    #[test]
    fn automatic_refresh_runs_at_most_hourly() {
        let t = TestDb::new();
        let mut c = t.conn();
        let first = refresh_holdings(&mut c, "2026-10-31", 1_000_000, false).unwrap();
        assert!(!first.skipped);
        assert!(refresh_holdings(&mut c, "2026-10-31", 1_000_000 + 3599, false).unwrap().skipped);
        assert!(!refresh_holdings(&mut c, "2026-10-31", 1_000_000 + 3599, true).unwrap().skipped, "手动刷新不受限");
        assert!(!refresh_holdings(&mut c, "2026-10-31", 1_000_000 + 3600 * 2, false).unwrap().skipped);
    }

    #[test]
    fn holdings_without_native_price_bypass_the_hourly_limit() {
        use crate::core;
        let t = TestDb::new();
        let mut c = t.conn();
        let inv = core::add_account(&mut c, "券商", "investment", "CNY", 1_00, "2026-10-01").unwrap();
        let input = HoldingInput { account_id: inv, name: "VOO".into(), asset_class: "us_equity_index".into(), amount: 1_00, shares: Some(1.0), secid: Some("107.VOO".into()), price: Some(500.0), ..Default::default() };
        portfolio::add_holding(&mut c, &input, "2026-10-31").unwrap();
        assert!(!portfolio::missing_prices(&mut c).unwrap());
        c.query_drop("INSERT INTO setting (name, value) VALUES ('holding_prices_at', '1000000')").unwrap();
        let r = refresh_holdings(&mut c, "2026-10-31", 1_000_000 + 60, false).unwrap();
        assert!(r.skipped, "都有现价时照常每小时一次");
        assert_eq!(r.updated, 0, "余额已经等于持仓合计");
        // 跳过联网时,也会把没同步的账户余额改成持仓合计
        core::set_valuation(&mut c, inv, "2026-10-31", 9_99).unwrap();
        let r = refresh_holdings(&mut c, "2026-10-31", 1_000_000 + 60, false).unwrap();
        assert_eq!((r.skipped, r.updated), (true, 1));
        assert_eq!(core::list_accounts(&mut c).unwrap()[0].balance, 1_00);
        c.query_drop("UPDATE holding SET price = NULL").unwrap();
        assert!(portfolio::missing_prices(&mut c).unwrap(), "升级后的旧持仓没有现价,会触发刷新");
    }

    #[test]
    fn sina_codes_map_markets() {
        let ids = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let s = |v: &[&str]| sina_codes(&ids(v), |_| None).into_iter().map(|(c, _)| c).collect::<Vec<_>>();
        assert_eq!(
            s(&["1.600519", "0.000001", "0.430047", "0.920001", "116.00700", "105.AAPL", "106.BRK_B", "133.USDCNH", "100.HSI"]),
            ["sh600519", "sz000001", "bj430047", "bj920001", "rt_hk00700", "gb_aapl", "gb_brk$b", "fx_susdcnh"],
            "认不出的市场(100 恒指)略过"
        );
        let all = ["105.BRK_B", "106.BRK_B", "107.BRK_B"];
        assert!(s(&all).is_empty(), "分不清美股市场时不用新浪");
        let nyse = sina_codes(&ids(&all), |c| (c == "BRK_B").then_some("106"));
        assert_eq!(nyse, [("gb_brk$b".to_string(), "106.BRK_B".to_string())], "查到交易所就只留那一个");
        assert_eq!(tencent_us_market(r#"v_usAAPL="200~苹果~AAPL.OQ~340.10~344.63";"#), Some("105"));
        assert_eq!(tencent_us_market(r#"v_usBRK.B="200~伯克希尔B~BRK.B.N~549.14";"#), Some("106"));
        assert_eq!(tencent_us_market(r#"v_usEWY="200~韩国ETF~EWY.AM~157.53";"#), Some("107"));
        assert_eq!(tencent_us_market(r#"v_pv_none_match="1";"#), None);
        assert_eq!(s(&["1.000001", "0.000001"]), ["sh000001", "sz000001"]);
    }

    #[test]
    fn parses_sina() {
        let text = r#"var hq_str_sh600519="贵州茅台,1410.000,1392.050,1439.100,1447.000,1395.020,1439.100,1439.110,8269122,11849258015.000,521,1439.100,300,1439.000,200,1438.900,10700,1438.880,500,1438.700,400,1439.110,400,1439.120,200,1439.250,700,1439.280,300,1439.300,2026-11-02,15:34:59,00,D|3200|4605120.00";
var hq_str_bj430047="诺思兰德,0.000,0.000,0.000,0.000,0.000,0.000,0.000,0,0.000,0,0.000,0,0.000,0,0.000,0,0.000,0,0.000,0,0.000,0,0.000,0,0.000,0,0.000,0,0.000,2026-09-22,11:45:01,-3";
var hq_str_rt_hk00700="TENCENT,腾讯控股,387.600,383.600,397.600,384.600,392.200,8.600,2.242,392.000,392.200,8881214407.650,22621767,14.252,0.000,654.376,375.200,2026/11/02,16:08:20,100|0,N|Y|Y,392.000|372.400|411.600,0|||0.000|0.000|0.000, |0,Y";
var hq_str_gb_aapl="苹果,339.7450,-1.42,2026-11-02 22:58:29,-4.8850,342.6700,343.8000,339.7100,351.7300,242.8900,6541136,42134306,4959814018616,8.30,40.930000,0.00,0.68,0.00,0.00,14598637268,63,0.0000,0.00,0.00,,Nov 02 09:58AM EST,344.6300,0,1,2026,2236707574.0000,0.0000,0.0000,0.0000,0.0000,344.6300";
var hq_str_fx_susdcnh="22:58:25,6.645900,6.646400,6.652800,122,6.652400,6.653800,6.641600,6.645900,离岸人民币（香港）,-0.100000,-0.006900,0.001834,,6.995700,6.642700,,2026-11-02";
var hq_str_gb_zzzz="";
"#;
        let codes: Vec<(String, String)> = [("sh600519", "1.600519"), ("bj430047", "0.430047"), ("rt_hk00700", "116.00700"), ("gb_aapl", "105.AAPL"), ("fx_susdcnh", "133.USDCNH"), ("gb_zzzz", "105.ZZZZ")]
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        let q = parse_sina(text, &codes);
        assert_eq!(q.len(), 5, "空的(查不到)略过");
        let mt = &q[0];
        assert_eq!((mt.secid.as_str(), mt.code.as_str(), mt.name.as_str()), ("1.600519", "600519", "贵州茅台"));
        assert_eq!((mt.price, mt.prev_close, mt.open, mt.high, mt.low), (Some(1439.1), Some(1392.05), Some(1410.0), Some(1447.0), Some(1395.02)));
        assert!((mt.change.unwrap() - 47.05).abs() < 1e-6 && (mt.change_pct.unwrap() - 3.38).abs() < 0.01);
        assert_eq!(mt.time.as_deref(), Some("2026-11-02 15:34"));
        assert_eq!((q[1].name.as_str(), q[1].price, q[1].change, q[1].prev_close), ("诺思兰德", None, None, None), "没有成交的 0 价当作没有数据");
        let hk = &q[2];
        assert_eq!((hk.name.as_str(), hk.price, hk.prev_close, hk.change_pct, hk.time.as_deref()), ("腾讯控股", Some(392.2), Some(383.6), Some(2.242), Some("2026-11-02 16:08")));
        let us = &q[3];
        assert_eq!((us.price, us.change, us.change_pct, us.prev_close, us.time.as_deref()), (Some(339.745), Some(-4.885), Some(-1.42), Some(344.63), Some("2026-11-02 22:58")));
        let fx = &q[4];
        assert_eq!((fx.secid.as_str(), fx.price, fx.prev_close, fx.time.as_deref()), ("133.USDCNH", Some(6.6459), Some(6.6528), Some("2026-11-02 22:58")));
    }

    /// 只走新浪兜底:cargo test live_sina -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_sina() {
        let ids: Vec<String> = ["1.600519", "0.000001", "116.00700", "105.AAPL", "106.BRK_B", "133.USDCNH", "133.HKDCNH"].iter().map(|s| s.to_string()).collect();
        let q = fetch_sina(&sina_codes(&ids, |_| None)).unwrap();
        println!("{q:#?}");
        assert_eq!(q.len(), ids.len());
        assert!(q.iter().all(|x| x.price.or(x.prev_close).is_some()));
        let us: Vec<String> = ["105.EWY", "106.EWY", "107.EWY"].iter().map(|s| s.to_string()).collect();
        assert_eq!(sina_codes(&us, us_exchange), [("gb_ewy".to_string(), "107.EWY".to_string())]);
    }

    /// 真实请求行情接口:cargo test live_market -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_market() {
        let q = resolve("600519").unwrap();
        println!("{q:?}");
        assert_eq!(q.secid, "1.600519");
        for r in ["1d", "5d", "1m", "1y", "5y"] {
            let s = fetch_series("1.600519", r).unwrap();
            println!("{r}: {} 个点, base {:?}, {:?} .. {:?}", s.points.len(), s.base, s.points.first(), s.points.last());
            assert!(!s.points.is_empty());
        }
        assert_eq!(resolve("AAPL").unwrap().secid, "105.AAPL");
        assert_eq!(resolve("00700").unwrap().name, "腾讯控股");
    }

    /// 真实请求行情与基金接口:cargo test live_holding_prices -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_holding_prices() {
        for (code, source) in [("110020", "fund"), ("000001", "fund"), ("510300", "exchange"), ("02800", "exchange"), ("EWY", "exchange")] {
            let p = lookup_price(code, source).unwrap();
            println!("{p:?}");
            assert!(p.price_cny > 0.0);
        }
    }
}
