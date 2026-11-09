//! 一次性工具:把旧版本(SQLite)的数据导入 MySQL。
//!
//!   cargo run --example import_sqlite -- <旧的 ledger.db 路径>
//!
//! - 以只读方式打开旧文件,不会修改它。
//! - 仅当 MySQL 里还没有账户/估值/账目/预算/聊天记录/设置时才导入,绝不覆盖已有数据。
//! - 旧库没有 created_at 列时记为 0,表示"这些账目早已体现在余额里",导入后各账户余额与旧版一致。
use ledger_lib::{backup::TABLES, db};
use mysql::prelude::Queryable;
use mysql::{TxOpts, Value};
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use std::error::Error;
use std::path::PathBuf;

fn app_data_dir() -> PathBuf {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let base = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else if cfg!(windows) {
        PathBuf::from(std::env::var("APPDATA").unwrap_or_default())
    } else {
        std::env::var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|_| home.join(".local/share"))
    };
    base.join("com.maker.ledger")
}

fn main() -> Result<(), Box<dyn Error>> {
    let src = std::env::args().nth(1).ok_or("用法:import_sqlite <旧的 ledger.db 路径>")?;
    let lite = Connection::open_with_flags(&src, OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    let cfg = db::load_config(&app_data_dir())?;
    let pool = db::connect(&cfg)?;
    let mut my = pool.get_conn()?;

    for t in ["account", "valuation", "txn", "budget", "chat_message", "setting"] {
        let n: i64 = my.query_first(format!("SELECT COUNT(*) FROM `{t}`"))?.unwrap_or(0);
        if n > 0 {
            return Err(format!("MySQL 的 {t} 表里已有 {n} 条数据,为避免覆盖已中止。").into());
        }
    }

    // 先在内存里读完并校验,再一次性写入
    let mut all: Vec<Vec<Vec<Value>>> = Vec::new();
    for (name, cols, _) in TABLES {
        let have: Vec<String> = {
            let mut st = lite.prepare(&format!("SELECT name FROM pragma_table_info('{name}')"))?;
            let names = st.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<_, _>>()?;
            names
        };
        if have.is_empty() {
            all.push(Vec::new()); // 旧库没有这张表(例如还没有 setting)
            continue;
        }
        let select: Vec<String> = cols
            .iter()
            .map(|c| if have.iter().any(|h| h == c) { format!("`{c}`") } else if *c == "created_at" { "0".into() } else { format!("NULL /* 缺列 {c} */") })
            .collect();
        let mut st = lite.prepare(&format!("SELECT {} FROM `{name}` ORDER BY 1", select.join(",")))?;
        let rows = st
            .query_map([], |r| {
                (0..cols.len())
                    .map(|i| {
                        Ok(match r.get_ref(i)? {
                            ValueRef::Null => Value::NULL,
                            ValueRef::Integer(n) => Value::Int(n),
                            ValueRef::Text(b) => Value::Bytes(b.to_vec()),
                            other => return Err(rusqlite::Error::InvalidColumnType(i, format!("{name}.{}", cols[i]), other.data_type())),
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
            })?
            .collect::<Result<Vec<_>, _>>()?;
        all.push(rows);
    }

    let mut tx = my.start_transaction(TxOpts::default())?;
    // 默认分类由两边的建表脚本各自写入,编号可能不同;以旧库为准,账目才能对上分类
    for (name, _, _) in TABLES.iter().rev() {
        tx.query_drop(format!("DELETE FROM `{name}`"))?;
    }
    for ((name, cols, _), rows) in TABLES.iter().zip(all) {
        let n = rows.len();
        if n > 0 {
            let marks = vec!["?"; cols.len()].join(",");
            let list = cols.iter().map(|c| format!("`{c}`")).collect::<Vec<_>>().join(",");
            tx.exec_batch(format!("INSERT INTO `{name}` ({list}) VALUES ({marks})"), rows)?;
        }
        println!("{name:<13} 导入 {n} 行");
    }
    tx.commit()?;
    println!("完成。旧文件 {src} 未被修改,确认无误后可自行归档或删除。");
    Ok(())
}
