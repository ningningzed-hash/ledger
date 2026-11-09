//! 用户目录位于原数据库；每个后续用户拥有单独的业务数据库。
//! 会话只在进程内保留，所有业务命令必须凭会话取得固定用户的连接池。
use crate::{backup, core::{Error, Result}, db::{self, Conn, DbConfig}, profile};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::{SaltString, rand_core::OsRng}};
use mysql::{Pool, prelude::Queryable};
use rand::RngCore;
use serde::Serialize;
use std::{path::PathBuf, sync::Mutex, time::{Duration, Instant}};

const SESSION_TTL: Duration = Duration::from_secs(12 * 60 * 60);
const AUTH_ERROR: &str = "登录已失效，请重新登录";

#[derive(Clone, Serialize)]
pub struct User { pub id: u64, pub username: String }
#[derive(Serialize)]
pub struct Status { pub has_users: bool, pub has_legacy_data: bool }
#[derive(Serialize)]
pub struct Login {
    pub user: User,
    pub token: String,
    pub warning: Option<String>,
}
#[derive(Clone)]
struct Session { user: User, token: String, pool: Pool, expires: Instant }
#[derive(Default)]
struct Attempts { failures: u32, blocked_until: Option<Instant> }

pub struct Auth {
    cfg: DbConfig,
    directory: Pool,
    backup_root: PathBuf,
    session: Mutex<Option<Session>>,
    // 串行处理登录/注册/退出，防止迟到的登录响应重新激活已退出的会话。
    operations: Mutex<Attempts>,
    dummy_hash: String,
}

fn random_id() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn hash_password(password: &str) -> Result<String> {
    Argon2::default().hash_password(password.as_bytes(), &SaltString::generate(&mut OsRng))
        .map(|h| h.to_string()).map_err(|_| Error::Invalid("密码处理失败，请重试".into()))
}
fn verify(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
}
fn username(value: &str) -> Result<String> {
    let value = value.trim().to_ascii_lowercase();
    if !(3..=32).contains(&value.len()) || !value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Err(Error::Invalid("用户名需为 3–32 位字母、数字、下划线或短横线".into()));
    }
    Ok(value)
}
fn validate_password(value: &str) -> Result<()> {
    if value.chars().count() < 8 || value.len() > 128 || value.trim().is_empty() {
        return Err(Error::Invalid("密码至少 8 个字符，最多 128 字节，不能全为空格".into()));
    }
    Ok(())
}

impl Auth {
    pub fn new(cfg: DbConfig, backup_root: PathBuf) -> Result<Self> {
        if cfg.database.len() > 42 || !cfg.database.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return Err(Error::Invalid("数据库名需为不超过 42 位的字母、数字或下划线".into()));
        }
        let directory = db::connect(&cfg)?;
        directory.get_conn()?.query_drop("CREATE TABLE IF NOT EXISTS auth_user (
            id BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
            username VARCHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL UNIQUE,
            password_hash VARCHAR(255) NOT NULL,
            database_name VARCHAR(64) NOT NULL UNIQUE,
            created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4")?;
        Ok(Self { cfg, directory, backup_root, session: Mutex::new(None), operations: Mutex::new(Attempts::default()), dummy_hash: hash_password(&random_id())? })
    }

    fn legacy_data(c: &mut Conn) -> Result<bool> {
        for (table, _, _) in backup::TABLES {
            // 内置分类、产品不算旧账本数据。
            if table == "category" { continue; }
            let filter = match table {
                "product" => " WHERE builtin=0",
                "setting" => " WHERE name NOT IN ('product_seeded','product_seeded_asia')",
                _ => "",
            };
            let found: Option<u8> = c.query_first(format!("SELECT 1 FROM `{table}`{filter} LIMIT 1"))?;
            if found.is_some() { return Ok(true); }
        }
        Ok(false)
    }

    pub fn status(&self) -> Result<Status> {
        let mut c = self.directory.get_conn()?;
        let has_users = c.query_first::<u8, _>("SELECT 1 FROM auth_user LIMIT 1")?.is_some();
        Ok(Status { has_users, has_legacy_data: !has_users && Self::legacy_data(&mut c)? })
    }

    fn install(&self, user: User, pool: Pool) -> Result<Login> {
        let token = random_id();
        let warning = match pool.get_conn() {
            Ok(mut c) => backup::auto_backup(&mut c, &self.backup_root.join(format!("user-{}", user.id))).err().map(|e| format!("已登录，但自动备份失败：{e}")),
            Err(e) => Some(format!("已登录，但自动备份失败：{e}")),
        };
        *self.session.lock().unwrap() = Some(Session { user: user.clone(), token: token.clone(), pool, expires: Instant::now() + SESSION_TTL });
        Ok(Login { user, token, warning })
    }

    pub fn register(&self, name: &str, password: &str, claim_legacy: bool) -> Result<Login> {
        let name = username(name)?;
        validate_password(password)?;
        let _guard = self.operations.lock().unwrap();
        let hash = hash_password(password)?;
        let mut c = self.directory.get_conn()?;
        // 同时打开两个应用实例时，也只能有一个用户接管旧账本。
        let lock = format!("{}:register", self.cfg.database);
        let acquired: Option<u8> = c.exec_first("SELECT GET_LOCK(?, 10)", (&lock,))?;
        if acquired != Some(1) { return Err(Error::Invalid("另一位用户正在注册，请稍后重试".into())); }
        let result = (|| {
            if c.exec_first::<u64, _, _>("SELECT id FROM auth_user WHERE username=?", (&name,))?.is_some() {
                return Err(Error::Invalid("这个用户名已注册，请登录或换一个用户名".into()));
            }
            let first = c.query_first::<u8, _>("SELECT 1 FROM auth_user LIMIT 1")?.is_none();
            let legacy = first && Self::legacy_data(&mut c)?;
            if legacy && !claim_legacy {
                return Err(Error::Invalid("检测到已有账本，请确认将现有数据归入这个首个用户".into()));
            }
            let database = if first { self.cfg.database.clone() } else { format!("{}_u_{}", self.cfg.database, &random_id()[..16]) };
            if !first {
                c.query_drop(format!("CREATE DATABASE `{database}` CHARACTER SET utf8mb4"))
                    .map_err(|_| Error::Db("无法创建用户账本，请运行 scripts/setup-mysql.sh 补齐用户数据库权限后重试".into()))?;
            }
            let pool = if first { self.directory.clone() } else { db::connect(&DbConfig { database: database.clone(), ..self.cfg.clone() })? };
            if !legacy {
                profile::save(&mut pool.get_conn()?, profile::Profile { nickname: name.clone(), ..Default::default() })?;
            }
            c.exec_drop("INSERT INTO auth_user (username,password_hash,database_name) VALUES (?,?,?)", (&name, hash, database))?;
            let user = User { id: c.last_insert_id(), username: name };
            self.install(user, pool)
        })();
        // GET_LOCK 随物理连接保留，归还连接池前必须显式释放。
        let _ = c.exec_drop("DO RELEASE_LOCK(?)", (&lock,));
        result
    }

    pub fn login(&self, name: &str, password: &str) -> Result<Login> {
        let mut attempts = self.operations.lock().unwrap();
        if attempts.blocked_until.is_some_and(|until| until > Instant::now()) {
            return Err(Error::Invalid("尝试次数过多，请 30 秒后重试".into()));
        }
        if password.len() > 128 { return Err(Error::Invalid("用户名或密码不正确".into())); }
        let name = name.trim().to_ascii_lowercase();
        let row: Option<(u64, String, String)> = self.directory.get_conn()?.exec_first(
            "SELECT id,password_hash,database_name FROM auth_user WHERE username=?", (&name,))?;
        let valid = verify(password, row.as_ref().map(|r| r.1.as_str()).unwrap_or(&self.dummy_hash));
        if !valid || row.is_none() {
            attempts.failures += 1;
            if attempts.failures >= 5 {
                attempts.blocked_until = Some(Instant::now() + Duration::from_secs(30));
                attempts.failures = 0;
            }
            return Err(Error::Invalid("用户名或密码不正确".into()));
        }
        let (id, _, database) = row.unwrap();
        // 数据库名只能由注册流程生成，绝不接受来自客户端的数据库名/用户 ID。
        if database != self.cfg.database && !database.starts_with(&format!("{}_u_", self.cfg.database)) {
            return Err(Error::Invalid("用户账本配置无效".into()));
        }
        let pool = db::connect(&DbConfig { database, ..self.cfg.clone() })?;
        *attempts = Attempts::default();
        self.install(User { id, username: name }, pool)
    }

    pub fn pool(&self, token: &str) -> Result<Pool> {
        let guard = self.session.lock().unwrap();
        let s = guard.as_ref().filter(|s| !token.is_empty() && s.token == token && s.expires > Instant::now())
            .ok_or_else(|| Error::Auth(AUTH_ERROR.into()))?;
        Ok(s.pool.clone())
    }
    pub fn resume(&self, token: &str) -> Result<Login> {
        let guard = self.session.lock().unwrap();
        let s = guard.as_ref().filter(|s| s.token == token && s.expires > Instant::now())
            .ok_or_else(|| Error::Auth(AUTH_ERROR.into()))?;
        Ok(Login { user: s.user.clone(), token: s.token.clone(), warning: None })
    }
    pub fn logout(&self, token: &str) -> Result<()> {
        let _guard = self.operations.lock().unwrap();
        let mut session = self.session.lock().unwrap();
        // 旧窗口发来的延迟退出请求不能注销新用户。
        if session.as_ref().is_some_and(|s| s.token == token) { *session = None; }
        Ok(())
    }
    pub fn change_password(&self, token: &str, current: &str, new: &str) -> Result<()> {
        validate_password(new)?;
        let _guard = self.operations.lock().unwrap();
        let login = self.resume(token)?;
        let mut c = self.directory.get_conn()?;
        let hash: String = c.exec_first("SELECT password_hash FROM auth_user WHERE id=?", (login.user.id,))?
            .ok_or_else(|| Error::Auth(AUTH_ERROR.into()))?;
        if !verify(current, &hash) { return Err(Error::Invalid("当前密码不正确".into())); }
        c.exec_drop("UPDATE auth_user SET password_hash=? WHERE id=?", (hash_password(new)?, login.user.id))?;
        *self.session.lock().unwrap() = None;
        Ok(())
    }
    pub fn active(&self) -> Option<(String, Pool)> {
        self.session.lock().unwrap().as_ref().filter(|s| s.expires > Instant::now()).map(|s| (s.token.clone(), s.pool.clone()))
    }
    // 调用方持锁发送通知，切换/退出后不会继续暴露上一个用户的提醒。
    pub fn if_current(&self, token: &str, f: impl FnOnce()) {
        let guard = self.session.lock().unwrap();
        if guard.as_ref().is_some_and(|s| s.token == token && s.expires > Instant::now()) { f(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{core, testdb::TestDb};
    struct Fixture { auth: Auth, db: TestDb, _dir: tempfile::TempDir }
    impl Fixture {
        fn new() -> Self {
            let db = TestDb::new();
            let dir = tempfile::tempdir().unwrap();
            let auth = Auth::new(db.config(), dir.path().into()).unwrap();
            Self { auth, db, _dir: dir }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let mut c = self.db.conn();
            let names: Vec<String> = c.query("SELECT database_name FROM auth_user").unwrap();
            for name in names {
                if name != self.db.name {
                    assert!(name.starts_with(&format!("{}_u_", self.db.name)));
                    c.query_drop(format!("DROP DATABASE `{name}`")).unwrap();
                }
            }
        }
    }
    #[test]
    fn users_are_isolated_and_old_sessions_cannot_access_or_switch_ledgers() {
        let f = Fixture::new();
        assert!(f.auth.pool("").is_err());
        let a = f.auth.register("alice", "alice-password", false).unwrap();
        let pool_a = f.auth.pool(&a.token).unwrap();
        let mut ca = pool_a.get_conn().unwrap();
        let id = core::add_account(&mut ca, "ALICE-PRIVATE", "cash", "CNY", 90000, "2026-11-08").unwrap();
        core::add_txn(&mut ca, Some(id), None, "expense", 100, "2026-11-08", "私有账目").unwrap();
        core::add_chat(&mut ca, "user", "ALICE-CHAT").unwrap();
        let mut settings = core::get_settings(&mut ca).unwrap(); settings.llm_api_key = "ALICE-KEY".into(); core::save_settings(&mut ca, &settings).unwrap();
        let b = f.auth.register("bob", "bob-password", false).unwrap();
        let mut cb = f.auth.pool(&b.token).unwrap().get_conn().unwrap();
        assert!(core::list_accounts(&mut cb).unwrap().is_empty());
        assert!(core::list_txns(&mut cb, "2026-11").unwrap().is_empty());
        assert!(core::list_chat(&mut cb).unwrap().is_empty());
        assert!(core::get_settings(&mut cb).unwrap().llm_api_key.is_empty());
        assert_eq!(profile::get(&mut cb).unwrap().nickname, "bob");
        assert!(!backup::dump(&mut cb).unwrap().to_string().contains("ALICE"));
        assert!(f.auth.pool(&a.token).is_err());
        assert!(f.auth.pool("forged-token").is_err());
        f.auth.logout(&a.token).unwrap();
        assert!(f.auth.pool(&b.token).is_ok());
        // A 已开始的后台任务仍绑定 A 的数据库，不能将结果写给 B。
        core::add_chat(&mut pool_a.get_conn().unwrap(), "assistant", "LATE-ALICE").unwrap();
        assert!(core::list_chat(&mut cb).unwrap().is_empty());
        f.auth.logout(&b.token).unwrap(); assert!(f.auth.pool(&b.token).is_err());
        let a2 = f.auth.login("ALICE", "alice-password").unwrap();
        let mut c = f.auth.pool(&a2.token).unwrap().get_conn().unwrap();
        assert_eq!(core::list_accounts(&mut c).unwrap()[0].name, "ALICE-PRIVATE");
        assert_eq!(core::list_chat(&mut c).unwrap().len(), 2);
        assert!(f._dir.path().join(format!("user-{}", a.user.id)).is_dir());
        assert!(f._dir.path().join(format!("user-{}", b.user.id)).is_dir());
    }
    #[test]
    fn legacy_claim_password_validation_and_expiry() {
        let f = Fixture::new();
        core::add_account(&mut f.db.conn(), "旧账本", "cash", "CNY", 12345, "2026-11-08").unwrap();
        assert!(f.auth.status().unwrap().has_legacy_data);
        assert!(f.auth.register("owner", "good-password", false).is_err());
        assert!(f.auth.register("x", "good-password", true).is_err());
        assert!(f.auth.register("owner", "short", true).is_err());
        let a = f.auth.register("Owner", "good-password", true).unwrap();
        assert_eq!(core::list_accounts(&mut f.auth.pool(&a.token).unwrap().get_conn().unwrap()).unwrap()[0].balance, 12345);
        assert!(f.auth.register("OWNER", "other-password", true).is_err());
        assert!(f.auth.login("owner", "wrong").is_err());
        assert!(f.auth.login("unknown", "wrong").is_err());
        let hash: String = f.db.conn().query_first("SELECT password_hash FROM auth_user").unwrap().unwrap();
        assert!(hash.starts_with("$argon2id$")); assert!(!hash.contains("good-password"));
        assert!(!backup::dump(&mut f.db.conn()).unwrap().to_string().contains("password_hash"));
        assert!(f.auth.change_password(&a.token, "wrong", "new-password").is_err());
        f.auth.change_password(&a.token, "good-password", "new-password").unwrap();
        assert!(f.auth.resume(&a.token).is_err());
        assert!(f.auth.login("owner", "good-password").is_err());
        let new = f.auth.login("owner", "new-password").unwrap();
        f.auth.session.lock().unwrap().as_mut().unwrap().expires = Instant::now() - Duration::from_secs(1);
        assert!(f.auth.pool(&new.token).is_err()); assert!(f.auth.resume(&new.token).is_err());
        assert!(f.auth.active().is_none());
    }
    #[test]
    fn repeated_failed_login_is_throttled() {
        let f = Fixture::new();
        f.auth.register("owner", "good-password", false).unwrap();
        for _ in 0..5 { assert!(f.auth.login("owner", "wrong").is_err()); }
        assert!(f.auth.login("owner", "good-password").err().unwrap().to_string().contains("30 秒"));
    }
}
