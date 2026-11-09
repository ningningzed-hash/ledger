import { describe, expect, it } from "vitest";
import type { Notice, PnlSummary, RebalanceRow, Readiness } from "./api";
import {
  bpsToPercent, emergencyText, fillText, hasLegacyPhilosophy, holdingMode, noticeClass, noticeTag, parseShares, percentToBps, priceText, productNote, refreshText,
  rowSummary, sharesText, signedPoints, signedYuan, parseCostPrice, signedPct, pnlText, pnlClass, costPriceText, ytdText, excludedText, costHint,
} from "./portfolioText";

describe("percentToBps / bpsToPercent", () => {
  it("把百分数换成基点,保留两位小数", () => {
    expect(percentToBps("3.5")).toBe(350);
    expect(percentToBps(" 8.4% ")).toBe(840);
    expect(percentToBps("0")).toBe(0);
    expect(percentToBps("100")).toBe(10_000);
    expect(percentToBps("0.07")).toBe(7);
  });
  it("空串表示清除,非法或越界返回 undefined", () => {
    expect(percentToBps("")).toBeNull();
    expect(percentToBps("   ")).toBeNull();
    for (const bad of ["abc", "-1", "100.01", "3.555", "3,5", "1e2"]) expect(percentToBps(bad)).toBeUndefined();
  });
  it("基点显示为两位小数", () => {
    expect(bpsToPercent(350)).toBe("3.50");
    expect(bpsToPercent(7)).toBe("0.07");
    expect(bpsToPercent(0)).toBe("0.00");
  });
});

describe("带符号的差额", () => {
  it("百分点差", () => {
    expect(signedPoints(18)).toBe("+18.0");
    expect(signedPoints(-73)).toBe("-73.0");
    expect(signedPoints(0)).toBe("0.0");
    expect(signedPoints(0.04)).toBe("0.0");
    expect(signedPoints(-0.04)).toBe("0.0");
  });
  it("金额(分)", () => {
    expect(signedYuan(18_000_00)).toBe("+¥18,000.00");
    expect(signedYuan(-73_000_00)).toBe("-¥73,000.00");
    expect(signedYuan(0)).toBe("¥0.00");
  });
});

describe("提示与说明文字", () => {
  const n = (level: Notice["level"]): Notice => ({ code: "x", level, message: "m" });
  it("提示条:优先和注意醒目,并带文字标签", () => {
    expect(noticeClass(n("priority"))).toBe("notice warn");
    expect(noticeClass(n("warn"))).toBe("notice warn");
    expect(noticeClass(n("info"))).toBe("notice info");
    expect([noticeTag(n("priority")), noticeTag(n("warn")), noticeTag(n("info"))]).toEqual(["优先", "注意", "提示"]);
  });
  const r = (m: number | null): Readiness => ({
    liquid: 0, avg_monthly_expense: null, avg_monthly_surplus: null, emergency_months: m, liabilities: 0, high_interest_debts: [], high_debt_declared: false,
  });
  it("应急金", () => {
    expect(emergencyText(r(null))).toContain("数据不足");
    expect(emergencyText(r(2))).toBe("建议至少 3 个月");
    expect(emergencyText(r(6))).not.toContain("6.0");
  });
  it("补足时间", () => {
    expect(fillText(15, 73_000_00)).toContain("约 15 个月");
    expect(fillText(1, 100)).toContain("约 1 个月");
    expect(fillText(null, 73_000_00)).toContain("先改善结余");
    expect(fillText(null, 0)).toBeNull();
  });
  it("一行的可读描述", () => {
    const row = { label: "债券", current_pct: 0, target_pct: 40, action: "add" } as RebalanceRow;
    expect(rowSummary(row)).toBe("债券:当前 0.0%,目标 40.0%,增配");
  });
});

describe("hasLegacyPhilosophy", () => {
  it("只识别旧版理念里禁止推荐具体产品的那句", () => {
    expect(hasLegacyPhilosophy("7. 不预测市场:不判断短期涨跌,不推荐具体的股票、基金或其他证券,不做买卖时点的建议。")).toBe(true);
    expect(hasLegacyPhilosophy("7. 不预测市场:不判断短期涨跌,不做买卖时点的建议。给组合建议时,只在程序提供的候选产品清单范围内说明可选的产品类别和产品,不自行推荐清单之外的股票、基金或其他证券。")).toBe(false);
    expect(hasLegacyPhilosophy("")).toBe(false);
  });
});

describe("按份额自动估值", () => {
  it("份额输入", () => {
    expect(parseShares("1000")).toBe(1000);
    expect(parseShares(" 1,234.5678 ")).toBe(1234.5678);
    expect(parseShares("")).toBeNull();
    for (const bad of ["0", "-1", "1.23456", "abc", "1e3"]) expect(parseShares(bad)).toBeUndefined();
  });
  it("查价结果:场内带时间,场外基金带净值日期,外币附人民币", () => {
    expect(priceText({ secid: "1.510300", name: "沪深300ETF", price: 4.065, currency: "CNY", price_cny: 4.065, time: "2026-10-31 15:00" }))
      .toBe("沪深300ETF ¥4.065(10-31 15:00)");
    expect(priceText({ secid: "OF.110020", name: "联接A", price: 1.6526, currency: "CNY", price_cny: 1.6526, time: "2026-10-30" }))
      .toBe("联接A ¥1.6526(净值日期 2026-10-30)");
    expect(priceText({ secid: "107.EWY", name: "韩国ETF", price: 152.89, currency: "USD", price_cny: 1017.2, time: null }))
      .toBe("韩国ETF $152.89,约合 ¥1017.2");
  });
  it("估值方式与份额", () => {
    expect(holdingMode({ secid: null })).toBe("手动");
    expect(holdingMode({ secid: "OF.110020" })).toBe("自动 · 场外基金");
    expect(holdingMode({ secid: "116.02800" })).toBe("自动 · 场内");
    expect(sharesText(12345.5)).toBe("12,345.5");
  });
  it("刷新提示", () => {
    expect(refreshText({ skipped: false, updated: 0, failed: [] })).toContain("没有按份额登记");
    expect(refreshText({ skipped: false, updated: 2, failed: [] })).toBe("已按最新价更新 2 条持仓");
    expect(refreshText({ skipped: false, updated: 1, failed: [{ name: "基金", reason: "超时" }] })).toBe("已按最新价更新 1 条持仓。未能更新 — 基金:超时");
  });
});

describe("productNote", () => {
  it("去掉内置示例统一的免责后缀", () => {
    expect(productNote("跟踪沪深 300 指数。示例,仅供参考,以发行方披露为准。")).toBe("跟踪沪深 300 指数。");
    expect(productNote("自己写的")).toBe("自己写的");
  });
});

describe("成本价与盈亏", () => {
  const summary = (over: Partial<PnlSummary> = {}): PnlSummary => ({ cost: 36000_00, value: 39600_00, pnl: 3600_00, pnl_pct: 10, counted: 1, excluded: 0, ytd: null, ...over });

  it("parseCostPrice 接受正数,最多 6 位小数", () => {
    expect(parseCostPrice("")).toBeNull();
    expect(parseCostPrice(" 500.25 ")).toBe(500.25);
    expect(parseCostPrice("1,200")).toBe(1200);
    expect(parseCostPrice("0.123456")).toBe(0.123456);
    for (const bad of ["0", "-1", "abc", "1.1234567", "1e3"]) expect(parseCostPrice(bad)).toBeUndefined();
  });

  it("带符号的收益率与盈亏", () => {
    expect(signedPct(10)).toBe("+10.00%");
    expect(signedPct(-5.375)).toBe("-5.38%");
    expect(signedPct(0)).toBe("0.00%");
    expect(signedPct(null)).toBe("—");
    expect(pnlText(null)).toBe("—");
    expect(pnlText(3600_00)).toBe("+¥3,600.00");
    expect([pnlClass(1), pnlClass(-1), pnlClass(0), pnlClass(null)]).toEqual(["up", "down", "", ""]);
  });

  it("成本价按原币显示", () => {
    expect(costPriceText({ cost_price: 500.25, currency: "USD" })).toBe("$500.25");
    expect(costPriceText({ cost_price: 300, currency: "HKD" })).toBe("HK$300");
    expect(costPriceText({ cost_price: 1200, currency: "CNY" })).toBe("¥1,200");
    expect(costPriceText({ cost_price: null, currency: "USD" })).toBe("—");
  });

  it("今年以来与未计入说明", () => {
    expect(ytdText(summary())).toBe("今年以来:还没有足够的数据");
    expect(ytdText(summary({ ytd: { pnl: 960_00, since: "2025-12-31", full: true } }))).toBe("今年以来 +¥960.00");
    expect(ytdText(summary({ ytd: { pnl: -40_00, since: "2026-11-02", full: false } }))).toBe("今年以来 -¥40.00(从 11 月 2 日开始统计)");
    expect(excludedText(summary())).toBe("");
    expect(excludedText(summary({ excluded: 2 }))).toContain("2 只未计入");
  });

  it("份额变了而成本价没改时提醒", () => {
    expect(costHint(10, 20, 500, 500)).toContain("平均成本");
    expect(costHint(10, 20, 500, 525)).toBe("");
    expect(costHint(10, 10, 500, 500)).toBe("");
    expect(costHint(10, 20, null, null)).toBe("");
    expect(costHint(null, 20, null, 500)).toBe("");
  });
});
