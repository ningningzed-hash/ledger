# Proposal

## Why

Ledger 已有本地账本工具，但缺少外部财经数据接入、可复用分析流程，以及让用户了解与管理助手能力的入口。

## What Changes

- 新增能力市场，展示两个 MCP 与四个内置 Skill，支持搜索筛选、详情、按用户启停、连接测试和立即使用。
- 使用官方 Rust MCP SDK 接入固定版本的 AKTools 和 yfinance MCP；使用独立子进程与有界超时，错误透明展示。
- 内置月度复盘、资产健康、ETF 对比、账单核对四个流程；加载实际 SKILL.md，按意图或用户明确选择执行。
- 聊天展示执行过程、工具状态与来源；外部结果不作为指令，禁用能力不能被调用。
- 预算、资产估值写入与记账一样需确认卡片；能力配置、状态和会话隔离到当前用户。

## Capabilities

### New Capabilities
- `capability-market`: 能力目录、启停、连接测试和任务入口。
- `agent-capabilities`: MCP 工具、Skill 加载、执行过程和写入确认。

### Modified Capabilities

无。

## Impact

Rust agent 与 IPC、MCP 子进程运行时、用户 setting 表、React 能力市场和聊天；新增 rmcp/Tokio 依赖及 Python MCP 安装脚本。保留现有账本与本地模型协议。
