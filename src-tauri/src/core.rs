//! 业务层:不依赖 Tauri,UI 命令和 Agent 工具调用共用。
use mysql::prelude::Queryable;
use crate::db::Conn;
use mysql::TxOpts;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Auth(String),
    #[error("{0}")]
    Db(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Llm(String),
    /// 行情等联网请求失败
    #[error("{0}")]
    Net(String),
    #[error("文件操作失败:{0}")]
    Io(#[from] std::io::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

impl From<mysql::Error> for Error {
    fn from(e: mysql::Error) -> Self {
        Error::Db(format!("数据库错误:{e}"))
    }
}

impl Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

#[derive(Serialize, Deserialize)]
pub struct Account {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub currency: String,
    /// 当前余额/估值(分)
    pub balance: i64,
}

#[derive(Serialize, Deserialize)]
pub struct Txn {
    pub id: i64,
    pub account_id: Option<i64>,
    pub account: Option<String>,
    pub category_id: Option<i64>,
    pub category: Option<String>,
    pub kind: String,
    pub amount: i64,
    pub date: String,
    pub note: String,
}

#[derive(Serialize, Deserialize)]
pub struct Category {
    pub id: i64,
    pub name: String,
    pub kind: String,
}

#[derive(Serialize)]
pub struct BudgetStatus {
    pub month: String,
    pub budget: Option<i64>,
    pub spent: i64,
    pub income: i64,
}

#[derive(Serialize)]
pub struct NetWorth {
    pub assets: i64,
    pub liabilities: i64,
    pub net: i64,
}

/// 进程内严格单调递增的时间戳(微秒),保证"先估值后记账"的先后判断不受时钟精度影响
fn next_stamp() -> i64 {
    use std::sync::atomic::{AtomicI64, Ordering};
    static LAST: AtomicI64 = AtomicI64::new(0);
    let now = chrono::Utc::now().timestamp_micros();
    let mut prev = LAST.load(Ordering::Relaxed);
    loop {
        let next = now.max(prev + 1);
        match LAST.compare_exchange_weak(prev, next, Ordering::SeqCst, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(p) => prev = p,
        }
    }
}

fn check_kind(kind: &str, allowed: &[&str]) -> Result<()> {
    if allowed.contains(&kind) {
        Ok(())
    } else {
        Err(Error::Invalid(format!("invalid kind: {kind}")))
    }
}

fn check_date(d: &str) -> Result<()> {
    chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d")
        .map(|_| ())
        .map_err(|_| Error::Invalid(format!("invalid date: {d}")))
}

/// 同一连接上最近一次自增主键
pub(crate) fn last_id(c: &mut impl Queryable) -> Result<i64> {
    Ok(c.query_first::<i64, _>("SELECT LAST_INSERT_ID()")?.unwrap_or(0))
}

pub fn add_account(c: &mut Conn, name: &str, kind: &str, currency: &str, balance: i64, date: &str) -> Result<i64> {
    check_kind(kind, &["cash", "deposit", "property", "investment", "liability"])?;
    check_date(date)?;
    let mut tx = c.start_transaction(TxOpts::default())?;
    tx.exec_drop("INSERT INTO account (name, kind, currency) VALUES (?,?,?)", (name, kind, currency))?;
    let id = last_id(&mut tx)?;
    set_valuation(&mut tx, id, date, balance)?;
    tx.commit()?;
    Ok(id)
}

/// 记录/更新某日估值(同一天重复写入则覆盖)
pub fn set_valuation(c: &mut impl Queryable, account_id: i64, date: &str, amount: i64) -> Result<()> {
    check_date(date)?;
    c.exec_drop(
        "INSERT INTO valuation (account_id, date, amount, created_at) VALUES (?,?,?,?) AS new
         ON DUPLICATE KEY UPDATE amount=new.amount, created_at=new.created_at",
        (account_id, date, amount, next_stamp()),
    )?;
    Ok(())
}

/// 账户余额 = 最新估值 + 估值之后录入、且日期不早于估值日的收支。
/// 存款/现金:收入加、支出减;负债(如信用卡):支出加(欠款增加)、收入减。
/// 因此删除账目会自动回退余额;手动更新估值则会重置基准。
///
/// 两个 `?` 都是"截至日期":只看该日及之前的估值与收支,用来回看历史余额。
/// 当日期之前账户还没有任何估值时,该账户记为 0(那时它还不存在)。
const BALANCE_SQL: &str = "
    SELECT a.id, a.name, a.kind, a.currency,
           CAST(COALESCE(v.amount, 0) + COALESCE((
               SELECT SUM(CASE WHEN (a.kind='liability') = (t.kind='expense') THEN t.amount ELSE -t.amount END)
               FROM txn t
               WHERE t.account_id=a.id AND v.id IS NOT NULL
                 AND t.date >= v.date AND t.date <= ? AND t.created_at > v.created_at
           ), 0) AS SIGNED)
    FROM account a
    LEFT JOIN valuation v ON v.id = (
        SELECT id FROM valuation WHERE account_id=a.id AND date <= ? ORDER BY date DESC, id DESC LIMIT 1)
    WHERE a.archived=0";

/// 比任何真实日期都晚,表示"不设上限"(包含未来日期的账目)
pub const NO_LIMIT: &str = "9999-12-31";

/// 截至某日(含)各账户的余额
pub fn accounts_as_of(c: &mut impl Queryable, as_of: &str) -> Result<Vec<Account>> {
    Ok(c.exec_map(
        format!("{BALANCE_SQL} ORDER BY a.id"),
        (as_of, as_of),
        |(id, name, kind, currency, balance): (i64, String, String, String, i64)| Account { id, name, kind, currency, balance },
    )?)
}

pub fn list_accounts(c: &mut impl Queryable) -> Result<Vec<Account>> {
    accounts_as_of(c, NO_LIMIT)
}

pub fn net_worth_as_of(c: &mut impl Queryable, as_of: &str) -> Result<NetWorth> {
    let (mut assets, mut liabilities) = (0, 0);
    for a in accounts_as_of(c, as_of)? {
        if a.kind == "liability" { liabilities += a.balance } else { assets += a.balance }
    }
    Ok(NetWorth { assets, liabilities, net: assets - liabilities })
}

pub fn net_worth(c: &mut impl Queryable) -> Result<NetWorth> {
    net_worth_as_of(c, NO_LIMIT)
}

pub fn list_categories(c: &mut impl Queryable) -> Result<Vec<Category>> {
    Ok(c.query_map("SELECT id, name, kind FROM category ORDER BY id", |(id, name, kind): (i64, String, String)| Category { id, name, kind })?)
}

pub fn add_txn(
    c: &mut impl Queryable, account_id: Option<i64>, category_id: Option<i64>,
    kind: &str, amount: i64, date: &str, note: &str,
) -> Result<i64> {
    check_kind(kind, &["expense", "income"])?;
    check_date(date)?;
    if amount <= 0 {
        return Err(Error::Invalid("amount must be positive".into()));
    }
    if let Some(aid) = account_id {
        let acc_kind: Option<String> = c.exec_first("SELECT kind FROM account WHERE id=? AND archived=0", (aid,))?;
        match acc_kind.as_deref() {
            None => return Err(Error::Invalid("账户不存在".into())),
            Some("property") => return Err(Error::Invalid("房产账户不支持记收支,请直接更新它的估值".into())),
            _ => {}
        }
    }
    c.exec_drop(
        "INSERT INTO txn (account_id, category_id, kind, amount, date, note, created_at) VALUES (?,?,?,?,?,?,?)",
        (account_id, category_id, kind, amount, date, note, next_stamp()),
    )?;
    last_id(c)
}

pub fn delete_txn(c: &mut impl Queryable, id: i64) -> Result<()> {
    c.exec_drop("DELETE FROM txn WHERE id=?", (id,))?;
    Ok(())
}

const TXN_SELECT: &str = "SELECT t.id, t.account_id, a.name, t.category_id, c.name, t.kind, t.amount, t.date, t.note
     FROM txn t LEFT JOIN category c ON c.id=t.category_id LEFT JOIN account a ON a.id=t.account_id";

type TxnRow = (i64, Option<i64>, Option<String>, Option<i64>, Option<String>, String, i64, String, String);

fn map_txn((id, account_id, account, category_id, category, kind, amount, date, note): TxnRow) -> Txn {
    Txn { id, account_id, account, category_id, category, kind, amount, date, note }
}

/// month 形如 "2026-10"
pub fn list_txns(c: &mut impl Queryable, month: &str) -> Result<Vec<Txn>> {
    let sql = format!("{TXN_SELECT} WHERE SUBSTR(t.date,1,7)=? ORDER BY t.date DESC, t.id DESC");
    Ok(c.exec_map(sql, (month,), map_txn)?)
}

/// 全部账目,按日期正序(用于导出)
pub fn list_all_txns(c: &mut impl Queryable) -> Result<Vec<Txn>> {
    Ok(c.query_map(format!("{TXN_SELECT} ORDER BY t.date, t.id"), map_txn)?)
}

pub fn set_budget(c: &mut Conn, month: &str, category_id: Option<i64>, amount: i64) -> Result<()> {
    if amount < 0 {
        return Err(Error::Invalid("budget must be non-negative".into()));
    }
    // UNIQUE 对 NULL 不生效,总预算需先删后插;放进事务保证不会只删不插
    let mut tx = c.start_transaction(TxOpts::default())?;
    match category_id {
        Some(cid) => tx.exec_drop("DELETE FROM budget WHERE month=? AND category_id=?", (month, cid))?,
        None => tx.exec_drop("DELETE FROM budget WHERE month=? AND category_id IS NULL", (month,))?,
    }
    tx.exec_drop("INSERT INTO budget (month, category_id, amount) VALUES (?,?,?)", (month, category_id, amount))?;
    tx.commit()?;
    Ok(())
}

pub fn budget_status(c: &mut impl Queryable, month: &str) -> Result<BudgetStatus> {
    let budget: Option<i64> = c.exec_first("SELECT amount FROM budget WHERE month=? AND category_id IS NULL", (month,))?;
    let mut sum = |kind: &str| -> Result<i64> {
        Ok(c.exec_first::<i64, _, _>(
            "SELECT CAST(COALESCE(SUM(amount),0) AS SIGNED) FROM txn WHERE kind=? AND SUBSTR(date,1,7)=?",
            (kind, month),
        )?
        .unwrap_or(0))
    };
    let (spent, income) = (sum("expense")?, sum("income")?);
    Ok(BudgetStatus { month: month.into(), budget, spent, income })
}

pub fn add_chat(c: &mut impl Queryable, role: &str, content: &str) -> Result<()> {
    c.exec_drop(
        "INSERT INTO chat_message (role, content, created_at) VALUES (?,?,?)",
        (role, content, chrono::Local::now().to_rfc3339()),
    )?;
    Ok(())
}

#[derive(Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

pub fn list_chat(c: &mut impl Queryable) -> Result<Vec<ChatMessage>> {
    Ok(c.query_map("SELECT role, content FROM chat_message ORDER BY id", |(role, content): (String, String)| ChatMessage { role, content })?)
}

/// 最近 n 条对话(按时间正序)
pub fn recent_chat(c: &mut impl Queryable, n: i64) -> Result<Vec<ChatMessage>> {
    Ok(c.exec_map(
        "SELECT role, content FROM (SELECT id, role, content FROM chat_message ORDER BY id DESC LIMIT ?) r ORDER BY id",
        (n,),
        |(role, content): (String, String)| ChatMessage { role, content },
    )?)
}

#[derive(Serialize)]
pub struct CategorySum {
    pub category: String,
    pub total: i64,
    pub count: i64,
}

/// 某月各分类支出合计,按金额降序
pub fn category_summary(c: &mut impl Queryable, month: &str) -> Result<Vec<CategorySum>> {
    category_summary_range(c, month, month)
}

/// 从 `from` 月到 `to` 月(含)各分类支出合计,按金额降序
pub fn category_summary_range(c: &mut impl Queryable, from: &str, to: &str) -> Result<Vec<CategorySum>> {
    Ok(c.exec_map(
        "SELECT COALESCE(c.name,'未分类'), CAST(SUM(t.amount) AS SIGNED), COUNT(*)
         FROM txn t LEFT JOIN category c ON c.id=t.category_id
         WHERE t.kind='expense' AND SUBSTR(t.date,1,7) BETWEEN ? AND ?
         GROUP BY t.category_id, c.name ORDER BY SUM(t.amount) DESC, c.name",
        (from, to),
        |(category, total, count): (String, i64, i64)| CategorySum { category, total, count },
    )?)
}

pub fn find_category(c: &mut impl Queryable, name: &str, kind: &str) -> Result<Option<i64>> {
    let name = name.trim();
    let exact: Option<i64> = c.exec_first("SELECT id FROM category WHERE name=? AND kind=?", (name, kind))?;
    if exact.is_some() || name.is_empty() {
        return Ok(exact);
    }
    // 宽松匹配:名称互相包含(如“吃饭”不匹配,“餐饮费”匹配“餐饮”)
    Ok(list_categories(c)?
        .into_iter()
        .find(|x| x.kind == kind && (name.contains(&x.name) || x.name.contains(name)))
        .map(|x| x.id))
}

/// 精确匹配优先;否则要求包含匹配唯一
pub fn find_account(c: &mut impl Queryable, name: &str) -> Result<Option<Account>> {
    let name = name.trim();
    let all = list_accounts(c)?;
    if let Some(p) = all.iter().position(|a| a.name == name) {
        return Ok(all.into_iter().nth(p));
    }
    let mut hits: Vec<Account> = all.into_iter().filter(|a| !name.is_empty() && (a.name.contains(name) || name.contains(&a.name))).collect();
    Ok(if hits.len() == 1 { hits.pop() } else { None })
}

// ---- 设置 ----

pub const DEFAULT_URL: &str = "http://localhost:11434";
pub const DEFAULT_MODEL: &str = "qwen3:14b";
/// 默认走 Ollama 原生接口;路径以 /chat/completions 结尾时按 OpenAI 兼容格式(DeepSeek 等)请求
pub const DEFAULT_PATH: &str = "/api/chat";
pub const DEFAULT_PERSONA: &str = include_str!("../prompts/persona.md");
pub const DEFAULT_PHILOSOPHY: &str = include_str!("../prompts/philosophy.md");

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Settings {
    pub llm_url: String,
    pub llm_model: String,
    pub llm_path: String,
    pub llm_api_key: String,
    pub persona: String,
    pub philosophy: String,
}

fn get_setting(c: &mut impl Queryable, key: &str) -> Result<Option<String>> {
    Ok(c.exec_first("SELECT value FROM setting WHERE name=?", (key,))?)
}

pub fn get_settings(c: &mut impl Queryable) -> Result<Settings> {
    let mut pick = |k: &str, d: &str| -> Result<String> {
        Ok(get_setting(c, k)?.filter(|v| !v.trim().is_empty()).unwrap_or_else(|| d.to_string()))
    };
    Ok(Settings {
        llm_url: pick("llm_url", DEFAULT_URL)?,
        llm_model: pick("llm_model", DEFAULT_MODEL)?,
        llm_path: pick("llm_path", DEFAULT_PATH)?,
        llm_api_key: pick("llm_api_key", "")?,
        persona: pick("persona", DEFAULT_PERSONA)?,
        philosophy: pick("philosophy", DEFAULT_PHILOSOPHY)?,
    })
}

pub fn save_settings(c: &mut Conn, s: &Settings) -> Result<()> {
    let url = s.llm_url.trim().trim_end_matches('/');
    let model = s.llm_model.trim();
    let path = s.llm_path.trim();
    let path = if path.is_empty() { DEFAULT_PATH.to_string() } else if path.starts_with('/') { path.to_string() } else { format!("/{path}") };
    if url.is_empty() || model.is_empty() {
        return Err(Error::Invalid("模型地址和模型名不能为空".into()));
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(Error::Invalid("模型地址需以 http:// 或 https:// 开头".into()));
    }
    let mut tx = c.start_transaction(TxOpts::default())?;
    for (k, v) in [("llm_url", url), ("llm_model", model), ("llm_path", path.as_str()), ("llm_api_key", s.llm_api_key.trim()), ("persona", s.persona.trim()), ("philosophy", s.philosophy.trim())] {
        tx.exec_drop(
            "INSERT INTO setting (name, value) VALUES (?,?) AS new ON DUPLICATE KEY UPDATE value=new.value",
            (k, v),
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// 恢复人设与理财理念为内置默认值
pub fn reset_prompts(c: &mut impl Queryable) -> Result<()> {
    c.query_drop("DELETE FROM setting WHERE name IN ('persona','philosophy')")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdb::TestDb;

    #[test]
    fn net_worth_and_budget() {
        let t = TestDb::new();
        let mut c = t.conn();
        add_account(&mut c, "招行", "deposit", "CNY", 100_000_00, "2026-10-01").unwrap();
        add_account(&mut c, "房贷", "liability", "CNY", 40_000_00, "2026-10-01").unwrap();
        assert_eq!(net_worth(&mut c).unwrap().net, 60_000_00);

        set_budget(&mut c, "2026-10", None, 5000_00).unwrap();
        set_budget(&mut c, "2026-10", None, 6000_00).unwrap(); // 覆盖,且不会留下两条
        add_txn(&mut c, None, None, "expense", 35_00, "2026-10-27", "午饭").unwrap();
        let s = budget_status(&mut c, "2026-10").unwrap();
        assert_eq!((s.budget, s.spent), (Some(6000_00), 35_00));
        let n: i64 = c.query_first("SELECT COUNT(*) FROM budget").unwrap().unwrap();
        assert_eq!(n, 1);
        assert!(add_txn(&mut c, None, None, "expense", 0, "2026-10-27", "").is_err());
    }

    fn bal(c: &mut crate::db::Conn, id: i64) -> i64 {
        list_accounts(c).unwrap().into_iter().find(|a| a.id == id).unwrap().balance
    }

    #[test]
    fn balance_follows_transactions_and_resets_on_new_valuation() {
        let t = TestDb::new();
        let mut c = t.conn();
        let a = add_account(&mut c, "招行", "deposit", "CNY", 1000_00, "2026-10-01").unwrap();
        let t1 = add_txn(&mut c, Some(a), None, "expense", 100_00, "2026-10-05", "").unwrap();
        add_txn(&mut c, Some(a), None, "income", 30_00, "2026-10-06", "").unwrap();
        assert_eq!(bal(&mut c, a), 930_00);

        // 删除账目 → 余额回退
        delete_txn(&mut c, t1).unwrap();
        assert_eq!(bal(&mut c, a), 1030_00);

        // 重新估值 → 以估值为准;此前录入的账目不再重复计入
        set_valuation(&mut c, a, "2026-10-10", 2000_00).unwrap();
        assert_eq!(bal(&mut c, a), 2000_00);
        // 估值之后录入的新账目继续计入
        add_txn(&mut c, Some(a), None, "expense", 50_00, "2026-10-11", "").unwrap();
        assert_eq!(bal(&mut c, a), 1950_00);
    }

    #[test]
    fn same_day_valuation_is_overwritten_not_duplicated() {
        let t = TestDb::new();
        let mut c = t.conn();
        let a = add_account(&mut c, "招行", "deposit", "CNY", 1000_00, "2026-10-27").unwrap();
        add_txn(&mut c, Some(a), None, "expense", 10_00, "2026-10-27", "").unwrap();
        set_valuation(&mut c, a, "2026-10-27", 500_00).unwrap(); // 同日重估,基准重置
        assert_eq!(bal(&mut c, a), 500_00);
        let n: i64 = c.query_first("SELECT COUNT(*) FROM valuation").unwrap().unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn backdated_entry_before_valuation_is_not_double_counted() {
        let t = TestDb::new();
        let mut c = t.conn();
        let a = add_account(&mut c, "招行", "deposit", "CNY", 1000_00, "2026-10-01").unwrap();
        set_valuation(&mut c, a, "2026-10-20", 800_00).unwrap(); // 真实余额已含此前所有消费
        add_txn(&mut c, Some(a), None, "expense", 60_00, "2026-10-15", "补记").unwrap(); // 补记 10-15,早于估值日
        assert_eq!(bal(&mut c, a), 800_00);
    }

    #[test]
    fn entry_after_deleting_latest_still_counts() {
        let t = TestDb::new();
        let mut c = t.conn();
        let a = add_account(&mut c, "招行", "deposit", "CNY", 1000_00, "2026-10-27").unwrap();
        let x = add_txn(&mut c, Some(a), None, "expense", 10_00, "2026-10-27", "").unwrap();
        delete_txn(&mut c, x).unwrap();
        add_txn(&mut c, Some(a), None, "expense", 20_00, "2026-10-27", "").unwrap();
        assert_eq!(bal(&mut c, a), 980_00);
    }

    #[test]
    fn property_and_missing_accounts_reject_transactions() {
        let t = TestDb::new();
        let mut c = t.conn();
        let h = add_account(&mut c, "住宅", "property", "CNY", 100_0000, "2026-10-01").unwrap();
        assert!(add_txn(&mut c, Some(h), None, "expense", 1_00, "2026-10-02", "").is_err());
        assert!(add_txn(&mut c, Some(999), None, "expense", 1_00, "2026-10-02", "").is_err());
        assert!(list_txns(&mut c, "2026-10").unwrap().is_empty());
    }

    #[test]
    fn category_summary_and_lookup() {
        let t = TestDb::new();
        let mut c = t.conn();
        let food = find_category(&mut c, "餐饮费", "expense").unwrap(); // 宽松匹配
        assert!(food.is_some());
        add_txn(&mut c, None, food, "expense", 30_00, "2026-10-02", "").unwrap();
        add_txn(&mut c, None, food, "expense", 20_00, "2026-10-03", "").unwrap();
        add_txn(&mut c, None, None, "expense", 5_00, "2026-10-03", "").unwrap();
        let s = category_summary(&mut c, "2026-10").unwrap();
        assert_eq!((s[0].category.as_str(), s[0].total, s[0].count), ("餐饮", 50_00, 2));
        assert_eq!(s[1].category, "未分类");
    }

    #[test]
    fn settings_defaults_validate_and_reset() {
        let t = TestDb::new();
        let mut c = t.conn();
        let mut s = get_settings(&mut c).unwrap();
        assert_eq!(s.llm_model, "qwen3:14b");
        s.persona = "自定义人设".into();
        save_settings(&mut c, &s).unwrap();
        s.persona = "再改一次".into(); // 覆盖已有键
        save_settings(&mut c, &s).unwrap();
        assert_eq!(get_settings(&mut c).unwrap().persona, "再改一次");
        reset_prompts(&mut c).unwrap();
        assert!(get_settings(&mut c).unwrap().persona.contains("个人财务助手"));
        s.llm_model = "  ".into();
        assert!(save_settings(&mut c, &s).is_err());
        assert_eq!(get_settings(&mut c).unwrap().llm_model, "qwen3:14b");
    }

    #[test]
    fn chat_history_keeps_order_and_limit() {
        let t = TestDb::new();
        let mut c = t.conn();
        for i in 0..5 {
            add_chat(&mut c, if i % 2 == 0 { "user" } else { "assistant" }, &format!("消息{i}")).unwrap();
        }
        let recent = recent_chat(&mut c, 3).unwrap();
        assert_eq!(recent.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(), ["消息2", "消息3", "消息4"]);
        assert_eq!(list_chat(&mut c).unwrap().len(), 5);
    }

    #[test]
    fn chinese_text_round_trips() {
        let t = TestDb::new();
        let mut c = t.conn();
        add_txn(&mut c, None, None, "expense", 1_00, "2026-10-01", "火锅🍲 emoji 和 “引号”").unwrap();
        assert_eq!(list_txns(&mut c, "2026-10").unwrap()[0].note, "火锅🍲 emoji 和 “引号”");
    }
}
