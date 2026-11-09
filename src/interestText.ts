// 利息计算结果的展示用文字与格式化(纯函数,便于单元测试)。后端 calculate_interest 的金额已经是"元"。

export const RESULT_LABELS: Record<string, string> = {
  interest: "利息", total: "本息合计", maturity_amount: "到期本息", months: "存期(月)", days: "天数", deposited: "累计存入",
  payout_each: "每期付息", payout_count: "付息期数", payout_last: "最后一期付息",
  estimated_gain: "估算收益", estimated_total: "估算本息",
  periods: "期数", monthly_payment: "月供", total_interest: "总利息", total_paid: "还款总额",
  monthly_principal: "每月还本", first_payment: "首期月供", last_payment: "末期月供", monthly_decrease: "每月递减",
  payment_each: "每期还款", total_cost: "总成本(利息与手续费)", monthly_rate_pct: "月利率", nominal_annual_pct: "名义年化", effective_annual_pct: "实际年化",
  balance_before: "提前还款前剩余本金", balance_after: "提前还款后剩余本金", original_total_interest: "原总利息",
  new_total_interest: "新总利息", saved_interest: "节省利息", new_monthly_payment: "新月供",
  original_remaining_months: "原剩余期数", new_remaining_months: "新剩余期数",
  first_period: "首期构成", last_period: "末期构成", mode: "方式",
};

const COUNT_KEYS = new Set(["months", "days", "periods", "payout_count", "original_remaining_months", "new_remaining_months"]);

export const isPercentKey = (k: string) => k.endsWith("_pct");

const yuan = (v: number) => `¥${v.toLocaleString("zh-CN", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;

/** 把一个结果字段格式化成展示文字:金额 ¥1,234.50、百分数 13.8%、个数 360 */
export function formatResultValue(key: string, value: number | string): string {
  if (typeof value === "string") return value === "shorten" ? "缩短期限(月供不变)" : value === "reduce" ? "减少月供(期限不变)" : value;
  if (isPercentKey(key)) return `${value}%`;
  if (COUNT_KEYS.has(key)) return String(value);
  return yuan(value);
}

/** 首期/末期构成:{payment, principal, interest, balance} → "月供 ¥… · 本金 ¥… · 利息 ¥…" */
export function formatPeriod(p: Record<string, number>): string {
  return `月供 ${yuan(p.payment)} · 本金 ${yuan(p.principal)} · 利息 ${yuan(p.interest)}`;
}

export type FieldKind = "money" | "percent" | "int" | "number" | "select";
export type Field = { key: string; label: string; kind: FieldKind; optional?: boolean; hint?: string; options?: { value: string; label: string }[]; def?: string };
export type CalcDef = { type: string; label: string; fields: Field[] };

const PRINCIPAL: Field = { key: "principal", label: "本金(元)", kind: "money" };
const RATE: Field = { key: "rate", label: "年利率(%)", kind: "percent", hint: "如 3.5" };
const LOAN_TERM: Field = { key: "years", label: "贷款年限(年)", kind: "number", hint: "如 30" };

export const CALCS: CalcDef[] = [
  { type: "demand_deposit", label: "活期存款", fields: [PRINCIPAL, RATE, { key: "days", label: "存放天数", kind: "int" },
    { key: "basis", label: "天数基准", kind: "select", def: "360", options: [{ value: "360", label: "360 天(国内银行常用)" }, { value: "365", label: "365 天" }] }] },
  { type: "fixed_deposit", label: "整存整取定期", fields: [PRINCIPAL, RATE, { key: "months", label: "存期(月)", kind: "int" }] },
  { type: "cd", label: "大额存单", fields: [PRINCIPAL, RATE, { key: "months", label: "存期(月)", kind: "int" },
    { key: "payout", label: "付息方式", kind: "select", def: "maturity", options: [{ value: "maturity", label: "到期一次付息" }, { value: "monthly", label: "按月付息" }, { value: "yearly", label: "按年付息" }] }] },
  { type: "installment_savings", label: "零存整取", fields: [{ key: "monthly_amount", label: "每月存入(元)", kind: "money" }, RATE, { key: "months", label: "存期(月)", kind: "int" }] },
  { type: "money_fund_estimate", label: "货币基金/理财估算", fields: [PRINCIPAL, { ...RATE, label: "年化收益率(%)", hint: "你自己输入,不代表未来收益" }, { key: "days", label: "持有天数", kind: "int" }] },
  { type: "loan_equal_payment", label: "贷款:等额本息", fields: [{ ...PRINCIPAL, label: "贷款额(元)" }, RATE, LOAN_TERM] },
  { type: "loan_equal_principal", label: "贷款:等额本金", fields: [{ ...PRINCIPAL, label: "贷款额(元)" }, RATE, LOAN_TERM] },
  { type: "effective_rate", label: "分期的实际年化", fields: [{ ...PRINCIPAL, label: "分期本金(元)" }, { key: "installments", label: "分期期数", kind: "int" },
    { key: "fee_rate", label: "每期手续费率(%)", kind: "percent", optional: true, hint: "与下一项二选一" },
    { key: "payment", label: "每期还款额(元)", kind: "money", optional: true, hint: "与上一项二选一" }] },
  { type: "prepayment", label: "提前还款省多少利息", fields: [{ ...PRINCIPAL, label: "贷款额(元)" }, RATE, LOAN_TERM,
    { key: "prepay_after", label: "已还几期后提前还", kind: "int", hint: "0 表示还款开始前" }, { key: "prepay_amount", label: "提前还的本金(元)", kind: "money" },
    { key: "prepay_mode", label: "提前还款后", kind: "select", def: "shorten", options: [{ value: "shorten", label: "月供不变,缩短期限" }, { value: "reduce", label: "期限不变,减少月供" }] }] },
];

/** 把表单输入整理成工具参数;缺少必填项或不是数字时返回错误文字 */
export function buildArgs(def: CalcDef, values: Record<string, string>): { args: Record<string, unknown> } | { error: string } {
  const args: Record<string, unknown> = { type: def.type };
  for (const f of def.fields) {
    const raw = (values[f.key] ?? f.def ?? "").trim();
    if (raw === "") {
      if (f.optional) continue;
      return { error: `请填写“${f.label.replace(/\(.*?\)/, "")}”` };
    }
    if (f.kind === "select") { args[f.key] = raw; continue; }
    const n = Number(raw);
    if (!Number.isFinite(n)) return { error: `“${f.label.replace(/\(.*?\)/, "")}”需要是数字` };
    if (n < 0) return { error: `“${f.label.replace(/\(.*?\)/, "")}”不能是负数` };
    args[f.key] = n;
  }
  if (def.type === "effective_rate" && args.fee_rate === undefined && args.payment === undefined) {
    return { error: "每期手续费率和每期还款额至少填一项" };
  }
  return { args };
}
