//! MySQL 连接与表结构。
//!
//! 连接信息保存在应用数据目录的 `database.json`(权限 600),也可用环境变量
//! `LEDGER_DB_URL`(形如 mysql://user:pass@host:3306/ledger)覆盖。
use mysql::prelude::Queryable;
use mysql::{Opts, OptsBuilder, Pool, PoolConstraints, PoolOpts, PooledConn};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::core::{Error, Result};

pub type Conn = PooledConn;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DbConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub database: String,
}

impl Default for DbConfig {
    fn default() -> Self {
        Self { host: "127.0.0.1".into(), port: 3306, user: "ledger".into(), password: String::new(), database: "ledger".into() }
    }
}

impl DbConfig {
    /// 不含密码的描述,用于界面展示
    pub fn describe(&self) -> String {
        format!("mysql://{}@{}:{}/{}", self.user, self.host, self.port, self.database)
    }

    pub fn opts(&self, with_db: bool) -> OptsBuilder {
        OptsBuilder::new()
            .ip_or_hostname(Some(self.host.clone()))
            .tcp_port(self.port)
            .user(Some(self.user.clone()))
            .pass(Some(self.password.clone()))
            .db_name(if with_db { Some(self.database.clone()) } else { None })
            .tcp_connect_timeout(Some(Duration::from_secs(3)))
            .init(vec!["SET NAMES utf8mb4".to_string()])
            .pool_opts(PoolOpts::default().with_constraints(PoolConstraints::new(1, 4).expect("valid constraints")))
    }
}

pub fn config_path(dir: &Path) -> PathBuf {
    dir.join("database.json")
}

pub fn config_from_url(url: &str) -> Result<DbConfig> {
    let opts = Opts::from_url(url).map_err(|e| Error::Invalid(format!("数据库地址格式不正确:{e}")))?;
    Ok(DbConfig {
        host: opts.get_ip_or_hostname().to_string(),
        port: opts.get_tcp_port(),
        user: opts.get_user().unwrap_or("ledger").to_string(),
        password: opts.get_pass().unwrap_or("").to_string(),
        database: opts.get_db_name().unwrap_or("ledger").to_string(),
    })
}

pub fn load_config(dir: &Path) -> Result<DbConfig> {
    if let Ok(url) = std::env::var("LEDGER_DB_URL") {
        return config_from_url(&url);
    }
    let path = config_path(dir);
    let text = std::fs::read_to_string(&path).map_err(|_| {
        Error::Db(format!("未找到数据库配置 {}。请先运行 scripts/setup-mysql.sh 创建数据库并生成配置。", path.display()))
    })?;
    serde_json::from_str(&text).map_err(|e| Error::Db(format!("数据库配置 {} 格式不正确:{e}", path.display())))
}

/// 建立连接池并确保表结构存在
pub fn connect(cfg: &DbConfig) -> Result<Pool> {
    let unreachable = |e: mysql::Error| {
        Error::Db(format!(
            "无法连接 MySQL({}):{e}。请确认 MySQL 已启动(brew services start mysql),并检查用户名、密码与数据库是否存在。",
            cfg.describe()
        ))
    };
    let pool = Pool::new(cfg.opts(true)).map_err(unreachable)?;
    let mut conn = pool.get_conn().map_err(unreachable)?;
    migrate(&mut conn)?;
    crate::portfolio::seed_products(&mut conn)?;
    Ok(pool)
}

/// 建表。所有金额为整数"分";日期/月份存为文本(YYYY-MM-DD / YYYY-MM),便于直接比较。
const STATEMENTS: [&str; 21] = [
    "CREATE TABLE IF NOT EXISTS account (
        id        BIGINT PRIMARY KEY AUTO_INCREMENT,
        name      VARCHAR(255) NOT NULL,
        kind      VARCHAR(16)  NOT NULL,           -- cash|deposit|property|investment|liability
        currency  VARCHAR(8)   NOT NULL DEFAULT 'CNY',
        archived  TINYINT      NOT NULL DEFAULT 0
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    // 估值快照。created_at 是单调递增时间戳,用来判断某笔账目是在估值之前还是之后录入
    "CREATE TABLE IF NOT EXISTS valuation (
        id         BIGINT PRIMARY KEY AUTO_INCREMENT,
        account_id BIGINT      NOT NULL,
        date       VARCHAR(10) NOT NULL,
        amount     BIGINT      NOT NULL,
        created_at BIGINT      NOT NULL DEFAULT 0,
        UNIQUE KEY uq_valuation_day (account_id, date),
        CONSTRAINT fk_valuation_account FOREIGN KEY (account_id) REFERENCES account(id) ON DELETE CASCADE
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    "CREATE TABLE IF NOT EXISTS category (
        id   BIGINT PRIMARY KEY AUTO_INCREMENT,
        name VARCHAR(64) NOT NULL UNIQUE,
        kind VARCHAR(16) NOT NULL                  -- expense|income
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    "CREATE TABLE IF NOT EXISTS txn (
        id          BIGINT PRIMARY KEY AUTO_INCREMENT,
        account_id  BIGINT NULL,
        category_id BIGINT NULL,
        kind        VARCHAR(16) NOT NULL,          -- expense|income
        amount      BIGINT      NOT NULL,
        date        VARCHAR(10) NOT NULL,
        note        TEXT        NOT NULL,
        created_at  BIGINT      NOT NULL DEFAULT 0,
        KEY idx_txn_date (date),
        KEY idx_txn_account (account_id),
        CONSTRAINT chk_txn_amount CHECK (amount > 0),
        CONSTRAINT fk_txn_account  FOREIGN KEY (account_id)  REFERENCES account(id),
        CONSTRAINT fk_txn_category FOREIGN KEY (category_id) REFERENCES category(id)
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    // 月度预算:category_id 为空表示总预算
    "CREATE TABLE IF NOT EXISTS budget (
        id          BIGINT PRIMARY KEY AUTO_INCREMENT,
        month       VARCHAR(7) NOT NULL,
        category_id BIGINT NULL,
        amount      BIGINT NOT NULL,
        UNIQUE KEY uq_budget (month, category_id),
        CONSTRAINT fk_budget_category FOREIGN KEY (category_id) REFERENCES category(id)
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    "CREATE TABLE IF NOT EXISTS chat_message (
        id         BIGINT PRIMARY KEY AUTO_INCREMENT,
        role       VARCHAR(16) NOT NULL,           -- user|assistant
        content    MEDIUMTEXT  NOT NULL,
        created_at VARCHAR(40) NOT NULL
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    "CREATE TABLE IF NOT EXISTS setting (
        name  VARCHAR(64) PRIMARY KEY,
        value MEDIUMTEXT NOT NULL
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    // 目标:存钱(账户余额达到目标金额)或还债(负债降到目标金额)。start_amount 只用于还债,
    // 记录建立目标时的欠款,用来算"已还了多少"
    "CREATE TABLE IF NOT EXISTS goal (
        id           BIGINT PRIMARY KEY AUTO_INCREMENT,
        name         VARCHAR(100) NOT NULL,
        kind         VARCHAR(16)  NOT NULL,        -- save|payoff
        target       BIGINT       NOT NULL,
        start_amount BIGINT       NOT NULL DEFAULT 0,
        deadline     VARCHAR(10)  NULL,
        created_on   VARCHAR(10)  NOT NULL
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    "CREATE TABLE IF NOT EXISTS goal_account (
        goal_id    BIGINT NOT NULL,
        account_id BIGINT NOT NULL,
        PRIMARY KEY (goal_id, account_id),
        CONSTRAINT fk_goal_account_goal    FOREIGN KEY (goal_id)    REFERENCES goal(id)    ON DELETE CASCADE,
        CONSTRAINT fk_goal_account_account FOREIGN KEY (account_id) REFERENCES account(id) ON DELETE CASCADE
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    // 订阅(周期性收支)。到期的账目由程序自动记入 txn;schedule 由 frequency + start_date 决定,
    // 月度按 start_date 的"日"、年度按 start_date 的"月日"重复,当月没有那一天就取月底
    "CREATE TABLE IF NOT EXISTS recurring (
        id          BIGINT PRIMARY KEY AUTO_INCREMENT,
        name        VARCHAR(100) NOT NULL,
        kind        VARCHAR(16)  NOT NULL,         -- expense|income
        amount      BIGINT       NOT NULL,
        frequency   VARCHAR(16)  NOT NULL,         -- monthly|yearly
        start_date  VARCHAR(10)  NOT NULL,
        end_date    VARCHAR(10)  NULL,
        paused      TINYINT      NOT NULL DEFAULT 0,
        category_id BIGINT NULL,
        account_id  BIGINT NULL,
        CONSTRAINT fk_recurring_category FOREIGN KEY (category_id) REFERENCES category(id),
        CONSTRAINT fk_recurring_account  FOREIGN KEY (account_id)  REFERENCES account(id)
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    // 每个(订阅, 日期)只会处理一次:主键保证不会重复入账。txn_id 为空表示这一期被跳过
    // (暂停或结束期间)或者入账的账目后来被用户删除了——两种情况都不应再补记
    "CREATE TABLE IF NOT EXISTS recurring_run (
        recurring_id BIGINT      NOT NULL,
        due_date     VARCHAR(10) NOT NULL,
        txn_id       BIGINT      NULL,
        PRIMARY KEY (recurring_id, due_date),
        CONSTRAINT fk_run_recurring FOREIGN KEY (recurring_id) REFERENCES recurring(id) ON DELETE CASCADE,
        CONSTRAINT fk_run_txn       FOREIGN KEY (txn_id)       REFERENCES txn(id)       ON DELETE SET NULL
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    // 账户年利率(基点:3.5% 存 350)。单独成表,不改 account 的结构
    "CREATE TABLE IF NOT EXISTS account_rate (
        account_id BIGINT PRIMARY KEY,
        rate_bps   INT NOT NULL,
        CONSTRAINT chk_rate_bps CHECK (rate_bps BETWEEN 0 AND 10000),
        CONSTRAINT fk_rate_account FOREIGN KEY (account_id) REFERENCES account(id) ON DELETE CASCADE
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    // 持仓:挂在投资类账户下,金额(分)。登记了份额和行情来源(secid)的持仓,金额由程序按最新价自动更新;
    // shares 为份额 × 10000;secid 为行情接口的"市场.代码",场外基金写作 OF.代码
    "CREATE TABLE IF NOT EXISTS holding (
        id          BIGINT PRIMARY KEY AUTO_INCREMENT,
        account_id  BIGINT       NOT NULL,
        name        VARCHAR(100) NOT NULL,
        code        VARCHAR(32)  NULL,
        asset_class VARCHAR(24)  NOT NULL,
        amount      BIGINT       NOT NULL,
        updated_on  VARCHAR(10)  NOT NULL,
        shares      BIGINT       NULL,
        secid       VARCHAR(32)  NULL,
        KEY idx_holding_account (account_id),
        CONSTRAINT chk_holding_amount CHECK (amount > 0),
        CONSTRAINT fk_holding_account FOREIGN KEY (account_id) REFERENCES account(id) ON DELETE CASCADE
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    // 候选产品清单:只是参考,不记渠道、地区,也不存费率等会过期的数据
    "CREATE TABLE IF NOT EXISTS product (
        id          BIGINT PRIMARY KEY AUTO_INCREMENT,
        name        VARCHAR(100) NOT NULL UNIQUE,
        code        VARCHAR(32)  NULL,
        asset_class VARCHAR(24)  NOT NULL,
        note        VARCHAR(500) NOT NULL DEFAULT '',
        builtin     TINYINT      NOT NULL DEFAULT 0
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    // 自选行情:只存代码,价格每次实时取。secid 是行情接口的"市场.代码"(如 1.600519、105.AAPL)
    "CREATE TABLE IF NOT EXISTS watchlist (
        id    BIGINT PRIMARY KEY AUTO_INCREMENT,
        secid VARCHAR(32)  NOT NULL UNIQUE,
        code  VARCHAR(16)  NOT NULL,
        name  VARCHAR(100) NOT NULL
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    // 账单导入:每个(来源, 交易单号)只导入一次。txn_id 为空表示导入的账目后来被删了——也不再重新导入
    "CREATE TABLE IF NOT EXISTS import_ref (
        source      VARCHAR(16)  NOT NULL,         -- wechat|alipay
        ref_id      VARCHAR(191) NOT NULL,
        txn_id      BIGINT       NULL,
        imported_on VARCHAR(10)  NOT NULL,
        PRIMARY KEY (source, ref_id),
        CONSTRAINT fk_import_txn FOREIGN KEY (txn_id) REFERENCES txn(id) ON DELETE SET NULL
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    // 导入时学到的规则:交易对方 → 分类(kind=counterparty)、支付方式 → 账户(kind=method)
    "CREATE TABLE IF NOT EXISTS import_rule (
        kind      VARCHAR(16)  NOT NULL,
        pattern   VARCHAR(150) NOT NULL,
        target_id BIGINT       NOT NULL,
        PRIMARY KEY (kind, pattern)
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    // 保存下来的月度复盘(每月一份,重新保存即覆盖)
    "CREATE TABLE IF NOT EXISTS review (
        month      VARCHAR(7)  PRIMARY KEY,
        content    MEDIUMTEXT  NOT NULL,
        created_at VARCHAR(16) NOT NULL
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    // 持仓盈亏快照:每天一份,value / cost 为当天能算出盈亏的持仓的总市值、总成本(人民币,分)
    "CREATE TABLE IF NOT EXISTS holding_snapshot (
        date  VARCHAR(10) PRIMARY KEY,
        value BIGINT      NOT NULL,
        cost  BIGINT      NOT NULL
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    // 涨跌过大提醒:每只证券每个行情日最多提醒一次。只是运行状态,不进备份
    "CREATE TABLE IF NOT EXISTS alert_sent (
        secid     VARCHAR(32) NOT NULL,
        quote_day VARCHAR(10) NOT NULL,
        PRIMARY KEY (secid, quote_day)
    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    "INSERT IGNORE INTO category (name, kind) VALUES
        ('餐饮','expense'),('交通','expense'),('购物','expense'),('居住','expense'),
        ('娱乐','expense'),('医疗','expense'),('其他','expense'),
        ('工资','income'),('理财收益','income'),('其他收入','income')",
];

/// 后来加到已有表上的列:(表, 列, 定义)。MySQL 不支持 ADD COLUMN IF NOT EXISTS,先查 information_schema
const ADDED_COLUMNS: [(&str, &str, &str); 4] = [
    ("holding", "shares", "BIGINT NULL"),
    ("holding", "secid", "VARCHAR(32) NULL"),
    // 成本价与最近一次估值的现价,都是证券交易币种的原币价格
    ("holding", "cost_price", "DOUBLE NULL"),
    ("holding", "price", "DOUBLE NULL"),
];

pub fn migrate(c: &mut Conn) -> Result<()> {
    for sql in STATEMENTS {
        c.query_drop(sql)?;
    }
    for (table, col, def) in ADDED_COLUMNS {
        let has: Option<i64> = c.exec_first(
            "SELECT 1 FROM information_schema.columns WHERE table_schema = DATABASE() AND table_name = ? AND column_name = ?",
            (table, col),
        )?;
        if has.is_none() {
            c.query_drop(format!("ALTER TABLE `{table}` ADD COLUMN `{col}` {def}"))?;
        }
    }
    Ok(())
}
