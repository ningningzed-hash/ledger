import { useAuth } from "./auth";
import { useEffect, useRef, useState } from "react";
import { Account, Category, ChatMessage, TxnDraft, Trace, ActionDraft, useApi, fmt, thisMonth, toCents } from "./api";
import { Markdown } from "./markdown";
import { modelLabel } from "./modelLabel";
import { CHAT_WIDTH_KEY, DEFAULT_CHAT_WIDTH, clampChatWidth, readChatWidth } from "./chatSize";

const errMsg = (e: unknown) => (typeof e === "string" ? e : String(e));

type DraftState = { draft: TxnDraft; status: "pending" | "done" | "cancelled"; result?: string };
type Msg = ChatMessage & { drafts?: DraftState[]; review?: string; actions?: ActionDraft[]; traces?: Trace[] };

/** 外部(洞察卡「问助手」等)请求发送的问题;n 变化才会再次发送 */
export type AskRequest = { text: string; n: number; capability?: string };

/**
 * 全局助手抽屉:任意页面 ⌘K / Ctrl+K 或右下角悬浮球唤起。
 * context 是当前页面的说明,随每条消息发给助手。
 */
export function ChatDrawer({ open, onClose, context, ask, accounts, cats, onChanged, onSettings, settingsOpen, onImport }: {
  open: boolean; onClose: () => void; context: string; ask: AskRequest | null;
  accounts: Account[]; cats: Category[]; onChanged: () => void;
  onSettings: () => void; settingsOpen: boolean; onImport: () => void;
}) {
  const api = useApi();
  const [label, setLabel] = useState("");
  const [msgs, setMsgs] = useState<Msg[]>([]);
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<Trace[]>([]);
  const box = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const { user } = useAuth();
  const widthKey = `${CHAT_WIDTH_KEY}.${user.id}`;
  const [width, setWidth] = useState(() => readChatWidth(widthKey));
  const [viewport, setViewport] = useState(window.innerWidth);
  const [expanded, setExpanded] = useState(false);
  const [resizing, setResizing] = useState(false);
  const drag = useRef<{ x: number; width: number } | null>(null);
  const visibleWidth = expanded ? viewport : clampChatWidth(width, viewport);
  useEffect(() => {
    const onResize = () => setViewport(window.innerWidth);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  useEffect(() => {
    const timer = window.setTimeout(() => {
      try { localStorage.setItem(widthKey, String(width)); } catch { /* 仍可在本次会话调整大小 */ }
    }, 200);
    return () => window.clearTimeout(timer);
  }, [width, widthKey]);
  useEffect(() => { if (!open) { drag.current = null; setResizing(false); } }, [open]);

  // 设置窗口关闭后重新读取,副标题随模型配置更新
  useEffect(() => {
    if (!settingsOpen) api.getSettings().then((s) => setLabel(modelLabel(s.llm_url, s.llm_model))).catch(() => {});
  }, [settingsOpen]);
  useEffect(() => { api.listChat().then(setMsgs).catch(() => {}); }, []);
  useEffect(() => { box.current?.scrollTo(0, box.current.scrollHeight); }, [msgs, open]);
  useEffect(() => { if (open) inputRef.current?.focus(); }, [open]);

  const send = async (text: string, capability?: string) => {
    text = text.trim();
    if (!text || busy) return;
    setInput(""); setBusy(true); setProgress([]);
    setMsgs((m) => [...m, { role: "user", content: text }]);
    const execution: Trace[] = [];
    try {
      const r = await api.sendChat(text, context, capability ?? null, (event) => { execution.push(event); setProgress([...execution]); });
      setMsgs((m) => [...m, { role: "assistant", content: r.text, review: capability === "monthly-review" ? thisMonth() : undefined, actions: r.actions, traces: r.traces, drafts: r.drafts.map((draft) => ({ draft, status: "pending" })) }]);
      onChanged();
    } catch (e) {
      setMsgs((m) => [...m, { role: "assistant", content: errMsg(e), error: true, traces: execution }]);
    } finally { setBusy(false); }
  };
  const sendRef = useRef(send);
  sendRef.current = send;
  const consumedAsk = useRef<number | null>(null);
  useEffect(() => {
    if (ask && !busy && consumedAsk.current !== ask.n) {
      consumedAsk.current = ask.n;
      void sendRef.current(ask.text, ask.capability);
    }
  }, [ask, busy]);

  const review = () => send(`请复盘 ${thisMonth()} 的收支与预算`, "monthly-review");

  const setDraft = (mi: number, di: number, patch: Partial<DraftState>) =>
    setMsgs((ms) => ms.map((m, i) => (i === mi && m.drafts ? { ...m, drafts: m.drafts.map((d, j) => (j === di ? { ...d, ...patch } : d)) } : m)));

  return (
    <>
      {open && <div className="drawer-mask" onClick={onClose} />}
      <aside className={`chat drawer${open ? " open" : ""}${resizing ? " resizing" : ""}`} style={{ width: visibleWidth }}
        inert={!open} aria-hidden={!open} aria-label="财务助手"
        onKeyDown={(e) => { if (e.key === "Escape" && !settingsOpen) { e.stopPropagation(); onClose(); } }}>
        <div className="chat-resize" role="separator" tabIndex={0} aria-label="调整聊天窗口宽度"
          aria-orientation="vertical" aria-valuemin={Math.min(360, viewport)} aria-valuemax={viewport} aria-valuenow={Math.round(visibleWidth)}
          title="拖动调整宽度，双击恢复默认；也可用左右方向键"
          onDoubleClick={() => { setExpanded(false); setWidth(DEFAULT_CHAT_WIDTH); }}
          onPointerDown={(e) => {
            if (e.button !== 0) return;
            e.preventDefault(); e.currentTarget.focus();
            e.currentTarget.setPointerCapture(e.pointerId);
            drag.current = { x: e.clientX, width: visibleWidth };
            setExpanded(false); setWidth(visibleWidth); setResizing(true);
          }}
          onPointerMove={(e) => {
            if (drag.current) setWidth(clampChatWidth(drag.current.width + drag.current.x - e.clientX, window.innerWidth));
          }}
          onPointerUp={(e) => {
            drag.current = null; setResizing(false);
            if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
          }}
          onLostPointerCapture={() => { drag.current = null; setResizing(false); }}
          onPointerCancel={() => { drag.current = null; setResizing(false); }}
          onKeyDown={(e) => {
            if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(e.key)) return;
            e.preventDefault(); setExpanded(false);
            setWidth(clampChatWidth(e.key === "Home" ? DEFAULT_CHAT_WIDTH : e.key === "End" ? viewport : visibleWidth + (e.key === "ArrowLeft" ? 40 : -40), viewport));
          }}><span /></div>
        <header>
          <div className="av">✦</div>
          <div>财务助手<small>{label}</small></div>
          <button className="gear" title="助手设置" onClick={onSettings}>⚙</button>
          <button className="gear close" title={expanded ? "还原窗口宽度" : "展开到整个窗口"} aria-label={expanded ? "还原窗口宽度" : "展开聊天窗口"} aria-pressed={expanded} onClick={() => setExpanded((v) => !v)}>{expanded ? "⊡" : "⛶"}</button>
          <button className="gear close" title="收起(Esc)" aria-label="收起助手" onClick={onClose}>×</button>
        </header>
        <div className="chat-size-hint">拖动左侧边缘调整宽度 · 双击边缘恢复默认</div>
        <div className="msgs" ref={box}>
          {msgs.length === 0 && <div className="m a">你好!可以说“午饭 35”记一笔,问“上个月打车花了多少”,或者把微信 / 支付宝账单拖进窗口,我来帮你记。</div>}
          {msgs.map((m, i) => (
            <div key={i} className={`m ${m.role === "user" ? "u" : "a"}${m.error ? " err" : ""}${m.drafts?.length || m.review ? " rich" : ""}`}>
              {m.role === "assistant" && !m.error ? <Markdown text={m.content} /> : m.content}
              {m.traces && <TraceList traces={m.traces} />}
              {m.actions?.map((action, j) => <ActionCard key={j} action={action} onChanged={onChanged} />)}
              {m.drafts?.map((d, j) => (
                <DraftCard key={j} state={d} accounts={accounts} cats={cats}
                  onChange={(draft) => setDraft(i, j, { draft })}
                  onCancel={() => setDraft(i, j, { status: "cancelled" })}
                  onConfirm={async () => {
                    const result = await api.confirmDraft(d.draft);
                    setDraft(i, j, { status: "done", result });
                    onChanged();
                  }} />
              ))}
              {m.review && <SaveReview month={m.review} content={m.content} />}
            </div>
          ))}
          {busy && <div className="m a"><div className="thinking"><i /><i /><i /></div><p role="status">{progress[progress.length - 1]?.name} · {progress[progress.length - 1]?.detail ?? "正在准备"}</p><TraceList traces={progress} /></div>}
        </div>
        <div className="chips">
          <button disabled={busy} onClick={review}>本月复盘</button>
          <button disabled={busy} onClick={onImport}>导入账单</button>
          <button disabled={busy} onClick={() => send("这个月钱主要花在哪了?和上个月比呢?")}>钱花在哪了?</button>
          <button disabled={busy} onClick={() => send("帮我看看资产配置是否合理")}>资产配置体检</button>
        </div>
        <form onSubmit={(e) => { e.preventDefault(); send(input); }}>
          <button type="button" className="attach" title="导入微信 / 支付宝账单" aria-label="导入账单" onClick={onImport}>📎</button>
          <input ref={inputRef} value={input} onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => e.key === "Escape" && onClose()}
            placeholder={busy ? "助手思考中…" : "问问你的账,或说“午饭 35”"} />
          <button disabled={busy}>↑</button>
        </form>
      </aside>
    </>
  );
}

/** 记账确认卡片:金额 / 分类 / 账户 / 日期 / 备注都可以改,点确认才入账 */
function DraftCard({ state, accounts, cats, onChange, onConfirm, onCancel }: {
  state: DraftState; accounts: Account[]; cats: Category[];
  onChange: (d: TxnDraft) => void; onConfirm: () => Promise<void>; onCancel: () => void;
}) {
  const { draft: d, status } = state;
  const [amount, setAmount] = useState((d.amount / 100).toFixed(2));
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  const cents = toCents(amount);
  const income = d.kind === "income";

  if (status !== "pending") {
    return (
      <div className={`draft done${status === "cancelled" ? " cancelled" : ""}`}>
        {status === "done" ? <>✓ {state.result}</> : <>已取消:{income ? "收入" : "支出"} ¥{fmt(d.amount)}{d.note && ` · ${d.note}`}</>}
      </div>
    );
  }
  return (
    <form className="draft" onSubmit={async (e) => {
      e.preventDefault();
      if (!Number.isFinite(cents) || cents <= 0) return setErr("金额需大于 0");
      setBusy(true); setErr("");
      try { await onConfirm(); } catch (x) { setErr(errMsg(x)); setBusy(false); }
    }}>
      <div className="draft-head">
        <span className={`chip ${income ? "good" : "muted"}`}>{income ? "+ 收入" : "− 支出"}</span>
        <span className="muted">待确认</span>
      </div>
      <label className="draft-amt">
        <span>{income ? "+¥" : "−¥"}</span>
        <input className="num" type="number" step="0.01" min="0.01" value={amount} aria-label="金额"
          onChange={(e) => { setAmount(e.target.value); const c = toCents(e.target.value); if (Number.isFinite(c) && c > 0) onChange({ ...d, amount: c }); }} />
      </label>
      <div className="draft-grid">
        <select className="inp" value={d.category_id ?? ""} aria-label="分类"
          onChange={(e) => { const id = e.target.value ? +e.target.value : null; onChange({ ...d, category_id: id, category: cats.find((c) => c.id === id)?.name ?? null }); }}>
          <option value="">不分类</option>
          {cats.filter((c) => c.kind === d.kind).map((c) => <option key={c.id} value={c.id}>{c.name}</option>)}
        </select>
        <select className="inp" value={d.account_id ?? ""} aria-label="账户"
          onChange={(e) => { const id = e.target.value ? +e.target.value : null; onChange({ ...d, account_id: id, account: accounts.find((a) => a.id === id)?.name ?? null }); }}>
          <option value="">不关联账户</option>
          {accounts.filter((a) => a.kind !== "property").map((a) => <option key={a.id} value={a.id}>{a.name}</option>)}
        </select>
        <input className="inp" type="date" value={d.date} aria-label="日期" onChange={(e) => e.target.value && onChange({ ...d, date: e.target.value })} />
        <input className="inp" value={d.note} placeholder="备注" aria-label="备注" onChange={(e) => onChange({ ...d, note: e.target.value })} />
      </div>
      {err && <div className="draft-err">{err}</div>}
      <div className="draft-acts">
        <button type="button" className="btn alt" disabled={busy} onClick={onCancel}>取消</button>
        <button className="btn" disabled={busy}>{busy ? "入账中…" : "确认入账"}</button>
      </div>
    </form>
  );
}

function SaveReview({ month, content }: { month: string; content: string }) {
  const api = useApi();
  const [state, setState] = useState<"idle" | "busy" | "saved">("idle");
  const [err, setErr] = useState("");
  return (
    <div className="review-save">
      <button className="btn alt" disabled={state !== "idle"} onClick={async () => {
        setState("busy"); setErr("");
        try { await api.saveReview(month, content); setState("saved"); } catch (e) { setErr(errMsg(e)); setState("idle"); }
      }}>{state === "saved" ? "✓ 已保存到报表" : state === "busy" ? "保存中…" : "保存到报表"}</button>
      {err && <span className="draft-err">{err}</span>}
    </div>
  );
}

function TraceList({ traces }: { traces: Trace[] }) {
  if (!traces.length) return null;
  return <details className="cap-traces"><summary>执行记录 · {traces.length} 步</summary><ol>{traces.map((t, i) => <li key={i} data-status={t.status}><b>{t.status === "error" ? "!" : t.status === "success" ? "✓" : "…"} {t.name}</b><span>{t.detail}</span></li>)}</ol></details>;
}
function ActionCard({ action, onChanged }: { action: ActionDraft; onChanged: () => void }) {
  const api = useApi();
  const [status, setStatus] = useState<"pending" | "busy" | "done" | "cancelled">("pending");
  const [error, setError] = useState("");
  return <div className="draft"><b>{action.tool === "set_monthly_budget" ? "预算调整" : "估值更新"}</b><p>{action.summary}</p>
    {error && <p className="draft-err" role="alert">{error}</p>}
    {status === "done" ? <span>✓ 已保存</span> : status === "cancelled" ? <span>已取消</span> : <div className="draft-acts">
      <button className="btn alt" disabled={status === "busy"} onClick={() => setStatus("cancelled")}>取消</button>
      <button className="btn" disabled={status === "busy"} onClick={async () => {setStatus("busy");setError("");try {await api.confirmAction(action);setStatus("done");onChanged();} catch(e) {setStatus("pending");setError(errMsg(e));}}}>{status === "busy" ? "保存中…" : "确认修改"}</button>
    </div>}
  </div>;
}
