import { useEffect, useState } from "react";
import { Insight, useApi } from "./api";
import type { Data } from "./App";

const ICON: Record<Insight["level"], string> = { bad: "!", warn: "!", info: "i" };
const TAG: Record<Insight["level"], string> = { bad: "超支", warn: "注意", info: "提醒" };

/** 总览页的洞察卡:规则算出的提醒,点「问助手」直接追问。没有提醒时不显示 */
export function InsightsCard({ d, onAsk }: { d: Data; onAsk: (q: string) => void }) {
  const api = useApi();
  const [items, setItems] = useState<Insight[]>([]);
  // 账目、预算有变化(d 会整体替换)就重新算
  useEffect(() => { api.getInsights().then(setItems).catch(() => setItems([])); }, [d]);
  if (!items.length) return null;
  return (
    <div className="card span12 insights">
      <h3>需要留意</h3>
      {items.map((i, k) => (
        <div key={`${i.code}-${k}`} className={`ins ins-${i.level}`}>
          <span className={`chip ${i.level === "info" ? "muted" : i.level}`}><i>{ICON[i.level]}</i>{TAG[i.level]}</span>
          <div className="ins-t"><b>{i.title}</b><span>{i.detail}</span></div>
          <button className="btn alt" onClick={() => onAsk(i.question)}>问助手</button>
        </div>
      ))}
    </div>
  );
}
