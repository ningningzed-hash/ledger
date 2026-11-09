// 目标卡片上的文案。纯函数,便于测试各种状态下说的话是否正确。
import type { Goal } from "./api";
import { fmt } from "./api";
import { monthName } from "./charts";

export type GoalText = {
  /** 还差多少 / 已达成 */
  remaining: string;
  /** 截止日期与剩余时间 */
  deadline: string | null;
  /** 为了按期达成每月需要多少 */
  required: string | null;
  /** 近几个月的实际速度 */
  pace: string | null;
  /** 按这个速度预计何时达成 */
  projected: string | null;
};

export const STATUS_LABEL: Record<Goal["status"], { text: string; icon: string; tone: "good" | "warn" | "bad" | "muted" }> = {
  achieved: { text: "已达成", icon: "✓", tone: "good" },
  on_track: { text: "进度正常", icon: "✓", tone: "good" },
  behind: { text: "稍落后", icon: "!", tone: "warn" },
  overdue: { text: "已逾期", icon: "!", tone: "bad" },
  unknown: { text: "暂无法判断", icon: "·", tone: "muted" },
  open: { text: "没有截止日", icon: "·", tone: "muted" },
};

export function describeGoal(g: Goal): GoalText {
  const save = g.kind === "save";
  const remaining = g.achieved ? "已达成" : save ? `还差 ¥${fmt(g.remaining)}` : `还剩 ¥${fmt(g.remaining)} 待还`;

  let deadline: string | null = null;
  if (g.deadline) {
    if (g.status === "overdue") deadline = `${g.deadline} 已过期`;
    else if (g.achieved) deadline = `${g.deadline} 截止`;
    else if (g.months_left === 0) deadline = `${g.deadline} 截止(本月)`;
    else deadline = `${g.deadline} 截止,还有 ${g.months_left} 个月`;
  }
  const required = g.required_per_month === null || g.achieved ? null : `每月需${save ? "存" : "还"} ¥${fmt(g.required_per_month)}`;

  let pace: string | null = null;
  const p = g.pace_per_month;
  if (!g.achieved) {
    if (p === null) pace = "账户记录不足,暂时无法估算速度";
    else if (p === 0) pace = "近几个月没有变化";
    else if (p > 0) pace = `近几个月平均每月${save ? "增加" : "还掉"} ¥${fmt(p)}`;
    else pace = save ? `近几个月平均每月减少 ¥${fmt(-p)}` : `近几个月欠款平均每月增加 ¥${fmt(-p)}`;
  }

  let projected: string | null = null;
  if (!g.achieved) {
    if (g.projected_month) projected = `按这个速度,预计 ${monthName(g.projected_month)} 达成`;
    else if (p !== null && p <= 0) projected = "按目前的趋势暂时无法达成";
  }
  return { remaining, deadline, required, pace, projected };
}

/** 已达成的排在后面,其余保持原顺序 */
export function sortGoals(goals: Goal[]): Goal[] {
  return [...goals].sort((a, b) => Number(a.achieved) - Number(b.achieved));
}
