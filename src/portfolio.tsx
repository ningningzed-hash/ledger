import { FormEvent, ReactNode, useEffect, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import {
  ACCOUNT_KINDS, ASSET_CLASSES, Account, Advice, AssetClass, Holding, HoldingInput, PnlSummary, Price, PriceSource, Product, ProductInput, RebalanceRow,
  RiskLevel, RiskProfile, useApi, classLabel, fmt, toCents,
} from "./api";
import { barScale, linear } from "./charts";
import {
  ACTION_LABEL, CURRENCY_NAME, LEVEL_LABEL, bpsToPercent, costHint, costPriceText, emergencyText, excludedText, fillText, holdingMode, noticeClass,
  noticeTag, parseCostPrice, parseShares, percentToBps, pnlClass, pnlText, priceText, productNote, refreshText, rowSummary, sharesText, signedPct,
  signedPoints, signedYuan, ytdText,
} from "./portfolioText";
import { ChartHead, Legend, Tip, useWidth } from "./viz";

const errText = (e: unknown) => (typeof e === "string" ? e : String(e));

type Which = null | "holdings" | "profile" | "products";

/** 报表页下方的"组合建议":前置条件、当前 vs 目标配比、再平衡、候选产品、利息视角 */
export function PortfolioSection({ mode, refreshKey, onChanged }: { mode: "chart" | "table"; refreshKey: number; onChanged: () => void }) {
  const api = useApi();
  const [advice, setAdvice] = useState<Advice | null>(null);
  const [pnl, setPnl] = useState<PnlSummary | null>(null);
  const [error, setError] = useState("");
  const [open, setOpen] = useState<Which>(null);
  const [refText, setRefText] = useState("");
  const [refBps, setRefBps] = useState<number | null>(null);
  const [refErr, setRefErr] = useState("");
  const [pricing, setPricing] = useState(false);
  const [priceMsg, setPriceMsg] = useState("");

  // 按份额登记的持仓:打开页面时顺带按最新价更新(后端每小时最多联网一次),手动刷新不受限
  const refreshPrices = async (force: boolean) => {
    if (force) { setPricing(true); setPriceMsg(""); }
    try {
      const r = await api.refreshHoldingPrices(force);
      if (r.updated > 0) onChanged();
      if (force) setPriceMsg(refreshText(r));
    } catch (e) { if (force) setPriceMsg(errText(e)); }
    setPricing(false);
  };
  useEffect(() => { refreshPrices(false); }, []);
  const hasAuto = advice?.portfolio.holdings.some((h) => h.secid) ?? false;

  useEffect(() => {
    let alive = true;
    api.getPortfolioAdvice(refBps)
      .then((a) => { if (alive) { setAdvice(a); setError(""); } })
      .catch((e) => { if (alive) setError(errText(e)); });
    return () => { alive = false; };
  }, [refBps, refreshKey]);
  // 盈亏汇总只是附加信息,取不到时不显示,不影响组合建议
  useEffect(() => { api.pnlSummary().then(setPnl).catch(() => setPnl(null)); }, [refreshKey]);

  const applyRef = () => {
    const bps = percentToBps(refText);
    if (bps === undefined) return setRefErr("参考利率需是 0 到 100 之间的数字,最多两位小数");
    setRefErr("");
    setRefBps(bps);
  };
  const close = () => { setOpen(null); onChanged(); };

  return (
    <section className="adv" aria-label="组合建议">
      <div className="head sec-head">
        <h2>组合建议</h2>
        <div className="sec-actions">
          {hasAuto && <button className="btn alt" disabled={pricing} onClick={() => refreshPrices(true)}>{pricing ? "刷新中…" : "刷新估值"}</button>}
          <button className="btn alt" onClick={() => setOpen("holdings")}>持仓</button>
          <button className="btn alt" onClick={() => setOpen("profile")}>风险问卷</button>
          <button className="btn alt" onClick={() => setOpen("products")}>产品清单</button>
        </div>
      </div>
      {error && <div className="notice bad">{error}</div>}
      {priceMsg && <div className="notice info" onClick={() => setPriceMsg("")}>{priceMsg}</div>}
      {!advice && !error && <div className="empty">加载中…</div>}
      {advice && (
        <div className="grid">
          <Notices advice={advice} />
          <Guidance advice={advice} onOpen={setOpen} />
          <Readiness advice={advice} />
          {advice.portfolio.holdings.length > 0 && <PnlCard summary={pnl} onOpen={() => setOpen("holdings")} />}
          {advice.rebalance && (mode === "chart" ? (
            <>
              <div className="card viz span12"><AllocationBullets rows={advice.rebalance.rows} /></div>
              <div className="card span12"><RebalanceTable advice={advice} compact /></div>
            </>
          ) : (
            <div className="card span12"><RebalanceTable advice={advice} /></div>
          ))}
          {advice.rebalance && <CandidateCard rows={advice.rebalance.rows} />}
          {mode === "table" && advice.portfolio.holdings.length > 0 && <div className="card span12"><HoldingsTable holdings={advice.portfolio.holdings} /></div>}
          <InterestCard advice={advice} refText={refText} setRefText={setRefText} applyRef={applyRef} refErr={refErr} />
          <p className="disclaimer span12">{advice.disclaimer}目标配比按固定规则估算,不预测行情,不给买卖时点;产品以发行方披露为准。</p>
        </div>
      )}
      {open === "holdings" && <HoldingsModal onClose={close} />}
      {open === "profile" && <ProfileModal current={advice?.profile ?? null} onClose={close} />}
      {open === "products" && <ProductsModal onClose={close} />}
    </section>
  );
}

// ---------- 提示、引导、前置条件 ----------

// 缺问卷、缺持仓的提示由下面的引导卡承担;应急金"数据不足"已显示在应急金卡片上,这里不再重复
const COVERED_BY_GUIDANCE = new Set(["no_profile", "no_holdings"]);
const covered = (n: Advice["notices"][number]) => COVERED_BY_GUIDANCE.has(n.code) || (n.code === "emergency_fund" && n.level === "info");

function Notices({ advice }: { advice: Advice }) {
  const list = advice.notices.filter((n) => !covered(n));
  if (list.length === 0) return null;
  return (
    <div className="span12 notices">
      {list.map((n, i) => (
        <div key={`${n.code}${i}`} className={noticeClass(n)}><b className="ntag">{noticeTag(n)}</b>{n.message}</div>
      ))}
    </div>
  );
}

function Guidance({ advice, onOpen }: { advice: Advice; onOpen: (w: Which) => void }) {
  const needProfile = advice.profile_missing;
  const needHoldings = advice.portfolio.holdings.length === 0;
  if (!needProfile && !needHoldings) return null;
  return (
    <div className="card span12">
      <div className="empty">
        {needProfile && needHoldings ? "完成风险问卷并登记持仓后,这里会显示目标配比和调整建议" : needProfile ? "先完成风险问卷,才能给出目标配比" : "先登记持仓,才能看到当前占比和调整建议"}
        <div className="guide-btns">
          {needProfile && <button className="btn" onClick={() => onOpen("profile")}>填写风险问卷</button>}
          {needHoldings && <button className="btn" onClick={() => onOpen("holdings")}>登记持仓</button>}
        </div>
      </div>
    </div>
  );
}

function Readiness({ advice }: { advice: Advice }) {
  const r = advice.readiness;
  const tile = (label: string, value: string, sub: string, cls = "") => (
    <div className="card tile span3">
      <div className="muted">{label}</div>
      <div className={`tv num ${cls}`}>{value}</div>
      <div className="muted">{sub}</div>
    </div>
  );
  const low = r.emergency_months !== null && r.emergency_months < 3;
  const surplus = r.avg_monthly_surplus;
  return (
    <>
      {tile("应急金", r.emergency_months === null ? "—" : `${r.emergency_months.toFixed(1)} 个月`, emergencyText(r), low ? "neg-text" : "")}
      {tile("月均结余", surplus === null ? "—" : `${surplus < 0 ? "-" : ""}¥${fmt(Math.abs(surplus))}`, surplus === null ? "数据不足" : "近 3 个月", surplus !== null && surplus < 0 ? "neg-text" : "")}
      {tile("负债合计", `¥${fmt(r.liabilities)}`, r.high_interest_debts.length ? `${r.high_interest_debts.length} 笔高息` : r.high_debt_declared ? "有高息负债" : "无高息负债")}
      {tile("持仓合计", `¥${fmt(advice.portfolio.total)}`, advice.portfolio.coverage_pct === null ? "—" : `占投资账户 ${advice.portfolio.coverage_pct.toFixed(0)}%`)}
    </>
  );
}

// ---------- 当前 vs 目标:子弹图(条=当前,竖线=目标) ----------

const ROW_H = 38;
const LABEL_W = 84;
const VALUE_W = 150;

function AllocationBullets({ rows }: { rows: RebalanceRow[] }) {
  const [wrap, width] = useWidth<HTMLDivElement>();
  const [hover, setHover] = useState<number | null>(null);
  const top = 6;
  const bottom = 26;
  const H = top + rows.length * ROW_H + bottom;
  const plotL = LABEL_W;
  const plotR = Math.max(plotL + 80, width - VALUE_W);
  const scale = barScale(rows.flatMap((r) => [r.current_pct, r.target_pct]), 4);
  const x = linear([scale.min, scale.max], [plotL, plotR]);
  const cy = (i: number) => top + i * ROW_H + ROW_H / 2;

  const onKey = (e: React.KeyboardEvent) => {
    const n = rows.length;
    if (e.key === "ArrowDown" || e.key === "ArrowRight") setHover(((hover ?? -1) + 1) % n);
    else if (e.key === "ArrowUp" || e.key === "ArrowLeft") setHover(((hover ?? 0) - 1 + n) % n);
    else if (e.key === "Home") setHover(0);
    else if (e.key === "End") setHover(n - 1);
    else return;
    e.preventDefault();
  };
  const h = hover === null ? null : rows[hover];

  return (
    <>
      <ChartHead title="当前配比 vs 目标配比">
        <Legend items={[{ cls: "sw-now", label: "当前" }, { cls: "sw-target", label: "目标" }]} />
      </ChartHead>
      <div className="viz-wrap" ref={wrap}>
        {width > 0 && (
          <svg width={width} height={H} role="img" tabIndex={0} aria-label={`当前配比与目标配比对比。${rows.map(rowSummary).join(";")}`}
            onKeyDown={onKey} onFocus={() => hover === null && setHover(0)} onBlur={() => setHover(null)}>
            <defs>
              <pattern id="hatch-now" width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
                <rect className="hatch-bg" width="6" height="6" /><path className="hatch-ink" d="M0 0V6" strokeWidth="2" />
              </pattern>
            </defs>
            {scale.ticks.map((t) => (
              <g key={t}>
                <line x1={x(t)} x2={x(t)} y1={top} y2={H - bottom} className={t === 0 ? "base-line" : "grid-line"} />
                <text x={x(t)} y={H - 8} textAnchor="middle" className="ax">{t}%</text>
              </g>
            ))}
            {rows.map((r, i) => (
              <g key={r.asset_class} onPointerMove={() => setHover(i)} onPointerLeave={() => setHover(null)}>
                <rect x={0} y={top + i * ROW_H} width={width} height={ROW_H} fill="transparent" />
                {hover === i && <rect x={0} y={top + i * ROW_H} width={width} height={ROW_H} className="row-hi" />}
                <text x={0} y={cy(i) + 4} className="row-label">{r.label}</text>
                <rect x={plotL} y={cy(i) - 7} width={Math.max(0, x(r.current_pct) - plotL)} height={14} rx={0} className="bar-now" />
                <rect x={x(r.target_pct) - 1.5} y={cy(i) - 12} width={3} height={24} className="mark-target" />
                <text x={width - VALUE_W + 14} y={cy(i) + 4} className="row-val num">
                  {r.current_pct.toFixed(1)}% → {r.target_pct.toFixed(1)}%
                </text>
              </g>
            ))}
          </svg>
        )}
        {h && hover !== null && (
          <Tip x={x(Math.max(h.current_pct, h.target_pct))} y={cy(hover) - 44} width={width} title={h.label}
            rows={[
              { color: "var(--v-nw)", label: "当前", value: `${h.current_pct.toFixed(1)}%  ${signedYuan(h.current_amount).replace("+", "")}` },
              { color: "var(--text)", label: "目标", value: `${h.target_pct.toFixed(1)}%  ${signedYuan(h.target_amount).replace("+", "")}` },
              { label: ACTION_LABEL[h.action], value: h.diff_amount === null ? "差额小于 5 个百分点" : `${signedPoints(h.diff_pct)} 个百分点 ${signedYuan(h.diff_amount)}` },
            ]} />
        )}
      </div>
    </>
  );
}

// ---------- 再平衡表(表格视图含全部数值) ----------

function RebalanceTable({ advice, compact = false }: { advice: Advice; compact?: boolean }) {
  const rb = advice.rebalance!;
  const fill = fillText(rb.months_to_fill, rb.buy_total);
  return (
    <>
      <ChartHead title="再平衡建议" sub="可以不卖出,只用新增资金逐步调整" />
      <table className="tbl">
        <thead>
          <tr>
            <th>大类</th>
            {!compact && <><th>当前金额</th><th>当前占比</th><th>目标占比</th></>}
            <th>差额(百分点)</th><th>调整金额</th><th>建议</th>
          </tr>
        </thead>
        <tbody>
          {rb.rows.map((r) => (
            <tr key={r.asset_class}>
              <td>{r.label}</td>
              {!compact && <><td className="num">¥{fmt(r.current_amount)}</td><td className="num">{r.current_pct.toFixed(1)}%</td><td className="num">{r.target_pct.toFixed(1)}%</td></>}
              <td className="num">{signedPoints(r.diff_pct)}</td>
              <td className="num">{r.diff_amount === null ? "—" : signedYuan(r.diff_amount)}</td>
              <td><span className={`chip ${r.action === "balanced" ? "good" : r.action === "add" ? "warn" : "muted"}`}>{ACTION_LABEL[r.action]}</span></td>
            </tr>
          ))}
        </tbody>
      </table>
      <div className="rb-foot muted">
        {rb.reserved_for_short_term_goals > 0 && <p>其中 ¥{fmt(rb.reserved_for_short_term_goals)} 为 36 个月内到期的存钱目标预留,计入现金类。</p>}
        {fill && <p>{fill}。</p>}
      </div>
    </>
  );
}

function CandidateCard({ rows }: { rows: RebalanceRow[] }) {
  const adds = rows.filter((r) => r.action === "add" && r.candidates);
  if (adds.length === 0) return null;
  return (
    <div className="card span12">
      <ChartHead title="候选产品" sub="来自你的产品清单" />
      <div className="cand-list">
        {adds.map((r) => (
          <div className="cand-group" key={r.asset_class}>
            <b>{r.label}</b>
            {r.candidates!.products.map((p) => (
              <div className="cand" key={p.id}>
                <span className="cand-name">{p.name}{p.code && <em className="num">{p.code}</em>}</span>
                {productNote(p.note) && <span className="muted">{productNote(p.note)}</span>}
              </div>
            ))}
            {r.candidates!.hint && <p className="muted">{r.candidates!.hint}</p>}
          </div>
        ))}
      </div>
    </div>
  );
}

/** 持仓盈亏汇总:只算能算出盈亏的持仓(按份额估值且填了成本价) */
function PnlCard({ summary, onOpen }: { summary: PnlSummary | null; onOpen: () => void }) {
  if (!summary) return null;
  if (summary.counted === 0) {
    return (
      <div className="card span12">
        <div className="empty">
          给按份额估值的持仓填上成本价,这里会显示浮动盈亏和收益率
          <div className="guide-btns"><button className="btn" onClick={onOpen}>填写成本价</button></div>
        </div>
      </div>
    );
  }
  const tile = (label: string, value: string, cls = "") => (
    <div><div className="muted">{label}</div><div className={`tv num chg ${cls}`}>{value}</div></div>
  );
  return (
    <div className="card span12">
      <ChartHead title="持仓盈亏" />
      <div className="pnl-tiles">
        {tile("成本", `¥${fmt(summary.cost)}`)}
        {tile("市值", `¥${fmt(summary.value)}`)}
        {tile("浮动盈亏", signedYuan(summary.pnl), pnlClass(summary.pnl))}
        {tile("收益率", signedPct(summary.pnl_pct), pnlClass(summary.pnl))}
      </div>
      <p className="muted">{ytdText(summary)}{summary.excluded > 0 && `。${excludedText(summary)}`}</p>
      <p className="muted">盈亏按原币价格计算,不含汇率变化,外币按最近一次估值的汇率折成人民币。</p>
    </div>
  );
}

function HoldingsTable({ holdings }: { holdings: Holding[] }) {
  return (
    <>
      <ChartHead title="持仓明细" />
      <table className="tbl">
        <thead><tr><th>名称</th><th>账户</th><th>大类</th><th>份额</th><th>成本价</th><th>金额</th><th>盈亏</th><th>收益率</th><th>估值</th><th>更新日期</th></tr></thead>
        <tbody>
          {holdings.map((h) => (
            <tr key={h.id}>
              <td>{h.name}{h.code && <span className="muted"> {h.code}</span>}</td><td>{h.account}</td><td>{classLabel(h.asset_class)}</td>
              <td className="num">{h.shares === null ? "—" : sharesText(h.shares)}</td>
              <td className="num">{costPriceText(h)}</td>
              <td className="num">¥{fmt(h.amount)}</td>
              <td className={`num chg ${pnlClass(h.pnl)}`}>{pnlText(h.pnl)}</td><td className={`num chg ${pnlClass(h.pnl)}`}>{signedPct(h.pnl_pct)}</td>
              <td>{holdingMode(h)}</td><td className="num">{h.updated_on}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </>
  );
}

// ---------- 利息视角 ----------

function InterestCard({ advice, refText, setRefText, applyRef, refErr }: {
  advice: Advice; refText: string; setRefText: (s: string) => void; applyRef: () => void; refErr: string;
}) {
  const iv = advice.interest;
  const ov = iv.overview;
  return (
    <div className="card span12">
      <ChartHead title="利息视角" sub="按当前余额和登记的年利率估算" />
      <div className="iv-grid">
        <div>
          <div className="muted">每月预计利息</div>
          <div className="num iv-big">+¥{fmt(ov.monthly_income)} <span className="muted">收入</span></div>
          <div className="num iv-big neg-text">-¥{fmt(ov.monthly_cost)} <span className="muted">支出</span></div>
          {ov.unregistered.length > 0 && <p className="muted">未登记年利率:{ov.unregistered.join("、")}</p>}
        </div>
        <div>
          <div className="muted">闲置现金(保留 6 个月支出后)</div>
          <div className="num iv-big">{iv.idle_cash === null ? "—" : `¥${fmt(iv.idle_cash)}`}</div>
          <label className="lbl" htmlFor="ref-rate">换个年利率(%)对比</label>
          <div className="row-inline">
            <input id="ref-rate" className="f ref-in" inputMode="decimal" placeholder="如 2.0" value={refText}
              onChange={(e) => setRefText(e.target.value)} onKeyDown={(e) => e.key === "Enter" && applyRef()} />
            <button className="btn alt" onClick={applyRef}>对比</button>
          </div>
          {refErr && <div className="notice bad">{refErr}</div>}
          {iv.reference_yearly_interest !== null && <p>按 {bpsToPercent(iv.reference_rate_bps ?? 0)}% 每年约 <b className="num">¥{fmt(iv.reference_yearly_interest)}</b>{iv.weighted_rate_bps !== null && <>(现有加权平均 {bpsToPercent(iv.weighted_rate_bps)}%)</>}</p>}
          {iv.extra_yearly_interest !== null && <p>比现有利率每年多约 <b className="num">¥{fmt(iv.extra_yearly_interest)}</b></p>}
          {iv.hint && <p className="muted">{iv.hint}</p>}
        </div>
        <div>
          <div className="muted">还债的确定收益</div>
          {iv.debts.length === 0 && <p className="muted">还没有登记年利率的负债。</p>}
          {iv.debts.map((d) => (
            <p key={d.name}>
              <b>{d.name}</b> <span className="muted">{bpsToPercent(d.rate_bps)}%</span>{d.high && <span className="chip warn">高息</span>}
              <br />还清每年少付 <span className="num">¥{fmt(d.yearly_interest)}</span>
            </p>
          ))}
        </div>
      </div>
    </div>
  );
}

// ---------- 弹窗:持仓 / 问卷 / 产品清单 ----------

function Modal({ title, wide = false, onClose, children }: { title: string; wide?: boolean; onClose: () => void; children: ReactNode }) {
  return (
    <div className="modal on" onClick={(e) => e.target === e.currentTarget && onClose()}>
      <div className={`sheet wide${wide ? " xl" : ""}`} role="dialog" aria-label={title}>
        <h2>{title}</h2>
        {children}
        <button type="button" className="cancel" onClick={onClose}>关闭</button>
      </div>
    </div>
  );
}

const ClassSelect = ({ value, onChange }: { value: AssetClass; onChange: (v: AssetClass) => void }) => (
  <select className="f" value={value} onChange={(e) => onChange(e.target.value as AssetClass)}>
    {ASSET_CLASSES.map((c) => <option key={c.key} value={c.key}>{c.label}</option>)}
  </select>
);

function HoldingsModal({ onClose }: { onClose: () => void }) {
  const api = useApi();
  const [list, setList] = useState<Holding[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [editing, setEditing] = useState<Holding | null>(null);
  const [accId, setAccId] = useState(0);
  const [name, setName] = useState("");
  const [code, setCode] = useState("");
  const [cls, setCls] = useState<AssetClass>("us_equity_index");
  const [amount, setAmount] = useState("");
  // 自动:份额 × 最新价;手动:自己填金额
  const [auto, setAuto] = useState(true);
  const [source, setSource] = useState<PriceSource>("exchange");
  const [shares, setShares] = useState("");
  const [costPrice, setCostPrice] = useState("");
  const [price, setPrice] = useState<Price | null>(null);
  const [looking, setLooking] = useState(false);
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);

  const load = async () => {
    try {
      const [h, a] = await Promise.all([api.listHoldings(), api.listAccounts()]);
      setList(h);
      const inv = a.filter((x) => x.kind === "investment");
      setAccounts(inv);
      setAccId((cur) => (inv.some((x) => x.id === cur) ? cur : inv[0]?.id ?? 0));
    } catch (e) { setErr(errText(e)); }
  };
  useEffect(() => { load(); }, []);

  const reset = () => { setEditing(null); setName(""); setCode(""); setAmount(""); setShares(""); setCostPrice(""); setPrice(null); setErr(""); };
  const edit = (h: Holding) => {
    setEditing(h); setAccId(h.account_id); setName(h.name); setCode(h.code ?? ""); setCls(h.asset_class); setAmount((h.amount / 100).toFixed(2)); setErr("");
    setAuto(h.secid !== null); setSource(h.secid?.startsWith("OF.") ? "fund" : "exchange");
    setShares(h.shares === null ? "" : String(h.shares)); setCostPrice(h.cost_price === null ? "" : String(h.cost_price)); setPrice(null);
  };
  const lookup = async () => {
    if (!code.trim()) return setErr("请先填写代码");
    setLooking(true); setErr("");
    try {
      const p = await api.lookupPrice(code.trim(), source);
      setPrice(p);
      if (!name.trim()) setName(p.name);
    } catch (ex) { setPrice(null); setErr(errText(ex)); }
    setLooking(false);
  };
  const sharesNum = parseShares(shares);
  const estimate = price && typeof sharesNum === "number" ? Math.round(sharesNum * price.price_cny * 100) : null;
  const costNum = parseCostPrice(costPrice);
  // 成本价按证券的交易币种填:查过价就用查到的币种,编辑时用原来的
  const currency = price?.currency ?? (editing && editing.secid && code.trim() === (editing.code ?? "") ? editing.currency : null);
  const hint = editing ? costHint(editing.shares, sharesNum, editing.cost_price, costNum) : "";
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!accId) return setErr("请先选择投资账户");
    if (!name.trim()) return setErr("请填写名称");
    let cents = 0;
    if (auto) {
      if (!code.trim()) return setErr("按份额自动更新需要填写代码");
      if (typeof sharesNum !== "number") return setErr("份额必须大于 0,最多 4 位小数");
      if (costNum === undefined) return setErr("成本价必须大于 0,最多 6 位小数");
    } else {
      cents = toCents(amount);
      if (!Number.isFinite(cents) || cents <= 0) return setErr("金额必须大于 0");
    }
    const input: HoldingInput = {
      account_id: accId, name: name.trim(), code: code.trim() || null, asset_class: cls, amount: cents,
      shares: auto ? (sharesNum as number) : null, source: auto ? source : null, cost_price: auto ? (costNum ?? null) : null,
    };
    setBusy(true);
    try {
      if (editing) await api.updateHolding(editing.id, input); else await api.addHolding(input);
      reset(); await load();
    } catch (ex) { setErr(errText(ex)); }
    setBusy(false);
  };
  const remove = async (h: Holding) => {
    if (!(await ask(`删除持仓「${h.name}」?`, { title: "删除持仓", kind: "warning" }))) return;
    try { await api.deleteHolding(h.id); await load(); } catch (ex) { setErr(errText(ex)); }
  };

  return (
    <Modal title="持仓" wide onClose={onClose}>
      <p className="muted" style={{ marginBottom: 10 }}>按份额登记的持仓会按最新价自动更新金额,外币折成人民币。登记了持仓的投资账户,余额等于账户里持仓的合计;账户里的现金可以登记成一条"现金类"的手动持仓。</p>
      {list.length > 0 && (
        <table className="tbl hold-tbl" style={{ marginBottom: 14 }}>
          <thead><tr><th>名称</th><th>账户</th><th>大类</th><th>份额</th><th>成本价</th><th>金额</th><th>盈亏</th><th>更新</th><th /></tr></thead>
          <tbody>
            {list.map((h) => (
              <tr key={h.id}>
                <td>{h.name}{h.code && <span className="muted"> {h.code}</span>}</td><td>{h.account}</td><td>{classLabel(h.asset_class)}</td>
                <td className="num">{h.shares === null ? <span className="muted">手动</span> : sharesText(h.shares)}</td>
                <td className="num">{costPriceText(h)}</td>
                <td className="num">¥{fmt(h.amount)}</td>
                <td className={`num chg ${pnlClass(h.pnl)}`}>{pnlText(h.pnl)}{h.pnl_pct !== null && <div className="sub">{signedPct(h.pnl_pct)}</div>}</td>
                <td className="num" title={h.updated_on}>{h.updated_on.slice(5)}</td>
                <td className="row-acts"><button className="lnk" onClick={() => edit(h)}>编辑</button><button className="lnk" onClick={() => remove(h)}>删除</button></td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {accounts.length === 0 ? (
        <div className="notice info">还没有“{ACCOUNT_KINDS.investment}”类账户:先到“资产”页添加一个,再回来登记持仓。</div>
      ) : (
        <form onSubmit={submit}>
          <h3 className="sub-h">{editing ? `编辑「${editing.name}」` : "新增持仓"}</h3>
          <label className="lbl">所在账户</label>
          <select className="f" value={accId} onChange={(e) => setAccId(Number(e.target.value))}>
            {accounts.map((a) => <option key={a.id} value={a.id}>{a.name}</option>)}
          </select>
          <div className="tog">
            <button type="button" className={auto ? "on" : ""} onClick={() => setAuto(true)}>按份额自动估值</button>
            <button type="button" className={auto ? "" : "on"} onClick={() => setAuto(false)}>手动填金额</button>
          </div>
          {auto && (
            <>
              <label className="lbl">价格来源</label>
              <select className="f" value={source} onChange={(e) => { setSource(e.target.value as PriceSource); setPrice(null); }}>
                <option value="exchange">场内:股票、ETF</option>
                <option value="fund">场外基金</option>
              </select>
            </>
          )}
          <div className="two-col">
            <div>
              <label className="lbl">{auto ? "代码" : "代码(可不填)"}</label>
              <div className="row-inline">
                <input className="f" maxLength={32} placeholder={auto ? (source === "fund" ? "如 110020" : "如 510300、02800、EWY") : ""} value={code}
                  onChange={(e) => { setCode(e.target.value); setPrice(null); }} onBlur={() => auto && code.trim() && !price && lookup()} />
                {auto && <button type="button" className="btn alt" disabled={looking} onMouseDown={(e) => e.preventDefault()} onClick={lookup}>{looking ? "查询中…" : "查价"}</button>}
              </div>
            </div>
            <div><label className="lbl">资产大类</label><ClassSelect value={cls} onChange={setCls} /></div>
          </div>
          {auto && price && <p className="muted" style={{ margin: "-4px 0 8px" }}>{priceText(price)}</p>}
          <label className="lbl">名称</label>
          <input className="f" maxLength={100} placeholder={auto ? "查价后自动填入" : "如:标普 500 指数基金"} value={name} onChange={(e) => setName(e.target.value)} />
          {auto ? (
            <>
              <label className="lbl">份额</label>
              <input className="f" inputMode="decimal" value={shares} onChange={(e) => setShares(e.target.value)} />
              {estimate !== null && <p className="muted" style={{ margin: "-4px 0 8px" }}>约 ¥{fmt(estimate)}</p>}
              <label className="lbl">成本价{currency ? `(${CURRENCY_NAME[currency]})` : "(按交易币种,可不填)"}</label>
              <input className="f" inputMode="decimal" placeholder={currency === "USD" ? "如 VOO 买入均价 500.25 美元" : "买入均价,用来算盈亏"} value={costPrice}
                onChange={(e) => setCostPrice(e.target.value)} />
              {hint && <p className="muted" style={{ margin: "-4px 0 8px" }}>{hint}</p>}
            </>
          ) : (
            <>
              <label className="lbl">金额(元)</label>
              <input className="f" type="number" step="0.01" min="0" value={amount} onChange={(e) => setAmount(e.target.value)} />
            </>
          )}
          {err && <div className="notice bad" style={{ margin: "4px 0 10px" }}>{err}</div>}
          <button className="go" disabled={busy}>{editing ? "保存修改" : "添加"}</button>
          {editing && <button type="button" className="cancel" onClick={reset}>取消编辑</button>}
        </form>
      )}
      {accounts.length === 0 && err && <div className="notice bad">{err}</div>}
    </Modal>
  );
}

function ProfileModal({ current, onClose }: { current: RiskProfile | null; onClose: () => void }) {
  const api = useApi();
  const [level, setLevel] = useState<RiskLevel>(current?.level ?? "balanced");
  const [dd, setDd] = useState(String(current?.max_drawdown ?? 30));
  const [years, setYears] = useState(String(current?.horizon_years ?? 10));
  const [debt, setDebt] = useState(current?.high_debt ?? false);
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const d = Number(dd);
    const y = Number(years);
    if (!Number.isInteger(d) || d < 5 || d > 60) return setErr("最大回撤填 5 到 60 之间的整数");
    if (!Number.isInteger(y) || y < 1 || y > 50) return setErr("年数填 1 到 50 之间的整数");
    setBusy(true);
    try { await api.saveRiskProfile({ level, max_drawdown: d, horizon_years: y, high_debt: debt }); onClose(); }
    catch (ex) { setErr(errText(ex)); setBusy(false); }
  };

  return (
    <div className="modal on" onClick={(e) => e.target === e.currentTarget && onClose()}>
      <form className="sheet" onSubmit={submit} role="dialog" aria-label="风险偏好问卷">
        <h2>风险偏好问卷</h2>
        <label className="lbl">风险等级</label>
        <div className="tog tog3">
          {(Object.keys(LEVEL_LABEL) as RiskLevel[]).map((l) => (
            <button type="button" key={l} className={level === l ? "on" : ""} onClick={() => setLevel(l)}>{LEVEL_LABEL[l]}</button>
          ))}
        </div>
        <label className="lbl">能接受的最大回撤(%)</label>
        <input className="f" type="number" min="5" max="60" step="1" value={dd} onChange={(e) => setDd(e.target.value)} />
        <label className="lbl">这笔钱几年后用(年)</label>
        <input className="f" type="number" min="1" max="50" step="1" value={years} onChange={(e) => setYears(e.target.value)} />
        <label className="check">
          <input type="checkbox" checked={debt} onChange={(e) => setDebt(e.target.checked)} />
          <span>我有年利率 8% 以上的负债</span>
        </label>
        {err && <div className="notice bad" style={{ margin: "4px 0 10px" }}>{err}</div>}
        <button className="go" disabled={busy}>保存</button>
        <button type="button" className="cancel" onClick={onClose}>取消</button>
      </form>
    </div>
  );
}

function ProductsModal({ onClose }: { onClose: () => void }) {
  const api = useApi();
  const [list, setList] = useState<Product[]>([]);
  const [editing, setEditing] = useState<Product | null>(null);
  const [name, setName] = useState("");
  const [code, setCode] = useState("");
  const [cls, setCls] = useState<AssetClass>("bond");
  const [note, setNote] = useState("");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);

  const load = () => api.listProducts().then(setList).catch((e) => setErr(errText(e)));
  useEffect(() => { load(); }, []);

  const reset = () => { setEditing(null); setName(""); setCode(""); setNote(""); setErr(""); };
  const edit = (p: Product) => { setEditing(p); setName(p.name); setCode(p.code ?? ""); setCls(p.asset_class); setNote(p.note); setErr(""); };
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!name.trim()) return setErr("请填写名称");
    const input: ProductInput = { name: name.trim(), code: code.trim() || null, asset_class: cls, note: note.trim() };
    setBusy(true);
    try {
      if (editing) await api.updateProduct(editing.id, input); else await api.addProduct(input);
      reset(); await load();
    } catch (ex) { setErr(errText(ex)); }
    setBusy(false);
  };
  const remove = async (p: Product) => {
    if (!(await ask(`从清单里删除「${p.name}」?${p.builtin ? "\n这是内置示例,删除后不会再被补回。" : ""}`, { title: "删除产品", kind: "warning" }))) return;
    try { await api.deleteProduct(p.id); await load(); } catch (ex) { setErr(errText(ex)); }
  };

  return (
    <Modal title="候选产品清单" wide onClose={onClose}>
      <p className="muted" style={{ marginBottom: 10 }}>组合建议和助手只从这份清单里挑产品。</p>
      <table className="tbl" style={{ marginBottom: 14 }}>
        <thead><tr><th>名称</th><th>大类</th><th className="note-cell">说明</th><th /></tr></thead>
        <tbody>
          {list.map((p) => (
            <tr key={p.id}>
              <td>{p.name}{p.code && <span className="muted"> {p.code}</span>}{p.builtin && <span className="badge">示例</span>}</td>
              <td>{classLabel(p.asset_class)}</td><td className="muted note-cell">{productNote(p.note)}</td>
              <td className="row-acts"><button className="lnk" onClick={() => edit(p)}>编辑</button><button className="lnk" onClick={() => remove(p)}>删除</button></td>
            </tr>
          ))}
          {list.length === 0 && <tr><td colSpan={4} className="muted">清单是空的。</td></tr>}
        </tbody>
      </table>
      <form onSubmit={submit}>
        <h3 className="sub-h">{editing ? `编辑「${editing.name}」` : "新增产品"}</h3>
        <div className="two-col">
          <div><label className="lbl">名称</label><input className="f" maxLength={100} value={name} onChange={(e) => setName(e.target.value)} /></div>
          <div><label className="lbl">代码(可不填)</label><input className="f" maxLength={32} value={code} onChange={(e) => setCode(e.target.value)} /></div>
        </div>
        <label className="lbl">资产大类</label>
        <ClassSelect value={cls} onChange={setCls} />
        <label className="lbl">说明(可不填)</label>
        <input className="f" maxLength={500} value={note} onChange={(e) => setNote(e.target.value)} />
        {err && <div className="notice bad" style={{ margin: "4px 0 10px" }}>{err}</div>}
        <button className="go" disabled={busy}>{editing ? "保存修改" : "添加"}</button>
        {editing && <button type="button" className="cancel" onClick={reset}>取消编辑</button>}
      </form>
    </Modal>
  );
}
