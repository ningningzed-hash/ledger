//! Official MCP client, fixed executables, per-task processes and read-only allowlist.
use crate::{capabilities::Probe, core::{Error, Result}};
use rmcp::{ServiceExt, RoleClient, service::RunningService, transport::TokioChildProcess, model::CallToolRequestParams};
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};
use tokio::{process::Command, runtime::Runtime, time::timeout};

type Client = RunningService<RoleClient, ()>;
fn err(e: impl std::fmt::Display) -> Error { Error::Net(format!("MCP: {e}")) }
pub fn allowed(id: &str, name: &str) -> bool {
    match id {
        "aktools" => ["search_interfaces","describe_interface","get_data"].contains(&name),
        "yfinance" => ["yfinance_get_ticker_info","yfinance_get_price_history","yfinance_search","yfinance_get_fund_data","yfinance_get_ticker_news"].contains(&name),
        _ => false,
    }
}
fn executable(id: &str) -> Result<PathBuf> {
    let bin = match id { "aktools" => "aktools-mcp", "yfinance" => "yfmcp", _ => return Err(err("未知 MCP")) };
    let root = std::env::var_os("LEDGER_MCP_HOME").map(PathBuf::from).unwrap_or_else(||
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share/ledger/mcp"));
    let path = root.join(id).join(if cfg!(windows) {"Scripts"} else {"bin"}).join(if cfg!(windows) {format!("{bin}.exe")} else {bin.into()});
    if !path.is_file() { return Err(err("运行时未安装。请在项目目录执行 sh scripts/setup-mcp.sh（Python 3.12+），再测试连接。")); }
    Ok(path)
}
async fn connect(id: &str) -> Result<Client> {
    let mut command = Command::new(executable(id)?);
    command.kill_on_drop(true);
    let transport = TokioChildProcess::new(command).map_err(err)?;
    timeout(Duration::from_secs(40), ().serve(transport)).await.map_err(|_|err("连接超时（40 秒）"))?.map_err(err)
}
async fn discover(client: &Client, id: &str) -> Result<Vec<Value>> {
    // Single bounded page is sufficient for these pinned servers.
    let page = timeout(Duration::from_secs(15),client.list_tools(None)).await.map_err(|_|err("工具发现超时"))?.map_err(err)?;
    let list: Vec<Value> = page.tools.into_iter().filter(|t|allowed(id,&t.name)).take(12).map(|t|json!({"type":"function","function":{
        "name":format!("mcp_{id}__{}",t.name), "description":format!("[{id} 外部只读数据] {}",t.description.unwrap_or_default()),"parameters":t.input_schema
    }})).collect();
    if list.is_empty() { return Err(err("未发现允许使用的只读工具")); }
    Ok(list)
}
pub struct Session { clients: Vec<(String,Client)>, runtime: Runtime, pub tools: Vec<Value> }
impl Session {
    pub fn new() -> Result<Self> { Ok(Self {clients:vec![],runtime:Runtime::new().map_err(err)?,tools:vec![]}) }
    pub fn add(&mut self,id:&str) -> Result<usize> {
        self.runtime.block_on(async {
            let client=connect(id).await?;
            let defs=match discover(&client,id).await {Ok(d)=>d,Err(e)=>{let _=client.cancel().await;return Err(e)}};
            let n=defs.len(); self.tools.extend(defs); self.clients.push((id.into(),client)); Ok(n)
        })
    }
    pub fn call(&self,name:&str,args:&Value)->Result<Value> {
        if !self.tools.iter().any(|t|t["function"]["name"]==name) {return Err(err("工具未启用或不在允许列表"));}
        let (id,tool)=name.strip_prefix("mcp_").and_then(|s|s.split_once("__")).ok_or_else(||err("工具名不正确"))?;
        let client=&self.clients.iter().find(|(key,_)|key==id).ok_or_else(||err("服务未连接"))?.1;
        let arguments=args.as_object().cloned().ok_or_else(||err("参数必须为对象"))?;
        if args.to_string().len()>8192 {return Err(err("查询参数过长"));}
        let result=self.runtime.block_on(async {
            timeout(Duration::from_secs(45),client.call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(arguments))).await.map_err(|_|err("查询超时（45 秒）"))?.map_err(err)
        })?;
        let value=serde_json::to_value(&result).map_err(err)?;
        Ok(normalize_result(id, value, result.is_error.unwrap_or(false)))
    }
}
// Some MCP servers return a JSON error inside text while leaving isError=false.
fn normalize_result(id: &str, value: Value, protocol_error: bool) -> Value {
    let texts: Vec<&str> = value["content"].as_array().into_iter().flatten()
        .filter_map(|block|block["text"].as_str()).collect();
    let embedded_error = texts.iter().filter_map(|text|serde_json::from_str::<Value>(text).ok())
        .find_map(|v|v.get("error").filter(|e| !e.is_null()).cloned());
    let error = embedded_error.or_else(|| protocol_error.then(|| {
        json!(texts.first().copied().unwrap_or("MCP 数据源返回错误"))
    }));
    let raw = value.to_string();
    let data = if raw.len() > 24000 {
        json!({"truncated":true,"preview":raw.chars().take(6000).collect::<String>()})
    } else { value };
    let mut out = json!({"source":id,"queried_at":chrono::Utc::now().to_rfc3339(),"isError":error.is_some(),"data":data});
    if let Some(error) = error {
        let message = error.as_str().map(String::from).unwrap_or_else(||error.to_string());
        let message = if message.to_lowercase().contains("rate limit") || message.contains("Too Many Requests") {
            format!("数据源请求限流，请稍后重试。{message}")
        } else { message };
        out["error"] = json!(message.chars().take(2000).collect::<String>());
    }
    out
}
impl Drop for Session {
    fn drop(&mut self) {
        for (_,client) in self.clients.drain(..) {
            self.runtime.block_on(async { let _=timeout(Duration::from_secs(8),client.cancel()).await; });
        }
    }
}
pub fn probe(id:&str)->Probe {
    let result=(|| {let mut session=Session::new()?; session.add(id)?; Ok::<_,Error>(session.tools.iter().filter_map(|t|t["function"]["name"].as_str().map(String::from)).collect::<Vec<_>>())})();
    match result {
        Ok(tools)=>Probe {ok:true,checked_at:chrono::Utc::now().to_rfc3339(),message:format!("握手成功，发现 {} 个只读工具；行情可用性以实际查询为准。",tools.len()),tools},
        Err(e)=>Probe {ok:false,checked_at:chrono::Utc::now().to_rfc3339(),message:e.to_string(),tools:vec![]}
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn surfaces_embedded_rate_limit_without_hiding_protocol_errors() {
        let limited=normalize_result("yfinance",json!({"content":[{"text":"{\"error\":\"Rate limit reached\",\"error_code\":\"NETWORK_ERROR\"}"}]}),false);
        assert_eq!(limited["isError"],true);
        assert!(limited["error"].as_str().unwrap().contains("限流"));
        let protocol=normalize_result("aktools",json!({"content":[{"text":"invalid symbol"}]}),true);
        assert_eq!(protocol["error"],"invalid symbol");
        let success=normalize_result("yfinance",json!({"content":[{"text":"{\"price\":123}"}]}),false);
        assert_eq!(success["isError"],false);
        assert!(success.get("error").is_none());
    }
    #[test] fn rejects_unapproved_tools() { assert!(!allowed("yfinance","execute_python")); assert!(!allowed("other","get_data")); assert!(allowed("aktools","get_data")); }
    #[test] #[ignore="requires installed runtimes and Internet"]
    fn live_data_queries() {
        let mut session=Session::new().unwrap();
        session.add("aktools").unwrap();session.add("yfinance").unwrap();
        let search=session.call("mcp_aktools__search_interfaces",&json!({"keyword":"ETF"})).unwrap();
        println!("AKTools search: {}",search.to_string().chars().take(1200).collect::<String>());
        assert_ne!(search["isError"],true);
        let data=session.call("mcp_aktools__get_data",&json!({"name":"tool_trade_date_hist_sina","max_rows":3})).unwrap();
        println!("AKTools calendar: {}",data.to_string().chars().take(1200).collect::<String>());
        assert_ne!(data["isError"],true);
        let quote=session.call("mcp_yfinance__yfinance_get_ticker_info",&json!({"symbol":"VOO"})).unwrap();
        println!("Yahoo VOO: {}",quote.to_string().chars().take(1200).collect::<String>());
        assert_ne!(quote["isError"],true);
        assert!(session.call("mcp_yfinance__execute_python",&json!({})).is_err());
    }
    #[test] #[ignore="requires installed Python MCP runtimes"] fn installed_servers_handshake() { for id in ["aktools","yfinance"] {let p=probe(id);assert!(p.ok,"{}: {}",id,p.message);assert!(!p.tools.is_empty());println!("{id}: {:?}",p.tools);} }
}
