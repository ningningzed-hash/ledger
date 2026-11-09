import { describe, expect, it } from "vitest";
import { parseBlocks, parseInline } from "./markdown";

describe("parseInline", () => {
  it("识别粗体和行内代码,其余保持原样", () => {
    expect(parseInline("本月 **剩余 ¥5,320** 元,见 `预算`")).toEqual([
      { t: "text", v: "本月 " },
      { t: "b", v: "剩余 ¥5,320" },
      { t: "text", v: " 元,见 " },
      { t: "code", v: "预算" },
    ]);
  });

  it("孤立的星号不会被当成粗体", () => {
    expect(parseInline("5 * 3 = 15,**未闭合")).toEqual([{ t: "text", v: "5 * 3 = 15,**未闭合" }]);
  });

  it("HTML 标签只当文字,不会变成元素", () => {
    expect(parseInline("<img src=x onerror=alert(1)>")).toEqual([{ t: "text", v: "<img src=x onerror=alert(1)>" }]);
  });
});

describe("parseBlocks", () => {
  it("段落内的换行保留,空行分段", () => {
    const b = parseBlocks("第一行\n第二行\n\n第二段");
    expect(b.map((x) => x.t)).toEqual(["p", "p"]);
    expect(b[0].t === "p" && b[0].lines.length).toBe(2);
  });

  it("有序列表带嵌套,且编号不会被嵌套打断", () => {
    const b = parseBlocks("1. **资产分布**:\n   - 住宅占比过高\n   - 基金占比偏低\n2. **预算**:\n   - 剩余 ¥5,320");
    expect(b).toHaveLength(1);
    const list = b[0];
    expect(list.t === "list" && list.ordered).toBe(true);
    if (list.t !== "list") throw new Error();
    expect(list.items).toHaveLength(2);
    expect(list.items.map((i) => i.sub.length)).toEqual([2, 1]);
  });

  it("有序列表保留起始编号", () => {
    const b = parseBlocks("3. 第三\n4. 第四");
    expect(b[0].t === "list" && b[0].start).toBe(3);
  });

  it("无序列表支持 - * •", () => {
    const b = parseBlocks("- a\n* b\n• c");
    expect(b).toHaveLength(1);
    expect(b[0].t === "list" && b[0].items.length).toBe(3);
  });

  it("标题与代码块", () => {
    const b = parseBlocks("## 小结\n```\nlet a = 1;\n- 不是列表\n```");
    expect(b[0]).toMatchObject({ t: "h", level: 2 });
    expect(b[1]).toEqual({ t: "code", v: "let a = 1;\n- 不是列表" });
  });

  it("表格", () => {
    const b = parseBlocks("| 类别 | 占比 |\n| --- | ---: |\n| 房产 | 94% |\n| 存款 | 4% |\n\n结束");
    expect(b[0].t).toBe("table");
    if (b[0].t !== "table") throw new Error();
    expect(b[0].head).toHaveLength(2);
    expect(b[0].rows).toHaveLength(2);
    expect(b[1].t).toBe("p");
  });

  it("未闭合的代码块不会吞掉程序", () => {
    expect(() => parseBlocks("```\n没有结尾")).not.toThrow();
  });

  it("纯文本与空串", () => {
    expect(parseBlocks("")).toEqual([]);
    expect(parseBlocks("已记录支出 ¥35.00(餐饮)。")).toHaveLength(1);
  });
});
