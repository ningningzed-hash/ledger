import { describe, expect, it } from "vitest";
import type { SubPreview, Subscription } from "./api";
import { daysText, detailText, nextText, previewText, scheduleText } from "./subText";

const base: Subscription = {
  id: 1, name: "房租", kind: "expense", amount: 300000, frequency: "monthly", start_date: "2026-08-05", end_date: null,
  paused: false, category_id: null, category: "居住", account_id: null, account: "招行储蓄卡",
  posted_count: 3, last_posted: "2026-10-05", next_due: "2026-11-05", status: "active", monthly_equivalent: 300000,
};
const s = (o: Partial<Subscription>): Subscription => ({ ...base, ...o });

describe("scheduleText", () => {
  it("月度与年度", () => {
    expect(scheduleText(base)).toBe("每月 5 日");
    expect(scheduleText(s({ frequency: "yearly", start_date: "2025-03-10" }))).toBe("每年 3 月 10 日");
  });
  it("29–31 日说明月底处理", () => {
    expect(scheduleText(s({ start_date: "2026-01-31" }))).toBe("每月 31 日(没有这一天的月份取月底)");
    expect(scheduleText(s({ start_date: "2026-01-28" }))).toBe("每月 28 日");
  });
});

describe("daysText", () => {
  it("今天、明天、几天后、几天前,跨月跨年也对", () => {
    expect(daysText("2026-10-28", "2026-10-28")).toBe("今天");
    expect(daysText("2026-10-29", "2026-10-28")).toBe("明天");
    expect(daysText("2026-11-05", "2026-10-28")).toBe("8 天后");
    expect(daysText("2027-01-01", "2026-12-31")).toBe("明天");
    expect(daysText("2027-03-01", "2027-02-28")).toBe("明天"); // 2027 年 2 月只有 28 天
    expect(daysText("2026-10-25", "2026-10-28")).toBe("3 天前");
  });
  it("夏令时切换日不影响天数", () => {
    expect(daysText("2026-11-02", "2026-10-31")).toBe("2 天后");
    expect(daysText("2026-03-30", "2026-03-27")).toBe("3 天后");
  });
});

describe("nextText", () => {
  it("生效中显示下一期,暂停与结束各有说法", () => {
    expect(nextText(base, "2026-10-28")).toBe("下次 2026-11-05(8 天后)");
    expect(nextText(s({ status: "paused", paused: true, next_due: null }), "2026-10-28")).toBe("已暂停,不会自动记账");
    expect(nextText(s({ status: "ended", end_date: "2026-09-30", next_due: null }), "2026-10-28")).toBe("2026-09-30 已结束");
    expect(nextText(s({ next_due: null }), "2026-10-28")).toBe("—");
  });
});

describe("detailText", () => {
  it("分类、账户与入账情况", () => {
    expect(detailText(base)).toBe("居住 · 从「招行储蓄卡」扣款 · 已自动入账 3 笔,最近一笔 2026-10-05");
    expect(detailText(s({ category: null, account: null, posted_count: 0, last_posted: null, kind: "income" }))).toBe("未分类 · 不关联账户 · 还没有自动入账过");
    expect(detailText(s({ kind: "income" }))).toContain("收入");
  });
});

describe("previewText", () => {
  const p = (o: Partial<SubPreview>): SubPreview => ({ due_now: 0, first_due: null, last_due: null, next_due: "2026-11-05", ...o });
  it("还没到期:不会立刻记账", () => {
    expect(previewText(p({}))).toEqual({ text: "保存后不会立刻记账。首次入账:2026-11-05", tone: "info" });
  });
  it("已到期:提醒会立刻补记几笔", () => {
    expect(previewText(p({ due_now: 1, first_due: "2026-10-28", last_due: "2026-10-28" }))).toEqual({
      text: "这个日期已经到了,保存后会立刻补记 1 笔账目(2026-10-28)。之后下一期:2026-11-05", tone: "warn",
    });
    expect(previewText(p({ due_now: 3, first_due: "2026-08-05", last_due: "2026-10-05" })).text).toContain("3 笔账目(2026-08-05 起到 2026-10-05)");
  });
  it("补记太多:报错", () => {
    expect(previewText(p({ due_now: 61 })).tone).toBe("bad");
    expect(previewText(p({ due_now: 60, first_due: "a", last_due: "b" })).tone).toBe("warn");
  });
  it("没有后续期数", () => {
    expect(previewText(p({ due_now: 2, first_due: "a", last_due: "b", next_due: null })).text).toContain("没有后续期数");
  });
});
