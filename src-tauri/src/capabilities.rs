//! Fixed capability catalog; settings belong to the authenticated user's database.
use crate::{core::{Error, Result}, db::Conn};
use mysql::prelude::Queryable;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Serialize)]
pub struct Capability {
    pub id: &'static str, pub name: &'static str, pub kind: &'static str,
    pub description: &'static str, pub source: &'static str, pub license: &'static str,
    pub version: &'static str, pub prompt: &'static str, pub instructions: &'static str,
    pub enabled: bool, pub probe: Option<Probe>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Probe { pub ok: bool, pub checked_at: String, pub tools: Vec<String>, pub message: String }

pub fn catalog() -> Vec<Capability> {
    [
        ("aktools", "AKTools 财经数据", "mcp", "A 股、基金与宏观数据。通过 AKShare 搜索接口、查看参数并查询数据。", "https://github.com/akfamily/aktools", "MIT", "0.1.4", "帮我查询沪深 300 ETF 的公开数据，先确认产品代码。", "只读财经接口；首次使用请测试连接。握手成功不代表所有行情上游均可访问。"),
        ("yfinance", "Yahoo Finance", "mcp", "海外股票与 ETF 的价格、基本信息和基金持仓数据。", "https://github.com/narumiruna/yfinance-mcp", "MIT", "0.14.0", "查询 VOO 的基金信息，注明来源和数据日期。", "只开放只读查询工具。数据由 Yahoo Finance 提供，受其数据使用条款约束。"),
        ("monthly-review", "月度复盘", "skill", "串联收支、预算、订阅与目标，形成有数据依据的月度复盘。", "Ledger 内置 · skills/monthly-review/SKILL.md", "项目内置", "1.0", "请执行本月月度复盘。", include_str!("../skills/monthly-review/SKILL.md")),
        ("asset-health", "资产健康体检", "skill", "检查应急金、债务、资产集中度与持仓估值，给出行动建议。", "Ledger 内置 · skills/asset-health/SKILL.md", "项目内置", "1.0", "请执行资产健康体检。", include_str!("../skills/asset-health/SKILL.md")),
        ("etf-compare", "ETF 对比", "skill", "使用已启用的 MCP 查询产品，比较费用、持仓与风险。", "Ledger 内置 · skills/etf-compare/SKILL.md", "项目内置", "1.0", "我想对比 ETF，请先询问产品代码和比较目的。", include_str!("../skills/etf-compare/SKILL.md")),
        ("bill-reconcile", "账单核对", "skill", "核对账目范围，查找疑似重复和异常支出，保留人工确认。", "Ledger 内置 · skills/bill-reconcile/SKILL.md", "项目内置", "1.0", "请帮我核对本月账单，检查疑似重复和异常。", include_str!("../skills/bill-reconcile/SKILL.md")),
    ].into_iter().map(|(id,name,kind,description,source,license,version,prompt,instructions)| Capability {
        id,name,kind,description,source,license,version,prompt,instructions,enabled:kind=="skill",probe:None
    }).collect()
}
fn read(c: &mut Conn, key: &str) -> Result<Option<String>> {
    Ok(c.exec_first("SELECT value FROM setting WHERE name=?", (key,))?)
}
fn write(c: &mut Conn, key: &str, value: &str) -> Result<()> {
    c.exec_drop("INSERT INTO setting(name,value) VALUES(?,?) ON DUPLICATE KEY UPDATE value=VALUES(value)", (key,value))?; Ok(())
}
pub fn list(c: &mut Conn) -> Result<Vec<Capability>> {
    let mut list = catalog();
    for item in &mut list {
        if let Some(v) = read(c, &format!("capability.{}.enabled",item.id))? { item.enabled = v == "true"; }
        item.probe = read(c, &format!("capability.{}.probe",item.id))?.and_then(|v|serde_json::from_str(&v).ok());
    }
    Ok(list)
}
pub fn set_enabled(c: &mut Conn, id: &str, enabled: bool) -> Result<()> {
    let item = list(c)?.into_iter().find(|i|i.id==id).ok_or_else(||Error::Invalid("未知能力".into()))?;
    if enabled && item.kind=="mcp" && !item.probe.is_some_and(|p|p.ok) { return Err(Error::Invalid("请先测试连接成功后再启用".into())); }
    write(c,&format!("capability.{id}.enabled"), if enabled {"true"} else {"false"})
}
pub fn save_probe(c: &mut Conn, id: &str, probe: &Probe) -> Result<()> {
    write(c,&format!("capability.{id}.probe"),&serde_json::to_string(probe).unwrap())
}
pub fn select_skill<'a>(items: &'a [Capability], explicit: Option<&str>, input: &str) -> Result<Option<&'a Capability>> {
    if let Some(id) = explicit {
        let item = items.iter().find(|i|i.id==id && i.enabled).ok_or_else(||Error::Invalid("能力未启用，请先在能力市场启用".into()))?;
        return Ok((item.kind=="skill").then_some(item));
    }
    let lower = input.to_lowercase();
    let id = if lower.contains("etf") && (input.contains("对比")||input.contains("比较")) {"etf-compare"}
        else if input.contains("复盘") {"monthly-review"}
        else if input.contains("体检") || input.contains("资产配置") {"asset-health"}
        else if input.contains("核对") || input.contains("对账") {"bill-reconcile"} else {""};
    Ok(items.iter().find(|i|i.id==id && i.enabled))
}
pub fn load_skill(items: &[Capability], id: &str) -> Result<Value> {
    let item=items.iter().find(|i|i.id==id && i.kind=="skill" && i.enabled).ok_or_else(||Error::Invalid("Skill 不存在或未启用".into()))?;
    Ok(json!({"skill":item.name,"instructions":item.instructions,"source":item.source}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdb::TestDb;
    #[test]
    fn capability_settings_and_probe_are_user_isolated() {
        let a=TestDb::new();let b=TestDb::new();let mut c=a.conn();
        assert!(set_enabled(&mut c,"aktools",true).is_err());
        save_probe(&mut c,"aktools",&Probe {ok:true,checked_at:"2026-11-08".into(),tools:vec!["search_interfaces".into()],message:"test".into()}).unwrap();
        set_enabled(&mut c,"aktools",true).unwrap();
        set_enabled(&mut c,"monthly-review",false).unwrap();
        let first=list(&mut c).unwrap();let second=list(&mut b.conn()).unwrap();
        assert!(first[0].enabled);assert!(!first[2].enabled);
        assert!(!second[0].enabled);assert!(second[0].probe.is_none());assert!(second[2].enabled);
        assert!(load_skill(&first,"monthly-review").is_err());
        assert!(select_skill(&first,Some("monthly-review"),"复盘").is_err());
        assert!(set_enabled(&mut c,"invented",true).is_err());
    }
    #[test]
    fn explicit_selection_and_disabled_skills_are_respected() {
        let mut items=catalog();
        assert_eq!(select_skill(&items,None,"帮我对比 ETF").unwrap().unwrap().id,"etf-compare");
        assert_eq!(select_skill(&items,Some("asset-health"),"复盘").unwrap().unwrap().id,"asset-health");
        items[2].enabled=false;
        assert!(select_skill(&items,None,"复盘").unwrap().is_none());
        assert!(load_skill(&items,"etf-compare").unwrap()["instructions"].as_str().unwrap().contains("get_data"));
    }
}
