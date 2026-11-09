// 行情页的纯函数:价格格式、涨跌文字、区间统计、横轴刻度。不依赖 DOM,便于单元测试。
import type { PricePoint, PriceRange } from "./api";

export const RANGES: { key: PriceRange; label: string }[] = [
  { key: "1d", label: "当日" }, { key: "5d", label: "5日" }, { key: "1m", label: "1月" },
  { key: "3m", label: "3月" }, { key: "1y", label: "1年" }, { key: "5y", label: "5年" },
];

/** 价格:低价股多给一位小数,大数字加千分位 */
export function priceText(v: number | null | undefined): string {
  if (v === null || v === undefined || !Number.isFinite(v)) return "—";
  const d = Math.abs(v) < 1 ? 3 : 2;
  return v.toLocaleString("en-US", { minimumFractionDigits: d, maximumFractionDigits: d });
}

/** 带符号的涨跌幅:+2.81% / −1.20% / 0.00%(负号用真正的减号,和 + 对齐) */
export function pctText(v: number | null | undefined): string {
  if (v === null || v === undefined || !Number.isFinite(v)) return "—";
  const r = Math.round(v * 100) / 100;
  if (r === 0) return "0.00%";
  return `${r > 0 ? "+" : "−"}${Math.abs(r).toFixed(2)}%`;
}

export function changeText(v: number | null | undefined): string {
  if (v === null || v === undefined || !Number.isFinite(v)) return "—";
  if (Math.abs(v) < 1e-9) return "0.00";
  return `${v > 0 ? "+" : "−"}${priceText(Math.abs(v))}`;
}

/** 涨跌对应的样式类(红涨绿跌);文字本身带正负号,颜色只是辅助 */
export function trendClass(v: number | null | undefined): string {
  if (v === null || v === undefined || !Number.isFinite(v) || Math.abs(v) < 1e-9) return "flat";
  return v > 0 ? "up" : "down";
}

export type RangeStats = { base: number; last: number; change: number; pct: number; high: number; low: number; amplitude: number };

/**
 * 区间统计。基准优先用区间开始前的价格(当日走势即昨收),取不到就用区间第一个点。
 * 振幅 = (最高 − 最低) ÷ 基准。
 */
export function rangeStats(points: PricePoint[], base: number | null): RangeStats | null {
  if (!points.length) return null;
  const prices = points.map((p) => p.price);
  const b = base !== null && base > 0 ? base : prices[0];
  const last = prices[prices.length - 1];
  const high = Math.max(...prices), low = Math.min(...prices);
  const change = last - b;
  return { base: b, last, change, pct: b ? (change / b) * 100 : 0, high, low, amplitude: b ? ((high - low) / b) * 100 : 0 };
}

export type TimeTick = { i: number; text: string };

/** 每个点所属的"段":当日按小时、5 日按天、1 月按天、3 月和 1 年按月、5 年按年 */
function group(t: string, range: PriceRange): string {
  switch (range) {
    case "1d": return t.slice(11, 13);
    case "5d": case "1m": return t.slice(0, 10);
    case "3m": case "1y": return t.slice(0, 7);
    default: return t.slice(0, 4);
  }
}

function tickText(t: string, range: PriceRange): string {
  switch (range) {
    case "1d": return `${t.slice(11, 13)}:00`;
    case "5d": case "1m": return `${Number(t.slice(5, 7))}/${Number(t.slice(8, 10))}`;
    case "3m": case "1y": return `${Number(t.slice(5, 7))}月`;
    default: return t.slice(0, 4);
  }
}

/**
 * 横轴刻度:标在每一段的第一个点上(第一段若是从中途开始就不标,免得标签说错时间)。
 * 相邻刻度的距离小于 minGap 像素就跳过,保证不重叠。
 */
export function timeTicks(ts: string[], xs: number[], range: PriceRange, minGap = 48): TimeTick[] {
  const out: TimeTick[] = [];
  let lastX = -Infinity;
  for (let i = 1; i < ts.length; i++) {
    if (group(ts[i], range) === group(ts[i - 1], range)) continue;
    if (xs[i] - lastX < minGap) continue;
    out.push({ i, text: tickText(ts[i], range) });
    lastX = xs[i];
  }
  return out;
}

/** 提示框标题:分钟线显示到分钟,日线/周线显示日期 */
export function pointTitle(t: string, intraday: boolean): string {
  const [y, m, d] = t.slice(0, 10).split("-").map(Number);
  const date = `${y}年${m}月${d}日`;
  return intraday && t.length > 10 ? `${date} ${t.slice(11, 16)}` : date;
}

/** 提醒阈值输入("5"、"3.5%")→ 数字;非法(不大于 0、超过 50、超过两位小数)返回 undefined */
export function parseAlertPct(input: string): number | undefined {
  const t = input.trim().replace(/%$/, "").trim();
  if (!/^\d+(\.\d{1,2})?$/.test(t)) return undefined;
  const n = parseFloat(t);
  return n > 0 && n <= 50 ? n : undefined;
}

/** 提醒开关旁的说明 */
export const alertText = (enabled: boolean, pct: number) =>
  enabled ? `自选和场内持仓当日涨跌超过 ±${pct}% 时发系统通知,每只每天最多一次。应用开着时每小时检查一次。` : "关闭中。打开后,自选和场内持仓当日涨跌过大时会发系统通知。";
