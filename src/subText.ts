// 订阅页的文案。纯函数,便于测试各种情况下说的话是否正确。
import type { SubPreview, SubStatus, Subscription } from "./api";

const MS_PER_DAY = 86_400_000;
const utc = (s: string) => {
  const [y, m, d] = s.split("-").map(Number);
  return Date.UTC(y, m - 1, d);
};

/** 周期描述。月度按开始日期的"日"重复,当月没有这一天就取月底 */
export function scheduleText(s: Pick<Subscription, "frequency" | "start_date">): string {
  const [, m, d] = s.start_date.split("-").map(Number);
  if (s.frequency === "yearly") return `每年 ${m} 月 ${d} 日`;
  return d > 28 ? `每月 ${d} 日(没有这一天的月份取月底)` : `每月 ${d} 日`;
}

/** "明天 / 8 天后 / 今天":today 与 date 都是 YYYY-MM-DD */
export function daysText(date: string, today: string): string {
  const n = Math.round((utc(date) - utc(today)) / MS_PER_DAY);
  if (n === 0) return "今天";
  if (n === 1) return "明天";
  if (n < 0) return `${-n} 天前`;
  return `${n} 天后`;
}

export const STATUS_TEXT: Record<SubStatus, { text: string; tone: "good" | "muted" }> = {
  active: { text: "生效中", tone: "good" },
  paused: { text: "已暂停", tone: "muted" },
  ended: { text: "已结束", tone: "muted" },
};

/** 列表右侧:下一期是哪天 */
export function nextText(s: Subscription, today: string): string {
  if (s.status === "paused") return "已暂停,不会自动记账";
  if (s.status === "ended") return s.end_date ? `${s.end_date} 已结束` : "已结束";
  return s.next_due ? `下次 ${s.next_due}(${daysText(s.next_due, today)})` : "—";
}

/** 次要信息:分类、账户、已入账期数 */
export function detailText(s: Subscription): string {
  const parts = [s.category ?? "未分类", s.account ? `从「${s.account}」${s.kind === "income" ? "收入" : "扣款"}` : "不关联账户"];
  parts.push(s.posted_count > 0 ? `已自动入账 ${s.posted_count} 笔${s.last_posted ? `,最近一笔 ${s.last_posted}` : ""}` : "还没有自动入账过");
  return parts.join(" · ");
}

/** 保存前的预览:会立刻补记几笔、下一期是哪天 */
export function previewText(p: SubPreview, maxBackfill = 60): { text: string; tone: "info" | "warn" | "bad" } {
  const next = p.next_due ? `之后下一期:${p.next_due}` : "没有后续期数";
  if (p.due_now > maxBackfill) return { text: `开始日期太早,保存后会一次补记超过 ${maxBackfill} 笔账目,请把日期设得近一些`, tone: "bad" };
  if (p.due_now === 0) return { text: `保存后不会立刻记账。首次入账:${p.next_due ?? "—"}`, tone: "info" };
  const range = p.due_now === 1 ? p.first_due : `${p.first_due} 起到 ${p.last_due}`;
  return { text: `这个日期已经到了,保存后会立刻补记 ${p.due_now} 笔账目(${range})。${next}`, tone: "warn" };
}

/** 合计提示 */
export function monthlyHint(hasYearly: boolean): string {
  return hasYearly ? "年度订阅按 12 个月平均折算" : "只统计生效中的订阅";
}
