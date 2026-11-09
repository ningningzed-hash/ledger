//! 自动备份与恢复。
//!
//! - 备份是一个 JSON 文件,包含全部数据表(与 MySQL 版本无关,换电脑/换库都能用)。
//!   导出在一个一致性快照里完成,不会读到写了一半的数据。
//! - 恢复在单个事务里"清空再写入":文件校验失败不会动现有数据,写入中途出错会整体回滚。
//!   校验通过后、覆盖前,会先把当前数据另存一份安全备份。
//! - 应用启动时每天自动备份一次,仅保留最近 7 份(界面上不再提供手动导出)。
use crate::core::{Error, Result};
use crate::db::Conn;
use mysql::prelude::Queryable;
use mysql::{TxOpts, Value};
use serde_json::{json, Map, Value as Json};
use std::fs;
use std::path::{Path, PathBuf};

const AUTO_PREFIX: &str = "ledger-";
const SAFETY_PREFIX: &str = "pre-restore-";
const KEEP_AUTO: usize = 7;
const KEEP_SAFETY: usize = 3;
const FORMAT: &str = "ledger-backup";
const VERSION: i64 = 1;
const MAX_BACKUP_BYTES: u64 = 512 * 1024 * 1024;

/// (表名, 列, 排序表达式)。按外键依赖顺序排列:被引用的表在前。
pub const TABLES: [(&str, &[&str], &str); 19] = [
    ("category", &["id", "name", "kind"], "`id`"),
    ("account", &["id", "name", "kind", "currency", "archived"], "`id`"),
    ("valuation", &["id", "account_id", "date", "amount", "created_at"], "`id`"),
    ("txn", &["id", "account_id", "category_id", "kind", "amount", "date", "note", "created_at"], "`id`"),
    ("budget", &["id", "month", "category_id", "amount"], "`id`"),
    ("chat_message", &["id", "role", "content", "created_at"], "`id`"),
    ("setting", &["name", "value"], "`name`"),
    ("goal", &["id", "name", "kind", "target", "start_amount", "deadline", "created_on"], "`id`"),
    ("goal_account", &["goal_id", "account_id"], "`goal_id`,`account_id`"),
    ("recurring", &["id", "name", "kind", "amount", "frequency", "start_date", "end_date", "paused", "category_id", "account_id"], "`id`"),
    ("recurring_run", &["recurring_id", "due_date", "txn_id"], "`recurring_id`,`due_date`"),
    ("account_rate", &["account_id", "rate_bps"], "`account_id`"),
    ("holding", &["id", "account_id", "name", "code", "asset_class", "amount", "updated_on", "shares", "secid", "cost_price", "price"], "`id`"),
    ("product", &["id", "name", "code", "asset_class", "note", "builtin"], "`id`"),
    ("watchlist", &["id", "secid", "code", "name"], "`id`"),
    ("import_ref", &["source", "ref_id", "txn_id", "imported_on"], "`source`,`ref_id`"),
    ("import_rule", &["kind", "pattern", "target_id"], "`kind`,`pattern`"),
    ("review", &["month", "content", "created_at"], "`month`"),
    ("holding_snapshot", &["date", "value", "cost"], "`date`"),
];

/// 后来才加入的表:旧版本导出的备份里没有它们,恢复时当作空表(其余的表缺失则是无效备份)
const OPTIONAL: [&str; 12] = [
    "goal", "goal_account", "recurring", "recurring_run", "account_rate", "holding", "product", "watchlist", "import_ref", "import_rule",
    "review", "holding_snapshot",
];

fn quoted(cols: &[&str]) -> String {
    cols.iter().map(|c| format!("`{c}`")).collect::<Vec<_>>().join(",")
}

fn to_json(v: Value) -> Json {
    match v {
        Value::NULL => Json::Null,
        Value::Int(i) => json!(i),
        Value::UInt(u) => json!(u),
        Value::Float(f) => json!(f),
        Value::Double(d) => json!(d),
        Value::Bytes(b) => Json::String(String::from_utf8_lossy(&b).into_owned()),
        other => Json::String(format!("{other:?}")),
    }
}

/// 读出全部数据(同一个一致性快照)
pub fn dump(c: &mut Conn) -> Result<Json> {
    let mut tx = c.start_transaction(TxOpts::default().set_with_consistent_snapshot(true))?;
    let mut tables = Map::new();
    for (name, cols, order) in TABLES {
        // 用预编译协议读取,数字列才会以数字(而不是字符串)返回
        // API 密钥不进备份文件
        let filter = if name == "setting" { " WHERE `name` <> 'llm_api_key'" } else { "" };
        let rows: Vec<mysql::Row> = tx.exec(format!("SELECT {} FROM `{name}`{filter} ORDER BY {order}", quoted(cols)), ())?;
        let arr = rows
            .into_iter()
            .map(|row| {
                let obj: Map<String, Json> = cols.iter().map(|c| c.to_string()).zip(row.unwrap().into_iter().map(to_json)).collect();
                Json::Object(obj)
            })
            .collect();
        tables.insert(name.to_string(), Json::Array(arr));
    }
    tx.commit()?;
    Ok(json!({
        "format": FORMAT,
        "version": VERSION,
        "exported_at": chrono::Local::now().to_rfc3339(),
        "tables": tables,
    }))
}

/// 把当前数据写成备份文件(已存在则覆盖)。先写临时文件再改名,避免留下写了一半的备份。
pub fn write_dump(c: &mut Conn, dest: &Path) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(&dump(c)?).map_err(|e| Error::Invalid(format!("生成备份失败:{e}")))?;
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = dest.with_extension("json.tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, dest)?;
    Ok(())
}

fn list_files(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension().is_some_and(|e| e == "json")
                && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(prefix))
        })
        .collect();
    v.sort(); // 文件名含日期/时间,字典序即时间序
    v
}

fn prune(dir: &Path, prefix: &str, keep: usize) {
    let files = list_files(dir, prefix);
    for old in files.iter().take(files.len().saturating_sub(keep)) {
        let _ = fs::remove_file(old);
    }
}

/// 当天还没有自动备份就创建一份,返回新文件路径
pub fn auto_backup(c: &mut Conn, dir: &Path) -> Result<Option<PathBuf>> {
    let dest = dir.join(format!("{AUTO_PREFIX}{}.json", chrono::Local::now().format("%Y-%m-%d")));
    if dest.exists() {
        return Ok(None);
    }
    write_dump(c, &dest)?;
    prune(dir, AUTO_PREFIX, KEEP_AUTO);
    Ok(Some(dest))
}

type Rows = Vec<Vec<Value>>;

/// 解析并校验备份文件,返回按 TABLES 顺序排列的各表数据。任何不符都在这里拒绝,不碰数据库。
fn parse(src: &Path) -> Result<Vec<Rows>> {
    let bad = || Error::Invalid("这不是有效的 Ledger 备份文件".into());
    let md = fs::metadata(src).map_err(|_| Error::Invalid("无法读取备份文件".into()))?;
    if !md.is_file() || md.len() > MAX_BACKUP_BYTES {
        return Err(bad());
    }
    let text = fs::read_to_string(src).map_err(|_| bad())?;
    let doc: Json = serde_json::from_str(&text).map_err(|_| bad())?;
    if doc["format"] != FORMAT {
        return Err(bad());
    }
    if doc["version"] != VERSION {
        return Err(Error::Invalid(format!("不支持的备份版本:{}", doc["version"])));
    }

    let mut out = Vec::new();
    for (name, cols, _) in TABLES {
        let arr = match doc["tables"][name].as_array() {
            Some(a) => a,
            None if OPTIONAL.contains(&name) => {
                out.push(Vec::new());
                continue;
            }
            None => return Err(bad()),
        };
        let mut rows = Vec::with_capacity(arr.len());
        for obj in arr {
            let mut row = Vec::with_capacity(cols.len());
            for col in cols {
                row.push(match &obj[*col] {
                    Json::Null => Value::NULL,
                    Json::String(s) => Value::Bytes(s.clone().into_bytes()),
                    // 成本价、现价等 DOUBLE 列导出为小数
                    Json::Number(n) => match n.as_i64() {
                        Some(i) => Value::Int(i),
                        None => Value::Double(n.as_f64().filter(|f| f.is_finite()).ok_or_else(bad)?),
                    },
                    _ => return Err(bad()),
                });
            }
            rows.push(row);
        }
        out.push(rows);
    }
    Ok(out)
}

/// 用备份文件覆盖当前全部数据。会先在 `dir` 下保存一份当前数据的安全备份。
pub fn restore(c: &mut Conn, dir: &Path, src: &Path) -> Result<()> {
    let data = parse(src)?;

    let safety = dir.join(format!("{SAFETY_PREFIX}{}.json", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    write_dump(c, &safety)?;
    prune(dir, SAFETY_PREFIX, KEEP_SAFETY);

    // 单个事务:出错时不 commit,drop 时自动回滚,现有数据原样保留
    let mut tx = c.start_transaction(TxOpts::default())?;
    for (name, _, _) in TABLES.iter().rev() {
        tx.query_drop(format!("DELETE FROM `{name}`"))?;
    }
    for ((name, cols, _), rows) in TABLES.iter().zip(data) {
        if rows.is_empty() {
            continue;
        }
        let marks = vec!["?"; cols.len()].join(",");
        tx.exec_batch(format!("INSERT INTO `{name}` ({}) VALUES ({marks})", quoted(cols)), rows)?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core;
    use crate::testdb::TestDb;

    fn seed(c: &mut Conn) {
        let a = core::add_account(c, "招行", "deposit", "CNY", 1000_00, "2026-10-01").unwrap();
        let food = core::find_category(c, "餐饮", "expense").unwrap();
        core::add_txn(c, Some(a), food, "expense", 35_00, "2026-10-27", "午饭 🍜 “好吃”").unwrap();
        core::set_budget(c, "2026-10", None, 6000_00).unwrap();
        core::set_budget(c, "2026-10", food, 1500_00).unwrap();
        core::add_chat(c, "user", "你好").unwrap();
        crate::goals::add_goal(
            c,
            &crate::goals::GoalInput { name: "攒 5 万".into(), kind: "save".into(), target: 50_000_00, deadline: Some("2027-12-31".into()), account_ids: vec![a] },
            "2026-10-28",
        )
        .unwrap();
        core::save_settings(c, &core::Settings {
            llm_url: "http://localhost:11434".into(), llm_model: "qwen3:14b".into(),
            llm_path: "/api/chat".into(), llm_api_key: "sk-secret".into(),
            persona: "自定义人设".into(), philosophy: "自定义理念".into(),
        }).unwrap();
    }

    fn tables(c: &mut Conn) -> Json {
        dump(c).unwrap()["tables"].clone()
    }

    #[test]
    fn export_then_restore_roundtrip_is_lossless() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, file) = (tmp.path().join("backups"), tmp.path().join("export.json"));
        let (a, b) = (TestDb::new(), TestDb::new());
        let (mut ca, mut cb) = (a.conn(), b.conn());
        seed(&mut ca);
        write_dump(&mut ca, &file).unwrap();
        write_dump(&mut ca, &file).unwrap(); // 覆盖已有文件不报错
        assert!(!tmp.path().join("export.json.tmp").exists(), "不应残留临时文件");

        // 另一个库里是不同的数据
        core::add_account(&mut cb, "别的账户", "cash", "CNY", 1_00, "2026-10-01").unwrap();
        core::add_chat(&mut cb, "user", "旧聊天").unwrap();

        restore(&mut cb, &dir, &file).unwrap();
        assert_eq!(tables(&mut cb), tables(&mut ca), "恢复后两边数据应逐表完全一致");
        let accs = core::list_accounts(&mut cb).unwrap();
        assert_eq!((accs.len(), accs[0].name.as_str(), accs[0].balance), (1, "招行", 965_00));
        assert_eq!(core::get_settings(&mut cb).unwrap().persona, "自定义人设");

        // 恢复后仍可写,自增编号接着往后走,且留下了安全备份
        core::add_txn(&mut cb, Some(accs[0].id), None, "expense", 5_00, "2026-10-28", "").unwrap();
        assert_eq!(core::list_accounts(&mut cb).unwrap()[0].balance, 960_00);
        assert_eq!(list_files(&dir, SAFETY_PREFIX).len(), 1);
    }

    #[test]
    fn goals_survive_a_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (TestDb::new(), TestDb::new());
        let (mut ca, mut cb) = (a.conn(), b.conn());
        seed(&mut ca);
        let file = tmp.path().join("x.json");
        write_dump(&mut ca, &file).unwrap();
        restore(&mut cb, &tmp.path().join("b"), &file).unwrap();
        let g = crate::goals::list_goals(&mut cb, "2026-10-28").unwrap();
        assert_eq!((g.len(), g[0].name.as_str(), g[0].accounts.len(), g[0].current), (1, "攒 5 万", 1, 965_00));
    }

    #[test]
    fn subscriptions_survive_a_roundtrip_without_reposting() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (TestDb::new(), TestDb::new());
        let (mut ca, mut cb) = (a.conn(), b.conn());
        seed(&mut ca);
        let acc = core::list_accounts(&mut ca).unwrap()[0].id;
        let input = crate::recurring::RecurringInput {
            name: "房租".into(), kind: "expense".into(), amount: 3_000_00, frequency: "monthly".into(),
            start_date: "2026-08-05".into(), end_date: None, category_id: None, account_id: Some(acc),
        };
        assert_eq!(crate::recurring::add_recurring(&mut ca, &input, "2026-10-28").unwrap().posted, 3);
        // 用户删掉了其中一期的账目:这个"已处理"的事实也必须一起带过去
        let sep = core::list_all_txns(&mut ca).unwrap().into_iter().find(|t| t.date == "2026-09-05").unwrap();
        core::delete_txn(&mut ca, sep.id).unwrap();

        let file = tmp.path().join("x.json");
        write_dump(&mut ca, &file).unwrap();
        restore(&mut cb, &tmp.path().join("b"), &file).unwrap();
        assert_eq!(tables(&mut cb), tables(&mut ca), "逐表一致,包括订阅与入账记录");

        let before = core::list_all_txns(&mut cb).unwrap().len();
        assert_eq!(crate::recurring::run_due(&mut cb, "2026-10-28").unwrap().posted, 0, "恢复后再运行不能重复入账,也不能把删掉的那期补回来");
        assert_eq!(core::list_all_txns(&mut cb).unwrap().len(), before);
        let l = crate::recurring::list_recurring(&mut cb, "2026-10-28").unwrap();
        assert_eq!((l.items.len(), l.items[0].posted_count, l.items[0].next_due.as_deref()), (1, 2, Some("2026-11-05")));
    }

    #[test]
    fn older_backups_without_goal_tables_still_restore() {
        let tmp = tempfile::tempdir().unwrap();
        let t = TestDb::new();
        let mut c = t.conn();
        seed(&mut c); // 现有数据里有一个目标
        let mut doc = dump(&mut c).unwrap();
        doc["tables"].as_object_mut().unwrap().remove("goal");
        doc["tables"].as_object_mut().unwrap().remove("goal_account");
        doc["tables"].as_object_mut().unwrap().remove("recurring");
        doc["tables"].as_object_mut().unwrap().remove("recurring_run");
        let old = tmp.path().join("v1-before-goals.json");
        fs::write(&old, serde_json::to_string(&doc).unwrap()).unwrap();

        restore(&mut c, &tmp.path().join("b"), &old).unwrap();
        assert!(crate::goals::list_goals(&mut c, "2026-10-28").unwrap().is_empty(), "恢复是整体替换:旧备份里没有目标,目标就被清空");
        assert!(crate::recurring::list_recurring(&mut c, "2026-10-28").unwrap().items.is_empty(), "订阅同理");
        assert_eq!(core::list_accounts(&mut c).unwrap().len(), 1, "其余数据照常恢复");
    }

    #[test]
    fn holding_costs_and_snapshots_roundtrip_and_old_backups_restore() {
        let tmp = tempfile::tempdir().unwrap();
        let t = TestDb::new();
        let mut c = t.conn();
        seed(&mut c);
        let inv = core::add_account(&mut c, "券商", "investment", "CNY", 100_000_00, "2026-11-01").unwrap();
        c.exec_drop(
            "INSERT INTO holding (account_id, name, code, asset_class, amount, updated_on, shares, secid, cost_price, price) VALUES (?, 'VOO', 'VOO', 'equity_us', 3960000, '2026-11-02', 100000, '107.VOO', 500.25, 550.0)",
            (inv,),
        )
        .unwrap();
        c.query_drop("INSERT INTO holding_snapshot (date, value, cost) VALUES ('2026-11-01', 3900000, 3600000)").unwrap();
        let file = tmp.path().join("new.json");
        fs::write(&file, serde_json::to_string(&dump(&mut c).unwrap()).unwrap()).unwrap();
        c.query_drop("UPDATE holding SET cost_price=NULL, price=NULL").unwrap();
        c.query_drop("DELETE FROM holding_snapshot").unwrap();
        restore(&mut c, &tmp.path().join("b"), &file).unwrap();
        let got: Option<(Option<f64>, Option<f64>)> = c.query_first("SELECT cost_price, price FROM holding").unwrap();
        assert_eq!(got, Some((Some(500.25), Some(550.0))));
        let snap: Option<(String, i64, i64)> = c.query_first("SELECT date, value, cost FROM holding_snapshot").unwrap();
        assert_eq!(snap, Some(("2026-11-01".into(), 3900000, 3600000)));

        // 旧版本的备份:holding 没有这两列,也没有快照表
        let mut doc = dump(&mut c).unwrap();
        doc["tables"].as_object_mut().unwrap().remove("holding_snapshot");
        for h in doc["tables"]["holding"].as_array_mut().unwrap() {
            let o = h.as_object_mut().unwrap();
            o.remove("cost_price");
            o.remove("price");
        }
        let old = tmp.path().join("old.json");
        fs::write(&old, serde_json::to_string(&doc).unwrap()).unwrap();
        restore(&mut c, &tmp.path().join("b"), &old).unwrap();
        let got: Option<(String, Option<f64>, Option<f64>)> = c.query_first("SELECT name, cost_price, price FROM holding").unwrap();
        assert_eq!(got, Some(("VOO".into(), None, None)));
        let n: Option<i64> = c.query_first("SELECT COUNT(*) FROM holding_snapshot").unwrap();
        assert_eq!(n, Some(0));
    }

    #[test]
    fn invalid_files_are_rejected_and_data_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("backups");
        let t = TestDb::new();
        let mut c = t.conn();
        seed(&mut c);
        let before = tables(&mut c);

        let write = |name: &str, text: &str| {
            let p = tmp.path().join(name);
            fs::write(&p, text).unwrap();
            p
        };
        let good = serde_json::to_string(&dump(&mut c).unwrap()).unwrap();
        let cases = [
            write("junk.json", "这不是备份"),
            write("other.json", r#"{"format":"something-else"}"#),
            write("future.json", &good.replace("\"version\":1", "\"version\":99")),
            write("missing.json", &good.replace("\"chat_message\"", "\"chat_msg\"")),
            write("badtype.json", &good.replace("\"amount\":3500", "\"amount\":[1]")),
            tmp.path().join("nope.json"),
            tmp.path().to_path_buf(), // 目录
        ];
        for p in &cases {
            assert!(restore(&mut c, &dir, p).is_err(), "应拒绝 {p:?}");
        }
        assert_eq!(tables(&mut c), before);
        assert!(list_files(&dir, SAFETY_PREFIX).is_empty(), "校验失败不应动当前数据,也不必留安全备份");
    }

    #[test]
    fn failure_midway_rolls_back_everything() {
        let tmp = tempfile::tempdir().unwrap();
        let t = TestDb::new();
        let mut c = t.conn();
        seed(&mut c);
        let before = tables(&mut c);

        // 文件格式合法,但有一笔账目引用了不存在的账户 → 写入时违反外键
        let mut doc = dump(&mut c).unwrap();
        doc["tables"]["txn"][0]["account_id"] = json!(999);
        doc["tables"]["chat_message"] = json!([]); // 同时制造"已清空"的前置动作,验证回滚
        let p = tmp.path().join("broken.json");
        fs::write(&p, serde_json::to_string(&doc).unwrap()).unwrap();

        assert!(restore(&mut c, &tmp.path().join("b"), &p).is_err());
        assert_eq!(tables(&mut c), before, "中途失败必须整体回滚,原数据一条不少");
        // 连接仍可正常使用
        core::add_chat(&mut c, "user", "还能写").unwrap();
    }

    #[test]
    fn auto_backup_runs_once_per_day_and_prunes() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("backups");
        let t = TestDb::new();
        let mut c = t.conn();
        seed(&mut c);
        assert!(auto_backup(&mut c, &dir).unwrap().is_some());
        assert!(auto_backup(&mut c, &dir).unwrap().is_none());

        for d in 1..=9 {
            fs::write(dir.join(format!("{AUTO_PREFIX}2020-01-0{d}.json")), "x").unwrap();
        }
        prune(&dir, AUTO_PREFIX, KEEP_AUTO);
        let left = list_files(&dir, AUTO_PREFIX);
        assert_eq!(left.len(), KEEP_AUTO);
        assert!(left.iter().any(|p| p.to_str().unwrap().contains(&chrono::Local::now().format("%Y-%m-%d").to_string())), "今天的备份必须保留");
    }
}
