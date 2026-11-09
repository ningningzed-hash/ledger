import { describe, expect, it } from "vitest";
import { isLocalUrl, modelLabel } from "./modelLabel";

describe("modelLabel", () => {
  it("识别本机地址", () => {
    for (const u of ["http://localhost:11434", "http://127.0.0.1:11434", "http://[::1]:11434", "http://0.0.0.0:8080"]) {
      expect(isLocalUrl(u)).toBe(true);
    }
  });
  it("其余(含无法解析)视为远程", () => {
    expect(isLocalUrl("https://api.deepseek.com")).toBe(false);
    expect(isLocalUrl("http://192.168.1.5:11434")).toBe(false);
    expect(isLocalUrl("not a url")).toBe(false);
  });
  it("拼出副标题", () => {
    expect(modelLabel("http://localhost:11434", "qwen3:14b")).toBe("qwen3:14b · 本地");
    expect(modelLabel("https://api.deepseek.com", "deepseek-chat")).toBe("deepseek-chat · 远程");
  });
});
