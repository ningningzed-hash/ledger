//! 测试辅助:每个测试在真实 MySQL 上建一个独立的临时库,结束时自动删除。
//!
//! 连接信息取自环境变量 `LEDGER_TEST_DB_URL`,否则读应用的 database.json
//! (先运行 scripts/setup-mysql.sh)。临时库名固定以 `ledger_test_` 开头,
//! 脚本为应用用户授予了这个前缀的库的权限,所以不会碰到真实数据。
use crate::db::{self, Conn, DbConfig};
use mysql::prelude::Queryable;
use mysql::{Pool, PoolConstraints, PoolOpts};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

pub const PREFIX: &str = "ledger_test_";

/// 与 Tauri 的 app_data_dir 一致:<系统数据目录>/<identifier>
pub fn app_data_dir() -> PathBuf {
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

fn server_config() -> DbConfig {
    let loaded = match std::env::var("LEDGER_TEST_DB_URL") {
        Ok(url) => db::config_from_url(&url),
        Err(_) => db::load_config(&app_data_dir()),
    };
    loaded
        .unwrap_or_else(|e| panic!("测试需要可用的 MySQL:{e}\n(运行 scripts/setup-mysql.sh,或设置 LEDGER_TEST_DB_URL)"))
}

pub struct TestDb {
    pub pool: Pool,
    pub name: String,
    cfg: DbConfig,
}

impl TestDb {
    pub fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let cfg = server_config();
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let name = format!("{PREFIX}{}_{}_{}", std::process::id(), nanos % 1_000_000_000, N.fetch_add(1, Ordering::SeqCst));

        let admin = Pool::new(cfg.opts(false)).expect("连接 MySQL 失败");
        admin
            .get_conn()
            .unwrap()
            .query_drop(format!("CREATE DATABASE `{name}` CHARACTER SET utf8mb4"))
            .expect("创建临时库失败(应用用户是否被授予 ledger_test_% 的权限?)");

        let test_cfg = DbConfig { database: name.clone(), ..cfg.clone() };
        let pool = Pool::new(test_cfg.opts(true).pool_opts(PoolOpts::default().with_constraints(PoolConstraints::new(1, 4).unwrap()))).unwrap();
        db::migrate(&mut pool.get_conn().unwrap()).unwrap();
        Self { pool, name, cfg }
    }

    pub fn config(&self) -> DbConfig {
        DbConfig { database: self.name.clone(), ..self.cfg.clone() }
    }

    pub fn conn(&self) -> Conn {
        self.pool.get_conn().unwrap()
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        assert!(self.name.starts_with(PREFIX));
        if let Ok(admin) = Pool::new(self.cfg.opts(false)) {
            if let Ok(mut c) = admin.get_conn() {
                let _ = c.query_drop(format!("DROP DATABASE IF EXISTS `{}`", self.name));
            }
        }
    }
}
