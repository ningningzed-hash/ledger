## ADDED Requirements

### Requirement: 助手回复按 Markdown 显示
系统 SHALL 将助手回复中的粗体、行内代码、标题、有序与无序列表(含一层嵌套)、表格和代码块渲染为对应的排版,而不是显示原始标记符号。

#### Scenario: 粗体与列表
- **WHEN** 助手回复包含 `**资产分布**` 以及其下缩进的 `-` 条目
- **THEN** 界面显示加粗的"资产分布"和缩进的子列表,不出现星号或连字符标记

#### Scenario: 有序列表被嵌套打断
- **WHEN** 助手回复为 `1.` 项、其缩进子项、`2.` 项依次出现
- **THEN** 两个有序项编号连续显示为 1 和 2

#### Scenario: 普通文本
- **WHEN** 助手回复不含任何 Markdown 语法
- **THEN** 原样显示,换行保留

### Requirement: 回复内容不得执行
系统 MUST NOT 把助手回复当作 HTML 解析;回复中的 HTML 标签 SHALL 作为普通文字显示。

#### Scenario: 回复含 HTML
- **WHEN** 助手回复包含 `<img src=x onerror=alert(1)>`
- **THEN** 该文本按字面显示,不创建元素、不执行脚本

### Requirement: 用户消息与错误按纯文本显示
系统 SHALL 以纯文本显示用户消息与错误提示。

#### Scenario: 用户输入星号
- **WHEN** 用户发送 `5 * 3 = 15`
- **THEN** 消息原样显示
