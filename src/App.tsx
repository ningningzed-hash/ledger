import { ReactNode, useCallback, useEffect, useRef, useState } from "react";
import { Account, BudgetStatus, Category, NetWorth, Settings, Txn, UserProfile, useApi, fmt, thisMonth, toCents, today } from "./api";
import { ask } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { AskRequest, ChatDrawer } from "./chat";
import { ImportModal, isBillFile, pickBill } from "./importBill";
import { Assets, Budget, CAT_ICON, Home, Txns } from "./views";
import { GoalsView } from "./goals";
import { SubscriptionsView } from "./subscriptions";
import { ReportView } from "./reports";
import { InvestView } from "./invest";
import { CapabilitiesView } from "./capabilities";
import { ProfileView } from "./profile";
import { hasLegacyPhilosophy } from "./portfolioText";
import "./App.css";

export type Data = {
  month: string; accounts: Account[]; cats: Category[]; txns: Txn[]; status: BudgetStatus | null; nw: NetWorth | null;
};

const svg = (d: string) => (
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d={d} /></svg>
);

/**
 * 侧栏导航配置:往这个数组里加一项即可出现在左边栏。
 * soon: true 表示占位(灰色不可点);group 相同的相邻项会归到同一个小标题下。
 */
export const NAV: { key: string; label: string; icon: ReactNode; group?: string; soon?: boolean }[] = [
  { key: "home", label: "总览", icon: svg("M3 3h7v9H3zM14 3h7v5h-7zM14 12h7v9h-7zM3 16h7v5H3z") },
  { key: "txn", label: "账目", icon: svg("M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01") },
  { key: "asset", label: "资产", icon: svg("M3 21h18M5 21V10l7-6 7 6v11M10 21v-6h4v6") },
  { key: "budget", label: "预算", icon: svg("M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM12 7v5l3 2") },
  { key: "goal", label: "目标", icon: svg("M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8z") },
  { key: "sub", label: "订阅", icon: svg("M21 12a9 9 0 1 1-3-6.7M21 4v5h-5") },
  { key: "report", label: "报表", icon: svg("M4 20V10M10 20V4M16 20v-7M22 20H2") },
  { key: "invest", label: "理财", icon: svg("M3 17l6-6 4 4 8-8M15 7h6v6") },
  { key: "capabilities", label: "能力市场", icon: svg("M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM17.5 14v7M14 17.5h7") },
  { key: "profile", label: "个人中心", icon: svg("M20 21v-2a7 7 0 0 0-14 0v2M12 3a4 4 0 1 0 0 8 4 4 0 0 0 0-8z") },
];

/** 侧栏底部小组件:同样是配置,后续可追加 */
const WIDGETS: ((d: Data) => ReactNode)[] = [
  (d) => d.status && (
    <div className="wid" key="budget">
      <h4>本月剩余预算</h4>
      {d.status.budget !== null ? (
        <>
          <div className="num" style={{ fontSize: 20, fontWeight: 700 }}>¥{fmt(d.status.budget - d.status.spent)}</div>
          <div className="catbar"><i style={{ width: `${Math.min(100, (d.status.spent / d.status.budget) * 100)}%`, background: "var(--accent)" }} /></div>
        </>
      ) : <div className="muted">未设置</div>}
    </div>
  ),
  (d) => (
    <div className="wid" key="acc">
      <h4>账户速览</h4>
      {d.accounts.slice(0, 3).map((a) => (
        <div className="l" key={a.id}><span>{a.name}</span><b className="num">{fmt(a.balance)}</b></div>
      ))}
      {d.accounts.length === 0 && <div className="muted">暂无账户</div>}
    </div>
  ),
];

const errMsg = (e: unknown) => (typeof e === "string" ? e : String(e));
const shiftMonth = (m: string, n: number) => {
  const [y, mo] = m.split("-").map(Number);
  const d = new Date(y, mo - 1 + n, 1);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}`;
};

/** 告诉助手用户正在看哪个页面 */
function pageContext(view: string, month: string): string {
  const label = NAV.find((n) => n.key === view)?.label ?? view;
  const withMonth = ["home", "txn", "budget"].includes(view);
  return `「${label}」页${withMonth ? `,正在看 ${month} 的数据` : ""}`;
}

export default function App() {
  const api = useApi();
  const [view, setView] = useState("home");
  const [profile, setProfile] = useState<UserProfile | null>(null);
  const [profileError, setProfileError] = useState("");
  const navigated = useRef(false);
  const navigate = (next: string) => { navigated.current = true; setView(next); };
  const loadProfile = useCallback(async () => {
    setProfileError("");
    try {
      const p = await api.getProfile();
      setProfile(p);
      if (!navigated.current) setView(p.start_page);
    } catch (e) { setProfileError(errMsg(e)); }
  }, []);
  useEffect(() => { loadProfile(); }, [loadProfile]);
  useEffect(() => { document.documentElement.dataset.theme = profile?.theme ?? "system"; }, [profile?.theme]);
  const [month, setMonth] = useState(thisMonth());
  const [data, setData] = useState<Data>({ month, accounts: [], cats: [], txns: [], status: null, nw: null });
  const [error, setError] = useState("");
  const [modal, setModal] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [toast, setToast] = useState("");
  const [chatOpen, setChatOpen] = useState(false);
  const [askReq, setAskReq] = useState<AskRequest | null>(null);
  const [importPath, setImportPath] = useState<string | null>(null);

  // ⌘K / Ctrl+K 唤起或收起助手
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") { e.preventDefault(); setChatOpen((o) => !o); }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
  // 把账单文件拖进窗口即可导入
  useEffect(() => {
    let off: (() => void) | undefined;
    let alive = true;
    getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type !== "drop") return;
      const bill = e.payload.paths.find(isBillFile);
      if (bill) setImportPath(bill);
      else if (e.payload.paths.length) setError("只能导入微信 / 支付宝导出的账单文件(.csv / .xlsx)");
    }).then((u) => { if (alive) off = u; else u(); }).catch(() => {});
    return () => { alive = false; off?.(); };
  }, []);
  const openImport = async () => {
    try { const p = await pickBill(); if (p) setImportPath(p); } catch (e) { setError(errMsg(e)); }
  };
  const askAssistant = (text: string, capability?: string) => { setChatOpen(true); setAskReq((r) => ({ text, capability, n: (r?.n ?? 0) + 1 })); };

  const refresh = useCallback(async () => {
    try {
      const [accounts, cats, txns, status, nw] = await Promise.all([
        api.listAccounts(), api.listCategories(), api.listTxns(month), api.budgetStatus(month), api.netWorth(),
      ]);
      setData({ month, accounts, cats, txns, status, nw });
      setError("");
    } catch (e) { setError(errMsg(e)); }
  }, [month]);
  useEffect(() => { refresh(); }, [refresh]);

  // 订阅到期的账目在"打开应用"和"窗口回到前台"时自动记上(每分钟最多一次)。
  // 用 ref 取最新的 refresh,这样切换月份不会重复触发
  const refreshRef = useRef(refresh);
  refreshRef.current = refresh;
  const lastRun = useRef(0);
  useEffect(() => {
    const runDue = async (force: boolean) => {
      if (!force && Date.now() - lastRun.current < 60_000) return;
      lastRun.current = Date.now();
      try {
        const r = await api.runSubscriptions();
        if (r.posted > 0) {
          await refreshRef.current();
          setToast(`已自动记入 ${r.posted} 笔订阅账目`);
        }
        if (r.failed.length > 0) setError(r.failed.map((f) => `订阅「${f.name}」自动入账失败:${f.reason}`).join(";"));
      } catch { /* 数据库连不上时,主刷新已经会显示原因 */ }
    };
    // 按份额登记的持仓顺带按最新价更新金额(后端每小时最多联网一次);失败不打扰,持仓里会显示更新日期。
    // 有更新时投资账户余额也变了,重新加载页面数据
    const refreshPrices = () => {
      api.refreshHoldingPrices(false).then((r) => { if (r.updated > 0) refreshRef.current(); }).catch(() => {});
    };
    runDue(true);
    refreshPrices();
    const onFocus = () => { runDue(false); refreshPrices(); };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, []);
  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(""), 6000);
    return () => clearTimeout(t);
  }, [toast]);

  const run = async (fn: () => Promise<unknown>) => {
    try { await fn(); await refresh(); } catch (e) { setError(errMsg(e)); }
  };

  const [y, m] = month.split("-");
  const monthNav = (
    <div className="month">
      <button onClick={() => setMonth(shiftMonth(month, -1))}>‹</button>
      <span>{y} 年 {+m} 月</span>
      <button onClick={() => setMonth(shiftMonth(month, 1))}>›</button>
    </div>
  );

  let lastGroup: string | undefined;
  return (
    <div className="app">
      <aside className="side">
        <div className="brand"><div className="logo">¥</div><span>Ledger</span></div>
        {NAV.map((n) => {
          const title = n.group && n.group !== lastGroup ? <div className="grp" key={`g-${n.group}`}>{n.group}</div> : null;
          lastGroup = n.group;
          return (
            <span key={n.key} style={{ display: "contents" }}>
              {title}
              <button className={`nav${view === n.key ? " on" : ""}${n.soon ? " soon" : ""}`} onClick={() => !n.soon && navigate(n.key)}>
                {n.icon}<span>{n.label}</span>{n.soon && <em>soon</em>}
              </button>
            </span>
          );
        })}
        <div className="sp" />
        {WIDGETS.map((w) => w(data))}
        <button className="addbtn" style={{ marginTop: 10 }} onClick={() => setModal(true)}>＋ <span>记一笔</span></button>
      </aside>

      <main className="main">
        {view === "home" && <Home d={data} nav={monthNav} go={navigate} onAsk={askAssistant} />}
        {view === "txn" && <Txns d={data} nav={monthNav} onDelete={(id) => run(() => api.deleteTxn(id))} onImport={openImport} />}
        {view === "asset" && (
          <Assets d={data}
            onAdd={(a) => run(() => api.addAccount(a))}
            onValue={(id, v) => run(() => api.setValuation(id, today(), v))} />
        )}
        {view === "goal" && <GoalsView accounts={data.accounts} />}
        {view === "sub" && (
          <SubscriptionsView accounts={data.accounts} cats={data.cats}
            onPosted={(n) => { refresh(); setToast(`已自动记入 ${n} 笔订阅账目`); }} />
        )}
        {view === "report" && <ReportView />}
        {view === "invest" && <InvestView />}
        {view === "capabilities" && <CapabilitiesView onUse={askAssistant} />}
        {view === "profile" && <ProfileView profile={profile} error={profileError} onRetry={loadProfile} onSaved={setProfile} onSettings={() => setSettingsOpen(true)} pages={NAV} />}
        {view === "budget" && <Budget d={data} nav={monthNav} onBudget={(v) => run(() => api.setBudget(month, null, v))} />}
      </main>

      <ChatDrawer open={chatOpen} onClose={() => setChatOpen(false)} context={pageContext(view, month)} ask={askReq}
        accounts={data.accounts} cats={data.cats} onChanged={refresh} onImport={openImport}
        settingsOpen={settingsOpen} onSettings={() => setSettingsOpen(true)} />
      {!chatOpen && (
        <button className="fab" title="财务助手(⌘K)" aria-label="打开财务助手" onClick={() => setChatOpen(true)}>✦</button>
      )}
      {importPath && (
        <ImportModal path={importPath} accounts={data.accounts} cats={data.cats} onClose={() => setImportPath(null)}
          onDone={(r) => {
            setImportPath(null);
            refresh();
            setToast(`已导入 ${r.imported} 笔:支出 ¥${fmt(r.expense)},收入 ¥${fmt(r.income)}${r.already ? `(${r.already} 笔之前已导入,已跳过)` : ""}`);
          }} />
      )}
      {settingsOpen && <SettingsModal onClose={() => setSettingsOpen(false)} />}
      {modal && (
        <AddModal d={data} onClose={() => setModal(false)}
          onSave={(t) => { run(() => api.addTxn(t)); setModal(false); }} />
      )}
      {toast && <div className="toast" onClick={() => setToast("")}>{toast}</div>}
      {error && <div className="error" onClick={() => setError("")}>{error}</div>}
    </div>
  );
}

function AddModal({ d, onClose, onSave }: {
  d: Data; onClose: () => void;
  onSave: (t: { accountId: number | null; categoryId: number | null; kind: string; amount: number; date: string; note: string }) => void;
}) {
  const [kind, setKind] = useState<"expense" | "income">("expense");
  const [amount, setAmount] = useState("");
  const [categoryId, setCategoryId] = useState<number | null>(null);
  const [accountId, setAccountId] = useState("");
  const [date, setDate] = useState(today());
  const [note, setNote] = useState("");
  const cats = d.cats.filter((c) => c.kind === kind);
  const cents = toCents(amount);

  return (
    <div className="modal on" onClick={(e) => e.target === e.currentTarget && onClose()}>
      <form className="sheet" onSubmit={(e) => {
        e.preventDefault();
        if (!Number.isFinite(cents) || cents <= 0) return;
        onSave({ accountId: accountId ? +accountId : null, categoryId, kind, amount: cents, date, note });
      }}>
        <h2>记一笔</h2>
        <div className="tog kind-tog" role="radiogroup" aria-label="收支类型">
          <button type="button" role="radio" aria-checked={kind === "expense"} className={kind === "expense" ? "on" : ""} onClick={() => { setKind("expense"); setCategoryId(null); }}>− 支出</button>
          <button type="button" role="radio" aria-checked={kind === "income"} className={kind === "income" ? "on" : ""} onClick={() => { setKind("income"); setCategoryId(null); }}>+ 收入</button>
        </div>
        <label className="big-amt">
          <span className="sign" aria-hidden="true">{kind === "income" ? "+¥" : "−¥"}</span>
          <input className="big num" type="number" step="0.01" min="0.01" placeholder="0.00" autoFocus
            aria-label={kind === "income" ? "收入金额" : "支出金额"} value={amount} onChange={(e) => setAmount(e.target.value)} required />
        </label>
        <div className="cats">
          {cats.map((c) => (
            <button type="button" key={c.id} className={categoryId === c.id ? "on" : ""} onClick={() => setCategoryId(c.id)}>
              <span>{CAT_ICON[c.name] ?? "•"}</span>{c.name}
            </button>
          ))}
        </div>
        <select className="f" value={accountId} onChange={(e) => setAccountId(e.target.value)}>
          <option value="">账户(可不选)</option>
          {d.accounts.filter((a) => a.kind !== "property").map((a) => <option key={a.id} value={a.id}>{a.name}</option>)}
        </select>
        <input className="f" type="date" value={date} onChange={(e) => setDate(e.target.value)} required />
        <input className="f" placeholder="备注(可选)" value={note} onChange={(e) => setNote(e.target.value)} />
        <button className="go">{kind === "income" ? "保存收入" : "保存支出"}</button>
        <button type="button" className="cancel" onClick={onClose}>取消</button>
      </form>
    </div>
  );
}

function SettingsModal({ onClose }: { onClose: () => void }) {
  const api = useApi();
  const [s, setS] = useState<Settings | null>(null);
  const [msg, setMsg] = useState("");
  const [models, setModels] = useState<string[]>([]);
  const [loadingModels, setLoadingModels] = useState(false);
  useEffect(() => { api.getSettings().then(setS).catch((e) => setMsg(errMsg(e))); }, []);
  if (!s) return <div className="modal on"><div className="sheet"><h2>助手设置</h2><p role={msg ? "alert" : "status"}>{msg || "正在加载…"}</p><button className="cancel" onClick={onClose}>关闭</button></div></div>;
  const set = (k: keyof Settings) => (e: { target: { value: string } }) => setS({ ...s, [k]: e.target.value });

  return (
    <div className="modal on" onClick={(e) => e.target === e.currentTarget && onClose()}>
      <form className="sheet wide" onSubmit={async (e) => {
        e.preventDefault();
        try { setS(await api.saveSettings(s)); setMsg("已保存"); } catch (err) { setMsg(errMsg(err)); }
      }}>
        <h2>助手设置</h2>
        <label className="lbl">模型服务地址</label>
        <input className="f" value={s.llm_url} onChange={set("llm_url")} placeholder="http://localhost:11434 或 https://api.deepseek.com" />
        <label className="lbl">接口路径(OpenAI 兼容填 /chat/completions)</label>
        <input className="f" value={s.llm_path} onChange={set("llm_path")} placeholder="/api/chat 或 /chat/completions" />
        <label className="lbl">API Key(本地 Ollama 留空)</label>
        <input className="f" type="password" autoComplete="off" value={s.llm_api_key} onChange={set("llm_api_key")} />
        <label className="lbl">模型名称</label>
        <div className="row-inline">
          <input className="f" list="llm-models" value={s.llm_model} onChange={set("llm_model")} />
          <button type="button" className="btn alt" disabled={loadingModels}
            onClick={async () => {
              setLoadingModels(true); setMsg("");
              try {
                const list = await api.listModels(s);
                setModels(list); setMsg(`获取到 ${list.length} 个模型`);
              } catch (err) { setMsg(errMsg(err)); } finally { setLoadingModels(false); }
            }}>{loadingModels ? "获取中…" : "获取名单"}</button>
        </div>
        <datalist id="llm-models">{models.map((m) => <option key={m} value={m} />)}</datalist>
        <label className="lbl">人设 · 助手是谁、怎么说话</label>
        <textarea className="f ta" value={s.persona} onChange={set("persona")} />
        <label className="lbl">理财理念 · 助手给建议时遵循的原则</label>
        <textarea className="f ta" value={s.philosophy} onChange={set("philosophy")} />
        {hasLegacyPhilosophy(s.philosophy) && (
          <div className="notice warn" style={{ marginTop: 0, marginBottom: 10 }}>
            这是旧版理财理念,和“组合建议”冲突。点“恢复默认人设与理念”可换成新版,改过的内容请先复制。
          </div>
        )}
        {msg && <p className="muted" style={{ marginBottom: 8 }}>{msg}</p>}
        <button className="go">保存</button>
        <button type="button" className="cancel" onClick={async () => {
          if (!(await ask("人设和理财理念将恢复为默认内容,确定吗?", { title: "恢复默认", kind: "warning" }))) return;
          try { setS(await api.resetPrompts()); setMsg("已恢复默认"); } catch (err) { setMsg(errMsg(err)); }
        }}>恢复默认人设与理念</button>
        <button type="button" className="cancel" onClick={onClose}>关闭</button>
      </form>
    </div>
  );
}
