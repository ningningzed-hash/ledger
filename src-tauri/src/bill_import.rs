//! 账单导入:读微信 / 支付宝导出的账单文件,生成预览,用户勾选后批量入账。
//!
//! - 格式:微信「下载账单」的 XLSX / CSV,支付宝导出的 CSV(GBK 编码,新旧两种表头都认)。
//!   不按固定行号读,而是找到同时含「收/支」和「金额」的表头行,再按列名取值。
//! - 跳过:不计收支(转入零钱通、提现、还信用卡…)、交易关闭/失败、全额退款和退款记录;
//!   微信「已退款(￥x)」的部分退款按退款后的金额入账。
//! - 去重:导入过的交易单号记在 `import_ref`,再次导入同一文件不会重复;
//!   和手动记过的账目(同一天、同类型、同金额)疑似重复的,默认不勾选。
//! - 分类:先用学到的规则(这个交易对方上次被归到哪类),再用支付宝自带的交易分类,
//!   最后按商户关键词猜;都不中的归到「其他 / 其他收入」并标为未识别。
//!   入账时把用户最终选的分类、支付方式对应的账户记成规则,下次自动套用。
use crate::core::{self, Category, Error, Result};
use crate::db::Conn;
use calamine::{Data, Reader};
use mysql::prelude::Queryable;
use mysql::TxOpts;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::Path;

const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_ROWS: usize = 5000;
const MAX_NOTE_CHARS: usize = 120;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Wechat,
    Alipay,
}

impl Source {
    fn key(self) -> &'static str {
        match self {
            Source::Wechat => "wechat",
            Source::Alipay => "alipay",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Source::Wechat => "微信",
            Source::Alipay => "支付宝",
        }
    }
}

/// 从文件里读出的一条交易(还没套分类、去重)
#[derive(Debug, Clone, PartialEq)]
pub struct RawRow {
    pub ref_id: String,
    pub time: String,
    pub date: String,
    /// "expense" | "income";不计收支为空
    pub kind: Option<String>,
    /// 分;已扣除部分退款
    pub amount: i64,
    pub counterparty: String,
    pub goods: String,
    pub method: String,
    pub status: String,
    /// 支付宝的「交易分类」/ 微信的「交易类型」
    pub platform_type: String,
    /// 有值表示这条不入账
    pub skip: Option<String>,
}

#[derive(Serialize, Debug, Clone)]
pub struct PreviewRow {
    pub ref_id: String,
    pub time: String,
    pub date: String,
    pub kind: Option<String>,
    pub amount: i64,
    pub counterparty: String,
    pub goods: String,
    pub method: String,
    pub status: String,
    pub platform_type: String,
    pub note: String,
    pub category_id: Option<i64>,
    /// learned 学到的 | platform 平台分类 | keyword 关键词 | none 未识别
    pub category_source: String,
    /// 默认是否勾选
    pub selected: bool,
    /// 不能入账的原因(不计收支、已关闭、已导入过…)
    pub skip: Option<String>,
    /// 疑似和这笔手动记的账重复
    pub duplicate_of: Option<String>,
}

#[derive(Serialize, Debug, Clone)]
pub struct MethodMap {
    pub method: String,
    pub count: usize,
    /// 建议关联的账户(学到的规则或名称匹配)
    pub account_id: Option<i64>,
}

#[derive(Serialize, Debug)]
pub struct Preview {
    pub source: Source,
    pub source_label: String,
    pub file_name: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub rows: Vec<PreviewRow>,
    pub methods: Vec<MethodMap>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct CommitItem {
    pub ref_id: String,
    pub date: String,
    pub kind: String,
    pub amount: i64,
    pub category_id: Option<i64>,
    pub account_id: Option<i64>,
    pub note: String,
    pub counterparty: String,
    pub method: String,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct CommitResult {
    pub imported: usize,
    /// 这期间已被导入过、这次跳过的
    pub already: usize,
    pub expense: i64,
    pub income: i64,
}

// ---------- 读文件 ----------

/// 文件 → 单元格表。XLSX 用 calamine;文本先按 UTF-8,失败再按 GBK(支付宝)
pub fn read_table(bytes: &[u8]) -> Result<Vec<Vec<String>>> {
    if bytes.starts_with(b"PK\x03\x04") {
        return read_xlsx(bytes);
    }
    let text = match std::str::from_utf8(bytes) {
        Ok(s) => s.trim_start_matches('\u{FEFF}').to_string(),
        Err(_) => encoding_rs::GBK.decode(bytes).0.into_owned(),
    };
    Ok(parse_csv(&text))
}

fn read_xlsx(bytes: &[u8]) -> Result<Vec<Vec<String>>> {
    let bad = |e: String| Error::Invalid(format!("无法读取这个 Excel 文件:{e}"));
    let mut wb = calamine::open_workbook_auto_from_rs(Cursor::new(bytes.to_vec())).map_err(|e| bad(e.to_string()))?;
    let range = wb.worksheet_range_at(0).ok_or_else(|| bad("文件里没有工作表".into()))?.map_err(|e| bad(e.to_string()))?;
    Ok(range
        .rows()
        .map(|r| {
            r.iter()
                .map(|c| match c {
                    Data::DateTime(d) => d.as_datetime().map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_default(),
                    Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", *f as i64),
                    Data::Empty => String::new(),
                    other => other.to_string(),
                })
                .collect()
        })
        .collect())
}

/// 极简 CSV:支持双引号包裹、"" 转义、引号内换行
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut rows = vec![];
    let mut row = vec![];
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if quoted {
            match ch {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    cell.push('"');
                }
                '"' => quoted = false,
                _ => cell.push(ch),
            }
            continue;
        }
        match ch {
            '"' if cell.trim().is_empty() => {
                cell.clear();
                quoted = true;
            }
            ',' => row.push(std::mem::take(&mut cell)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            _ => cell.push(ch),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

// ---------- 解析账单 ----------

fn clean(s: &str) -> String {
    let t = s.trim().trim_matches('\t').trim();
    if t == "/" || t == "-" { String::new() } else { t.to_string() }
}

/// "¥35.00" / "￥1,234.5" / "35" → 分
fn parse_amount(s: &str) -> Option<i64> {
    let t: String = s.chars().filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
    let v: f64 = t.parse().ok()?;
    (v.is_finite() && v.abs() < 1e12).then(|| (v.abs() * 100.0).round() as i64)
}

/// "2026-10-27 12:03:11" / "2026/10/27 12:03" → ("2026-10-27 12:03:11", "2026-10-27")
fn parse_time(s: &str) -> Option<(String, String)> {
    let s = s.trim().replace('/', "-");
    let (d, t) = s.split_once(' ').unwrap_or((&s, ""));
    let date = chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()?;
    let date = date.format("%Y-%m-%d").to_string();
    let time = if t.is_empty() { date.clone() } else { format!("{date} {}", t.trim()) };
    Some((time, date))
}

/// 微信「已退款(￥10.00)」「已退款￥10.00」里的退款金额
fn partial_refund(status: &str) -> Option<i64> {
    let i = status.find("退款")?;
    let rest = &status[i..];
    rest.contains(['¥', '￥']).then(|| parse_amount(rest)).flatten()
}

struct Columns {
    time: usize,
    kind: usize,
    amount: usize,
    counterparty: Option<usize>,
    goods: Option<usize>,
    method: Option<usize>,
    status: Option<usize>,
    ptype: Option<usize>,
    order: Option<usize>,
}

fn find_col(header: &[String], names: &[&str]) -> Option<usize> {
    names.iter().find_map(|n| header.iter().position(|h| h.trim() == *n))
        .or_else(|| names.iter().find_map(|n| header.iter().position(|h| h.trim().starts_with(n))))
}

fn columns(header: &[String]) -> Option<Columns> {
    Some(Columns {
        time: find_col(header, &["交易时间", "交易创建时间", "付款时间"])?,
        kind: find_col(header, &["收/支"])?,
        amount: find_col(header, &["金额(元)", "金额（元）", "金额"])?,
        counterparty: find_col(header, &["交易对方"]),
        goods: find_col(header, &["商品", "商品说明", "商品名称"]),
        method: find_col(header, &["支付方式", "收/付款方式"]),
        status: find_col(header, &["当前状态", "交易状态"]),
        ptype: find_col(header, &["交易分类", "交易类型", "类型"]),
        order: find_col(header, &["交易单号", "交易订单号", "交易号"]),
    })
}

/// 认出账单来源并解析出每一条交易
pub fn parse_bill(table: &[Vec<String>]) -> Result<(Source, Vec<RawRow>)> {
    let not_bill = || Error::Invalid(
        "没认出这是微信或支付宝的账单。请使用微信「我 → 服务 → 钱包 → 账单 → 常见问题 → 下载账单」\
         或支付宝「我的 → 账单 → ··· → 开具交易流水证明」导出的文件(CSV 或 XLSX),不要改动内容。".into(),
    );
    let hi = table
        .iter()
        .position(|r| r.iter().any(|c| c.trim() == "收/支") && r.iter().any(|c| c.contains("金额")))
        .ok_or_else(not_bill)?;
    let header = &table[hi];
    let cols = columns(header).ok_or_else(not_bill)?;
    let preamble: String = table[..hi].iter().flatten().map(|s| s.as_str()).collect();
    let has = |n: &str| header.iter().any(|h| h.trim() == n);
    let source = if preamble.contains("微信") || has("当前状态") {
        Source::Wechat
    } else if preamble.contains("支付宝") || has("交易订单号") || has("交易号") {
        Source::Alipay
    } else {
        return Err(not_bill());
    };

    let get = |r: &[String], i: Option<usize>| i.and_then(|i| r.get(i)).map(|s| clean(s)).unwrap_or_default();
    let mut out = vec![];
    for r in &table[hi + 1..] {
        if r.iter().all(|c| c.trim().is_empty()) || r[0].trim_start().starts_with("---") {
            if out.is_empty() { continue } else { break }
        }
        let Some((time, date)) = r.get(cols.time).and_then(|s| parse_time(s)) else { continue };
        let Some(gross) = r.get(cols.amount).and_then(|s| parse_amount(s)) else { continue };
        let io = get(r, Some(cols.kind));
        let kind = match io.as_str() {
            "支出" => Some("expense".to_string()),
            "收入" => Some("income".to_string()),
            _ => None,
        };
        let (counterparty, goods, method, status, ptype) =
            (get(r, cols.counterparty), get(r, cols.goods), get(r, cols.method), get(r, cols.status), get(r, cols.ptype));
        let mut amount = gross;
        let skip = if kind.is_none() {
            Some("不计收支(转账到自己的账户、提现、还款等)".to_string())
        } else if ["关闭", "失败", "撤销", "未支付", "等待"].iter().any(|k| status.contains(k)) {
            Some(format!("交易未完成({status})"))
        } else if status.contains("全额退款") || ptype.contains("退款") || goods.starts_with("退款") || status == "退款成功" {
            Some("已退款或是退款记录".to_string())
        } else if let Some(refund) = partial_refund(&status) {
            amount = gross - refund;
            (amount <= 0).then(|| "已退款或是退款记录".to_string())
        } else {
            None
        };
        let order = get(r, cols.order);
        let ref_id = if order.is_empty() { format!("{time}|{gross}|{counterparty}|{goods}") } else { order };
        out.push(RawRow { ref_id, time, date, kind, amount, counterparty, goods, method, status, platform_type: ptype, skip });
        if out.len() >= MAX_ROWS {
            break;
        }
    }
    if out.is_empty() {
        return Err(Error::Invalid(format!("这份{}账单里没有交易记录", source.label())));
    }
    Ok((source, out))
}

// ---------- 分类 ----------

/// 支付宝自带的交易分类 → 本应用的分类
fn platform_category(ptype: &str, kind: &str) -> Option<&'static str> {
    let map: &[(&str, &str)] = if kind == "expense" {
        &[
            ("餐饮", "餐饮"), ("交通", "交通"), ("爱车", "交通"), ("住房", "居住"), ("物业", "居住"), ("充值缴费", "居住"),
            ("医疗", "医疗"), ("文化休闲", "娱乐"), ("休闲娱乐", "娱乐"), ("酒店旅游", "娱乐"), ("运动", "娱乐"),
            ("日用", "购物"), ("服饰", "购物"), ("数码", "购物"), ("美容", "购物"), ("母婴", "购物"), ("家居", "购物"), ("超市", "购物"),
        ]
    } else {
        &[("工资", "工资"), ("投资理财", "理财收益"), ("理财", "理财收益"), ("红包", "其他收入"), ("转账", "其他收入")]
    };
    map.iter().find(|(k, _)| ptype.contains(k)).map(|(_, v)| *v)
}

/// 商户名 / 商品里的关键词 → 分类。按顺序匹配,先具体后宽泛(「美团打车」先命中交通,再轮到餐饮的「美团」)
const EXPENSE_KEYWORDS: &[(&str, &[&str])] = &[
    ("交通", &["滴滴", "打车", "快车", "出租", "曹操出行", "T3出行", "高德", "地铁", "公交", "轨道交通", "12306", "铁路", "航空", "机票", "加油", "中石化", "中石油", "停车", "ETC", "单车", "哈啰", "青桔", "出行"]),
    ("医疗", &["医院", "药房", "药店", "大药", "诊所", "体检", "口腔", "医疗", "卫生院"]),
    ("居住", &["物业", "房租", "租金", "电费", "水费", "燃气", "国家电网", "电力", "宽带", "自如", "贝壳", "链家"]),
    ("娱乐", &["电影", "影城", "影院", "猫眼", "淘票票", "KTV", "游戏", "Steam", "腾讯视频", "爱奇艺", "优酷", "哔哩", "bilibili", "网易云", "QQ音乐", "健身", "景区", "门票", "酒店", "携程", "去哪儿", "飞猪"]),
    ("餐饮", &["美团", "饿了么", "外卖", "肯德基", "KFC", "麦当劳", "星巴克", "瑞幸", "库迪", "喜茶", "奈雪", "蜜雪", "茶百道", "古茗", "必胜客", "海底捞", "餐", "饭", "面馆", "粉", "咖啡", "奶茶", "茶饮", "食堂", "烧烤", "火锅", "小吃", "早点", "早餐", "烘焙", "面包", "饮品"]),
    ("购物", &["淘宝", "天猫", "京东", "拼多多", "唯品会", "抖音", "得物", "超市", "便利店", "盒马", "山姆", "沃尔玛", "永辉", "物美", "罗森", "全家", "7-11", "屈臣氏", "名创", "商城", "商店", "百货"]),
];
const INCOME_KEYWORDS: &[(&str, &[&str])] = &[
    ("工资", &["工资", "薪", "代发", "奖金"]),
    ("理财收益", &["收益", "利息", "分红", "基金", "理财"]),
    ("其他收入", &["红包", "转账", "退款"]),
];

fn keyword_category(text: &str, kind: &str) -> Option<&'static str> {
    let list = if kind == "expense" { EXPENSE_KEYWORDS } else { INCOME_KEYWORDS };
    let lower = text.to_lowercase();
    list.iter().find(|(_, ks)| ks.iter().any(|k| lower.contains(&k.to_lowercase()))).map(|(c, _)| *c)
}

fn note_of(counterparty: &str, goods: &str) -> String {
    // 商品说明常常就是交易对方的重复,或是没意义的“商户消费”
    let g = goods.trim();
    let generic = g.is_empty() || g == counterparty || ["商户消费", "收款方备注:二维码收款", "二维码收款", "转账"].contains(&g);
    let s = match (counterparty.is_empty(), generic) {
        (true, _) => g.to_string(),
        (false, true) => counterparty.to_string(),
        (false, false) => format!("{counterparty} · {g}"),
    };
    s.chars().take(MAX_NOTE_CHARS).collect()
}

// ---------- 学到的规则 ----------

fn rules(c: &mut impl Queryable, kind: &str) -> Result<HashMap<String, i64>> {
    Ok(c.exec_map("SELECT pattern, target_id FROM import_rule WHERE kind=?", (kind,), |(p, t): (String, i64)| (p, t))?
        .into_iter()
        .collect())
}

fn learn(c: &mut impl Queryable, kind: &str, pattern: &str, target: i64) -> Result<()> {
    if pattern.is_empty() {
        return Ok(());
    }
    let p: String = pattern.chars().take(150).collect();
    c.exec_drop("REPLACE INTO import_rule (kind, pattern, target_id) VALUES (?, ?, ?)", (kind, p, target))?;
    Ok(())
}

/// 计算方式里的尾号、括号等去掉后再和账户名比:"招商银行信用卡(1234)" → "招商银行信用卡"
fn method_base(m: &str) -> &str {
    m.split(['(', '（']).next().unwrap_or(m).trim()
}

// ---------- 预览 ----------

pub fn preview_file(c: &mut Conn, path: &Path) -> Result<Preview> {
    let meta = std::fs::metadata(path)?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(Error::Invalid("文件太大(超过 20 MB),请缩短账单的时间范围后重新导出".into()));
    }
    let bytes = std::fs::read(path)?;
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
    let (source, raw) = parse_bill(&read_table(&bytes)?)?;
    let mut p = build_preview(c, source, raw)?;
    p.file_name = file_name;
    Ok(p)
}

pub fn build_preview(c: &mut impl Queryable, source: Source, raw: Vec<RawRow>) -> Result<Preview> {
    let cats = core::list_categories(c)?;
    let cat_id = |name: &str, kind: &str| cats.iter().find(|x: &&Category| x.name == name && x.kind == kind).map(|x| x.id);
    let cat_ok = |id: i64, kind: &str| cats.iter().any(|x| x.id == id && x.kind == kind);
    let learned = rules(c, "counterparty")?;
    let imported: HashSet<String> = c
        .exec_map("SELECT ref_id FROM import_ref WHERE source=?", (source.key(),), |r: String| r)?
        .into_iter()
        .collect();

    // 区间内手动记的账(不是导入的),用来找疑似重复
    let dates: Vec<&str> = raw.iter().map(|r| r.date.as_str()).collect();
    let (from, to) = (dates.iter().min().map(|s| s.to_string()), dates.iter().max().map(|s| s.to_string()));
    let mut manual: HashMap<(String, String, i64), Vec<String>> = HashMap::new();
    if let (Some(f), Some(t)) = (&from, &to) {
        let rows: Vec<(String, String, i64, String)> = c.exec(
            "SELECT t.date, t.kind, t.amount, t.note FROM txn t
             WHERE t.date BETWEEN ? AND ? AND NOT EXISTS (SELECT 1 FROM import_ref r WHERE r.txn_id = t.id)",
            (f, t),
        )?;
        for (d, k, a, n) in rows {
            manual.entry((d, k, a)).or_default().push(n);
        }
    }

    let mut methods: Vec<MethodMap> = vec![];
    let mut rows = vec![];
    for r in raw {
        let note = note_of(&r.counterparty, &r.goods);
        let mut skip = r.skip.clone();
        if skip.is_none() && imported.contains(&r.ref_id) {
            skip = Some("之前已经导入过".into());
        }
        let (mut category_id, mut category_source) = (None, "none");
        let mut duplicate_of = None;
        if let (Some(kind), None) = (&r.kind, &skip) {
            if let Some(id) = learned.get(&r.counterparty).copied().filter(|id| cat_ok(*id, kind)) {
                (category_id, category_source) = (Some(id), "learned");
            } else if let Some(id) = platform_category(&r.platform_type, kind).and_then(|n| cat_id(n, kind)) {
                (category_id, category_source) = (Some(id), "platform");
            } else if let Some(id) = keyword_category(&format!("{} {} {}", r.counterparty, r.goods, r.platform_type), kind).and_then(|n| cat_id(n, kind)) {
                (category_id, category_source) = (Some(id), "keyword");
            } else {
                category_id = cat_id(if kind == "expense" { "其他" } else { "其他收入" }, kind);
            }
            // 同一天同金额的手动账目,每条只抵消一次
            if let Some(list) = manual.get_mut(&(r.date.clone(), kind.clone(), r.amount)) {
                if let Some(n) = list.pop() {
                    duplicate_of = Some(if n.is_empty() { "同一天同金额的一笔手动账目".into() } else { format!("手动记过的「{n}」") });
                }
            }
            if !r.method.is_empty() {
                match methods.iter_mut().find(|m| m.method == r.method) {
                    Some(m) => m.count += 1,
                    None => methods.push(MethodMap { method: r.method.clone(), count: 1, account_id: None }),
                }
            }
        }
        rows.push(PreviewRow {
            selected: skip.is_none() && duplicate_of.is_none(),
            ref_id: r.ref_id, time: r.time, date: r.date, kind: r.kind, amount: r.amount,
            counterparty: r.counterparty, goods: r.goods, method: r.method, status: r.status, platform_type: r.platform_type,
            note, category_id, category_source: category_source.into(), skip, duplicate_of,
        });
    }

    // 支付方式 → 账户:先用学到的,再按名称匹配(唯一命中才用)
    let learned_acc = rules(c, "method")?;
    let accounts: Vec<core::Account> = core::list_accounts(c)?.into_iter().filter(|a| a.kind != "property").collect();
    for m in &mut methods {
        m.account_id = learned_acc.get(&m.method).copied().filter(|id| accounts.iter().any(|a| a.id == *id)).or_else(|| {
            let base = method_base(&m.method);
            let hits: Vec<i64> = accounts.iter().filter(|a| base.contains(&a.name) || a.name.contains(base)).map(|a| a.id).collect();
            (hits.len() == 1).then(|| hits[0])
        });
    }
    methods.sort_by(|a, b| b.count.cmp(&a.count));
    rows.sort_by(|a, b| b.time.cmp(&a.time));
    Ok(Preview { source, source_label: source.label().into(), file_name: String::new(), from, to, rows, methods })
}

// ---------- 入账 ----------

pub fn commit(c: &mut Conn, source: Source, items: &[CommitItem]) -> Result<CommitResult> {
    if items.is_empty() {
        return Err(Error::Invalid("没有勾选任何账目".into()));
    }
    let mut tx = c.start_transaction(TxOpts::default())?;
    let mut res = CommitResult { imported: 0, already: 0, expense: 0, income: 0 };
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    for it in items {
        let exists: Option<i64> = tx.exec_first("SELECT 1 FROM import_ref WHERE source=? AND ref_id=?", (source.key(), &it.ref_id))?;
        if exists.is_some() {
            res.already += 1;
            continue;
        }
        let id = core::add_txn(&mut tx, it.account_id, it.category_id, &it.kind, it.amount, &it.date, &it.note)?;
        tx.exec_drop(
            "INSERT INTO import_ref (source, ref_id, txn_id, imported_on) VALUES (?, ?, ?, ?)",
            (source.key(), &it.ref_id, id, &today),
        )?;
        if let Some(cid) = it.category_id {
            learn(&mut tx, "counterparty", &it.counterparty, cid)?;
        }
        if let Some(aid) = it.account_id {
            learn(&mut tx, "method", &it.method, aid)?;
        }
        res.imported += 1;
        if it.kind == "expense" { res.expense += it.amount } else { res.income += it.amount }
    }
    tx.commit()?;
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdb::TestDb;

    const WECHAT: &str = "微信支付账单明细,,,,,,,,,,
微信昵称：[小明],,,,,,,,,,
起始时间：[2026-10-01 00:00:00] 终止时间：[2026-10-30 23:59:59],,,,,,,,,,
----------------------微信支付账单明细列表--------------------,,,,,,,,,,
交易时间,交易类型,交易对方,商品,收/支,金额(元),支付方式,当前状态,交易单号,商户单号,备注
2026-10-27 12:03:11,商户消费,美团,\"美团订单-午饭, 黄焖鸡\",支出,¥35.00,零钱,支付成功,4200001\t,M1\t,/
2026-10-26 08:10:00,商户消费,滴滴出行,快车,支出,¥23.50,招商银行信用卡(1234),支付成功,4200002\t,M2\t,/
2026-10-25 20:00:00,微信红包,张三,/,收入,¥66.00,/,已存入零钱,4200003\t,/,/
2026-10-24 10:00:00,零钱提现,招商银行(5678),/,/,¥500.00,零钱,提现已到账,4200004\t,/,/
2026-10-23 18:00:00,商户消费,某某服饰店,衣服,支出,¥200.00,零钱,已全额退款,4200005\t,M5\t,/
2026-10-22 18:00:00,商户消费,京东,耳机,支出,¥300.00,零钱,已退款(￥100.00),4200006\t,M6\t,/
2026-10-21 18:00:00,商户消费,王记小店,/,支出,¥12.00,零钱,支付成功,4200007\t,M7\t,/
";

    fn wechat() -> (Source, Vec<RawRow>) {
        parse_bill(&parse_csv(WECHAT)).unwrap()
    }

    #[test]
    fn csv_handles_quotes_commas_and_newlines() {
        let t = parse_csv("a,\"b,1\",\"say \"\"hi\"\"\"\r\n\"x\ny\",2\n");
        assert_eq!(t, vec![vec!["a", "b,1", "say \"hi\""], vec!["x\ny", "2"]]);
    }

    #[test]
    fn wechat_rows_parse_with_skips_and_partial_refund() {
        let (src, rows) = wechat();
        assert_eq!(src, Source::Wechat);
        assert_eq!(rows.len(), 7);
        let r0 = &rows[0];
        assert_eq!((r0.ref_id.as_str(), r0.date.as_str(), r0.kind.as_deref(), r0.amount), ("4200001", "2026-10-27", Some("expense"), 35_00));
        assert_eq!(r0.goods, "美团订单-午饭, 黄焖鸡");
        assert_eq!(rows[2].kind.as_deref(), Some("income"));
        assert!(rows[3].skip.as_deref().unwrap().contains("不计收支"));
        assert!(rows[4].skip.as_deref().unwrap().contains("退款"));
        assert_eq!((rows[5].amount, rows[5].skip.as_ref()), (200_00, None), "部分退款按退后金额");
    }

    #[test]
    fn alipay_gbk_new_and_old_headers() {
        let new = "------------------------------------------------------------------------------------\n\
            导出信息：\n姓名：小明\n支付宝账户：xx\n\
            交易时间,交易分类,交易对方,对方账号,商品说明,收/支,金额,收/付款方式,交易状态,交易订单号,商家订单号,备注,\n\
            2026/10/27 12:03,餐饮美食,肯德基,k***@x.com,早餐,支出,19.90,花呗,交易成功,2026102722001\t,M1\t,,\n\
            2026/10/26 09:00,投资理财,余额宝,/,收益发放,不计收支,0.52,余额宝,交易成功,2026102622002\t,,,\n\
            2026/10/25 09:00,日用百货,某店,/,退款-牙刷,不计收支,9.90,,退款成功,2026102522003_1\t,,,\n";
        let (gbk, _, _) = encoding_rs::GBK.encode(new);
        let (src, rows) = parse_bill(&read_table(&gbk).unwrap()).unwrap();
        assert_eq!(src, Source::Alipay);
        assert_eq!((rows[0].date.as_str(), rows[0].amount, rows[0].platform_type.as_str(), rows[0].method.as_str()), ("2026-10-27", 19_90, "餐饮美食", "花呗"));
        assert!(rows[1].skip.is_some() && rows[2].skip.is_some());

        let old = "支付宝交易记录明细查询\n账号:[xx]\n---------------------------------交易记录明细列表------------------------------------\n\
            交易号                  ,商家订单号               ,交易创建时间              ,付款时间                ,最近修改时间              ,交易来源地     ,类型              ,交易对方            ,商品名称                ,金额（元）   ,收/支     ,交易状态    ,服务费（元）   ,成功退款（元）  ,备注                  ,资金状态     ,\n\
            2026102722001       ,M1   ,2026-10-27 12:03:11 ,2026-10-27 12:03:12 ,2026-10-27 12:03:12 ,其他（包括阿里巴巴和外部商家）,即时到账交易          ,滴滴出行     ,快车     ,23.50   ,支出      ,交易成功    ,0.00     ,0.00     ,                    ,已支出      ,\n\
            ------------------------------------------------------------------------------------\n共1笔记录\n";
        let (src, rows) = parse_bill(&parse_csv(old)).unwrap();
        assert_eq!(src, Source::Alipay);
        assert_eq!((rows.len(), rows[0].counterparty.as_str(), rows[0].amount), (1, "滴滴出行", 23_50));
    }

    #[test]
    fn not_a_bill_is_rejected() {
        let err = parse_bill(&parse_csv("日期,金额\n2026-10-01,3\n")).unwrap_err().to_string();
        assert!(err.contains("没认出"), "{err}");
    }

    #[test]
    fn keywords_prefer_specific_categories() {
        assert_eq!(keyword_category("美团打车 快车", "expense"), Some("交通"));
        assert_eq!(keyword_category("美团 美团订单", "expense"), Some("餐饮"));
        assert_eq!(keyword_category("steam game", "expense"), Some("娱乐"));
        assert_eq!(keyword_category("王记小店", "expense"), None);
        assert_eq!(platform_category("餐饮美食", "expense"), Some("餐饮"));
        assert_eq!(note_of("美团", "美团订单-午饭"), "美团 · 美团订单-午饭");
        assert_eq!(note_of("王记小店", ""), "王记小店");
    }

    fn items(p: &Preview) -> Vec<CommitItem> {
        let acc = |m: &str| p.methods.iter().find(|x| x.method == m).and_then(|x| x.account_id);
        p.rows.iter().filter(|r| r.selected).map(|r| CommitItem {
            ref_id: r.ref_id.clone(), date: r.date.clone(), kind: r.kind.clone().unwrap(), amount: r.amount,
            category_id: r.category_id, account_id: acc(&r.method), note: r.note.clone(),
            counterparty: r.counterparty.clone(), method: r.method.clone(),
        }).collect()
    }

    #[test]
    fn preview_commit_dedupe_and_learning() {
        let t = TestDb::new();
        let mut c = t.conn();
        let card = core::add_account(&mut c, "招商银行信用卡", "liability", "CNY", 0, "2026-10-01").unwrap();
        let food = core::find_category(&mut c, "餐饮", "expense").unwrap();
        // 手动记过的一笔:和账单里 12 元那条同一天同金额
        core::add_txn(&mut c, None, food, "expense", 12_00, "2026-10-21", "小店").unwrap();

        let (src, raw) = wechat();
        let p = build_preview(&mut c, src, raw.clone()).unwrap();
        let by = |id: &str| p.rows.iter().find(|r| r.ref_id == id).unwrap();
        assert_eq!((by("4200001").category_id, by("4200001").category_source.as_str()), (food, "keyword"));
        assert_eq!(by("4200002").category_id, core::find_category(&mut c, "交通", "expense").unwrap());
        assert_eq!(by("4200003").category_id, core::find_category(&mut c, "其他收入", "income").unwrap());
        let dup = by("4200007");
        assert!(!dup.selected && dup.duplicate_of.as_deref().unwrap().contains("小店"), "疑似重复默认不勾选");
        assert_eq!(dup.category_source, "none");
        assert_eq!(p.methods.iter().find(|m| m.method.starts_with("招商")).unwrap().account_id, Some(card), "按名称匹配到账户");
        assert_eq!(p.rows.iter().filter(|r| r.selected).count(), 4);

        // 把「王记小店」改成餐饮再入账,学到规则
        let mut its = items(&p);
        let mut extra = its[0].clone();
        extra.ref_id = "4200007".into();
        extra.counterparty = "王记小店".into();
        extra.amount = 12_00;
        its.push(extra);
        let r = commit(&mut c, src, &its).unwrap();
        assert_eq!((r.imported, r.already, r.expense, r.income), (5, 0, 35_00 + 23_50 + 200_00 + 12_00, 66_00));
        assert_eq!(core::list_accounts(&mut c).unwrap().iter().find(|a| a.id == card).unwrap().balance, 23_50, "信用卡欠款随之增加(负债余额记为正数)");

        let p2 = build_preview(&mut c, src, raw).unwrap();
        assert!(p2.rows.iter().all(|r| !r.selected), "再导入同一份账单,全部跳过");
        assert_eq!(p2.rows.iter().find(|r| r.ref_id == "4200001").unwrap().skip.as_deref(), Some("之前已经导入过"));
        assert_eq!(commit(&mut c, src, &its).unwrap().imported, 0, "重复提交也不会重复入账");

        // 新账单里同一家店:用学到的分类
        let mut more = wechat().1;
        more.truncate(1);
        more[0].ref_id = "new-1".into();
        more[0].counterparty = "王记小店".into();
        more[0].goods = String::new();
        let p3 = build_preview(&mut c, src, more).unwrap();
        assert_eq!((p3.rows[0].category_id, p3.rows[0].category_source.as_str()), (food, "learned"));
    }
}
