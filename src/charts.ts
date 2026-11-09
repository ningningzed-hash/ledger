// 图表用的纯函数:刻度、金额缩写、坐标计算。不依赖 DOM,便于单元测试。

/** 金额(分)缩写,用于坐标轴:0 / 8,000 / 12万 / 1.5亿 */
export function compactYuan(cents: number): string {
  const y = cents / 100;
  const a = Math.abs(y);
  const sign = y < 0 && Math.round(a) !== 0 ? "-" : "";
  const trim = (n: number) => String(Math.round(n * 10) / 10);
  if (a >= 1e8) return sign + trim(a / 1e8) + "亿";
  if (a >= 1e4) return sign + trim(a / 1e4) + "万";
  return sign + Math.round(a).toLocaleString("en-US");
}

export type Scale = { min: number; max: number; ticks: number[] };

/** 把 [lo, hi] 扩成"好看"的刻度范围,约 count 个间隔 */
export function niceScale(lo: number, hi: number, count = 4): Scale {
  if (!Number.isFinite(lo) || !Number.isFinite(hi)) return { min: 0, max: 1, ticks: [0, 1] };
  if (lo > hi) [lo, hi] = [hi, lo];
  if (lo === hi) {
    // 所有值相同:以它为中心留出上下空间,避免除以零
    const half = Math.max(Math.abs(lo) * 0.05, 1);
    lo -= half;
    hi += half;
  }
  const raw = (hi - lo) / Math.max(1, count);
  const mag = Math.pow(10, Math.floor(Math.log10(raw)));
  const norm = raw / mag;
  const step = (norm < 1.5 ? 1 : norm < 3 ? 2 : norm < 7 ? 5 : 10) * mag;
  const min = Math.floor(lo / step) * step;
  const max = Math.ceil(hi / step) * step;
  const ticks: number[] = [];
  const n = Math.round((max - min) / step);
  for (let i = 0; i <= n; i++) ticks.push(Math.round((min + i * step) / mag * 1e6) / 1e6 * mag);
  return { min, max, ticks };
}

/** 折线图的纵轴范围:数据上下各留 8% 空间(净资产可能为负,不强行包含 0) */
export function lineScale(values: number[], count = 4): Scale {
  if (!values.length) return niceScale(0, 1, count);
  const lo = Math.min(...values);
  const hi = Math.max(...values);
  const pad = (hi - lo) * 0.08;
  return niceScale(lo - pad, hi + pad, count);
}

/** 柱状图的纵轴范围:必须从 0 开始 */
export function barScale(values: number[], count = 4): Scale {
  const hi = Math.max(0, ...values);
  return niceScale(0, hi === 0 ? 1 : hi, count);
}

/** 线性映射 */
export function linear(domain: [number, number], range: [number, number]): (v: number) => number {
  const [d0, d1] = domain;
  const [r0, r1] = range;
  if (d0 === d1) return () => (r0 + r1) / 2;
  return (v) => r0 + ((v - d0) / (d1 - d0)) * (r1 - r0);
}

/** n 个点沿横轴均匀分布;只有 1 个点时放在中间 */
export function spreadX(n: number, left: number, right: number): number[] {
  if (n <= 0) return [];
  if (n === 1) return [(left + right) / 2];
  return Array.from({ length: n }, (_, i) => left + ((right - left) * i) / (n - 1));
}

/** 离 x 最近的点的下标 */
export function nearestIndex(x: number, xs: number[]): number {
  let best = 0;
  for (let i = 1; i < xs.length; i++) if (Math.abs(xs[i] - x) < Math.abs(xs[best] - x)) best = i;
  return best;
}

/** 顶端圆角、底部(基线)平直的柱子路径 */
export function roundedTopBar(x: number, y: number, w: number, h: number, r = 4): string {
  if (h <= 0 || w <= 0) return "";
  const rr = Math.min(r, h, w / 2);
  return `M${x},${y + h}V${y + rr}Q${x},${y} ${x + rr},${y}H${x + w - rr}Q${x + w},${y} ${x + w},${y + rr}V${y + h}Z`;
}

/** 月份太多时只标一部分,保证相邻标签间距不小于 minGap 像素 */
export function labelEvery(n: number, span: number, minGap = 44): number {
  if (n <= 1) return 1;
  const gap = span / (n - 1);
  return Math.max(1, Math.ceil(minGap / Math.max(gap, 1)));
}

export type MonthTick = { i: number; text: string; year?: string };

/**
 * 横轴月份标签:从最新月份往回每隔 every 个标一次(所以最新月份一定有标签)。
 * 第一个标签、以及年份与上一个标签不同的标签,附带年份——哪怕一月恰好被跳过,也看得出换了年。
 */
export function monthTicks(months: string[], every: number): MonthTick[] {
  const last = months.length - 1;
  const out: MonthTick[] = [];
  let prevYear = "";
  for (let i = 0; i <= last; i++) {
    if ((last - i) % Math.max(1, every) !== 0) continue;
    const [y, mo] = months[i].split("-");
    out.push({ i, text: `${Number(mo)}月`, ...(y !== prevYear ? { year: y } : {}) });
    prevYear = y;
  }
  return out;
}

export const monthName = (m: string) => {
  const [y, mo] = m.split("-");
  return `${y}年${Number(mo)}月`;
};
