//! 本地账本使用者的资料与偏好，随 setting 表一起备份。
use crate::core::{Error, Result};
use mysql::prelude::Queryable;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub nickname: String,
    pub email: String,
    pub bio: String,
    pub theme: String,
    pub start_page: String,
}

impl Default for Profile {
    fn default() -> Self {
        Self { nickname: "账本主人".into(), email: String::new(), bio: String::new(), theme: "system".into(), start_page: "home".into() }
    }
}

impl Profile {
    fn validated(mut self) -> Result<Self> {
        self.nickname = self.nickname.trim().into();
        self.email = self.email.trim().into();
        self.bio = self.bio.trim().into();
        if self.nickname.is_empty() || self.nickname.chars().count() > 30 {
            return Err(Error::Invalid("昵称需为 1–30 个字符".into()));
        }
        if self.bio.chars().count() > 200 {
            return Err(Error::Invalid("个人简介不能超过 200 个字符".into()));
        }
        if !self.email.is_empty() {
            let parts: Vec<_> = self.email.split('@').collect();
            if self.email.len() > 254 || self.email.chars().any(char::is_whitespace)
                || parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
                return Err(Error::Invalid("请输入有效的邮箱地址".into()));
            }
        }
        if !["system", "light", "dark"].contains(&self.theme.as_str()) {
            return Err(Error::Invalid("外观设置无效".into()));
        }
        if !["home", "txn", "asset", "budget", "goal", "sub", "report", "invest", "capabilities", "profile"].contains(&self.start_page.as_str()) {
            return Err(Error::Invalid("默认首页无效".into()));
        }
        Ok(self)
    }
}

pub fn get(c: &mut impl Queryable) -> Result<Profile> {
    let raw: Option<String> = c.exec_first("SELECT value FROM setting WHERE name='user_profile'", ())?;
    match raw {
        None => Ok(Profile::default()),
        Some(raw) => serde_json::from_str::<Profile>(&raw)
            .map_err(|_| Error::Invalid("个人资料读取失败，请检查数据库中的 user_profile 设置".into()))?
            .validated(),
    }
}

pub fn save(c: &mut impl Queryable, profile: Profile) -> Result<Profile> {
    let profile = profile.validated()?;
    let value = serde_json::to_string(&profile).map_err(|e| Error::Invalid(e.to_string()))?;
    c.exec_drop("INSERT INTO setting (name,value) VALUES ('user_profile',?) AS new ON DUPLICATE KEY UPDATE value=new.value", (value,))?;
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdb::TestDb;

    #[test]
    fn profile_persists_and_invalid_updates_preserve_saved_data() {
        let db = TestDb::new();
        let mut c = db.conn();
        assert_eq!(get(&mut c).unwrap(), Profile::default());
        let p = Profile { nickname: " 宁宁 ".into(), email: "ning@example.com".into(), bio: "慢慢积累".into(), theme: "dark".into(), start_page: "invest".into() };
        let saved = save(&mut c, p).unwrap();
        assert_eq!(saved.nickname, "宁宁");
        assert_eq!(get(&mut db.conn()).unwrap(), saved);
        for bad in [
            Profile { nickname: " ".into(), ..saved.clone() },
            Profile { nickname: "字".repeat(31), ..saved.clone() },
            Profile { email: "not-an-email".into(), ..saved.clone() },
            Profile { email: "a@b@c".into(), ..saved.clone() },
            Profile { bio: "字".repeat(201), ..saved.clone() },
            Profile { theme: "invalid".into(), ..saved.clone() },
            Profile { start_page: "invalid".into(), ..saved.clone() },
        ] {
            assert!(save(&mut c, bad).is_err());
            assert_eq!(get(&mut c).unwrap(), saved);
        }
        let dump = crate::backup::dump(&mut c).unwrap();
        assert!(dump["tables"]["setting"].as_array().unwrap().iter().any(|s| s["name"] == "user_profile"));
    }
}
