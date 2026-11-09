import { describe, expect, it } from "vitest";
import {
  barScale, compactYuan, labelEvery, lineScale, linear, monthName, monthTicks, nearestIndex, niceScale,
  roundedTopBar, spreadX,
} from "./charts";

describe("compactYuan", () => {
  it("按万/亿缩写,小额保留千分位", () => {
    expect(compactYuan(0)).toBe("0");
    expect(compactYuan(800_000)).toBe("8,000");
    expect(compactYuan(1_200_000)).toBe("1.2万");
    expect(compactYuan(300_000_000)).toBe("300万");
    expect(compactYuan(150_000_000_000)).toBe("15亿");
  });
  it("负数带负号,不出现 -0", () => {
    expect(compactYuan(-250_000)).toBe("-2,500");
    expect(compactYuan(-1_200_000)).toBe("-1.2万");
    expect(compactYuan(-10)).toBe("0");
  });
});

describe("niceScale", () => {
  it("范围覆盖数据,刻度等距且含两端", () => {
    for (const [lo, hi] of [[0, 87], [0, 1], [3_100_000, 3_400_000], [-5000, 12000], [0.3, 0.9], [1e9, 1.2e9]]) {
      const s = niceScale(lo, hi);
      expect(s.min).toBeLessThanOrEqual(lo);
      expect(s.max).toBeGreaterThanOrEqual(hi);
      expect(s.ticks[0]).toBeCloseTo(s.min, 6);
      expect(s.ticks[s.ticks.length - 1]).toBeCloseTo(s.max, 6);
      expect(s.ticks.length).toBeGreaterThanOrEqual(2);
      expect(s.ticks.length).toBeLessThanOrEqual(9);
      const d = s.ticks[1] - s.ticks[0];
      s.ticks.slice(1).forEach((t, i) => expect(t - s.ticks[i]).toBeCloseTo(d, 4));
    }
  });
  it("所有值相同或输入异常时不崩溃", () => {
    const same = niceScale(500, 500);
    expect(same.min).toBeLessThan(500);
    expect(same.max).toBeGreaterThan(500);
    expect(niceScale(0, 0).max).toBeGreaterThan(0);
    expect(niceScale(NaN, 1)).toEqual({ min: 0, max: 1, ticks: [0, 1] });
    expect(niceScale(9, 1).min).toBeLessThanOrEqual(1); // 颠倒的参数
  });
});

describe("lineScale / barScale", () => {
  it("折线图不强行从 0 开始,让小幅变化看得见", () => {
    const s = lineScale([3_200_000_00, 3_286_450_00]);
    expect(s.min).toBeGreaterThan(3_000_000_00);
  });
  it("柱状图总是从 0 开始,全为 0 也有合理范围", () => {
    expect(barScale([120, 560]).min).toBe(0);
    const zero = barScale([0, 0]);
    expect(zero.min).toBe(0);
    expect(zero.max).toBeGreaterThan(0);
    expect(barScale([]).min).toBe(0);
  });
  it("净资产为负时范围仍然覆盖", () => {
    const s = lineScale([-300, -100]);
    expect(s.min).toBeLessThanOrEqual(-300);
    expect(s.max).toBeGreaterThanOrEqual(-100);
  });
});

describe("坐标", () => {
  it("linear 映射与退化", () => {
    const f = linear([0, 100], [200, 0]);
    expect([f(0), f(50), f(100)]).toEqual([200, 100, 0]);
    expect(linear([5, 5], [0, 10])(5)).toBe(5);
  });
  it("spreadX 与 nearestIndex", () => {
    expect(spreadX(0, 0, 100)).toEqual([]);
    expect(spreadX(1, 0, 100)).toEqual([50]);
    const xs = spreadX(5, 0, 100);
    expect(xs).toEqual([0, 25, 50, 75, 100]);
    expect(nearestIndex(-30, xs)).toBe(0);
    expect(nearestIndex(61, xs)).toBe(2);
    expect(nearestIndex(999, xs)).toBe(4);
    expect(nearestIndex(10, [50])).toBe(0);
  });
  it("roundedTopBar:高度不足时圆角收缩,零高度没有路径", () => {
    expect(roundedTopBar(0, 0, 10, 0)).toBe("");
    expect(roundedTopBar(0, 0, 0, 10)).toBe("");
    const thin = roundedTopBar(0, 0, 20, 2); // 圆角不能比柱子还高
    expect(thin).toContain("V2Q0,0 2,0");
    expect(roundedTopBar(10, 20, 16, 40)).toMatch(/^M10,60V24Q10,20 14,20H22Q26,20 26,24V60Z$/);
  });
});

describe("月份标签", () => {
  it("相邻标签间距不足时抽稀", () => {
    expect(labelEvery(6, 500)).toBe(1);
    expect(labelEvery(36, 500)).toBeGreaterThanOrEqual(3);
    expect(labelEvery(1, 500)).toBe(1);
    expect(labelEvery(120, 500)).toBeGreaterThan(labelEvery(36, 500));
  });
  const range = (from: [number, number], n: number) =>
    Array.from({ length: n }, (_, k) => {
      const t = from[1] - 1 + k;
      return `${from[0] + Math.floor(t / 12)}-${String((t % 12) + 1).padStart(2, "0")}`;
    });

  it("逐月标注时:第一个和每年一月带年份", () => {
    const ticks = monthTicks(range([2025, 11], 5), 1); // 2025-11 … 2026-03
    expect(ticks.map((t) => t.text)).toEqual(["11月", "12月", "1月", "2月", "3月"]);
    expect(ticks.filter((t) => t.year).map((t) => [t.text, t.year])).toEqual([["11月", "2025"], ["1月", "2026"]]);
  });
  it("隔月标注、一月被跳过时,换年后的第一个标签仍带年份", () => {
    const months = range([2024, 11], 24); // 2024-11 … 2026-10
    const ticks = monthTicks(months, 2); // 从 2026-10 往回:10,8,6,…,12(2024)
    expect(ticks[ticks.length - 1].i).toBe(23); // 最新月份一定有标签
    expect(ticks.some((t) => t.text === "1月")).toBe(false); // 一月确实没出现
    const years = ticks.filter((t) => t.year).map((t) => `${months[t.i]}`);
    expect(years).toEqual(["2024-12", "2025-02", "2026-02"]);
  });
  it("只有一个月、每隔很多个月时也不出错", () => {
    expect(monthTicks(["2026-10"], 3)).toEqual([{ i: 0, text: "10月", year: "2026" }]);
    expect(monthTicks([], 2)).toEqual([]);
    expect(monthTicks(range([2026, 1], 3), 0).length).toBe(3); // every<1 当作 1
    expect(monthName("2026-03")).toBe("2026年3月");
  });
});
