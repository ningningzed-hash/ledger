import { useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Account, Category, ImportPreview, ImportResult, ImportRow, useApi, fmt } from "./api";

const errText = (e: unknown) => (typeof e === "string" ? e : String(e));
export const BILL_EXTS = ["csv", "xlsx", "xls"];
export const isBillFile = (p: string) => BILL_EXTS.includes(p.split(".").pop()?.toLowerCase() ?? "");

/** 弹出文件选择框,返回选中的账单路径;取消返回 null */
export async function pickBill(): Promise<string | null> {
  const p = await open({ multiple: false, directory: false, filters: [{ name: "微信 / 支付宝账单", extensions: BILL_EXTS }] });
  return typeof p === "string" ? p : null;
}

type Tab = "todo" | "dup" | "skip";

/** 账单导入:预览 → 调整分类/账户、勾选 → 一键入账 */
export function ImportModal({ path, accounts, cats, onClose, onDone }: {
  path: string; accounts: Account[]; cats: Category[]; onClose: () => void; onDone: (r: ImportResult) => void;
}) {
  const api = useApi();
  const [p, setP] = useState<ImportPreview | null>(null);
  const [rows, setRows] = useState<ImportRow[]>([]);
  const [methodAcc, setMethodAcc] = useState<Record<string, number | null>>({});
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [tab, setTab] = useState<Tab>("todo");

  useEffect(() => {
    api.previewImport(path).then((pv) => {
      setP(pv);
      setRows(pv.rows);
      setMethodAcc(Object.fromEntries(pv.methods.map((m) => [m.method, m.account_id])));
    }).catch((e) => setError(errText(e)));
  }, [path]);

  const groups = useMemo(() => ({
    todo: rows.filter((r) => !r.skip && !r.duplicate_of),
    dup: rows.filter((r) => !r.skip && r.duplicate_of),
    skip: rows.filter((r) => r.skip),
  }), [rows]);
  const chosen = rows.filter((r) => r.selected && !r.skip);
  const total = (k: string) => chosen.filter((r) => r.kind === k).reduce((s, r) => s + r.amount, 0);
  const unknown = groups.todo.filter((r) => r.selected && r.category_source === "none").length;
  const ledgerAccounts = accounts.filter((a) => a.kind !== "property");

  const update = (ref: string, patch: Partial<ImportRow>) => setRows((rs) => rs.map((r) => (r.ref_id === ref ? { ...r, ...patch } : r)));
  // 改一笔的分类,同一交易对方里还没人工确认过的条目一起改("learned" 即已确认,不再被连带修改)
  const setCategory = (row: ImportRow, id: number | null) => setRows((rs) => rs.map((r) => {
    const same = r.ref_id === row.ref_id
      || (row.counterparty !== "" && r.counterparty === row.counterparty && r.kind === row.kind && !r.skip && r.category_source !== "learned");
    return same ? { ...r, category_id: id, category_source: "learned" } : r;
  }));
  const setAll = (list: ImportRow[], v: boolean) => setRows((rs) => rs.map((r) => (list.includes(r) ? { ...r, selected: v } : r)));

  const commit = async () => {
    if (!p || !chosen.length) return;
    setBusy(true); setError("");
    try {
      const r = await api.commitImport(p.source, chosen.map((x) => ({
        ref_id: x.ref_id, date: x.date, kind: x.kind!, amount: x.amount, category_id: x.category_id,
        account_id: methodAcc[x.method] ?? null, note: x.note, counterparty: x.counterparty, method: x.method,
      })));
      onDone(r);
    } catch (e) { setError(errText(e)); setBusy(false); }
  };

  const list = groups[tab];
  return (
    <div className="modal on" onClick={(e) => e.target === e.currentTarget && !busy && onClose()}>
      <div className="sheet wide xl imp">
        <h2>导入{p ? `${p.source_label}账单` : "账单"}</h2>
        {!p && !error && <div className="empty">正在读取账单…</div>}
        {error && <div className="notice bad" style={{ marginTop: 0, marginBottom: 12 }}>{error}</div>}
        {p && (
          <>
            <p className="muted imp-meta">
              {p.file_name} · {p.from === p.to ? p.from : `${p.from} 至 ${p.to}`} · 共 {rows.length} 条,
              可入账 {groups.todo.length} 条{groups.dup.length > 0 && `,疑似重复 ${groups.dup.length} 条`},跳过 {groups.skip.length} 条
            </p>

            {p.methods.length > 0 && (
              <div className="imp-methods">
                <div className="lbl">支付方式对应的账户(可不关联)</div>
                {p.methods.map((m) => (
                  <label key={m.method} className="imp-method">
                    <span title={m.method}>{m.method}<small className="muted"> · {m.count} 笔</small></span>
                    <select className="inp" value={methodAcc[m.method] ?? ""} onChange={(e) => setMethodAcc({ ...methodAcc, [m.method]: e.target.value ? +e.target.value : null })}>
                      <option value="">不关联账户</option>
                      {ledgerAccounts.map((a) => <option key={a.id} value={a.id}>{a.name}</option>)}
                    </select>
                  </label>
                ))}
              </div>
            )}

            <div className="imp-bar">
              <div className="seg" role="tablist">
                <button role="tab" aria-selected={tab === "todo"} className={tab === "todo" ? "on" : ""} onClick={() => setTab("todo")}>待入账 {groups.todo.length}</button>
                {groups.dup.length > 0 && <button role="tab" aria-selected={tab === "dup"} className={tab === "dup" ? "on" : ""} onClick={() => setTab("dup")}>疑似重复 {groups.dup.length}</button>}
                <button role="tab" aria-selected={tab === "skip"} className={tab === "skip" ? "on" : ""} onClick={() => setTab("skip")}>已跳过 {groups.skip.length}</button>
              </div>
              {tab !== "skip" && list.length > 0 && (
                <span className="imp-all">
                  <button className="lnk" onClick={() => setAll(list, true)}>全选</button>
                  <button className="lnk" onClick={() => setAll(list, false)}>全不选</button>
                </span>
              )}
            </div>
            {tab === "todo" && unknown > 0 && <p className="muted imp-hint">有 {unknown} 笔没认出分类,已先归到「其他」,标了<em className="chip warn">未识别</em>。改一笔,同一商户的会一起改,下次导入也会记住。</p>}
            {tab === "dup" && <p className="muted imp-hint">这些和你手动记过的账同一天、同金额,默认不导入。确认不是同一笔再勾选。</p>}

            <div className="imp-list">
              {list.map((r) => {
                const opts = cats.filter((c) => c.kind === r.kind);
                return (
                  <div key={r.ref_id} className={`imp-row${r.skip ? " skipped" : ""}${r.selected ? "" : " off"}`}>
                    {r.skip ? <span className="imp-cb" /> : (
                      <input type="checkbox" className="imp-cb" checked={r.selected} aria-label={`选择 ${r.note}`} onChange={(e) => update(r.ref_id, { selected: e.target.checked })} />
                    )}
                    <div className="imp-main">
                      <b title={r.note}>{r.note || "(无说明)"}</b>
                      <span className="muted">{r.time.slice(5, 16)}{r.method && ` · ${r.method}`}{r.skip && ` · ${r.skip}`}{r.duplicate_of && ` · 可能是${r.duplicate_of}`}</span>
                    </div>
                    {!r.skip && (
                      <div className="imp-cat">
                        {r.category_source === "none" && <em className="chip warn">未识别</em>}
                        <select className="inp" value={r.category_id ?? ""} aria-label="分类" onChange={(e) => setCategory(r, e.target.value ? +e.target.value : null)}>
                          <option value="">不分类</option>
                          {opts.map((c) => <option key={c.id} value={c.id}>{c.name}</option>)}
                        </select>
                      </div>
                    )}
                    <div className={`imp-amt num${r.kind === "income" ? " inc" : r.kind === "expense" ? " exp" : ""}`}>
                      {r.kind === "income" ? "+" : r.kind === "expense" ? "−" : ""}{fmt(r.amount)}
                    </div>
                  </div>
                );
              })}
              {list.length === 0 && <div className="empty">没有条目</div>}
            </div>

            <button className="go" disabled={busy || chosen.length === 0} onClick={commit}>
              {busy ? "入账中…" : chosen.length === 0 ? "没有勾选的账目" : `入账 ${chosen.length} 笔 · 支出 ¥${fmt(total("expense"))} · 收入 ¥${fmt(total("income"))}`}
            </button>
          </>
        )}
        <button className="cancel" disabled={busy} onClick={onClose}>取消</button>
      </div>
    </div>
  );
}
