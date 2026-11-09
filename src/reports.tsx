import { useEffect, useState } from "react";
import { ReviewsSection } from "./reviews";
import { CategorySum, MonthRow, NetPoint, Report, useApi, fmt, thisMonth } from "./api";
import {
  barScale, compactYuan, labelEvery, lineScale, linear, monthName, monthTicks, nearestIndex, roundedTopBar, spreadX,
} from "./charts";
import { ChartHead, Legend, TIP_H, Tip, tipFlips, useWidth } from "./viz";

// 一个实体在整页保持同一种颜色:净资产=蓝,收入=浅青,支出=橙(色板见 App.css 的 --v-*)
const RANGES: { label: string; months: number }[] = [
  { label: "近 6 个月", months: 6 },
  { label: "近 12 个月", months: 12 },
  { label: "全部", months: 0 },
];

const errText = (e: unknown) => (typeof e === "string" ? e : String(e));

export function ReportView() {
  const api = useApi();
  const [months, setMonths] = useState(12);
  const [mode, setMode] = useState<"chart" | "table">("chart");
  const [data, setData] = useState<Report | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  useEffect(() => {
    let alive = true;
    setLoading(true);
    api.getReport(thisMonth(), months)
      .then((r) => { if (alive) { setData(r); setError(""); } })
      .catch((e) => { if (alive) setError(errText(e)); })
      .finally(() => { if (alive) setLoading(false); });
    return () => { alive = false; };
  }, [months]);

  // 重新取数时保留上一次的画面并变淡,不闪烁、不跳动
  const frame = { opacity: loading && data ? 0.55 : 1, transition: "opacity .15s" };

  return (
    <>
      <div className="head"><h1>报表</h1></div>
      <div className="filters">
        <div className="seg" role="group" aria-label="时间范围">
          {RANGES.map((r) => (
            <button key={r.months} className={months === r.months ? "on" : ""} onClick={() => setMonths(r.months)}>{r.label}</button>
          ))}
        </div>
        <div className="seg" role="group" aria-label="显示方式">
          <button className={mode === "chart" ? "on" : ""} onClick={() => setMode("chart")}>图表</button>
          <button className={mode === "table" ? "on" : ""} onClick={() => setMode("table")}>表格</button>
        </div>
      </div>

      {error && <div className="notice bad">{error}</div>}
      {!data && !error && <div className="empty">加载中…</div>}
      {data && data.months.length === 0 && (
        <div className="card"><div className="empty">还没有可以统计的数据<br /><span className="muted">记几笔账后显示趋势</span></div></div>
      )}
      {data && data.months.length > 0 && (
        <div style={frame}>
          <p className="muted" style={{ margin: "-4px 0 14px" }}>
            {monthName(data.from)} – {monthName(data.to)} · 共 {data.months.length} 个月
          </p>
          <div className="grid">
            <Kpis months={data.months} />
            {mode === "chart" ? (
              <>
                <div className="card viz span12"><NetWorthChart points={data.net_worth} /></div>
                <div className="card viz span7"><FlowChart rows={data.months} /></div>
                <div className="card viz span5"><CategoryBars rows={data.categories} /></div>
              </>
            ) : (
              <>
                <div className="card span12"><MonthTable report={data} /></div>
                <div className="card span12"><CategoryTable rows={data.categories} /></div>
              </>
            )}
          </div>
        </div>
      )}
      <ReviewsSection />
    </>
  );
}

// ---------- 指标卡 ----------

function Kpis({ months }: { months: MonthRow[] }) {
  const income = months.reduce((s, m) => s + m.income, 0);
  const expense = months.reduce((s, m) => s + m.expense, 0);
  const saved = income - expense;
  const rate = income > 0 ? Math.round((saved / income) * 100) : null;
  const tile = (label: string, value: string, sub: string, cls = "") => (
    <div className="card tile span3">
      <div className="muted">{label}</div>
      <div className={`tv num ${cls}`}>{value}</div>
      <div className="muted">{sub}</div>
    </div>
  );
  return (
    <>
      {tile("总收入", `¥${fmt(income)}`, `月均 ¥${fmt(Math.round(income / months.length))}`)}
      {tile("总支出", `¥${fmt(expense)}`, `月均 ¥${fmt(Math.round(expense / months.length))}`)}
      {tile("结余", `${saved < 0 ? "-" : ""}¥${fmt(Math.abs(saved))}`, saved < 0 ? "入不敷出" : "收入减支出", saved < 0 ? "neg-text" : "")}
      {tile("储蓄率", rate === null ? "—" : `${rate}%`, rate === null ? "区间内没有收入" : "结余 ÷ 收入")}
    </>
  );
}

/** 打印、高对比度模式下色相不可靠,改用斜纹区分(45° 与 135°) */
function Hatches() {
  return (
    <defs>
      <pattern id="hatch-a" width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
        <rect className="hatch-bg" width="6" height="6" /><path className="hatch-ink" d="M0 0V6" strokeWidth="2" />
      </pattern>
      <pattern id="hatch-b" width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(135)">
        <rect className="hatch-bg" width="6" height="6" /><path className="hatch-ink" d="M0 0V6" strokeWidth="2" />
      </pattern>
    </defs>
  );
}

function MonthLabels({ months, xs, y, span }: { months: string[]; xs: number[]; y: number; span: number }) {
  return (
    <>
      {monthTicks(months, labelEvery(months.length, span)).map((t) => (
        <text key={months[t.i]} x={xs[t.i]} y={y} textAnchor="middle" className="ax">
          {t.text}
          {t.year && <tspan x={xs[t.i]} dy="13" className="ax-year">{t.year}</tspan>}
        </text>
      ))}
    </>
  );
}

// ---------- 净资产趋势(单序列折线 + 淡色面积) ----------

function NetWorthChart({ points }: { points: NetPoint[] }) {
  const [wrap, width] = useWidth<HTMLDivElement>();
  const [hover, setHover] = useState<number | null>(null);
  const H = 270, L = 58, R = 22, T = 26, B = 40;
  const vals = points.map((p) => p.net);
  const sc = lineScale(vals);
  const xs = spreadX(points.length, L, Math.max(L + 1, width - R));
  const y = linear([sc.min, sc.max], [H - B, T]);
  const last = points.length - 1;
  const line = points.map((p, i) => `${i ? "L" : "M"}${xs[i].toFixed(1)},${y(p.net).toFixed(1)}`).join("");
  const area = points.length > 1 ? `${line}L${xs[last]},${H - B}L${xs[0]},${H - B}Z` : "";
  const cur = hover ?? null;
  const labelRight = xs[last] > width - 90;

  // 提示框放在折线的"空白侧":看提示框伸向那一侧的相邻点,它在上方就放到点下方,反之放上方,
  // 这样提示框不会盖住紧挨着的线段
  let tipTop = 0;
  if (cur !== null) {
    const nb = tipFlips(xs[cur], width) ? cur - 1 : cur + 1;
    const below = nb < 0 || nb > last || y(vals[nb]) < y(vals[cur]);
    const py = y(vals[cur]);
    tipTop = Math.max(0, Math.min(H - TIP_H - 6, below ? py + 16 : py - 16 - TIP_H));
  }

  const onMove = (e: React.PointerEvent<SVGRectElement>) => {
    const box = e.currentTarget.getBoundingClientRect();
    setHover(nearestIndex(e.clientX - box.left + L, xs));
  };
  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowLeft") { e.preventDefault(); setHover((h) => Math.max(0, (h ?? last) - 1)); }
    else if (e.key === "ArrowRight") { e.preventDefault(); setHover((h) => Math.min(last, (h ?? last) + 1)); }
    else if (e.key === "Escape") setHover(null);
  };

  return (
    <>
      <ChartHead title="净资产趋势" sub={points.length < 2 ? "只有一个月的数据,之后每月多一个点" : "每月月底,资产减负债"} />
      <div className="viz-wrap" ref={wrap}>
        {width > 0 && (
          <svg width={width} height={H} role="img" tabIndex={0} onKeyDown={onKey} onBlur={() => setHover(null)} onFocus={() => setHover((h) => h ?? last)}
            aria-label={`净资产趋势,最新 ${fmt(vals[last])} 元`}>
            {sc.ticks.map((t) => (
              <g key={t}>
                <line className="grid-line" x1={L} x2={width - R} y1={y(t)} y2={y(t)} />
                <text className="ax" x={L - 8} y={y(t) + 4} textAnchor="end">{compactYuan(t)}</text>
              </g>
            ))}
            <MonthLabels months={points.map((p) => p.month)} xs={xs} y={H - B + 18} span={width - L - R} />
            {area && <path d={area} className="area-nw" />}
            {points.length > 1 && <path d={line} className="line-nw" />}
            {cur !== null && <line className="cross" x1={xs[cur]} x2={xs[cur]} y1={T - 8} y2={H - B} />}
            {/* 终点:2px 与底色同色的描边环,保证压在线上也清楚 */}
            <circle cx={xs[last]} cy={y(vals[last])} r={7} className="ring" />
            <circle cx={xs[last]} cy={y(vals[last])} r={5} className="dot-nw" />
            {cur === null && (
              <text className="end-label" x={xs[last] + (labelRight ? 4 : 0)} y={Math.max(12, y(vals[last]) - 14)} textAnchor={labelRight ? "end" : "middle"}>
                ¥{fmt(vals[last])}
              </text>
            )}
            {cur !== null && cur !== last && (
              <>
                <circle cx={xs[cur]} cy={y(vals[cur])} r={7} className="ring" />
                <circle cx={xs[cur]} cy={y(vals[cur])} r={5} className="dot-nw" />
              </>
            )}
            <rect x={L} y={0} width={Math.max(0, width - L - R)} height={H - B + 6} fill="transparent"
              onPointerMove={onMove} onPointerLeave={() => setHover(null)} />
          </svg>
        )}
        {cur !== null && width > 0 && (
          <Tip x={xs[cur]} y={tipTop} width={width} title={monthName(points[cur].month)}
            rows={[
              { color: "var(--v-nw)", label: "净资产", value: `¥${fmt(points[cur].net)}` },
              { label: "资产", value: `¥${fmt(points[cur].assets)}` },
              { label: "负债", value: `¥${fmt(points[cur].liabilities)}` },
            ]} />
        )}
      </div>
    </>
  );
}

// ---------- 月度收支(双序列分组柱) ----------

function FlowChart({ rows }: { rows: MonthRow[] }) {
  const [wrap, width] = useWidth<HTMLDivElement>();
  const [hover, setHover] = useState<number | null>(null);
  const H = 250, L = 52, R = 26, T = 22, B = 40;
  const sc = barScale(rows.flatMap((r) => [r.income, r.expense]));
  const y = linear([sc.min, sc.max], [H - B, T]);
  const n = rows.length;
  const band = Math.max(1, (width - L - R) / n);
  const cx = (i: number) => L + band * (i + 0.5);
  const bw = Math.max(3, Math.min(24, (band - 10) / 2));
  const gap = 2; // 相邻两根柱之间的底色缝
  const last = n - 1;
  const showCapLabels = band >= 40;

  const bar = (i: number, side: 0 | 1, v: number, cls: string) => {
    const x = side === 0 ? cx(i) - gap / 2 - bw : cx(i) + gap / 2;
    const top = y(v);
    return <path d={roundedTopBar(x, top, bw, H - B - top)} className={`${cls}${hover === i ? " lift" : ""}`} />;
  };

  return (
    <>
      <ChartHead title="每月收支">
        <Legend items={[{ cls: "sw-income", label: "收入" }, { cls: "sw-expense", label: "支出" }]} />
      </ChartHead>
      <div className="viz-wrap" ref={wrap}>
        {width > 0 && (
          <svg width={width} height={H} role="img" aria-label="每月收入与支出对比,详见表格视图">
            <Hatches />
            {sc.ticks.map((t) => (
              <g key={t}>
                <line className="grid-line" x1={L} x2={width - R} y1={y(t)} y2={y(t)} />
                <text className="ax" x={L - 8} y={y(t) + 4} textAnchor="end">{compactYuan(t)}</text>
              </g>
            ))}
            <line className="base-line" x1={L} x2={width - R} y1={H - B} y2={H - B} />
            <MonthLabels months={rows.map((r) => r.month)} xs={rows.map((_, i) => cx(i))} y={H - B + 18} span={width - L - R - band} />
            {rows.map((r, i) => (
              <g key={r.month}>
                {bar(i, 0, r.income, "bar-income")}
                {bar(i, 1, r.expense, "bar-expense")}
                {/* 命中区域覆盖整个月份区间,比柱子大得多 */}
                <rect x={L + band * i} y={T - 6} width={band} height={H - B - T + 6} fill="transparent" tabIndex={0}
                  aria-label={`${monthName(r.month)} 收入 ${fmt(r.income)} 元,支出 ${fmt(r.expense)} 元`}
                  onPointerEnter={() => setHover(i)} onPointerMove={() => setHover(i)} onPointerLeave={() => setHover(null)}
                  onFocus={() => setHover(i)} onBlur={() => setHover(null)} />
              </g>
            ))}
            {showCapLabels && (
              <>
                {rows[last].income > 0 && (
                  <text className="cap-label" x={cx(last) - gap / 2 - bw / 2} y={y(rows[last].income) - 6} textAnchor="end">
                    {compactYuan(rows[last].income)}
                  </text>
                )}
                {rows[last].expense > 0 && (
                  <text className="cap-label" x={cx(last) + gap / 2 + bw / 2} y={y(rows[last].expense) - 6} textAnchor="start">
                    {compactYuan(rows[last].expense)}
                  </text>
                )}
              </>
            )}
          </svg>
        )}
        {hover !== null && width > 0 && (
          <Tip x={cx(hover)} y={T} width={width} title={monthName(rows[hover].month)}
            rows={[
              { color: "var(--v-income)", label: "收入", value: `¥${fmt(rows[hover].income)}` },
              { color: "var(--v-expense)", label: "支出", value: `¥${fmt(rows[hover].expense)}` },
              { label: "结余", value: `${rows[hover].income < rows[hover].expense ? "-" : ""}¥${fmt(Math.abs(rows[hover].income - rows[hover].expense))}` },
            ]} />
        )}
      </div>
    </>
  );
}

// ---------- 分类支出(横向条形,单色按大小排序) ----------

function CategoryBars({ rows }: { rows: CategorySum[] }) {
  const [hover, setHover] = useState<{ i: number; x: number; y: number } | null>(null);
  const [wrap, width] = useWidth<HTMLDivElement>();
  const total = rows.reduce((s, r) => s + r.total, 0);
  const max = rows[0]?.total ?? 0;

  return (
    <>
      <ChartHead title="支出去向" sub={rows.length ? `区间内共 ¥${fmt(total)}` : undefined} />
      <div className="viz-wrap cat-list" ref={wrap}>
        {rows.length === 0 && <div className="empty">区间内没有支出</div>}
        {rows.map((r, i) => (
          <div className="cat-row" key={r.category} tabIndex={0}
            onPointerMove={(e) => { const b = wrap.current!.getBoundingClientRect(); setHover({ i, x: e.clientX - b.left, y: e.clientY - b.top }); }}
            onPointerLeave={() => setHover(null)}
            onFocus={(e) => setHover({ i, x: 80, y: e.currentTarget.offsetTop })} onBlur={() => setHover(null)}>
            <span className="cat-name">{r.category}</span>
            <div className="cat-track">
              <div className={`cat-bar${hover?.i === i ? " lift" : ""}`} style={{ width: `${max ? (r.total / max) * 82 : 0}%` }} />
              <span className="num cat-val">¥{fmt(r.total)}</span>
            </div>
          </div>
        ))}
        {hover && (
          <Tip x={hover.x} y={hover.y} width={width} title={rows[hover.i].category}
            rows={[
              { color: "var(--v-expense)", label: "支出", value: `¥${fmt(rows[hover.i].total)}` },
              { label: "占比", value: `${total ? Math.round((rows[hover.i].total / total) * 100) : 0}%` },
              { label: "笔数", value: `${rows[hover.i].count}` },
            ]} />
        )}
      </div>
    </>
  );
}

// ---------- 表格视图(图表里的每个数都能在这里查到) ----------

function MonthTable({ report }: { report: Report }) {
  const nw = new Map(report.net_worth.map((p) => [p.month, p]));
  return (
    <>
      <ChartHead title="按月汇总" />
      <table className="tbl">
        <thead><tr><th>月份</th><th>收入</th><th>支出</th><th>结余</th><th>净资产</th></tr></thead>
        <tbody>
          {[...report.months].reverse().map((m) => {
            const saved = m.income - m.expense;
            return (
              <tr key={m.month}>
                <td>{monthName(m.month)}</td>
                <td className="num">¥{fmt(m.income)}</td>
                <td className="num">¥{fmt(m.expense)}</td>
                <td className="num">{saved < 0 ? "-" : ""}¥{fmt(Math.abs(saved))}</td>
                <td className="num">¥{fmt(nw.get(m.month)?.net ?? 0)}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </>
  );
}

function CategoryTable({ rows }: { rows: CategorySum[] }) {
  const total = rows.reduce((s, r) => s + r.total, 0);
  return (
    <>
      <ChartHead title="支出去向" />
      <table className="tbl">
        <thead><tr><th>分类</th><th>笔数</th><th>金额</th><th>占比</th></tr></thead>
        <tbody>
          {rows.map((r) => (
            <tr key={r.category}>
              <td>{r.category}</td><td className="num">{r.count}</td>
              <td className="num">¥{fmt(r.total)}</td><td className="num">{total ? Math.round((r.total / total) * 100) : 0}%</td>
            </tr>
          ))}
          {rows.length === 0 && <tr><td colSpan={4} className="muted">区间内没有支出</td></tr>}
        </tbody>
      </table>
    </>
  );
}
