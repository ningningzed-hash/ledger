import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Capability, useApi } from "./api";
import { Markdown } from "./markdown";

export function CapabilitiesView({ onUse }: { onUse: (prompt: string, capability: string) => void }) {
  const api = useApi();
  const [items, setItems] = useState<Capability[]>([]);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState("all");
  const [detail, setDetail] = useState<string | null>(null);
  const [working, setWorking] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const load = async () => { setLoading(true);setError("");try {setItems(await api.listCapabilities());} catch(e) {setError(String(e));} finally {setLoading(false);} };
  useEffect(() => { void load(); }, []);
  const act = async (id: string, action: () => Promise<unknown>) => {
    setWorking(id);setError("");try {await action();setItems(await api.listCapabilities());} catch(e) {setError(String(e));} finally {setWorking(null);}
  };
  const selected = items.find((i) => i.id === detail);
  const visible = items.filter((i) => (filter === "all" || i.kind === filter || (filter === "enabled" && i.enabled)) && `${i.name} ${i.description} ${i.id}`.toLowerCase().includes(query.trim().toLowerCase()));
  const controls = (item: Capability) => <div className="cap-actions">
    {item.kind === "mcp" && <button className="btn alt" disabled={working !== null} onClick={() => act(item.id, () => api.testCapability(item.id))}>{working === item.id ? "处理中…" : "测试连接"}</button>}
    <button className="btn alt" disabled={working !== null || (!item.enabled && item.kind === "mcp" && !item.probe?.ok)} onClick={() => act(item.id, () => api.setCapabilityEnabled(item.id, !item.enabled))}>{item.enabled ? "停用" : "启用"}</button>
    <button className="btn" disabled={!item.enabled || working !== null} onClick={() => {setDetail(null);onUse(item.prompt, item.id);}}>立即使用 ↗</button>
  </div>;
  return <section className="cap-market">
    <div className="cap-hero"><div><span className="cap-eyebrow">LEDGER · CAPABILITIES</span><h2>让助手有能力完成任务</h2><p>MCP 连接财经数据，Skill 组织分析步骤。按你的需要启用，直接在对话中使用。</p></div><div className="cap-count"><b>{items.filter((i) => i.enabled).length}<small> / {items.length}</small></b><span>当前用户已启用</span></div></div>
    <div className="cap-toolbar"><div className="cap-filters" role="group" aria-label="能力类型">{[["all","全部"],["mcp","MCP 数据源"],["skill","Skill 流程"],["enabled","已启用"]].map(([key,label]) => <button key={key} className={filter === key ? "active" : ""} aria-pressed={filter === key} onClick={() => setFilter(key)}>{label}</button>)}</div><input className="inp" aria-label="搜索能力" placeholder="搜索名称或用途…" value={query} onChange={(e) => setQuery(e.target.value)} /></div>
    {error && <div className="cap-error" role="alert">{error} <button className="btn alt" onClick={load}>重新加载</button></div>}
    {loading ? <p role="status">正在加载能力…</p> : visible.length === 0 ? <p className="muted">没有匹配的能力，试试其他关键词或筛选条件。</p> : <div className="cap-grid">{visible.map((item) => <article className="cap-card" key={item.id}>
      <div className="cap-card-top"><span className={`cap-icon ${item.kind}`}>{item.kind === "mcp" ? "⌘" : "✦"}</span><span className={`cap-state ${item.enabled ? "on" : ""}`}>{item.enabled ? "已启用" : "未启用"}</span></div>
      <h3><button onClick={() => setDetail(item.id)}>{item.name}</button></h3><span className="cap-meta">{item.kind.toUpperCase()} · v{item.version} · {item.license}</span><p>{item.description}</p>
      <div className="cap-status">{item.kind === "skill" ? "内置流程 · 随应用可用" : item.probe ? <><span className={item.probe.ok ? "" : "draft-err"}>{item.probe.ok ? `连接测试通过 · ${item.probe.tools.length} 个工具` : "连接测试失败"}</span><small>{new Date(item.probe.checked_at).toLocaleString()}</small></> : "尚未测试连接"}</div>
      {controls(item)}<button className="cap-detail" onClick={() => setDetail(item.id)}>查看流程与来源 →</button>
    </article>)}</div>}
    <p className="cap-footnote">启用设置仅对当前用户生效。财经查询需要联网；本地账本分析沿用你配置的模型服务。</p>
    {selected && <div className="modal-back" onClick={() => setDetail(null)}><section className="cap-dialog" role="dialog" aria-modal="true" aria-labelledby="cap-title" onClick={(e) => e.stopPropagation()} onKeyDown={(e) => {if(e.key === "Escape") setDetail(null);}}>
      <button autoFocus className="cap-close" aria-label="关闭能力详情" onClick={() => setDetail(null)}>×</button><span className="cap-meta">{selected.kind.toUpperCase()} · v{selected.version} · {selected.license}</span><h2 id="cap-title">{selected.name}</h2><p>{selected.description}</p>
      <p className="cap-source">来源：{selected.source.startsWith("https://") ? <button onClick={() => openUrl(selected.source).catch((e) => setError(String(e)))}>{selected.source}</button> : selected.source}</p>
      <div className="cap-instructions"><Markdown text={selected.instructions} /></div>
      {selected.kind === "mcp" && <div className="cap-probe"><h4>连接状态</h4><p>{selected.probe?.message ?? "尚未测试。点击测试连接会启动本地 MCP 并读取工具清单。"}</p>{selected.probe && <><small>测试时间：{new Date(selected.probe.checked_at).toLocaleString()}</small><ul>{selected.probe.tools.map((t) => <li key={t}>{t}</li>)}</ul></>}<p>首次安装：在项目目录执行 <code>sh scripts/setup-mcp.sh</code>，需要 Python 3.12+。</p></div>}
      {controls(selected)}
    </section></div>}
  </section>;
}
