import { describe, expect, it } from "vitest";
import { CALCS, buildArgs, formatPeriod, formatResultValue } from "./interestText";

const def = (t: string) => CALCS.find((c) => c.type === t)!;

describe("formatResultValue", () => {
  it("金额、百分数、个数、方式", () => {
    expect(formatResultValue("interest", 3000)).toBe("¥3,000.00");
    expect(formatResultValue("monthly_payment", 4490.45)).toBe("¥4,490.45");
    expect(formatResultValue("effective_annual_pct", 13.8)).toBe("13.8%");
    expect(formatResultValue("periods", 360)).toBe("360");
    expect(formatResultValue("new_remaining_months", 0)).toBe("0");
    expect(formatResultValue("mode", "shorten")).toContain("缩短期限");
    expect(formatResultValue("mode", "reduce")).toContain("减少月供");
  });
  it("首期/末期构成", () => {
    expect(formatPeriod({ payment: 4490.45, principal: 1573.78, interest: 2916.67, balance: 0 })).toBe("月供 ¥4,490.45 · 本金 ¥1,573.78 · 利息 ¥2,916.67");
  });
});

describe("buildArgs", () => {
  it("把输入整理成数字参数,选择项取默认值", () => {
    const r = buildArgs(def("demand_deposit"), { principal: "100000", rate: "0.2", days: "90" });
    expect(r).toEqual({ args: { type: "demand_deposit", principal: 100000, rate: 0.2, days: 90, basis: "360" } });
  });
  it("缺必填项、非数字、负数都给出指向具体字段的错误", () => {
    expect(buildArgs(def("fixed_deposit"), { principal: "100", rate: "", months: "12" })).toEqual({ error: "请填写“年利率”" });
    expect(buildArgs(def("fixed_deposit"), { principal: "abc", rate: "3", months: "12" })).toEqual({ error: "“本金”需要是数字" });
    expect(buildArgs(def("fixed_deposit"), { principal: "-5", rate: "3", months: "12" })).toEqual({ error: "“本金”不能是负数" });
  });
  it("实际年化:手续费率和还款额至少填一项", () => {
    const base = { principal: "10000", installments: "12" };
    expect(buildArgs(def("effective_rate"), base)).toEqual({ error: "每期手续费率和每期还款额至少填一项" });
    expect(buildArgs(def("effective_rate"), { ...base, fee_rate: "0.6" })).toEqual({ args: { type: "effective_rate", principal: 10000, installments: 12, fee_rate: 0.6 } });
    expect(buildArgs(def("effective_rate"), { ...base, payment: "893.33" })).toMatchObject({ args: { payment: 893.33 } });
  });
  it("每种计算器都有类型和至少一个字段,类型互不重复", () => {
    expect(new Set(CALCS.map((c) => c.type)).size).toBe(CALCS.length);
    expect(CALCS.every((c) => c.fields.length > 0 && c.label)).toBe(true);
    expect(CALCS.map((c) => c.type)).toContain("prepayment");
  });
});
