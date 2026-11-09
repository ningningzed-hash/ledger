mod capabilities;
mod mcp;
mod advice;
mod auth;
pub mod alerts;
mod agent;
mod analytics;
pub mod backup;
pub mod bill_import;
pub mod core;
pub mod db;
pub mod goals;
mod insights;
mod interest;
pub mod market;
pub mod portfolio;
pub mod recurring;
pub mod report;
mod review;
mod profile;
#[cfg(test)]
mod testdb;

use agent::{Agent, OllamaAgent};
use db::Conn;
use mysql::Pool;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{Manager, State};

struct AppState {
    auth: Mutex<Option<Arc<auth::Auth>>>,
    agent: Arc<dyn Agent>,
    dir: PathBuf,
}

type R<T> = core::Result<T>;

impl AppState {
    fn auth(&self) -> R<Arc<auth::Auth>> {
        let mut guard = self.auth.lock().unwrap();
        if let Some(auth) = guard.as_ref() { return Ok(auth.clone()); }
        let auth = Arc::new(auth::Auth::new(db::load_config(&self.dir)?, self.dir.join("backups"))?);
        *guard = Some(auth.clone());
        Ok(auth)
    }
    fn pool(&self, token: &str) -> R<Pool> { self.auth()?.pool(token) }
    fn conn(&self, token: &str) -> R<Conn> { Ok(self.pool(token)?.get_conn()?) }
}

#[tauri::command]
fn add_account(s: State<AppState>, session_token: String, name: String, kind: String, currency: String, balance: i64, date: String) -> R<i64> {
    core::add_account(&mut s.conn(&session_token)?, &name, &kind, &currency, balance, &date)
}
#[tauri::command]
fn set_valuation(s: State<AppState>, session_token: String, account_id: i64, date: String, amount: i64) -> R<()> {
    core::set_valuation(&mut s.conn(&session_token)?, account_id, &date, amount)
}
#[tauri::command]
fn list_accounts(s: State<AppState>, session_token: String) -> R<Vec<core::Account>> {
    core::list_accounts(&mut s.conn(&session_token)?)
}
#[tauri::command]
fn net_worth(s: State<AppState>, session_token: String) -> R<core::NetWorth> {
    core::net_worth(&mut s.conn(&session_token)?)
}
#[tauri::command]
fn list_categories(s: State<AppState>, session_token: String) -> R<Vec<core::Category>> {
    core::list_categories(&mut s.conn(&session_token)?)
}
#[tauri::command]
fn add_txn(s: State<AppState>, session_token: String, account_id: Option<i64>, category_id: Option<i64>, kind: String, amount: i64, date: String, note: String) -> R<i64> {
    core::add_txn(&mut s.conn(&session_token)?, account_id, category_id, &kind, amount, &date, &note)
}
#[tauri::command]
fn delete_txn(s: State<AppState>, session_token: String, id: i64) -> R<()> {
    core::delete_txn(&mut s.conn(&session_token)?, id)
}
#[tauri::command]
fn list_txns(s: State<AppState>, session_token: String, month: String) -> R<Vec<core::Txn>> {
    core::list_txns(&mut s.conn(&session_token)?, &month)
}
#[tauri::command]
fn set_budget(s: State<AppState>, session_token: String, month: String, category_id: Option<i64>, amount: i64) -> R<()> {
    core::set_budget(&mut s.conn(&session_token)?, &month, category_id, amount)
}
#[tauri::command]
fn budget_status(s: State<AppState>, session_token: String, month: String) -> R<core::BudgetStatus> {
    core::budget_status(&mut s.conn(&session_token)?, &month)
}
/// today 由前端按用户本地日期传入(YYYY-MM-DD)
#[tauri::command]
fn list_goals(s: State<AppState>, session_token: String, today: String) -> R<Vec<goals::GoalView>> {
    goals::list_goals(&mut s.conn(&session_token)?, &today)
}
#[tauri::command]
fn add_goal(s: State<AppState>, session_token: String, input: goals::GoalInput, today: String) -> R<i64> {
    goals::add_goal(&mut s.conn(&session_token)?, &input, &today)
}
#[tauri::command]
fn update_goal(s: State<AppState>, session_token: String, id: i64, input: goals::GoalInput, today: String) -> R<()> {
    goals::update_goal(&mut s.conn(&session_token)?, id, &input, &today)
}
#[tauri::command]
fn delete_goal(s: State<AppState>, session_token: String, id: i64) -> R<()> {
    goals::delete_goal(&mut s.conn(&session_token)?, id)
}
/// 订阅:到期的账目由 run_recurring 自动记上(应用启动、窗口回到前台时由前端触发)
#[tauri::command]
fn list_recurring(s: State<AppState>, session_token: String, today: String) -> R<recurring::RecurringList> {
    recurring::list_recurring(&mut s.conn(&session_token)?, &today)
}
#[tauri::command]
fn add_recurring(s: State<AppState>, session_token: String, input: recurring::RecurringInput, today: String) -> R<recurring::Added> {
    recurring::add_recurring(&mut s.conn(&session_token)?, &input, &today)
}
#[tauri::command]
fn update_recurring(s: State<AppState>, session_token: String, id: i64, input: recurring::RecurringUpdate, today: String) -> R<usize> {
    recurring::update_recurring(&mut s.conn(&session_token)?, id, &input, &today)
}
#[tauri::command]
fn delete_recurring(s: State<AppState>, session_token: String, id: i64) -> R<()> {
    recurring::delete_recurring(&mut s.conn(&session_token)?, id)
}
#[tauri::command]
fn run_recurring(s: State<AppState>, session_token: String, today: String) -> R<recurring::RunSummary> {
    recurring::run_due(&mut s.conn(&session_token)?, &today)
}
#[tauri::command]
fn preview_recurring(s: State<AppState>, session_token: String, frequency: String, start_date: String, end_date: Option<String>, today: String) -> R<recurring::Preview> {
    s.pool(&session_token)?;
    recurring::preview(&frequency, &start_date, end_date.as_deref(), &today)
}
/// months:回看月数,0 表示从最早的数据开始;through:截止月份(当前月)
#[tauri::command]
fn get_report(s: State<AppState>, session_token: String, through: String, months: u32) -> R<report::Report> {
    report::report(&mut s.conn(&session_token)?, &through, months)
}
#[tauri::command]
fn list_chat(s: State<AppState>, session_token: String) -> R<Vec<core::ChatMessage>> {
    core::list_chat(&mut s.conn(&session_token)?)
}
/// 异步命令 + spawn_blocking:模型推理耗时较长,不能占用主线程或数据库连接。
/// 仅在成功时才保存这一轮对话,失败不会留下半截记录。
#[tauri::command]
async fn send_chat(s: State<'_, AppState>, session_token: String, content: String, context: Option<String>, capability: Option<String>, on_event: tauri::ipc::Channel<agent::Trace>) -> R<agent::Reply> {
    let (pool, agent) = (s.pool(&session_token)?, s.agent.clone());
    tauri::async_runtime::spawn_blocking(move || {
        let reply = agent.reply(&pool, &content, context.as_deref(), capability.as_deref(), &|event| { let _ = on_event.send(event); })?;
        let mut c = pool.get_conn()?;
        core::add_chat(&mut c, "user", &content)?;
        core::add_chat(&mut c, "assistant", &reply.text)?;
        Ok(reply)
    })
    .await
    .map_err(|e| core::Error::Llm(format!("后台任务失败:{e}")))?
}
#[tauri::command]
fn list_capabilities(s: State<AppState>, session_token: String) -> R<Vec<capabilities::Capability>> {
    capabilities::list(&mut s.conn(&session_token)?)
}
#[tauri::command]
fn set_capability_enabled(s: State<AppState>, session_token: String, id: String, enabled: bool) -> R<()> {
    capabilities::set_enabled(&mut s.conn(&session_token)?, &id, enabled)
}
#[tauri::command]
async fn test_capability(s: State<'_, AppState>, session_token: String, id: String) -> R<capabilities::Probe> {
    let pool=s.pool(&session_token)?;
    if !capabilities::catalog().iter().any(|i|i.id==id && i.kind=="mcp") { return Err(core::Error::Invalid("未知 MCP".into())); }
    blocking(move || {let probe=mcp::probe(&id);capabilities::save_probe(&mut pool.get_conn()?,&id,&probe)?;Ok(probe)}).await
}
#[tauri::command]
fn confirm_action(s: State<AppState>, session_token: String, action: agent::ActionDraft) -> R<String> {
    agent::confirm_action(&mut s.conn(&session_token)?, &action)
}
/// 用户在记账卡片上点确认(可能改过内容)
#[tauri::command]
fn confirm_draft(s: State<AppState>, session_token: String, draft: agent::TxnDraft) -> R<String> {
    agent::confirm_draft(&mut s.conn(&session_token)?, &draft)
}
#[tauri::command]
fn get_profile(s: State<AppState>, session_token: String) -> R<profile::Profile> {
    profile::get(&mut s.conn(&session_token)?)
}
#[tauri::command]
fn save_profile(s: State<AppState>, session_token: String, profile: profile::Profile) -> R<profile::Profile> {
    profile::save(&mut s.conn(&session_token)?, profile)
}
#[tauri::command]
fn get_settings(s: State<AppState>, session_token: String) -> R<core::Settings> {
    core::get_settings(&mut s.conn(&session_token)?)
}
#[tauri::command]
fn save_settings(s: State<AppState>, session_token: String, settings: core::Settings) -> R<core::Settings> {
    let mut c = s.conn(&session_token)?;
    core::save_settings(&mut c, &settings)?;
    core::get_settings(&mut c)
}
/// 用表单里的地址、路径和 Key 拉取模型名单(不要求已保存)
#[tauri::command]
async fn list_models(s: State<'_, AppState>, session_token: String, settings: core::Settings) -> R<Vec<String>> {
    s.pool(&session_token)?;
    tauri::async_runtime::spawn_blocking(move || agent::list_models(&settings))
        .await
        .map_err(|e| core::Error::Llm(format!("后台任务失败:{e}")))?
}
#[tauri::command]
fn reset_prompts(s: State<AppState>, session_token: String) -> R<core::Settings> {
    let mut c = s.conn(&session_token)?;
    core::reset_prompts(&mut c)?;
    core::get_settings(&mut c)
}
/// 持仓、候选产品清单、风险问卷、账户年利率、组合建议与利息计算
#[tauri::command]
fn list_holdings(s: State<AppState>, session_token: String) -> R<Vec<portfolio::Holding>> {
    portfolio::list_holdings(&mut s.conn(&session_token)?)
}
/// 填了份额的持仓先联网查最新价,算出金额后再保存
#[tauri::command]
async fn add_holding(s: State<'_, AppState>, session_token: String, input: portfolio::HoldingInput, today: String) -> R<i64> {
    s.pool(&session_token)?;
    let input = blocking(move || market::value_holding(input)).await?;
    portfolio::add_holding(&mut s.conn(&session_token)?, &input, &today)
}
#[tauri::command]
async fn update_holding(s: State<'_, AppState>, session_token: String, id: i64, input: portfolio::HoldingInput, today: String) -> R<()> {
    s.pool(&session_token)?;
    let input = blocking(move || market::value_holding(input)).await?;
    portfolio::update_holding(&mut s.conn(&session_token)?, id, &input, &today)
}
/// 按最新价更新自动估值的持仓(应用启动、窗口回到前台时由前端触发;force 为手动刷新)
#[tauri::command]
async fn refresh_holding_prices(s: State<'_, AppState>, session_token: String, today: String, force: bool) -> R<market::RefreshSummary> {
    let pool = s.pool(&session_token)?;
    blocking(move || market::refresh_holdings(&mut pool.get_conn()?, &today, chrono::Local::now().timestamp(), force)).await
}
/// 登记持仓时预览:source 为 exchange(场内)或 fund(场外基金)
#[tauri::command]
async fn lookup_price(s: State<'_, AppState>, session_token: String, code: String, source: String) -> R<market::Price> {
    s.pool(&session_token)?;
    blocking(move || market::lookup_price(&code, &source)).await
}
#[tauri::command]
fn delete_holding(s: State<AppState>, session_token: String, id: i64, today: String) -> R<()> {
    portfolio::delete_holding(&mut s.conn(&session_token)?, id, &today)
}
/// 涨跌过大提醒的开关与阈值
#[tauri::command]
fn get_alert_settings(s: State<AppState>, session_token: String) -> R<alerts::AlertSettings> {
    alerts::get_settings(&mut s.conn(&session_token)?)
}
#[tauri::command]
fn save_alert_settings(s: State<AppState>, session_token: String, enabled: bool, pct: f64) -> R<alerts::AlertSettings> {
    let mut c = s.conn(&session_token)?;
    // 用户刚在界面上打开开关(前端已请求过授权),先当作已授权,后台发通知时再确认
    alerts::set_notify_denied(&mut c, false)?;
    alerts::save_settings(&mut c, enabled, pct)
}

/// 后台每小时检查一次涨跌过大提醒。启动后先等几分钟,避开启动时的估值刷新
fn spawn_alert_loop(app: tauri::AppHandle) {
    const FIRST_DELAY: std::time::Duration = std::time::Duration::from_secs(3 * 60);
    const EVERY: std::time::Duration = std::time::Duration::from_secs(60 * 60);
    std::thread::spawn(move || {
        std::thread::sleep(FIRST_DELAY);
        loop {
            if let Err(e) = check_alerts(&app) {
                eprintln!("涨跌提醒检查失败(下一轮再试):{e}");
            }
            std::thread::sleep(EVERY);
        }
    });
}

fn check_alerts(app: &tauri::AppHandle) -> R<()> {
    use tauri_plugin_notification::{NotificationExt, PermissionState};
    let auth = app.state::<AppState>().auth()?;
    let Some((token, pool)) = auth.active() else { return Ok(()); };
    let mut c = pool.get_conn()?;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let hits = alerts::run_once(&mut c, &today, market::fetch_quotes)?;
    if hits.is_empty() { return Ok(()); }
    auth.if_current(&token, || {
        let granted = matches!(app.notification().permission_state(), Ok(PermissionState::Granted));
        let _ = alerts::set_notify_denied(&mut c, !granted);
        if granted {
            for h in &hits {
                let (title, body) = alerts::notice_text(h);
                if let Err(e) = app.notification().builder().title(title).body(body).show() {
                    eprintln!("发送通知失败:{e}");
                }
            }
        }
    });
    Ok(())
}

/// 持仓盈亏汇总(含今年以来)
#[tauri::command]
fn pnl_summary(s: State<AppState>, session_token: String, today: String) -> R<portfolio::PnlSummary> {
    portfolio::pnl_summary(&mut s.conn(&session_token)?, &today)
}
#[tauri::command]
fn list_products(s: State<AppState>, session_token: String) -> R<Vec<portfolio::Product>> {
    portfolio::list_products(&mut s.conn(&session_token)?)
}
#[tauri::command]
fn add_product(s: State<AppState>, session_token: String, input: portfolio::ProductInput) -> R<i64> {
    portfolio::add_product(&mut s.conn(&session_token)?, &input)
}
#[tauri::command]
fn update_product(s: State<AppState>, session_token: String, id: i64, input: portfolio::ProductInput) -> R<()> {
    portfolio::update_product(&mut s.conn(&session_token)?, id, &input)
}
#[tauri::command]
fn delete_product(s: State<AppState>, session_token: String, id: i64) -> R<()> {
    portfolio::delete_product(&mut s.conn(&session_token)?, id)
}
#[tauri::command]
fn get_risk_profile(s: State<AppState>, session_token: String) -> R<Option<portfolio::RiskProfile>> {
    portfolio::get_risk_profile(&mut s.conn(&session_token)?)
}
#[tauri::command]
fn save_risk_profile(s: State<AppState>, session_token: String, profile: portfolio::RiskProfile) -> R<()> {
    portfolio::save_risk_profile(&mut s.conn(&session_token)?, &profile)
}
/// rate_bps 为年利率的基点(3.5% = 350);为空表示清除
#[tauri::command]
fn set_account_rate(s: State<AppState>, session_token: String, account_id: i64, rate_bps: Option<i64>) -> R<()> {
    portfolio::set_account_rate(&mut s.conn(&session_token)?, account_id, rate_bps)
}
#[tauri::command]
fn interest_overview(s: State<AppState>, session_token: String) -> R<portfolio::InterestOverview> {
    portfolio::interest_overview(&mut s.conn(&session_token)?)
}
/// reference_rate_bps:用户输入的参考年利率(基点),只用于本次对比,不保存
#[tauri::command]
fn get_portfolio_advice(s: State<AppState>, session_token: String, today: String, reference_rate_bps: Option<i64>) -> R<advice::Advice> {
    advice::advice(&mut s.conn(&session_token)?, &today, reference_rate_bps)
}
/// 参数同助手的 calculate_interest:金额元、利率 %
#[tauri::command]
fn calculate_interest(s: State<AppState>, session_token: String, args: serde_json::Value) -> R<serde_json::Value> {
    s.pool(&session_token)?;
    interest::calculate(&args)
}

/// 月度复盘:先由程序算出数字表格,再请模型写解读(模型不可用时只给数字)。
/// 生成的内容会作为一轮对话存进聊天记录
#[tauri::command]
async fn generate_review(s: State<'_, AppState>, session_token: String, month: String, today: String) -> R<String> {
    let pool = s.pool(&session_token)?;
    blocking(move || {
        let (settings, f) = {
            let mut c = pool.get_conn()?;
            (core::get_settings(&mut c)?, review::facts(&mut c, &month, &today)?)
        };
        let md = review::facts_markdown(&f);
        let commentary = agent::complete(&settings, &review::interpret_prompt(&settings.persona), &review::facts_for_model(&f)).map_err(|e| e.to_string());
        let content = review::compose(&md, commentary);
        let mut c = pool.get_conn()?;
        core::add_chat(&mut c, "user", &format!("生成 {month} 的月度复盘"))?;
        core::add_chat(&mut c, "assistant", &content)?;
        Ok(content)
    })
    .await
}
#[tauri::command]
fn save_review(s: State<AppState>, session_token: String, month: String, content: String) -> R<()> {
    review::save(&mut s.conn(&session_token)?, &month, &content)
}
#[tauri::command]
fn list_reviews(s: State<AppState>, session_token: String) -> R<Vec<review::Saved>> {
    review::list(&mut s.conn(&session_token)?)
}
#[tauri::command]
fn delete_review(s: State<AppState>, session_token: String, month: String) -> R<()> {
    review::delete(&mut s.conn(&session_token)?, &month)
}
/// 总览洞察卡(纯规则,不调模型)
#[tauri::command]
fn get_insights(s: State<AppState>, session_token: String, today: String) -> R<Vec<insights::Insight>> {
    insights::insights(&mut s.conn(&session_token)?, &today)
}

/// 账单导入:path 为用户选中的微信/支付宝账单文件
#[tauri::command]
fn preview_import(s: State<AppState>, session_token: String, path: String) -> R<bill_import::Preview> {
    bill_import::preview_file(&mut s.conn(&session_token)?, std::path::Path::new(&path))
}
#[tauri::command]
fn commit_import(s: State<AppState>, session_token: String, source: bill_import::Source, items: Vec<bill_import::CommitItem>) -> R<bill_import::CommitResult> {
    bill_import::commit(&mut s.conn(&session_token)?, source, &items)
}

/// 行情:联网请求放到后台线程,不占主线程
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> R<T> + Send + 'static) -> R<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| core::Error::Net(format!("后台任务失败:{e}")))?
}
#[tauri::command]
fn list_watchlist(s: State<AppState>, session_token: String) -> R<Vec<market::WatchItem>> {
    market::list_watch(&mut s.conn(&session_token)?)
}
/// code:用户输入的代码,先联网确认存在再加入
#[tauri::command]
async fn add_watch(s: State<'_, AppState>, session_token: String, code: String) -> R<market::WatchItem> {
    s.pool(&session_token)?;
    let q = blocking(move || market::resolve(&code)).await?;
    market::add_watch(&mut s.conn(&session_token)?, &q)
}
#[tauri::command]
fn remove_watch(s: State<AppState>, session_token: String, id: i64) -> R<()> {
    market::remove_watch(&mut s.conn(&session_token)?, id)
}
#[tauri::command]
async fn get_quotes(s: State<'_, AppState>, session_token: String, secids: Vec<String>) -> R<Vec<market::Quote>> {
    s.pool(&session_token)?;
    blocking(move || market::fetch_quotes(&secids)).await
}
/// range:1d | 5d | 1m | 3m | 1y | 5y
#[tauri::command]
async fn get_price_series(s: State<'_, AppState>, session_token: String, secid: String, range: String) -> R<market::Series> {
    s.pool(&session_token)?;
    blocking(move || market::fetch_series(&secid, &range)).await
}

#[tauri::command]
async fn auth_status(s: State<'_, AppState>) -> R<auth::Status> {
    let auth = s.auth()?;
    blocking(move || auth.status()).await
}
#[tauri::command]
async fn register_user(s: State<'_, AppState>, username: String, password: String, claim_legacy: bool) -> R<auth::Login> {
    let auth = s.auth()?;
    blocking(move || auth.register(&username, &password, claim_legacy)).await
}
#[tauri::command]
async fn login_user(s: State<'_, AppState>, username: String, password: String) -> R<auth::Login> {
    let auth = s.auth()?;
    blocking(move || auth.login(&username, &password)).await
}
#[tauri::command]
fn resume_session(s: State<AppState>, session_token: String) -> R<auth::Login> { s.auth()?.resume(&session_token) }
#[tauri::command]
fn logout_user(s: State<AppState>, session_token: String) -> R<()> { s.auth()?.logout(&session_token) }
#[tauri::command]
async fn change_password(s: State<'_, AppState>, session_token: String, current_password: String, new_password: String) -> R<()> {
    let auth = s.auth()?;
    blocking(move || auth.change_password(&session_token, &current_password, &new_password)).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let state = AppState { auth: Mutex::new(None), agent: Arc::new(OllamaAgent), dir };
            // 登录成功后才访问用户账本并执行该用户的每日备份。
            app.manage(state);
            spawn_alert_loop(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            auth_status, register_user, login_user, resume_session, logout_user, change_password,
            add_account, set_valuation, list_accounts, net_worth, list_categories,
            add_txn, delete_txn, list_txns, set_budget, budget_status, list_chat, send_chat, confirm_draft, list_capabilities, set_capability_enabled, test_capability, confirm_action,
            get_settings, save_settings, reset_prompts, list_models,
            get_profile, save_profile,
            get_report,
            list_goals, add_goal, update_goal, delete_goal,
            list_recurring, add_recurring, update_recurring, delete_recurring, run_recurring, preview_recurring,
            list_holdings, add_holding, update_holding, delete_holding, pnl_summary, get_alert_settings, save_alert_settings, refresh_holding_prices, lookup_price, list_products, add_product, update_product, delete_product,
            get_risk_profile, save_risk_profile, set_account_rate, interest_overview, get_portfolio_advice, calculate_interest,
            list_watchlist, add_watch, remove_watch, get_quotes, get_price_series,
            preview_import, commit_import, generate_review, save_review, list_reviews, delete_review, get_insights
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
