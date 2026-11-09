import { describe, expect, it } from "vitest";
import { clampChatWidth } from "./chatSize";

describe("聊天窗口尺寸边界", () => {
  it("拖动时保持最小可用宽度且不超出窗口", () => {
    expect(clampChatWidth(200, 1440)).toBe(360);
    expect(clampChatWidth(900, 1440)).toBe(900);
    expect(clampChatWidth(2000, 1440)).toBe(1440);
  });
  it("窄屏和损坏的保存值仍能显示窗口", () => {
    expect(clampChatWidth(900, 320)).toBe(320);
    expect(clampChatWidth(200, 320)).toBe(320);
    expect(clampChatWidth(Number.NaN, 1440)).toBe(560);
    expect(clampChatWidth(Infinity, 400)).toBe(400);
  });
});
