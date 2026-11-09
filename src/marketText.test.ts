import { describe, expect, it } from "vitest";
import { alertText, changeText, parseAlertPct, pctText, pointTitle, priceText, rangeStats, timeTicks, trendClass } from "./marketText";

describe("价格与涨跌文字", () => {
  it("价格位数随大小变化", () => {
    expect(priceText(1392.05)).toBe("1,392.05");
    expect(priceText(0.456)).toBe("0.456");
    expect(priceText(null)).toBe("—");
  });
  it("涨跌带符号,四舍五入后为零就不带符号", () => {
    expect(pctText(2.814)).toBe("+2.81%");
    expect(pctText(-1.2)).toBe("−1.20%");
    expect(pctText(0.001)).toBe("0.00%");
    expect(changeText(-38.05)).toBe("−38.05");
    expect(changeText(undefined)).toBe("—");
  });
  it("红涨绿跌的样式类", () => {
    expect([trendClass(1), trendClass(-1), trendClass(0), trendClass(null)]).toEqual(["up", "down", "flat", "flat"]);
  });
});

describe("区间统计", () => {
  const pts = [10, 12, 9, 11].map((price, i) => ({ t: `2026-10-0${i + 1}`, price }));
  it("有基准时相对基准计算", () => {
    const s = rangeStats(pts, 10)!;
    expect([s.change, s.pct, s.high, s.low, s.amplitude]).toEqual([1, 10, 12, 9, 30]);
  });
  it("没有基准就用第一个点", () => {
    expect(rangeStats(pts, null)!.base).toBe(10);
    expect(rangeStats([], 5)).toBeNull();
  });
});

describe("横轴刻度", () => {
  it("当日按整点标,跳过从中途开始的第一段", () => {
    const ts = ["2026-10-30 09:35", "2026-10-30 09:40", "2026-10-30 10:00", "2026-10-30 10:05", "2026-10-30 11:00"];
    const xs = [0, 50, 100, 150, 200];
    expect(timeTicks(ts, xs, "1d")).toEqual([{ i: 2, text: "10:00" }, { i: 4, text: "11:00" }]);
  });
  it("太挤就跳过", () => {
    const ts = ["2026-08-31", "2026-09-01", "2026-10-01", "2026-11-02"];
    expect(timeTicks(ts, [0, 10, 20, 100], "1y")).toEqual([{ i: 1, text: "9月" }, { i: 3, text: "11月" }]);
  });
  it("5 年按年标", () => {
    expect(timeTicks(["2025-12-26", "2026-01-02"], [0, 100], "5y")).toEqual([{ i: 1, text: "2026" }]);
  });
});

it("提示框标题", () => {
  expect(pointTitle("2026-10-30 09:35", true)).toBe("2026年10月30日 09:35");
  expect(pointTitle("2026-10-30", false)).toBe("2026年10月30日");
});

describe("涨跌过大提醒", () => {
  it("阈值输入", () => {
    expect(parseAlertPct("5")).toBe(5);
    expect(parseAlertPct(" 3.5% ")).toBe(3.5);
    expect(parseAlertPct("50")).toBe(50);
    for (const bad of ["", "0", "-1", "50.01", "abc", "1.234"]) expect(parseAlertPct(bad)).toBeUndefined();
  });
  it("开关说明", () => {
    expect(alertText(true, 5)).toContain("±5%");
    expect(alertText(false, 5)).toContain("关闭中");
  });
});
