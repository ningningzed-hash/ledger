import { describe, expect, it } from "vitest";
import type { Goal } from "./api";
import { describeGoal, sortGoals } from "./goalText";

const base: Goal = {
  id: 1, name: "攒钱", kind: "save", target: 20_000_000, deadline: null, created_on: "2026-10-28", accounts: [],
  current: 10_000_000, remaining: 10_000_000, progress: 0.5, achieved: false,
  months_left: null, required_per_month: null, pace_per_month: null, projected_month: null, status: "open",
};
const g = (o: Partial<Goal>): Goal => ({ ...base, ...o });

describe("describeGoal", () => {
  it("存钱:没有截止日、速度未知", () => {
    const t = describeGoal(base);
    expect(t.remaining).toBe("还差 ¥100,000.00");
    expect(t.deadline).toBeNull();
    expect(t.required).toBeNull();
    expect(t.pace).toBe("账户记录不足,暂时无法估算速度");
    expect(t.projected).toBeNull();
  });

  it("存钱:有截止日、速度够快,给出每月所需与预计月份", () => {
    const t = describeGoal(g({ deadline: "2027-06-30", months_left: 8, required_per_month: 1_375_00, pace_per_month: 2_306_00, projected_month: "2027-03", status: "on_track" }));
    expect(t.deadline).toBe("2027-06-30 截止,还有 8 个月");
    expect(t.required).toBe("每月需存 ¥1,375.00");
    expect(t.pace).toBe("近几个月平均每月增加 ¥2,306.00");
    expect(t.projected).toBe("按这个速度,预计 2027年3月 达成");
  });

  it("截止日就在本月", () => {
    expect(describeGoal(g({ deadline: "2026-10-31", months_left: 0, required_per_month: 5_000_00 })).deadline).toBe("2026-10-31 截止(本月)");
  });

  it("速度为零或为负:不承诺达成时间", () => {
    expect(describeGoal(g({ pace_per_month: 0 })).pace).toBe("近几个月没有变化");
    expect(describeGoal(g({ pace_per_month: 0 })).projected).toBe("按目前的趋势暂时无法达成");
    const down = describeGoal(g({ pace_per_month: -50_000 }));
    expect(down.pace).toBe("近几个月平均每月减少 ¥500.00");
    expect(down.projected).toBe("按目前的趋势暂时无法达成");
  });

  it("已逾期", () => {
    const t = describeGoal(g({ deadline: "2026-12-31", status: "overdue", pace_per_month: 1000, projected_month: "2027-08" }));
    expect(t.deadline).toBe("2026-12-31 已过期");
    expect(t.projected).toBe("按这个速度,预计 2027年8月 达成"); // 逾期后仍告诉用户按现在的速度还要多久
  });

  it("已达成:不再说速度、所需金额和预计", () => {
    const t = describeGoal(g({ achieved: true, status: "achieved", remaining: 0, deadline: "2027-06-30", required_per_month: 1000, pace_per_month: 5000 }));
    expect(t).toEqual({ remaining: "已达成", deadline: "2027-06-30 截止", required: null, pace: null, projected: null });
  });

  it("还债的措辞", () => {
    const t = describeGoal(g({ kind: "payoff", target: 0, remaining: 21_000_00, required_per_month: 2_625_00, pace_per_month: 2_306_00, deadline: "2027-06-30", months_left: 8, projected_month: "2027-08" }));
    expect(t.remaining).toBe("还剩 ¥21,000.00 待还");
    expect(t.required).toBe("每月需还 ¥2,625.00");
    expect(t.pace).toBe("近几个月平均每月还掉 ¥2,306.00");
    expect(describeGoal(g({ kind: "payoff", pace_per_month: -100_00 })).pace).toBe("近几个月欠款平均每月增加 ¥100.00");
  });
});

describe("sortGoals", () => {
  it("已达成的排到最后,其余顺序不变,不改动原数组", () => {
    const list = [g({ id: 1, achieved: true }), g({ id: 2 }), g({ id: 3, achieved: true }), g({ id: 4 })];
    expect(sortGoals(list).map((x) => x.id)).toEqual([2, 4, 1, 3]);
    expect(list.map((x) => x.id)).toEqual([1, 2, 3, 4]);
  });
});
