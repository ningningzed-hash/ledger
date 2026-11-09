import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { createApi } from "./api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class { onmessage = (_event: unknown) => {}; } }));
const call = vi.mocked(invoke);
beforeEach(() => call.mockReset());

describe("会话绑定的账本 API", () => {
  it("切换后，旧页面的延迟写入仍携带旧令牌，不能操作新账本", async () => {
    const a = createApi("session-a", vi.fn());
    const b = createApi("session-b", vi.fn());
    call.mockResolvedValue(undefined);
    await b.listAccounts();
    await a.deleteTxn(42);
    expect(call.mock.calls).toEqual([
      ["list_accounts", { sessionToken: "session-b" }],
      ["delete_txn", { id: 42, sessionToken: "session-a" }],
    ]);
  });
  it("登录失效通知调用者，普通业务错误不会注销", async () => {
    const expired = vi.fn();
    const api = createApi("expired", expired);
    call.mockRejectedValueOnce("金额需大于 0");
    await expect(api.listAccounts()).rejects.toBe("金额需大于 0");
    expect(expired).not.toHaveBeenCalled();
    call.mockRejectedValueOnce("登录已失效，请重新登录");
    await expect(api.listChat()).rejects.toBe("登录已失效，请重新登录");
    expect(expired).toHaveBeenCalledOnce();
  });
});


describe("能力市场会话与执行通道", () => {
  it("能力开关和连接测试使用发起者令牌", async () => {
    call.mockResolvedValue(undefined);
    const a = createApi("a", vi.fn());
    const b = createApi("b", vi.fn());
    await a.setCapabilityEnabled("aktools", false);
    await b.testCapability("yfinance");
    expect(call.mock.calls).toEqual([
      ["set_capability_enabled", { id: "aktools", enabled: false, sessionToken: "a" }],
      ["test_capability", { id: "yfinance", sessionToken: "b" }],
    ]);
  });
  it("Skill 选择与进度通道只绑定本次请求", async () => {
    call.mockResolvedValue({ text: "完成", drafts: [], actions: [], traces: [] });
    const receive = vi.fn();
    await createApi("a", vi.fn()).sendChat("月度复盘", null, "monthly-review", receive);
    const args = call.mock.calls[0][1] as { capability: string; sessionToken: string; onEvent: { onmessage: (event: unknown) => void } };
    expect(args.capability).toBe("monthly-review");
    expect(args.sessionToken).toBe("a");
    const event = { name: "月度复盘", status: "success", detail: "已加载" };
    args.onEvent.onmessage(event);
    expect(receive).toHaveBeenCalledWith(event);
  });
});
