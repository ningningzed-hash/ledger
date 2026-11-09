import { FormEvent, useEffect, useState } from "react";
import { ACCOUNT_KINDS, InterestOverview, InterestResult, useApi, fmt } from "./api";
import { CALCS, CalcDef, Field, RESULT_LABELS, buildArgs, formatPeriod, formatResultValue } from "./interestText";
import { bpsToPercent, percentToBps } from "./portfolioText";
import { ChartHead } from "./viz";

const errText = (e: unknown) => (typeof e === "string" ? e : String(e));

/** 报表页最下方的"利息计算":各类计算器 + 账户年利率与预计利息 */
export function InterestPanel({ refreshKey, onChanged }: { refreshKey: number; onChanged: () => void }) {
  return (
    <section className="adv" aria-label="利息计算">
      <div className="grid">
        <div className="card span12"><Calculator /></div>
        <div className="card span12"><RateOverview refreshKey={refreshKey} onChanged={onChanged} /></div>
      </div>
    </section>
  );
}

// ---------- 计算器 ----------

function Calculator() {
  const api = useApi();
  const [type, setType] = useState(CALCS[0].type);
  const [values, setValues] = useState<Record<string, string>>({});
  const [result, setResult] = useState<InterestResult | null>(null);
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  const def = CALCS.find((c) => c.type === type) as CalcDef;

  const pick = (t: string) => { setType(t); setValues({}); setResult(null); setErr(""); };
  const set = (k: string, v: string) => setValues((cur) => ({ ...cur, [k]: v }));

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const built = buildArgs(def, values);
    if ("error" in built) { setResult(null); return setErr(built.error); }
    setBusy(true);
    try { setResult(await api.calculateInterest(built.args)); setErr(""); }
    catch (ex) { setResult(null); setErr(errText(ex)); }
    setBusy(false);
  };

  return (
    <>
      <ChartHead title="计算器" />
      <div className="seg calc-types" role="group" aria-label="计算类型">
        {CALCS.map((c) => <button key={c.type} className={type === c.type ? "on" : ""} onClick={() => pick(c.type)}>{c.label}</button>)}
      </div>
      <form className="calc-form" onSubmit={submit}>
        {def.fields.map((f) => <FieldInput key={`${type}.${f.key}`} f={f} value={values[f.key] ?? f.def ?? ""} onChange={(v) => set(f.key, v)} />)}
        <button className="btn" disabled={busy}>计算</button>
      </form>
      {err && <div className="notice bad" role="alert">{err}</div>}
      {result && <ResultView r={result} />}
    </>
  );
}

function FieldInput({ f, value, onChange }: { f: Field; value: string; onChange: (v: string) => void }) {
  const id = `calc-${f.key}`;
  return (
    <div className="calc-field">
      <label className="lbl" htmlFor={id}>{f.label}{f.optional && "(选填)"}</label>
      {f.kind === "select" ? (
        <select id={id} className="f" value={value} onChange={(e) => onChange(e.target.value)}>
          {f.options!.map((o) => <option key={o.value} value={o.value}>{o.label}</option>)}
        </select>
      ) : (
        <input id={id} className="f" inputMode="decimal" placeholder={f.hint} value={value} onChange={(e) => onChange(e.target.value)} />
      )}
    </div>
  );
}

function ResultView({ r }: { r: InterestResult }) {
  const schedule = r.result.schedule as unknown as Record<string, number>[] | undefined;
  const entries = Object.entries(r.result).filter(([k]) => k !== "schedule");
  return (
    <div className="calc-result" aria-live="polite">
      <table className="tbl">
        <tbody>
          {entries.map(([k, v]) => (
            <tr key={k}>
              <td>{RESULT_LABELS[k] ?? k}</td>
              <td className="num">{typeof v === "object" ? formatPeriod(v as unknown as Record<string, number>) : formatResultValue(k, v)}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {schedule && (
        <details>
          <summary>查看还款表</summary>
          <table className="tbl">
            <thead><tr><th>期数</th><th>月供</th><th>本金</th><th>利息</th><th>剩余本金</th></tr></thead>
            <tbody>
              {schedule.map((p) => (
                <tr key={p.period}><td>{p.period}</td><td className="num">¥{fmt(p.payment * 100)}</td><td className="num">¥{fmt(p.principal * 100)}</td>
                  <td className="num">¥{fmt(p.interest * 100)}</td><td className="num">¥{fmt(p.balance * 100)}</td></tr>
              ))}
            </tbody>
          </table>
        </details>
      )}
      <p className="muted calc-basis">公式:{r.formula}</p>
      <p className="muted calc-basis">假设:{r.assumptions.join(";")}。{r.note}。</p>
    </div>
  );
}

// ---------- 账户利息概览与年利率登记 ----------

function RateOverview({ refreshKey, onChanged }: { refreshKey: number; onChanged: () => void }) {
  const api = useApi();
  const [ov, setOv] = useState<InterestOverview | null>(null);
  const [err, setErr] = useState("");
  const [drafts, setDrafts] = useState<Record<number, string>>({});

  const load = () => api.interestOverview().then((o) => { setOv(o); setDrafts({}); }).catch((e) => setErr(errText(e)));
  useEffect(() => { load(); }, [refreshKey]);

  const save = async (id: number, current: number | null) => {
    const text = drafts[id];
    if (text === undefined) return;
    const bps = percentToBps(text);
    if (bps === undefined) return setErr("年利率需是 0 到 100 之间的数字,最多两位小数");
    if (bps === current) { setDrafts((d) => { const n = { ...d }; delete n[id]; return n; }); return; }
    try { await api.setAccountRate(id, bps); setErr(""); await load(); onChanged(); } catch (e) { setErr(errText(e)); }
  };

  return (
    <>
      <ChartHead title="账户利息概览" sub="填上年利率,按当前余额估算每月利息">
        {ov && <span className="muted num">每月预计收入 ¥{fmt(ov.monthly_income)} · 支出 ¥{fmt(ov.monthly_cost)}</span>}
      </ChartHead>
      {err && <div className="notice bad" role="alert">{err}</div>}
      {!ov && !err && <div className="empty">加载中…</div>}
      {ov && ov.accounts.length === 0 && <div className="empty">还没有现金、存款、理财或负债账户</div>}
      {ov && ov.accounts.length > 0 && (
        <table className="tbl">
          <thead><tr><th>账户</th><th>类型</th><th>余额</th><th>年利率(%)</th><th>每月预计利息</th></tr></thead>
          <tbody>
            {ov.accounts.map((a) => (
              <tr key={a.id}>
                <td>{a.name}</td><td>{ACCOUNT_KINDS[a.kind] ?? a.kind}</td><td className="num">¥{fmt(a.balance)}</td>
                <td>
                  <input className="f rate-in num" inputMode="decimal" placeholder="未登记" aria-label={`${a.name} 年利率`}
                    value={drafts[a.id] ?? (a.rate_bps === null ? "" : bpsToPercent(a.rate_bps))}
                    onChange={(e) => setDrafts((d) => ({ ...d, [a.id]: e.target.value }))}
                    onBlur={() => save(a.id, a.rate_bps)} onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()} />
                </td>
                <td className="num">{a.monthly_interest === null ? "—" : `${a.kind === "liability" ? "-" : "+"}¥${fmt(a.monthly_interest)}`}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <p className="muted calc-basis">实际以银行结算为准;清空即清除年利率。</p>
    </>
  );
}
