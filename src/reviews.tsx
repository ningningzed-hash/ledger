import { useEffect, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { SavedReview, useApi, thisMonth } from "./api";
import { Markdown } from "./markdown";
import { monthName } from "./charts";

const errText = (e: unknown) => (typeof e === "string" ? e : String(e));

/** 报表页底部:保存下来的月度复盘。也可以在这里直接生成本月的 */
export function ReviewsSection() {
  const api = useApi();
  const [list, setList] = useState<SavedReview[] | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const load = () => api.listReviews().then((l) => { setList(l); setOpen((o) => o ?? l[0]?.month ?? null); }).catch((e) => setError(errText(e)));
  useEffect(() => { load(); }, []);

  const generate = async () => {
    const month = thisMonth();
    if (list?.some((r) => r.month === month) && !(await ask(`${monthName(month)}已经有一份复盘,重新生成会覆盖它。继续吗?`, { title: "重新生成复盘", kind: "warning" }))) return;
    setBusy(true); setError("");
    try {
      const content = await api.generateReview(month);
      await api.saveReview(month, content);
      setOpen(month);
      await load();
    } catch (e) { setError(errText(e)); } finally { setBusy(false); }
  };

  return (
    <section className="card reviews" aria-label="月度复盘">
      <div className="chart-head">
        <div><h3>月度复盘</h3><p className="muted">在助手里点「本月复盘」生成后可保存到这里</p></div>
        <button className="btn" disabled={busy} onClick={generate}>{busy ? "生成中…" : "生成本月复盘"}</button>
      </div>
      {error && <div className="notice bad">{error}</div>}
      {list?.length === 0 && <div className="empty">还没有保存的复盘</div>}
      {list?.map((r) => (
        <div key={r.month} className="rv">
          <div className="rv-head">
            <button className="rv-toggle" aria-expanded={open === r.month} onClick={() => setOpen(open === r.month ? null : r.month)}>
              <span className="rv-caret">{open === r.month ? "▾" : "▸"}</span>{monthName(r.month)}
              <span className="muted">保存于 {r.created_at}</span>
            </button>
            <button className="lnk" onClick={async () => {
              if (!(await ask(`删除${monthName(r.month)}的复盘?`, { title: "删除复盘", kind: "warning" }))) return;
              try { await api.deleteReview(r.month); load(); } catch (e) { setError(errText(e)); }
            }}>删除</button>
          </div>
          {open === r.month && <div className="rv-body"><Markdown text={r.content} /></div>}
        </div>
      ))}
    </section>
  );
}
