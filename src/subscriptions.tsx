import { useEffect, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { Account, Category, SubFrequency, SubPreview, Subscription, SubscriptionInput, useApi, fmt, toCents, today } from "./api";
import { STATUS_TEXT, detailText, monthlyHint, nextText, previewText, scheduleText } from "./subText";

const errText = (e: unknown) => (typeof e === "string" ? e : String(e));

/** 日期不在中间换行 */
function Dates({ text }: { text: string }) {
  return (
    <>
      {text.split(/(\d{4}-\d{2}-\d{2})/).map((p, i) =>
        /^\d{4}-\d{2}-\d{2}$/.test(p) ? <span key={i} style={{ whiteSpace: "nowrap" }}>{p}</span> : p)}
    </>
  );
}

export function SubscriptionsView({ accounts, cats, onPosted }: {
  accounts: Account[]; cats: Category[]; onPosted: (n: number) => void;
}) {
  const api = useApi();
  const [data, setData] = useState<Awaited<ReturnType<typeof api.listSubscriptions>> | null>(null);
  const [error, setError] = useState("");
  const [editing, setEditing] = useState<Subscription | "new" | null>(null);

  const load = () =>
    api.listSubscriptions().then((d) => { setData(d); setError(""); }).catch((e) => setError(errText(e)));

  // 打开页面时先让到期的账目入账,再显示列表,这样看到的"已入账"是最新的
  useEffect(() => {
    api.runSubscriptions()
      .then((r) => { if (r.posted > 0) onPosted(r.posted); })
      .catch(() => {})
      .finally(load);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const changed = async (posted: number) => {
    if (posted > 0) onPosted(posted);
    await load();
  };

  const togglePause = async (s: Subscription) => {
    if (s.paused) {
      const yes = await ask(`恢复「${s.name}」?\n从下一期起继续自动记账;暂停期间错过的几期不会补记。`, { title: "恢复订阅" });
      if (!yes) return;
    }
    try {
      await changed(await api.updateSubscription(s.id, {
        name: s.name, amount: s.amount, end_date: s.end_date, category_id: s.category_id, account_id: s.account_id, paused: !s.paused,
      }));
    } catch (e) { setError(errText(e)); }
  };

  const remove = async (s: Subscription) => {
    const yes = await ask(`删除订阅「${s.name}」?\n已经自动记入的账目会保留,以后不再自动记账。`, { title: "删除订阅", kind: "warning" });
    if (!yes) return;
    try { await api.deleteSubscription(s.id); await load(); } catch (e) { setError(errText(e)); }
  };

  const items = data?.items ?? [];
  const hasYearly = items.some((i) => i.frequency === "yearly" && i.status === "active");
  const t = today();

  return (
    <>
      <div className="head">
        <h1>订阅</h1>
        <button className="btn" onClick={() => setEditing("new")}>＋ 新建订阅</button>
      </div>
      {error && <div className="notice bad" style={{ marginBottom: 12 }}>{error}</div>}
      {!data && !error && <div className="empty">加载中…</div>}

      {data && items.length > 0 && (
        <div className="grid" style={{ marginBottom: 16 }}>
          <div className="card tile" style={{ gridColumn: "span 6" }}>
            <div className="muted">每月固定支出</div>
            <div className="tv num">¥{fmt(data.monthly_expense)}</div>
            <div className="muted">{monthlyHint(hasYearly)}</div>
          </div>
          <div className="card tile" style={{ gridColumn: "span 6" }}>
            <div className="muted">每月固定收入</div>
            <div className="tv num">¥{fmt(data.monthly_income)}</div>
            <div className="muted">固定结余 {data.monthly_income - data.monthly_expense < 0 ? "-" : ""}¥{fmt(Math.abs(data.monthly_income - data.monthly_expense))}</div>
          </div>
        </div>
      )}

      {data && items.length === 0 && (
        <div className="card">
          <div className="empty">
            还没有订阅<br />
            <span className="muted">房租、房贷、工资、会员费……每月或每年固定重复的收支,设好之后到期会自动记账。</span>
            <div style={{ marginTop: 14 }}><button className="btn" onClick={() => setEditing("new")}>新建第一个订阅</button></div>
          </div>
        </div>
      )}

      {items.length > 0 && (
        <div className="card sub-list">
          {items.map((s) => {
            const st = STATUS_TEXT[s.status];
            return (
              <div className={`sub-row${s.status !== "active" ? " off" : ""}`} key={s.id}>
                <div className="sub-main">
                  <div className="sub-title">
                    <b>{s.name}</b>
                    <span className="badge">{s.kind === "income" ? "收入" : "支出"}</span>
                    {s.status !== "active" && <span className={`chip ${st.tone}`}>{st.text}</span>}
                  </div>
                  <div className="muted">{scheduleText(s)} · {nextText(s, t)}</div>
                  <div className="muted sub-detail">{detailText(s)}</div>
                </div>
                <div className="sub-side">
                  <b className={`num sub-amt ${s.kind === "income" ? "pos" : ""}`}>{s.kind === "income" ? "+" : "-"}¥{fmt(s.amount)}</b>
                  <div className="muted">{s.frequency === "yearly" ? `每年 · 折合每月 ¥${fmt(s.monthly_equivalent)}` : "每月"}</div>
                  <div className="sub-actions">
                    <button className="lnk" onClick={() => setEditing(s)}>编辑</button>
                    {s.status !== "ended" && <button className="lnk" onClick={() => togglePause(s)}>{s.paused ? "恢复" : "暂停"}</button>}
                    <button className="lnk" onClick={() => remove(s)}>删除</button>
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      )}
      {items.length > 0 && <p className="muted" style={{ margin: "12px 2px" }}>到期的账目会自动记入账目,删掉的不会再补记。</p>}

      {editing && (
        <SubModal sub={editing === "new" ? null : editing} accounts={accounts} cats={cats} onClose={() => setEditing(null)}
          onSaved={async (posted) => { setEditing(null); await changed(posted); }} />
      )}
    </>
  );
}

function SubModal({ sub, accounts, cats, onClose, onSaved }: {
  sub: Subscription | null; accounts: Account[]; cats: Category[]; onClose: () => void; onSaved: (posted: number) => void;
}) {
  const api = useApi();
  const edit = sub !== null;
  const [kind, setKind] = useState<"expense" | "income">(sub?.kind ?? "expense");
  const [name, setName] = useState(sub?.name ?? "");
  const [amount, setAmount] = useState(sub ? String(sub.amount / 100) : "");
  const [frequency, setFrequency] = useState<SubFrequency>(sub?.frequency ?? "monthly");
  const [start, setStart] = useState(sub?.start_date ?? today());
  const [end, setEnd] = useState(sub?.end_date ?? "");
  const [categoryId, setCategoryId] = useState(sub?.category_id ? String(sub.category_id) : "");
  const [accountId, setAccountId] = useState(sub?.account_id ? String(sub.account_id) : "");
  const [paused, setPaused] = useState(sub?.paused ?? false);
  const [preview, setPreview] = useState<SubPreview | null>(null);
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);

  // 新建时,随着日期/频率/结束日期的变化,向后端询问"保存后会立刻补记几笔"
  useEffect(() => {
    if (edit || !start) { setPreview(null); return; }
    let alive = true;
    const timer = setTimeout(() => {
      api.previewSubscription(frequency, start, end || null)
        .then((p) => { if (alive) setPreview(p); })
        .catch(() => { if (alive) setPreview(null); });
    }, 200);
    return () => { alive = false; clearTimeout(timer); };
  }, [edit, frequency, start, end]);

  const catOptions = cats.filter((c) => c.kind === kind);
  const accOptions = accounts.filter((a) => a.kind !== "property");
  const pv = preview ? previewText(preview) : null;

  const switchKind = (k: "expense" | "income") => {
    if (k === kind) return;
    setKind(k);
    setCategoryId(""); // 两种收支的分类不同
  };

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    const cents = toCents(amount);
    if (!name.trim()) return setErr("请填写订阅名称");
    if (!Number.isFinite(cents) || cents <= 0) return setErr("请填写大于 0 的金额");
    if (!start) return setErr("请选择开始日期");
    if (pv?.tone === "bad") return setErr(pv.text);
    setBusy(true);
    try {
      const common = { name: name.trim(), amount: cents, end_date: end || null, category_id: categoryId ? +categoryId : null, account_id: accountId ? +accountId : null };
      if (sub) {
        onSaved(await api.updateSubscription(sub.id, { ...common, paused }));
      } else {
        const input: SubscriptionInput = { ...common, kind, frequency, start_date: start };
        onSaved((await api.addSubscription(input)).posted);
      }
    } catch (ex) {
      setErr(errText(ex));
      setBusy(false);
    }
  };

  return (
    <div className="modal on" onClick={(e) => e.target === e.currentTarget && onClose()}>
      <form className="sheet" onSubmit={submit}>
        <h2>{edit ? `编辑「${sub.name}」` : "新建订阅"}</h2>
        {!edit && (
          <div className="tog">
            <button type="button" className={kind === "expense" ? "on" : ""} onClick={() => switchKind("expense")}>支出</button>
            <button type="button" className={kind === "income" ? "on" : ""} onClick={() => switchKind("income")}>收入</button>
          </div>
        )}

        <label className="lbl">名称</label>
        <input className="f" autoFocus maxLength={60} placeholder={kind === "expense" ? "如:房租、视频会员" : "如:工资"} value={name} onChange={(e) => setName(e.target.value)} />

        <label className="lbl">金额(元){edit && ",只影响以后的账目"}</label>
        <input className="f" type="number" step="0.01" min="0.01" value={amount} onChange={(e) => setAmount(e.target.value)} />

        {edit ? (
          <p className="muted" style={{ margin: "0 0 10px" }}>
            周期:{scheduleText(sub)}(不能修改,需要的话删除后重建)
          </p>
        ) : (
          <>
            <label className="lbl">重复周期</label>
            <select className="f" value={frequency} onChange={(e) => setFrequency(e.target.value as SubFrequency)}>
              <option value="monthly">每月</option>
              <option value="yearly">每年</option>
            </select>
            <label className="lbl">{frequency === "monthly" ? "开始日期(每月这天记账)" : "开始日期(每年这天记账)"}</label>
            <input className="f" type="date" value={start} onChange={(e) => setStart(e.target.value)} />
            {pv && <div className={`notice ${pv.tone}`} style={{ margin: "0 0 10px" }}><Dates text={pv.text} /></div>}
          </>
        )}

        <label className="lbl">结束日期(可不填)</label>
        <div className="row-inline">
          <input className="f" type="date" value={end} onChange={(e) => setEnd(e.target.value)} />
          {end && <button type="button" className="lnk" onClick={() => setEnd("")}>清除</button>}
        </div>

        <label className="lbl">分类(可不选)</label>
        <select className="f" value={categoryId} onChange={(e) => setCategoryId(e.target.value)}>
          <option value="">不分类</option>
          {catOptions.map((c) => <option key={c.id} value={c.id}>{c.name}</option>)}
        </select>

        <label className="lbl">关联账户(可不选)</label>
        <select className="f" value={accountId} onChange={(e) => setAccountId(e.target.value)}>
          <option value="">不关联账户</option>
          {accOptions.map((a) => <option key={a.id} value={a.id}>{a.name}</option>)}
        </select>

        {edit && (
          <label className="check">
            <input type="checkbox" checked={paused} onChange={(e) => setPaused(e.target.checked)} />
            <span>暂停自动记账<em>(恢复后从下一期继续,暂停期间的不补记)</em></span>
          </label>
        )}

        {err && <div className="notice bad" style={{ margin: "4px 0 10px" }}>{err}</div>}
        <button className="go" disabled={busy}>{edit ? "保存" : pv && pv.tone === "warn" ? "保存并补记" : "保存"}</button>
        <button type="button" className="cancel" onClick={onClose}>取消</button>
      </form>
    </div>
  );
}

