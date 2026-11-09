import { ReactNode, useState } from "react";
import { ACCOUNT_KINDS, Account, Txn, fmt, toCents } from "./api";
import type { Data } from "./App";
import { InsightsCard } from "./insights";

export const CAT_ICON: Record<string, string> = {
  餐饮: "🍜", 交通: "🚇", 购物: "🛍️", 居住: "🏠", 娱乐: "🎮", 医疗: "💊", 其他: "⋯",
  工资: "💼", 理财收益: "📈", 其他收入: "💰",
};
const KIND_ICON: Record<string, string> = { cash: "💵", deposit: "🏦", property: "🏠", investment: "📈", liability: "🧾" };
const KIND_COLOR: Record<string, string> = {
  property: "var(--c1)", deposit: "var(--c2)", investment: "var(--c3)", cash: "var(--c4)", liability: "var(--c5)",
};

const Head = ({ title, right }: { title: string; right?: ReactNode }) => (
  <div className="head"><h1>{title}</h1>{right}</div>
);

function TxnRow({ t }: { t: Txn }) {
  const exp = t.kind === "expense";
  return (
    <div className="row">
      <div className="ico">{CAT_ICON[t.category ?? ""] ?? "•"}</div>
      <div className="t"><b>{t.note || t.category || "未分类"}</b><span>{t.category ?? "未分类"}{t.account ? ` · ${t.account}` : ""} · {t.date.slice(5)}</span></div>
      <div className={`a num ${exp ? "neg" : "pos"}`}>{exp ? "−" : "+"}{fmt(t.amount)}</div>
    </div>
  );
}

export function Home({ d, nav, go, onAsk }: { d: Data; nav: ReactNode; go: (v: string) => void; onAsk: (q: string) => void }) {
  const { status, nw, accounts, txns } = d;
  if (!status || !nw) return null;

  const [y, m] = d.month.split("-").map(Number);
  const now = new Date();
  const isCurrent = now.getFullYear() === y && now.getMonth() + 1 === m;
  const daysInMonth = new Date(y, m, 0).getDate();
  const daysLeft = isCurrent ? daysInMonth - now.getDate() + 1 : 0;

  const pct = status.budget ? Math.min(100, (status.spent / status.budget) * 100) : 0;
  const remain = status.budget !== null ? status.budget - status.spent : 0;
  const [yuan, cents] = fmt(nw.net).split(".");

  const positive = accounts.filter((a) => a.kind !== "liability");
  const total = positive.reduce((s, a) => s + a.balance, 0);
  const byKind = Object.keys(ACCOUNT_KINDS).filter((k) => k !== "liability").map((k) => ({
    k, v: positive.filter((a) => a.kind === k).reduce((s, a) => s + a.balance, 0),
  })).filter((x) => x.v > 0);

  return (
    <>
      <Head title="总览" right={nav} />
      <div className="grid">
        <div className="card hero span7">
          <h3>净资产</h3>
          <div className="amt num"><small>¥</small>{yuan}<span style={{ fontSize: 20, opacity: 0.6 }}>.{cents}</span></div>
          <div className="sub">资产 ¥{fmt(nw.assets)} · 负债 ¥{fmt(nw.liabilities)}</div>
        </div>

        <div className="card span5">
          <h3>本月预算</h3>
          {status.budget !== null ? (
            <>
              <div className="ringwrap">
                <div className="ring">
                  <svg viewBox="0 0 120 120" width="128" height="128" style={{ transform: "rotate(-90deg)" }}>
                    <circle cx="60" cy="60" r="50" fill="none" stroke="var(--surface2)" strokeWidth="12" />
                    <circle cx="60" cy="60" r="50" fill="none" stroke={remain < 0 ? "var(--red)" : "#f5b700"} strokeWidth="12"
                      strokeLinecap="round" strokeDasharray="314" strokeDashoffset={314 - (314 * pct) / 100} />
                  </svg>
                  <div className="c"><b className="num">{Math.round(pct)}%</b><span>已使用</span></div>
                </div>
                <div className="kv">
                  <div><span>预算</span><b className="num">¥{fmt(status.budget)}</b></div>
                  <div><span>已支出</span><b className="num">¥{fmt(status.spent)}</b></div>
                  <div><span>{remain < 0 ? "超支" : "剩余"}</span>
                    <b className="num" style={{ color: remain < 0 ? "var(--red)" : "var(--green)" }}>¥{fmt(Math.abs(remain))}</b></div>
                </div>
              </div>
              {isCurrent && remain > 0 && (
                <div className="hint">还剩 <b>{daysLeft} 天</b>,日均可花 <b>¥{fmt(Math.floor(remain / daysLeft))}</b>。</div>
              )}
            </>
          ) : (
            <div className="empty">还没设置预算<br /><button className="lnk" onClick={() => go("budget")}>去设置</button></div>
          )}
        </div>

        <InsightsCard d={d} onAsk={onAsk} />

        <div className="card span7">
          <h3>最近账目 <button className="lnk" onClick={() => go("txn")}>查看全部</button></h3>
          {txns.slice(0, 5).map((t) => <TxnRow key={t.id} t={t} />)}
          {txns.length === 0 && <div className="empty">这个月还没有记录,点左下角“记一笔”</div>}
        </div>

        <div className="card span5">
          <h3>资产配置</h3>
          {total > 0 ? (
            <>
              <div className="alloc">{byKind.map((x) => <i key={x.k} style={{ width: `${(x.v / total) * 100}%`, background: KIND_COLOR[x.k] }} />)}</div>
              <div className="legend">
                {byKind.map((x) => (
                  <div key={x.k}><span className="dot" style={{ background: KIND_COLOR[x.k] }} />{ACCOUNT_KINDS[x.k]}<em>{Math.round((x.v / total) * 100)}%</em></div>
                ))}
              </div>
            </>
          ) : <div className="empty">添加资产后显示配置比例</div>}
        </div>
      </div>
    </>
  );
}

export function Txns({ d, nav, onDelete, onImport }: { d: Data; nav: ReactNode; onDelete: (id: number) => void; onImport: () => void }) {
  const income = d.status?.income ?? 0, spent = d.status?.spent ?? 0;
  const stat = (label: string, v: string, color?: string) => (
    <div><h3>{label}</h3><div className="num" style={{ fontSize: 24, fontWeight: 700, color }}>{v}</div></div>
  );
  return (
    <>
      <Head title="账目" right={<div className="head-acts"><button className="btn alt" onClick={onImport} title="也可以直接把账单文件拖进窗口">导入账单</button>{nav}</div>} />
      <div className="grid">
        <div className="card span12" style={{ display: "flex", gap: 40 }}>
          {stat("收入", `+¥${fmt(income)}`, "var(--green)")}
          {stat("支出", `−¥${fmt(spent)}`)}
          {stat("结余", `${income - spent < 0 ? "-" : ""}¥${fmt(Math.abs(income - spent))}`)}
        </div>
        <div className="card span12">
          {d.txns.map((t) => (
            <div key={t.id} style={{ display: "flex", alignItems: "center", gap: 10 }}>
              <div style={{ flex: 1 }}><TxnRow t={t} /></div>
              <button className="lnk" onClick={() => onDelete(t.id)}>删除</button>
            </div>
          ))}
          {d.txns.length === 0 && <div className="empty">这个月还没有记录</div>}
        </div>
      </div>
    </>
  );
}

export function Assets({ d, onAdd, onValue }: {
  d: Data;
  onAdd: (a: { name: string; kind: string; currency: string; balance: number; date: string }) => void;
  onValue: (id: number, cents: number) => void;
}) {
  const [name, setName] = useState("");
  const [kind, setKind] = useState("deposit");
  const [balance, setBalance] = useState("");
  const [editing, setEditing] = useState<Account | null>(null);
  const date = new Date().toLocaleDateString("sv-SE");
  return (
    <>
      <Head title="资产" />
      <div className="grid">
        <div className="card span12">
          <form className="frm" onSubmit={(e) => {
            e.preventDefault();
            const c = toCents(balance);
            if (!name.trim() || !Number.isFinite(c)) return;
            onAdd({ name: name.trim(), kind, currency: "CNY", balance: c, date });
            setName(""); setBalance("");
          }}>
            <input className="inp" style={{ flex: "2 1 200px" }} placeholder="名称,如 招行储蓄卡、深圳住宅" value={name} onChange={(e) => setName(e.target.value)} required />
            <select className="inp" value={kind} onChange={(e) => setKind(e.target.value)}>
              {Object.entries(ACCOUNT_KINDS).map(([k, v]) => <option key={k} value={k}>{v}</option>)}
            </select>
            <input className="inp" type="number" step="0.01" placeholder="当前金额(元)" value={balance} onChange={(e) => setBalance(e.target.value)} required />
            <button className="btn">添加</button>
          </form>
          {d.accounts.map((a) => (
            <div className="row" key={a.id}>
              <div className="ico">{KIND_ICON[a.kind]}</div>
              <div className="t"><b>{a.name}<span className="badge">{ACCOUNT_KINDS[a.kind]}</span></b></div>
              <button className="lnk" onClick={() => setEditing(a)}>更新余额/估值</button>
              <div className="a num">{a.kind === "liability" ? "-" : ""}¥{fmt(a.balance)}</div>
            </div>
          ))}
          {d.accounts.length === 0 && <div className="empty">还没有资产,先添加一个吧</div>}
          <p className="muted" style={{ marginTop: 10 }}>记账时选了账户,余额会自动增减;登记了持仓的投资账户,余额等于持仓合计,随行情自动更新;房产等其他资产请手动更新估值。</p>
        </div>
      </div>
      {editing && (
        <ValueModal account={editing} onClose={() => setEditing(null)}
          onSave={(c) => { onValue(editing.id, c); setEditing(null); }} />
      )}
    </>
  );
}

function ValueModal({ account, onClose, onSave }: { account: Account; onClose: () => void; onSave: (cents: number) => void }) {
  const [v, setV] = useState(String(account.balance / 100));
  const cents = toCents(v);
  return (
    <div className="modal on" onClick={(e) => e.target === e.currentTarget && onClose()}>
      <form className="sheet" onSubmit={(e) => { e.preventDefault(); if (Number.isFinite(cents) && cents >= 0) onSave(cents); }}>
        <h2>更新「{account.name}」</h2>
        <p className="muted" style={{ marginBottom: 10 }}>今天的实际金额(元)</p>
        <input className="f" type="number" step="0.01" min="0" autoFocus value={v} onChange={(e) => setV(e.target.value)} required />
        <button className="go">保存</button>
        <button type="button" className="cancel" onClick={onClose}>取消</button>
      </form>
    </div>
  );
}

export function Budget({ d, nav, onBudget }: { d: Data; nav: ReactNode; onBudget: (cents: number) => void }) {
  const [input, setInput] = useState("");
  const s = d.status;
  const spent = s?.spent ?? 0;
  const pct = s?.budget ? Math.min(100, (spent / s.budget) * 100) : 0;

  // 各分类本月支出(分类预算额度后端已支持,界面后续再加)
  const byCat = new Map<string, number>();
  d.txns.filter((t) => t.kind === "expense").forEach((t) => {
    const k = t.category ?? "未分类";
    byCat.set(k, (byCat.get(k) ?? 0) + t.amount);
  });
  const cats = [...byCat.entries()].sort((a, b) => b[1] - a[1]);

  return (
    <>
      <Head title="预算" right={nav} />
      <div className="grid">
        <div className="card span12">
          <h3>月度总预算</h3>
          <div className="num" style={{ fontSize: 30, fontWeight: 700 }}>
            {s?.budget != null ? `¥${fmt(s.budget)}` : "未设置"}
            <span style={{ fontSize: 14, color: "var(--muted)", fontWeight: 500 }}> 已用 ¥{fmt(spent)}</span>
          </div>
          <div className="catbar" style={{ height: 10, marginTop: 12 }}>
            <i style={{ width: `${pct}%`, background: s?.budget != null && spent > s.budget ? "var(--red)" : "var(--accent)" }} />
          </div>
          <form className="frm" style={{ marginTop: 16 }} onSubmit={(e) => {
            e.preventDefault();
            const c = toCents(input);
            if (Number.isFinite(c) && c >= 0) { onBudget(c); setInput(""); }
          }}>
            <input className="inp" type="number" step="0.01" min="0" placeholder="设置本月预算(元)" value={input} onChange={(e) => setInput(e.target.value)} />
            <button className="btn">保存</button>
          </form>
        </div>
        <div className="card span12">
          <h3>支出分类</h3>
          {cats.map(([name, v]) => (
            <div className="row" key={name}>
              <div className="ico">{CAT_ICON[name] ?? "•"}</div>
              <div className="t" style={{ flex: 1 }}>
                <b>{name}</b>
                <div className="catbar"><i style={{ width: `${(v / (cats[0][1] || 1)) * 100}%`, background: "var(--accent)" }} /></div>
              </div>
              <div className="a num">¥{fmt(v)}</div>
            </div>
          ))}
          {cats.length === 0 && <div className="empty">这个月还没有支出</div>}
        </div>
      </div>
    </>
  );
}
