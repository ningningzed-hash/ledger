## Why

Qwen 回复里常带 `**粗体**`、列表和表格。助手气泡此前按纯文本显示,用户会直接看到星号和编号符号,难以阅读。

## What Changes

- 助手回复按 Markdown 渲染:标题、粗体、行内代码、有序/无序列表(含一层嵌套)、表格、代码块。
- 用户自己的消息与错误提示仍按纯文本显示。
- 渲染不使用 `innerHTML`;回复中的 HTML 标签只作为文字显示。

## Capabilities

### New Capabilities
- `chat-rendering`: 助手消息的显示规则。

### Modified Capabilities

## Impact

- 前端新增 `markdown.tsx`(无新增运行时依赖)与 `vitest` 测试;聊天气泡样式。
