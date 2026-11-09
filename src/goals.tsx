import { useEffect, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { Account, Goal, GoalInput, GoalKind, useApi, fmt, toCents } from "./api";
import { STATUS_LABEL, describeGoal, sortGoals } from "./goalText";

const errText = (e: unknown) => (typeof e === "string" ? e : String(e));
const KIND_LABEL: Record<GoalKind, string> = { save: "存钱", payoff: "还债" };

export function GoalsView({ accounts }: { accounts: Account[] }) {
  const api = useApi();
  const [goals, setGoals] = useState<Goal[] | null>(null);
  const [error, setError] = useState("");
  const [editing, setEditing] = useState<Goal | "new" | null>(null);

  const load = () =>
    api.listGoals()
      .then((g) => { setGoals(g); setError(""); })
      .catch((e) => setError(errText(e)));
  useEffect(() => { load(); }, []);

  const remove = async (g: Goal) => {
    const yes = await ask(`删除目标「${g.name}」?\n关联的账户和账目不受影响。`, { title: "删除目标", kind: "warning" });
    if (!yes) return;
    try { await api.deleteGoal(g.id); await load(); } catch (e) { setError(errText(e)); }
  };

  return (
    <>
      <div className="head">
        <h1>目标</h1>
        <button className="btn" onClick={() => setEditing("new")}>＋ 新建目标</button>
      </div>
      {error && <div className="notice bad" style={{ marginBottom: 12 }}>{error}</div>}
      {!goals && !error && <div className="empty">加载中…</div>}
      {goals && goals.length === 0 && (
        <div className="card">
          <div className="empty">
            还没有目标<br />
            <span className="muted">比如“两年攒 20 万”“年底前还清信用卡”。目标关联一个或几个账户,进度随账户余额自动更新。</span>
            <div style={{ marginTop: 14 }}><button className="btn" onClick={() => setEditing("new")}>新建第一个目标</button></div>
          </div>
        </div>
      )}
      <div className="goal-list">
        {goals && sortGoals(goals).map((g) => <GoalCard key={g.id} g={g} onEdit={() => setEditing(g)} onDelete={() => remove(g)} />)}
      </div>
      {editing && (
        <GoalModal goal={editing === "new" ? null : editing} accounts={accounts} onClose={() => setEditing(null)}
          onSaved={() => { setEditing(null); load(); }} />
      )}
    </>
  );
}

function GoalCard({ g, onEdit, onDelete }: { g: Goal; onEdit: () => void; onDelete: () => void }) {
  const t = describeGoal(g);
  const st = STATUS_LABEL[g.status];
  const pct = Math.round(g.progress * 100);
  // 进度条颜色表示状态:正常=主色,落后=橙,逾期=红,达成=绿;状态同时有文字标签,不只靠颜色
  const tone = g.achieved ? "good" : g.status === "overdue" ? "bad" : g.status === "behind" ? "warn" : "";
  return (
    <div className="card goal">
      <div className="goal-top">
        <div className="goal-name">
          <b>{g.name}</b><span className="badge">{KIND_LABEL[g.kind]}</span>
        </div>
        <span className={`chip ${st.tone}`}><i aria-hidden>{st.icon}</i>{st.text}</span>
      </div>

      <div className="goal-nums">
        <div><span className="num goal-cur">¥{fmt(g.current)}</span><span className="muted"> / {g.kind === "save" ? "目标" : "还清线"} ¥{fmt(g.target)}</span></div>
        <b className="num goal-pct">{pct}%</b>
      </div>
      <div className="meter" role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100} aria-label={`${g.name} 进度`}>
        <i className={tone} style={{ width: `${Math.max(pct, g.progress > 0 ? 1.5 : 0)}%` }} />
      </div>

      <ul className="goal-facts">
        <li>{t.remaining}{t.deadline && <> · {t.deadline}</>}{t.required && <> · {t.required}</>}</li>
        {t.pace && <li className="muted">{t.pace}{t.projected && <> · {t.projected}</>}</li>}
        {!t.pace && t.projected && <li className="muted">{t.projected}</li>}
      </ul>

      <div className="goal-bottom">
        <span className="muted">关联:{g.accounts.length ? g.accounts.map((a) => a.name).join("、") : "(关联的账户已不可用)"}</span>
        <span>
          <button className="lnk" onClick={onEdit}>编辑</button>
          <button className="lnk" style={{ marginLeft: 12 }} onClick={onDelete}>删除</button>
        </span>
      </div>
    </div>
  );
}

function GoalModal({ goal, accounts, onClose, onSaved }: {
  goal: Goal | null; accounts: Account[]; onClose: () => void; onSaved: () => void;
}) {
  const api = useApi();
  const [kind, setKind] = useState<GoalKind>(goal?.kind ?? "save");
  const [name, setName] = useState(goal?.name ?? "");
  const [target, setTarget] = useState(goal ? String(goal.target / 100) : "");
  const [deadline, setDeadline] = useState(goal?.deadline ?? "");
  const [ids, setIds] = useState<number[]>(goal?.accounts.map((a) => a.id) ?? []);
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);

  // 存钱关联存款、现金、理财(不含房产和负债),还债只能关联负债账户
  const eligible = accounts.filter((a) => (kind === "payoff" ? a.kind === "liability" : a.kind !== "liability" && a.kind !== "property"));
  const picked = eligible.filter((a) => ids.includes(a.id));
  const total = picked.reduce((s, a) => s + a.balance, 0);

  const switchKind = (k: GoalKind) => {
    if (k === kind) return;
    setKind(k);
    setIds([]); // 两种目标可关联的账户不同,切换后重新选
    if (k === "payoff" && !target) setTarget("0");
  };
  const toggle = (id: number) => setIds((cur) => (cur.includes(id) ? cur.filter((x) => x !== id) : [...cur, id]));

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    const cents = toCents(target);
    if (!name.trim()) return setErr("请填写目标名称");
    if (!Number.isFinite(cents) || cents < 0) return setErr(kind === "save" ? "请填写目标金额" : "请填写还清线(欠款降到多少算还清,一般填 0)");
    if (kind === "save" && cents === 0) return setErr("目标金额必须大于 0");
    if (picked.length === 0) return setErr("请至少关联一个账户");
    const input: GoalInput = { name: name.trim(), kind, target: cents, deadline: deadline || null, account_ids: picked.map((a) => a.id) };
    setBusy(true);
    try {
      if (goal) await api.updateGoal(goal.id, input); else await api.addGoal(input);
      onSaved();
    } catch (ex) {
      setErr(errText(ex));
      setBusy(false);
    }
  };

  return (
    <div className="modal on" onClick={(e) => e.target === e.currentTarget && onClose()}>
      <form className="sheet" onSubmit={submit}>
        <h2>{goal ? "编辑目标" : "新建目标"}</h2>
        <div className="tog">
          <button type="button" className={kind === "save" ? "on" : ""} onClick={() => switchKind("save")}>存钱</button>
          <button type="button" className={kind === "payoff" ? "on" : ""} onClick={() => switchKind("payoff")}>还债</button>
        </div>

        <label className="lbl">名称</label>
        <input className="f" autoFocus maxLength={60} placeholder={kind === "save" ? "如:两年攒 20 万" : "如:还清信用卡"} value={name} onChange={(e) => setName(e.target.value)} />

        <label className="lbl">{kind === "save" ? "目标金额(元)" : "还清线(元),一般填 0"}</label>
        <input className="f" type="number" step="0.01" min="0" value={target} onChange={(e) => setTarget(e.target.value)} />

        <label className="lbl">截止日期(可不填)</label>
        <div className="row-inline">
          <input className="f" type="date" value={deadline} onChange={(e) => setDeadline(e.target.value)} />
          {deadline && <button type="button" className="lnk" onClick={() => setDeadline("")}>清除</button>}
        </div>

        <label className="lbl">关联账户(可多选)</label>
        <div className="acc-pick">
          {eligible.map((a) => (
            <label key={a.id} className={ids.includes(a.id) ? "on" : ""}>
              <input type="checkbox" checked={ids.includes(a.id)} onChange={() => toggle(a.id)} />
              <span>{a.name}</span><em className="num">¥{fmt(a.balance)}</em>
            </label>
          ))}
          {eligible.length === 0 && (
            <p className="muted">{kind === "save" ? "还没有资产账户,请先到“资产”页添加。" : "还没有负债账户,请先到“资产”页添加一个类型为“负债”的账户。"}</p>
          )}
        </div>
        {picked.length > 0 && <p className="muted" style={{ margin: "2px 0 8px" }}>已选 {picked.length} 个,当前合计 ¥{fmt(total)}</p>}

        {err && <div className="notice bad" style={{ margin: "4px 0 10px" }}>{err}</div>}
        <button className="go" disabled={busy}>保存</button>
        <button type="button" className="cancel" onClick={onClose}>取消</button>
      </form>
    </div>
  );
}
