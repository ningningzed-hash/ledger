import { useState } from "react";
import { InterestPanel } from "./interest";
import { PortfolioSection } from "./portfolio";
import { MarketView } from "./market";

type Tab = "market" | "advice" | "interest";

/** 侧栏"理财":行情、组合建议与利息计算。各标签页分开放,每页都不会太长。 */
export function InvestView() {
  const [tab, setTab] = useState<Tab>("market");
  const [mode, setMode] = useState<"chart" | "table">("chart");
  // 利息页改了账户年利率后,组合建议要重新取数
  const [bump, setBump] = useState(0);
  const changed = () => setBump((b) => b + 1);

  return (
    <>
      <div className="head"><h1>理财</h1></div>
      <div className="filters">
        <div className="seg" role="tablist" aria-label="理财">
          <button role="tab" aria-selected={tab === "market"} className={tab === "market" ? "on" : ""} onClick={() => setTab("market")}>行情</button>
          <button role="tab" aria-selected={tab === "advice"} className={tab === "advice" ? "on" : ""} onClick={() => setTab("advice")}>组合建议</button>
          <button role="tab" aria-selected={tab === "interest"} className={tab === "interest" ? "on" : ""} onClick={() => setTab("interest")}>利息计算</button>
        </div>
        {tab !== "interest" && (
          <div className="seg" role="group" aria-label="显示方式">
            <button className={mode === "chart" ? "on" : ""} onClick={() => setMode("chart")}>图表</button>
            <button className={mode === "table" ? "on" : ""} onClick={() => setMode("table")}>表格</button>
          </div>
        )}
      </div>
      {tab === "market" && <MarketView mode={mode} />}
      {tab === "advice" && <PortfolioSection mode={mode} refreshKey={bump} onChanged={changed} />}
      {tab === "interest" && <InterestPanel refreshKey={bump} onChanged={changed} />}
    </>
  );
}
