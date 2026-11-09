import { Channel, invoke as tauriInvoke } from "@tauri-apps/api/core";
import { createContext, useContext } from "react";

export function createApi(sessionToken: string, onExpired: () => void) {
  const invoke = async <T>(command: string, args: Record<string, unknown> = {}): Promise<T> => {
    try { return await tauriInvoke<T>(command, { ...args, sessionToken }); }
    catch (error) {
      if (String(error).includes("登录已失效")) onExpired();
      throw error;
    }
  };
  return buildApi(invoke);
}
export type LedgerApi = ReturnType<typeof createApi>;
export const ApiContext = createContext<LedgerApi | null>(null);
export function useApi(): LedgerApi {
  const api = useContext(ApiContext);
  if (!api) throw new Error("账本只能在登录后使用");
  return api;
}

export type Account = { id: number; name: string; kind: string; currency: string; balance: number };
export type Category = { id: number; name: string; kind: "expense" | "income" };
export type Txn = {
  id: number; account_id: number | null; category_id: number | null; category: string | null;
  kind: "expense" | "income"; amount: number; date: string; note: string; account: string | null;
};
export type BudgetStatus = { month: string; budget: number | null; spent: number; income: number };
export type NetWorth = { assets: number; liabilities: number; net: number };
export type Settings = { llm_url: string; llm_model: string; llm_path: string; llm_api_key: string; persona: string; philosophy: string };
export type UserProfile = { nickname: string; email: string; bio: string; theme: "system" | "light" | "dark"; start_page: string };
export type MonthRow = { month: string; income: number; expense: number };
export type NetPoint = { month: string; assets: number; liabilities: number; net: number };
export type CategorySum = { category: string; total: number; count: number };
export type Report = { from: string; to: string; months: MonthRow[]; net_worth: NetPoint[]; categories: CategorySum[] };
export type GoalKind = "save" | "payoff";
export type GoalStatus = "achieved" | "overdue" | "on_track" | "behind" | "unknown" | "open";
export type Goal = {
  id: number; name: string; kind: GoalKind; target: number; deadline: string | null; created_on: string;
  accounts: { id: number; name: string }[];
  current: number; remaining: number; progress: number; achieved: boolean;
  months_left: number | null; required_per_month: number | null; pace_per_month: number | null;
  projected_month: string | null; status: GoalStatus;
};
export type GoalInput = { name: string; kind: GoalKind; target: number; deadline: string | null; account_ids: number[] };
export type SubFrequency = "monthly" | "yearly";
export type SubStatus = "active" | "paused" | "ended";
export type Subscription = {
  id: number; name: string; kind: "expense" | "income"; amount: number; frequency: SubFrequency;
  start_date: string; end_date: string | null; paused: boolean;
  category_id: number | null; category: string | null; account_id: number | null; account: string | null;
  posted_count: number; last_posted: string | null; next_due: string | null; status: SubStatus; monthly_equivalent: number;
};
export type SubscriptionList = { items: Subscription[]; monthly_expense: number; monthly_income: number };
export type SubscriptionInput = {
  name: string; kind: "expense" | "income"; amount: number; frequency: SubFrequency;
  start_date: string; end_date: string | null; category_id: number | null; account_id: number | null;
};
export type SubscriptionUpdate = {
  name: string; amount: number; end_date: string | null; category_id: number | null; account_id: number | null; paused: boolean;
};
export type RunSummary = { posted: number; failed: { name: string; reason: string }[] };
export type SubPreview = { due_now: number; first_due: string | null; last_due: string | null; next_due: string | null };
// ---- 组合建议、持仓、候选清单、利息 ----
export type AssetClass = "us_equity_index" | "other_equity" | "bond" | "gold" | "cash" | "other";
export const ASSET_CLASSES: { key: AssetClass; label: string }[] = [
  { key: "us_equity_index", label: "美股宽基" }, { key: "other_equity", label: "非美股票" }, { key: "bond", label: "债券" },
  { key: "gold", label: "黄金" }, { key: "cash", label: "现金类" }, { key: "other", label: "其他" },
];
export const classLabel = (k: string) => ASSET_CLASSES.find((c) => c.key === k)?.label ?? "其他";
/** secid 有值表示金额按行情自动更新;OF. 开头的是场外基金 */
export type Holding = {
  id: number; account_id: number; account: string; name: string; code: string | null; asset_class: AssetClass; amount: number; updated_on: string;
  shares: number | null; secid: string | null;
  /** 成本价、最近一次估值的现价(原币);currency 为原币币种,手动金额的持仓为 null */
  cost_price: number | null; price: number | null; currency: Currency | null;
  /** 能算出盈亏时才有值:成本、盈亏(分,折合人民币),收益率(百分数) */
  cost: number | null; pnl: number | null; pnl_pct: number | null;
};
export type Currency = "CNY" | "HKD" | "USD";
/** 区间盈亏;full 为假表示快照不够早,只能从 since 开始算 */
export type PeriodPnl = { pnl: number; since: string; full: boolean };
export type PnlSummary = { cost: number; value: number; pnl: number; pnl_pct: number | null; counted: number; excluded: number; ytd: PeriodPnl | null };
/** 价格来源:exchange 场内(股票、ETF)| fund 场外基金 */
export type PriceSource = "exchange" | "fund";
/** 填了 shares 时,amount 由后端按最新价算出 */
export type HoldingInput = {
  account_id: number; name: string; code: string | null; asset_class: AssetClass; amount: number; shares: number | null; source: PriceSource | null;
  /** 成本价(原币),只有按份额估值的持仓可以填 */
  cost_price: number | null;
};
export type Price = { secid: string; name: string; price: number; currency: Currency; price_cny: number; time: string | null };
export type PriceRefresh = { skipped: boolean; updated: number; failed: { name: string; reason: string }[] };
export type Product = { id: number; name: string; code: string | null; asset_class: AssetClass; note: string; builtin: boolean };
export type ProductInput = { name: string; code: string | null; asset_class: AssetClass; note: string };
export type RiskLevel = "conservative" | "balanced" | "growth";
export type RiskProfile = { level: RiskLevel; max_drawdown: number; horizon_years: number; high_debt: boolean };
export type NoticeLevel = "priority" | "warn" | "info";
export type Notice = { code: string; level: NoticeLevel; message: string };
export type ClassShare = { asset_class: AssetClass; label: string; amount: number; share_pct: number };
export type PortfolioView = {
  holdings: Holding[]; total: number; shares: ClassShare[]; has_investment_account: boolean; investment_balance: number;
  coverage_pct: number | null; gap: number | null; stale: { id: number; name: string; days: number }[]; notices: Notice[];
};
export type HighDebt = { name: string; balance: number; rate_bps: number; yearly_interest: number };
export type Readiness = {
  liquid: number; avg_monthly_expense: number | null; avg_monthly_surplus: number | null; emergency_months: number | null;
  liabilities: number; high_interest_debts: HighDebt[]; high_debt_declared: boolean;
};
export type TargetPct = { us_equity_index: number; other_equity: number; bond: number; gold: number; cash: number };
export type RebalanceRow = {
  asset_class: AssetClass; label: string; current_amount: number; current_pct: number; target_amount: number; target_pct: number;
  diff_pct: number; diff_amount: number | null; action: "add" | "reduce" | "balanced";
  candidates: { products: Product[]; hint: string | null } | null;
};
export type Rebalance = { rows: RebalanceRow[]; buy_total: number; months_to_fill: number | null; reserved_for_short_term_goals: number };
export type AccountInterest = { id: number; name: string; kind: string; balance: number; rate_bps: number | null; monthly_interest: number | null };
export type InterestOverview = { accounts: AccountInterest[]; monthly_income: number; monthly_cost: number; unregistered: string[] };
export type DebtCost = { name: string; balance: number; rate_bps: number; yearly_interest: number; high: boolean };
export type InterestView = {
  overview: InterestOverview; idle_cash: number | null; weighted_rate_bps: number | null; reference_rate_bps: number | null;
  reference_yearly_interest: number | null; extra_yearly_interest: number | null; hint: string | null; debts: DebtCost[];
};
export type Advice = {
  disclaimer: string; profile: RiskProfile | null; profile_missing: boolean; readiness: Readiness; notices: Notice[];
  target: TargetPct | null; rebalance: Rebalance | null; interest: InterestView; portfolio: PortfolioView;
};
export type InterestResult = { type: string; result: Record<string, number | string>; formula: string; assumptions: string[]; note: string };

// ---- 行情 ----
export type WatchItem = { id: number; secid: string; code: string; name: string };
export type Quote = {
  secid: string; code: string; name: string; price: number | null; change: number | null; change_pct: number | null;
  open: number | null; high: number | null; low: number | null; prev_close: number | null; time: string | null;
};
/** 涨跌过大提醒:pct 为阈值(百分数);notify_denied 表示后台发通知时发现没有授权 */
export type AlertSettings = { enabled: boolean; pct: number; notify_denied: boolean };
export type PriceRange = "1d" | "5d" | "1m" | "3m" | "1y" | "5y";
export type PricePoint = { t: string; price: number };
export type PriceSeries = { secid: string; range: PriceRange; intraday: boolean; points: PricePoint[]; base: number | null };

// ---- 账单导入 ----
export type BillSource = "wechat" | "alipay";
export type ImportRow = {
  ref_id: string; time: string; date: string; kind: "expense" | "income" | null; amount: number;
  counterparty: string; goods: string; method: string; status: string; platform_type: string; note: string;
  category_id: number | null; category_source: "learned" | "platform" | "keyword" | "none";
  selected: boolean; skip: string | null; duplicate_of: string | null;
};
export type ImportPreview = {
  source: BillSource; source_label: string; file_name: string; from: string | null; to: string | null;
  rows: ImportRow[]; methods: { method: string; count: number; account_id: number | null }[];
};
export type ImportItem = {
  ref_id: string; date: string; kind: string; amount: number; category_id: number | null; account_id: number | null;
  note: string; counterparty: string; method: string;
};
export type ImportResult = { imported: number; already: number; expense: number; income: number };

export type ChatMessage = { role: "user" | "assistant"; content: string; error?: boolean };
/** 助手生成的待确认记账卡片,金额为分 */
export type TxnDraft = {
  kind: "expense" | "income"; amount: number; category_id: number | null; category: string | null;
  account_id: number | null; account: string | null; date: string; note: string;
};
export type Trace = { name: string; status: "running" | "success" | "error"; detail: string };
export type ActionDraft = { tool: string; args: Record<string, unknown>; summary: string };
export type CapabilityProbe = { ok: boolean; checked_at: string; tools: string[]; message: string };
export type Capability = {
  id: string; name: string; kind: "mcp" | "skill"; description: string; source: string; license: string;
  version: string; prompt: string; instructions: string; enabled: boolean; probe: CapabilityProbe | null;
};
export type ChatReply = { text: string; drafts: TxnDraft[]; actions: ActionDraft[]; traces: Trace[] };
export type SavedReview = { month: string; content: string; created_at: string };
export type Insight = { code: string; level: "bad" | "warn" | "info"; title: string; detail: string; question: string };

export const ACCOUNT_KINDS: Record<string, string> = {
  cash: "现金", deposit: "存款", property: "房产", investment: "理财", liability: "负债",
};

// 金额在后端是整数"分"
export const toCents = (yuan: string) => Math.round(parseFloat(yuan) * 100);
export const fmt = (cents: number) =>
  (cents / 100).toLocaleString("zh-CN", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
export const today = () => new Date().toLocaleDateString("sv-SE"); // YYYY-MM-DD
export const thisMonth = () => today().slice(0, 7);

function buildApi(invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>) { return {
  changePassword: (currentPassword: string, newPassword: string) => invoke<void>("change_password", { currentPassword, newPassword }),
  getProfile: () => invoke<UserProfile>("get_profile"),
  saveProfile: (profile: UserProfile) => invoke<UserProfile>("save_profile", { profile }),
  listAccounts: () => invoke<Account[]>("list_accounts"),
  addAccount: (a: { name: string; kind: string; currency: string; balance: number; date: string }) =>
    invoke<number>("add_account", a),
  setValuation: (accountId: number, date: string, amount: number) =>
    invoke<void>("set_valuation", { accountId, date, amount }),
  netWorth: () => invoke<NetWorth>("net_worth"),
  listCategories: () => invoke<Category[]>("list_categories"),
  addTxn: (t: {
    accountId: number | null; categoryId: number | null; kind: string;
    amount: number; date: string; note: string;
  }) => invoke<number>("add_txn", t),
  deleteTxn: (id: number) => invoke<void>("delete_txn", { id }),
  listTxns: (month: string) => invoke<Txn[]>("list_txns", { month }),
  setBudget: (month: string, categoryId: number | null, amount: number) =>
    invoke<void>("set_budget", { month, categoryId, amount }),
  budgetStatus: (month: string) => invoke<BudgetStatus>("budget_status", { month }),
  listChat: () => invoke<ChatMessage[]>("list_chat"),
  /** context:用户当前所在页面,帮助助手理解“这里”指什么 */
  sendChat: (content: string, context: string | null, capability: string | null = null, receive: (trace: Trace) => void = () => {}) => {
    const onEvent = new Channel<Trace>(); onEvent.onmessage = receive;
    return invoke<ChatReply>("send_chat", { content, context, capability, onEvent });
  },
  listCapabilities: () => invoke<Capability[]>("list_capabilities"),
  setCapabilityEnabled: (id: string, enabled: boolean) => invoke<void>("set_capability_enabled", { id, enabled }),
  testCapability: (id: string) => invoke<CapabilityProbe>("test_capability", { id }),
  confirmAction: (action: ActionDraft) => invoke<string>("confirm_action", { action }),
  /** 确认记账卡片,返回写进对话的那句"已入账…" */
  confirmDraft: (draft: TxnDraft) => invoke<string>("confirm_draft", { draft }),
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<Settings>("save_settings", { settings }),
  listModels: (settings: Settings) => invoke<string[]>("list_models", { settings }),
  resetPrompts: () => invoke<Settings>("reset_prompts"),
  listGoals: () => invoke<Goal[]>("list_goals", { today: today() }),
  addGoal: (input: GoalInput) => invoke<number>("add_goal", { input, today: today() }),
  updateGoal: (id: number, input: GoalInput) => invoke<void>("update_goal", { id, input, today: today() }),
  deleteGoal: (id: number) => invoke<void>("delete_goal", { id }),
  listSubscriptions: () => invoke<SubscriptionList>("list_recurring", { today: today() }),
  addSubscription: (input: SubscriptionInput) => invoke<{ id: number; posted: number }>("add_recurring", { input, today: today() }),
  /** 返回这次编辑顺带补记了几笔(例如修好账户后补上之前没记成的) */
  updateSubscription: (id: number, input: SubscriptionUpdate) => invoke<number>("update_recurring", { id, input, today: today() }),
  deleteSubscription: (id: number) => invoke<void>("delete_recurring", { id }),
  runSubscriptions: () => invoke<RunSummary>("run_recurring", { today: today() }),
  previewSubscription: (frequency: SubFrequency, startDate: string, endDate: string | null) =>
    invoke<SubPreview>("preview_recurring", { frequency, startDate, endDate, today: today() }),
  /** months:回看月数,0 表示从最早的数据开始 */
  getReport: (through: string, months: number) => invoke<Report>("get_report", { through, months }),
  listHoldings: () => invoke<Holding[]>("list_holdings"),
  addHolding: (input: HoldingInput) => invoke<number>("add_holding", { input, today: today() }),
  updateHolding: (id: number, input: HoldingInput) => invoke<void>("update_holding", { id, input, today: today() }),
  deleteHolding: (id: number) => invoke<void>("delete_holding", { id, today: today() }),
  pnlSummary: () => invoke<PnlSummary>("pnl_summary", { today: today() }),
  /** 按最新价更新自动估值的持仓;force 为假时后端每小时最多联网一次 */
  refreshHoldingPrices: (force: boolean) => invoke<PriceRefresh>("refresh_holding_prices", { today: today(), force }),
  lookupPrice: (code: string, source: PriceSource) => invoke<Price>("lookup_price", { code, source }),
  listProducts: () => invoke<Product[]>("list_products"),
  addProduct: (input: ProductInput) => invoke<number>("add_product", { input }),
  updateProduct: (id: number, input: ProductInput) => invoke<void>("update_product", { id, input }),
  deleteProduct: (id: number) => invoke<void>("delete_product", { id }),
  getRiskProfile: () => invoke<RiskProfile | null>("get_risk_profile"),
  saveRiskProfile: (profile: RiskProfile) => invoke<void>("save_risk_profile", { profile }),
  /** rateBps:年利率的基点(3.5% = 350);null 表示清除 */
  setAccountRate: (accountId: number, rateBps: number | null) => invoke<void>("set_account_rate", { accountId, rateBps }),
  interestOverview: () => invoke<InterestOverview>("interest_overview"),
  /** referenceRateBps:本次对比用的参考年利率(基点),不保存 */
  getPortfolioAdvice: (referenceRateBps: number | null) => invoke<Advice>("get_portfolio_advice", { today: today(), referenceRateBps }),
  /** 金额元、利率 %,同助手的 calculate_interest */
  calculateInterest: (args: Record<string, unknown>) => invoke<InterestResult>("calculate_interest", { args }),
  listWatchlist: () => invoke<WatchItem[]>("list_watchlist"),
  /** code:用户输入的代码(600519、00700、AAPL、sh000001…),后端联网确认后加入 */
  addWatch: (code: string) => invoke<WatchItem>("add_watch", { code }),
  removeWatch: (id: number) => invoke<void>("remove_watch", { id }),
  getQuotes: (secids: string[]) => invoke<Quote[]>("get_quotes", { secids }),
  getAlertSettings: () => invoke<AlertSettings>("get_alert_settings"),
  saveAlertSettings: (enabled: boolean, pct: number) => invoke<AlertSettings>("save_alert_settings", { enabled, pct }),
  getPriceSeries: (secid: string, range: PriceRange) => invoke<PriceSeries>("get_price_series", { secid, range }),
  previewImport: (path: string) => invoke<ImportPreview>("preview_import", { path }),
  commitImport: (source: BillSource, items: ImportItem[]) => invoke<ImportResult>("commit_import", { source, items }),
  /** 生成某月复盘(Markdown);同时写入聊天记录 */
  generateReview: (month: string) => invoke<string>("generate_review", { month, today: today() }),
  saveReview: (month: string, content: string) => invoke<void>("save_review", { month, content }),
  listReviews: () => invoke<SavedReview[]>("list_reviews"),
  deleteReview: (month: string) => invoke<void>("delete_review", { month }),
  getInsights: () => invoke<Insight[]>("get_insights", { today: today() }),
};
}
