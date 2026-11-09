// 组合建议与利息界面用到的纯文本/换算函数(不依赖 DOM,便于单元测试)。
import type { Currency, Holding, Notice, PnlSummary, Price, PriceRefresh, RebalanceRow, Readiness, RiskLevel } from "./api";

export const LEVEL_LABEL: Record<RiskLevel, string> = { conservative: "保守", balanced: "稳健", growth: "进取" };

/** 百分数输入("3.5")→ 基点(350)。空串返回 null;非数字或越界返回 undefined。 */
export function percentToBps(input: string): number | null | undefined {
  const t = input.trim().replace(/%$/, "");
  if (t === "") return null;
  if (!/^\d+(\.\d{1,2})?$/.test(t)) return undefined;
  const bps = Math.round(parseFloat(t) * 100);
  return bps >= 0 && bps <= 10_000 ? bps : undefined;
}

/** 基点 → 百分数文本,保留两位:350 → "3.50" */
export const bpsToPercent = (bps: number) => (bps / 100).toFixed(2);

/** 带符号的百分点差:+18.0 / -73.0 / 0.0 */
export function signedPoints(diff: number): string {
  if (Math.abs(diff) < 0.05) return "0.0";
  return `${diff > 0 ? "+" : "-"}${Math.abs(diff).toFixed(1)}`;
}

/** 带符号的金额(分)文本:+¥18,000.00 / -¥73,000.00 */
export function signedYuan(cents: number): string {
  const body = (Math.abs(cents) / 100).toLocaleString("zh-CN", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  if (cents === 0) return "¥0.00";
  return `${cents > 0 ? "+" : "-"}¥${body}`;
}

export const ACTION_LABEL: Record<RebalanceRow["action"], string> = { add: "增配", reduce: "减配", balanced: "基本合适" };

/** 提示条的样式:priority 与 warn 用醒目底色,其余为普通提示;文字内容才是主要信息,不只靠颜色 */
export function noticeClass(n: Notice): string {
  return n.level === "priority" ? "notice warn" : n.level === "warn" ? "notice warn" : "notice info";
}

export const noticeTag = (n: Notice) => (n.level === "priority" ? "优先" : n.level === "warn" ? "注意" : "提示");

/** 应急金卡片的小字(数值本身已在大字里) */
export function emergencyText(r: Readiness): string {
  if (r.emergency_months === null) return "支出数据不足";
  return r.emergency_months < 3 ? "建议至少 3 个月" : "现金、存款 ÷ 月支出";
}

/** "全部用新增资金补足"的说明 */
export function fillText(months: number | null, buyTotal: number): string | null {
  if (buyTotal <= 0) return null;
  if (months === null) return "月均结余不为正,先改善结余";
  return months <= 1 ? "按月均结余,约 1 个月补足" : `按月均结余,约 ${months} 个月补足`;
}

/** 旧版默认理财理念里禁止推荐具体产品,与"组合建议"的候选清单冲突;保存过这段旧文字的用户需要恢复默认 */
export const hasLegacyPhilosophy = (text: string) => text.includes("不推荐具体的股票、基金");

/** 条形图里每一行的可读描述(屏幕阅读器与提示框共用) */
export const rowSummary = (r: RebalanceRow) =>
  `${r.label}:当前 ${r.current_pct.toFixed(1)}%,目标 ${r.target_pct.toFixed(1)}%,${ACTION_LABEL[r.action]}`;

/** 份额输入 → 数字;空串返回 null;非法(非正数、超过 4 位小数)返回 undefined */
export function parseShares(input: string): number | null | undefined {
  const t = input.trim().replace(/,/g, "");
  if (t === "") return null;
  if (!/^\d+(\.\d{1,4})?$/.test(t)) return undefined;
  const n = parseFloat(t);
  return n > 0 && n <= 1e11 ? n : undefined;
}

export const CURRENCY_SIGN: Record<Currency, string> = { CNY: "¥", HKD: "HK$", USD: "$" };
export const CURRENCY_NAME: Record<Currency, string> = { CNY: "元", HKD: "港元", USD: "美元" };

/** 成本价输入 → 数字;空串返回 null;非法(非正数、超过 6 位小数)返回 undefined */
export function parseCostPrice(input: string): number | null | undefined {
  const t = input.trim().replace(/,/g, "");
  if (t === "") return null;
  if (!/^\d+(\.\d{1,6})?$/.test(t)) return undefined;
  const n = parseFloat(t);
  return n > 0 && n < 1e9 ? n : undefined;
}

/** 带符号的百分数:+10.00% / -10.00% / 0.00%;空值显示"—" */
export function signedPct(pct: number | null): string {
  if (pct === null) return "—";
  if (Math.abs(pct) < 0.005) return "0.00%";
  return `${pct > 0 ? "+" : "-"}${Math.abs(pct).toFixed(2)}%`;
}

/** 盈亏金额(分);算不出时显示"—" */
export const pnlText = (cents: number | null) => (cents === null ? "—" : signedYuan(cents));

/** 涨跌着色:沿用行情页的 up / down(红涨绿跌) */
export const pnlClass = (cents: number | null) => (cents === null || cents === 0 ? "" : cents > 0 ? "up" : "down");

/** 成本价显示:$500 / ¥1,200.5;没填为"—" */
export function costPriceText(h: Pick<Holding, "cost_price" | "currency">): string {
  if (h.cost_price === null) return "—";
  return `${CURRENCY_SIGN[h.currency ?? "CNY"]}${h.cost_price.toLocaleString("zh-CN", { maximumFractionDigits: 6 })}`;
}

/** 今年以来盈亏的说明;快照不够早时注明从哪天开始统计 */
export function ytdText(s: PnlSummary): string {
  if (!s.ytd) return "今年以来:还没有足够的数据";
  const from = s.ytd.full ? "" : `(从 ${Number(s.ytd.since.slice(5, 7))} 月 ${Number(s.ytd.since.slice(8, 10))} 日开始统计)`;
  return `今年以来 ${signedYuan(s.ytd.pnl)}${from}`;
}

/** 汇总卡片底部的小字:有没算进来的持仓时说明 */
export const excludedText = (s: PnlSummary) => (s.excluded > 0 ? `${s.excluded} 只未计入(手动金额、未填成本价或还没有现价)` : "");

/** 编辑持仓时份额变了,而成本价没跟着改:提醒更新为加仓 / 减仓后的平均成本 */
export const costHint = (oldShares: number | null, newShares: number | null | undefined, oldCost: number | null, newCost: number | null | undefined) =>
  oldShares !== null && typeof newShares === "number" && newShares !== oldShares && oldCost !== null && newCost === oldCost
    ? "份额变了,记得把成本价改成加仓 / 减仓后的平均成本"
    : "";

/** 查价结果一句话:沪深300ETF华泰柏瑞 ¥4.065(10-31 15:00);外币另附折合人民币 */
export function priceText(p: Price): string {
  const time = p.time ? `(${p.time.length > 10 ? p.time.slice(5) : `净值日期 ${p.time}`})` : "";
  const cny = p.currency === "CNY" ? "" : `,约合 ¥${p.price_cny.toFixed(4).replace(/0+$/, "").replace(/\.$/, "")}`;
  return `${p.name} ${CURRENCY_SIGN[p.currency]}${p.price}${cny}${time}`;
}

/** 持仓的估值方式:自动(场内 / 场外基金)或手动 */
export const holdingMode = (h: Pick<Holding, "secid">) => (!h.secid ? "手动" : h.secid.startsWith("OF.") ? "自动 · 场外基金" : "自动 · 场内");

/** 份额显示:去掉多余的 0,千分位 */
export const sharesText = (n: number) => n.toLocaleString("zh-CN", { maximumFractionDigits: 4 });

/** 手动刷新估值后的提示 */
export function refreshText(r: PriceRefresh): string {
  const failed = r.failed.map((f) => `${f.name}:${f.reason}`).join(";");
  if (r.updated === 0 && r.failed.length === 0) return "没有按份额登记的持仓,不需要刷新";
  return [r.updated > 0 ? `已按最新价更新 ${r.updated} 条持仓` : "", failed ? `未能更新 — ${failed}` : ""].filter(Boolean).join("。");
}

/** 内置示例的说明末尾都带同一句免责,清单里已有"示例"标记、页面底部也有免责声明,显示时去掉 */
export const productNote = (note: string) => note.replace("示例,仅供参考,以发行方披露为准。", "").trim();
