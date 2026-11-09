import { ReactNode, useLayoutEffect, useRef, useState } from "react";

// 图表共用的小部件:宽度监听、提示框、图例、标题。报表页与组合建议都用它。
// ---------- 通用:宽度、提示框、图例 ----------

export function useWidth<T extends HTMLElement>() {
  const ref = useRef<T>(null);
  const [w, setW] = useState(0);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    setW(Math.floor(el.clientWidth));
    const ro = new ResizeObserver(([e]) => setW(Math.floor(e.contentRect.width)));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, w] as const;
}

export type TipRow = { color?: string; label: string; value: string };

export const TIP_H = 104; // 三行内容的提示框高度,用于避让与夹紧
export const tipFlips = (x: number, width: number) => x > width * 0.6; // 靠右时提示框改到指针左侧

/** 提示框:数值在前、醒目;名称在后、次要;用色线标识系列(文字不使用系列色) */
export function Tip({ x, y, width, title, rows }: { x: number; y: number; width: number; title: string; rows: TipRow[] }) {
  const flip = tipFlips(x, width);
  return (
    <div className="tip" style={{ left: x, top: y, transform: `translate(${flip ? "calc(-100% - 12px)" : "12px"}, 0)` }} role="status">
      <div className="tip-title">{title}</div>
      {rows.map((r) => (
        <div className="tip-row" key={r.label}>
          {r.color && <i style={{ background: r.color }} />}
          <b className="num">{r.value}</b>
          <span>{r.label}</span>
        </div>
      ))}
    </div>
  );
}

export function Legend({ items }: { items: { cls: string; label: string }[] }) {
  return (
    <div className="legend-row">
      {items.map((i) => <span key={i.label}><i className={`sw ${i.cls}`} />{i.label}</span>)}
    </div>
  );
}

export function ChartHead({ title, sub, children }: { title: string; sub?: string; children?: ReactNode }) {
  return (
    <div className="chart-head">
      <div><h3>{title}</h3>{sub && <p className="muted">{sub}</p>}</div>
      {children}
    </div>
  );
}
