// 极简 Markdown 渲染,只覆盖助手常用的语法:标题、粗体、行内代码、
// 有序/无序列表(含一层嵌套)、表格、代码块。直接生成 React 元素,不使用 innerHTML,
// 所以模型输出里的 HTML 标签只会被当作普通文字显示。
import { ReactNode } from "react";

export type Inline = { t: "text" | "b" | "code"; v: string };
export type ListItem = { inl: Inline[]; sub: Inline[][] };
export type Block =
  | { t: "p"; lines: Inline[][] }
  | { t: "h"; level: number; inl: Inline[] }
  | { t: "list"; ordered: boolean; start: number; items: ListItem[] }
  | { t: "code"; v: string }
  | { t: "table"; head: Inline[][]; rows: Inline[][][] };

export function parseInline(s: string): Inline[] {
  const out: Inline[] = [];
  for (const part of s.split(/(\*\*[^*]+\*\*|`[^`]+`)/)) {
    if (!part) continue;
    if (part.startsWith("**") && part.endsWith("**") && part.length > 4) out.push({ t: "b", v: part.slice(2, -2) });
    else if (part.startsWith("`") && part.endsWith("`") && part.length > 2) out.push({ t: "code", v: part.slice(1, -1) });
    else out.push({ t: "text", v: part });
  }
  return out;
}

const LIST_RE = /^(\s*)([-*•]|\d+[.)、])\s+(.*)$/;
const TABLE_SEP_RE = /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/;

const splitRow = (line: string) =>
  line.trim().replace(/^\|/, "").replace(/\|$/, "").split("|").map((c) => parseInline(c.trim()));

export function parseBlocks(text: string): Block[] {
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  const blocks: Block[] = [];
  let para: Inline[][] = [];
  const flush = () => { if (para.length) { blocks.push({ t: "p", lines: para }); para = []; } };

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];

    if (line.trim().startsWith("```")) {
      flush();
      const code: string[] = [];
      for (i++; i < lines.length && !lines[i].trim().startsWith("```"); i++) code.push(lines[i]);
      blocks.push({ t: "code", v: code.join("\n") });
      continue;
    }
    if (!line.trim()) { flush(); continue; }

    const h = /^(#{1,6})\s+(.*)$/.exec(line);
    if (h) { flush(); blocks.push({ t: "h", level: h[1].length, inl: parseInline(h[2]) }); continue; }

    if (line.trim().startsWith("|") && i + 1 < lines.length && TABLE_SEP_RE.test(lines[i + 1])) {
      flush();
      const head = splitRow(line);
      const rows: Inline[][][] = [];
      for (i += 2; i < lines.length && lines[i].trim().startsWith("|"); i++) rows.push(splitRow(lines[i]));
      i--;
      blocks.push({ t: "table", head, rows });
      continue;
    }

    const li = LIST_RE.exec(line);
    if (li) {
      flush();
      const indent = li[1].replace(/\t/g, "    ").length;
      const ordered = /\d/.test(li[2]);
      const last = blocks[blocks.length - 1];
      const inl = parseInline(li[3]);
      if (indent >= 2 && last?.t === "list" && last.items.length) {
        last.items[last.items.length - 1].sub.push(inl); // 缩进的条目归入上一项
      } else if (last?.t === "list" && last.ordered === ordered && !para.length) {
        last.items.push({ inl, sub: [] });
      } else {
        blocks.push({ t: "list", ordered, start: ordered ? parseInt(li[2], 10) : 1, items: [{ inl, sub: [] }] });
      }
      continue;
    }

    para.push(parseInline(line));
  }
  flush();
  return blocks;
}

const renderInline = (inl: Inline[]): ReactNode[] =>
  inl.map((x, i) => (x.t === "b" ? <strong key={i}>{x.v}</strong> : x.t === "code" ? <code key={i}>{x.v}</code> : x.v));

export function Markdown({ text }: { text: string }) {
  return (
    <div className="md">
      {parseBlocks(text).map((b, i) => {
        switch (b.t) {
          case "p":
            return <p key={i}>{b.lines.map((l, j) => <span key={j}>{j > 0 && <br />}{renderInline(l)}</span>)}</p>;
          case "h":
            return <h4 key={i}>{renderInline(b.inl)}</h4>;
          case "code":
            return <pre key={i}><code>{b.v}</code></pre>;
          case "list": {
            const items = b.items.map((it, j) => (
              <li key={j}>
                {renderInline(it.inl)}
                {it.sub.length > 0 && <ul>{it.sub.map((s, k) => <li key={k}>{renderInline(s)}</li>)}</ul>}
              </li>
            ));
            return b.ordered ? <ol key={i} start={b.start}>{items}</ol> : <ul key={i}>{items}</ul>;
          }
          case "table":
            return (
              <table key={i}>
                <thead><tr>{b.head.map((c, j) => <th key={j}>{renderInline(c)}</th>)}</tr></thead>
                <tbody>{b.rows.map((r, j) => <tr key={j}>{r.map((c, k) => <td key={k}>{renderInline(c)}</td>)}</tr>)}</tbody>
              </table>
            );
        }
      })}
    </div>
  );
}
