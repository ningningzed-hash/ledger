import { FormEvent, useCallback, useEffect, useRef, useState } from "react";
import { isPermissionGranted, requestPermission } from "@tauri-apps/plugin-notification";
import { AlertSettings, Holding, PriceRange, PriceSeries, Quote, WatchItem, useApi } from "./api";
import { lineScale, linear, nearestIndex, spreadX } from "./charts";
import { RANGES, RangeStats, alertText, changeText, parseAlertPct, pctText, pointTitle, priceText, rangeStats, timeTicks, trendClass } from "./marketText";
import { ChartHead, TIP_H, Tip, tipFlips, useWidth } from "./viz";

const errText = (e: unknown) => (typeof e === "string" ? e : String(e));
/** 报价刷新间隔;分时走势(当日、5日)刷新得慢一些 */
const QUOTE_MS = 15_000;
const SERIES_MS = 60_000;
/** 自选为空时的快捷入口 */
const INDEXES = [{ code: "sh000001", label: "上证指数" }, { code: "sh000300", label: "沪深300" }, { code: "sz399006", label: "创业板指" }];

/** 窗口在前台时每隔 ms 调一次 fn(切到后台暂停,回到前台立刻补一次) */
function usePolling(fn: () => void, ms: number) {
  const ref = useRef(fn);
  ref.current = fn;
  useEffect(() => {
    const tick = () => { if (document.visibilityState === "visible") ref.current(); };
    const id = setInterval(tick, ms);
    document.addEventListener("visibilitychange", tick);
    return () => { clearInterval(id); document.removeEventListener("visibilitychange", tick); };
  }, [ms]);
}

/** 理财 → 行情:自选列表 + 实时报价 + 所选证券的历史走势 */
export function MarketView({ mode }: { mode: "chart" | "table" }) {
  const api = useApi();
  const [list, setList] = useState<WatchItem[] | null>(null);
  const [quotes, setQuotes] = useState<Record<string, Quote>>({});
  const [sel, setSel] = useState<string | null>(null);
  const [range, setRange] = useState<PriceRange>("1d");
  const [series, setSeries] = useState<PriceSeries | null>(null);
  const [seriesErr, setSeriesErr] = useState("");
  const [error, setError] = useState("");
  const [code, setCode] = useState("");
  const [adding, setAdding] = useState(false);
  const [holdings, setHoldings] = useState<Holding[]>([]);

  useEffect(() => {
    api.listWatchlist().then((l) => { setList(l); setSel((s) => s ?? l[0]?.secid ?? null); }).catch((e) => setError(errText(e)));
    api.listHoldings().then(setHoldings).catch(() => {});
  }, []);

  // 轮询偶尔失败时保留上一次的报价,不打扰;一直没拿到过、或连续失败几次才提示
  const quoteFails = useRef(0);
  const hasQuotes = useRef(false);
  const loadQuotes = useCallback(() => {
    if (!list?.length) return;
    api.getQuotes(list.map((w) => w.secid))
      .then((qs) => {
        setQuotes(Object.fromEntries(qs.map((q) => [q.secid, q])));
        if (quoteFails.current > 0) setError("");
        quoteFails.current = 0;
        hasQuotes.current = true;
      })
      .catch((e) => {
        quoteFails.current += 1;
        if (!hasQuotes.current || quoteFails.current >= 3) setError(errText(e));
      });
  }, [list]);
  useEffect(loadQuotes, [loadQuotes]);
  usePolling(loadQuotes, QUOTE_MS);

  const loadSeries = useCallback((quiet: boolean) => {
    if (!sel) { setSeries(null); return; }
    if (!quiet) { setSeries(null); setSeriesErr(""); }
    api.getPriceSeries(sel, range)
      .then((s) => { setSeries(s); setSeriesErr(""); })
      .catch((e) => { if (!quiet) setSeriesErr(errText(e)); });
  }, [sel, range]);
  useEffect(() => loadSeries(false), [loadSeries]);
  usePolling(() => { if (range === "1d" || range === "5d") loadSeries(true); }, SERIES_MS);

  const add = async (input: string, e?: FormEvent) => {
    e?.preventDefault();
    if (!input.trim() || adding) return;
    setAdding(true); setError("");
    try {
      const w = await api.addWatch(input);
      setList((l) => [...(l ?? []), w]);
      setSel(w.secid);
      setCode("");
    } catch (err) { setError(errText(err)); } finally { setAdding(false); }
  };
  const remove = async (w: WatchItem) => {
    try {
      await api.removeWatch(w.id);
      const rest = (list ?? []).filter((x) => x.id !== w.id);
      setList(rest);
      if (sel === w.secid) setSel(rest[0]?.secid ?? null);
    } catch (err) { setError(errText(err)); }
  };

  // 持仓里登记了代码、但还没加自选的,作为快捷入口
  const watched = new Set((list ?? []).map((w) => w.code.toUpperCase()));
  const fromHoldings = [...new Map(holdings.filter((h) => h.code && !h.secid?.startsWith("OF.") && !watched.has(h.code.trim().toUpperCase())).map((h) => [h.code!.trim(), h])).values()];
  const current = list?.find((w) => w.secid === sel) ?? null;
  const q = sel ? quotes[sel] : undefined;

  return (
    <div className="grid">
      <div className="card span4 watch">
        <h3>自选</h3>
        <form className="frm" onSubmit={(e) => add(code, e)}>
          <input className="inp" value={code} onChange={(e) => setCode(e.target.value)} placeholder="代码,如 600519、00700、AAPL" aria-label="股票代码" />
          <button className="btn" disabled={adding || !code.trim()}>{adding ? "查找中…" : "添加"}</button>
        </form>
        {error && <div className="notice bad" style={{ marginTop: 0, marginBottom: 10 }}>{error}</div>}
        {list?.map((w) => {
          const wq = quotes[w.secid];
          return (
            <div key={w.id} className={`row wrow${w.secid === sel ? " on" : ""}`} role="button" tabIndex={0}
              onClick={() => setSel(w.secid)} onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), setSel(w.secid))}>
              <div className="t"><b>{w.name}</b><span>{w.code}</span></div>
              <div className="wq">
                <b className="num">{priceText(wq?.price)}</b>
                <span className={`chg ${trendClass(wq?.change_pct)}`}>{pctText(wq?.change_pct)}</span>
              </div>
              <button className="x" title={`从自选移除 ${w.name}`} aria-label={`从自选移除 ${w.name}`} onClick={(e) => { e.stopPropagation(); remove(w); }}>×</button>
            </div>
          );
        })}
        {list && list.length === 0 && <div className="empty">还没有自选。输入代码添加,或点下面的快捷入口</div>}
        {list && (list.length === 0 || fromHoldings.length > 0) && (
          <div className="quick">
            {list.length === 0 && INDEXES.map((i) => <button key={i.code} disabled={adding} onClick={() => add(i.code)}>＋ {i.label}</button>)}
            {fromHoldings.map((h) => <button key={h.code} disabled={adding} title="来自持仓" onClick={() => add(h.code!)}>＋ {h.name}</button>)}
          </div>
        )}
        <p className="muted note">指数加 sh / sz 前缀,如 sh000001。行情有延迟,仅供参考。</p>
        <AlertBox />
      </div>

      <div className="card span8 viz">
        {current ? (
          <>
            <div className="qhead">
              <div>
                <h2>{current.name} <span className="muted">{current.code}</span></h2>
                <div className="qline">
                  <b className="num qprice">{priceText(q?.price)}</b>
                  <span className={`chg ${trendClass(q?.change)}`}>{changeText(q?.change)}　{pctText(q?.change_pct)}</span>
                </div>
                <div className="muted qmeta">
                  今开 {priceText(q?.open)} · 最高 {priceText(q?.high)} · 最低 {priceText(q?.low)} · 昨收 {priceText(q?.prev_close)}
                  {q?.time && <> · 更新于 {q.time.slice(5)}</>}
                </div>
              </div>
              <div className="seg" role="group" aria-label="时间范围">
                {RANGES.map((r) => (
                  <button key={r.key} className={range === r.key ? "on" : ""} aria-pressed={range === r.key} onClick={() => setRange(r.key)}>{r.label}</button>
                ))}
              </div>
            </div>
            {seriesErr && <div className="notice bad">{seriesErr}</div>}
            {!series && !seriesErr && <div className="empty">加载走势…</div>}
            {series && series.points.length === 0 && <div className="empty">这个时间范围内没有数据</div>}
            {series && series.points.length > 0 && (
              <>
                <Stats s={rangeStats(series.points, series.base)!} range={range} />
                {mode === "chart" ? <PriceChart series={series} /> : <PriceTable series={series} />}
              </>
            )}
          </>
        ) : (
          <div className="empty">在左边添加一只股票或指数,这里会显示实时价格和历史走势</div>
        )}
      </div>
    </div>
  );
}

function Stats({ s, range }: { s: RangeStats; range: PriceRange }) {
  const label = RANGES.find((r) => r.key === range)!.label;
  return (
    <div className="qstats">
      <div><span>{label}涨跌</span><b className={`num chg ${trendClass(s.change)}`}>{pctText(s.pct)}</b><small className="num">{changeText(s.change)}</small></div>
      <div><span>区间最高</span><b className="num">{priceText(s.high)}</b></div>
      <div><span>区间最低</span><b className="num">{priceText(s.low)}</b></div>
      <div><span>振幅</span><b className="num">{s.amplitude.toFixed(2)}%</b></div>
    </div>
  );
}

// ---------- 走势图:单序列折线 + 淡色面积 + 基准虚线 ----------

function PriceChart({ series }: { series: PriceSeries }) {
  const [wrap, width] = useWidth<HTMLDivElement>();
  const [hover, setHover] = useState<number | null>(null);
  const { points, base, intraday, range } = series;
  const H = 280, L = 62, R = 16, T = 18, B = 30;
  const vals = points.map((p) => p.price);
  const ref = base ?? vals[0];
  const sc = lineScale([...vals, ref]);
  const xs = spreadX(points.length, L, Math.max(L + 1, width - R));
  const y = linear([sc.min, sc.max], [H - B, T]);
  const last = points.length - 1;
  const line = points.map((p, i) => `${i ? "L" : "M"}${xs[i].toFixed(1)},${y(p.price).toFixed(1)}`).join("");
  const area = points.length > 1 ? `${line}L${xs[last]},${H - B}L${xs[0]},${H - B}Z` : "";
  const ticks = timeTicks(points.map((p) => p.t), xs, range);
  const cur = hover;
  const refLabel = range === "1d" ? "昨收" : base !== null ? "起点" : "首个点";

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
  const diff = cur !== null ? vals[cur] - ref : 0;

  return (
    <>
      <ChartHead title="价格走势" sub={`${intraday ? "5 分钟线" : range === "5y" ? "周线,前复权" : "日线,前复权"};虚线为${refLabel} ${priceText(ref)}`} />
      <div className="viz-wrap" ref={wrap}>
        {width > 0 && (
          <svg width={width} height={H} role="img" tabIndex={0} onKeyDown={onKey} onBlur={() => setHover(null)} onFocus={() => setHover((h) => h ?? last)}
            aria-label={`价格走势,最新 ${priceText(vals[last])},可切换到表格查看全部数据`}>
            {sc.ticks.map((t) => (
              <g key={t}>
                <line className="grid-line" x1={L} x2={width - R} y1={y(t)} y2={y(t)} />
                <text className="ax" x={L - 8} y={y(t) + 4} textAnchor="end">{priceText(t)}</text>
              </g>
            ))}
            {ticks.map((t) => <text key={t.i} className="ax" x={xs[t.i]} y={H - B + 18} textAnchor="middle">{t.text}</text>)}
            {area && <path d={area} className="area-nw" />}
            <line className="ref-line" x1={L} x2={width - R} y1={y(ref)} y2={y(ref)} />
            {points.length > 1 && <path d={line} className="line-nw" />}
            {cur !== null && <line className="cross" x1={xs[cur]} x2={xs[cur]} y1={T - 8} y2={H - B} />}
            <circle cx={xs[last]} cy={y(vals[last])} r={6} className="ring" />
            <circle cx={xs[last]} cy={y(vals[last])} r={4} className="dot-nw" />
            {cur !== null && cur !== last && (
              <>
                <circle cx={xs[cur]} cy={y(vals[cur])} r={6} className="ring" />
                <circle cx={xs[cur]} cy={y(vals[cur])} r={4} className="dot-nw" />
              </>
            )}
            <rect x={L} y={0} width={Math.max(0, width - L - R)} height={H - B + 6} fill="transparent"
              onPointerMove={onMove} onPointerLeave={() => setHover(null)} />
          </svg>
        )}
        {cur !== null && width > 0 && (
          <Tip x={xs[cur]} y={tipTop} width={width} title={pointTitle(points[cur].t, intraday)}
            rows={[
              { color: "var(--v-nw)", label: "价格", value: priceText(vals[cur]) },
              { label: `较${refLabel}`, value: `${changeText(diff)}  ${pctText(ref ? (diff / ref) * 100 : 0)}` },
            ]} />
        )}
      </div>
    </>
  );
}

function PriceTable({ series }: { series: PriceSeries }) {
  const { points, base, intraday } = series;
  const rows = points.map((p, i) => {
    const prev = i > 0 ? points[i - 1].price : base;
    return { ...p, chg: prev ? ((p.price - prev) / prev) * 100 : null };
  }).reverse();
  return (
    <div className="qtable">
      <table className="tbl">
        <thead><tr><th>{intraday ? "时间" : "日期"}</th><th>价格</th><th>较上一个点</th></tr></thead>
        <tbody>
          {rows.map((r) => (
            <tr key={r.t}>
              <td>{intraday ? r.t.slice(5) : r.t}</td>
              <td className="num">{priceText(r.price)}</td>
              <td className={`num chg ${trendClass(r.chg)}`}>{pctText(r.chg)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** 涨跌过大提醒:一个开关加一个阈值,对自选和场内持仓统一生效 */
function AlertBox() {
  const api = useApi();
  const [cfg, setCfg] = useState<AlertSettings | null>(null);
  const [pct, setPct] = useState("");
  const [granted, setGranted] = useState(true);
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api.getAlertSettings().then((c) => { setCfg(c); setPct(String(c.pct)); }).catch((e) => setErr(errText(e)));
    isPermissionGranted().then(setGranted).catch(() => {});
  }, []);
  if (!cfg) return err ? <div className="notice bad">{err}</div> : null;

  const save = async (enabled: boolean) => {
    const p = parseAlertPct(pct);
    if (p === undefined) return setErr("阈值填大于 0、不超过 50 的数字,最多两位小数");
    setBusy(true); setErr("");
    try {
      // 第一次打开时请求系统通知授权
      if (enabled && !(await isPermissionGranted())) setGranted((await requestPermission()) === "granted");
      const c = await api.saveAlertSettings(enabled, p);
      setCfg(c); setPct(String(c.pct));
    } catch (e) { setErr(errText(e)); }
    setBusy(false);
  };
  const denied = cfg.enabled && (!granted || cfg.notify_denied);

  return (
    <div className="alert-box">
      <h3>涨跌过大提醒</h3>
      <div className="alert-row">
        <label className="alert-on">
          <input type="checkbox" checked={cfg.enabled} disabled={busy} onChange={(e) => save(e.target.checked)} />开启
        </label>
        <span className="muted">当日涨跌超过 ±</span>
        <input className="inp pct" inputMode="decimal" value={pct} aria-label="提醒阈值(%)" onChange={(e) => setPct(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && save(cfg.enabled)} />
        <span className="muted">%</span>
        {String(cfg.pct) !== pct.trim().replace(/%$/, "") && <button className="btn alt" disabled={busy} onClick={() => save(cfg.enabled)}>保存</button>}
      </div>
      <p className="muted note">{alertText(cfg.enabled, cfg.pct)}</p>
      {denied && <div className="notice warn">系统通知未开启,收不到提醒。请到「系统设置 → 通知 → Ledger」允许通知。</div>}
      {err && <div className="notice bad">{err}</div>}
    </div>
  );
}
