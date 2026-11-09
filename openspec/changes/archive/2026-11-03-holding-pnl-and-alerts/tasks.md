# Tasks

## 1. 数据结构与迁移

- [x] 1.1 `db.rs`：`ADDED_COLUMNS` 增加 `holding.cost_price`、`holding.price`，`STATEMENTS` 增加 `holding_snapshot`、`alert_sent` 两张表；验证：对已有数据库启动后列与表齐全，重复迁移不报错，`cargo test` 通过
- [x] 1.2 `backup.rs`：holding 列表加入 `cost_price`、`price`，`holding_snapshot` 加入 `TABLES` 和 `OPTIONAL`；验证：新增用例"恢复不含新列和快照表的旧备份成功，字段为空、无快照"，导出 → 恢复往返用例通过

## 2. 持仓成本与盈亏(holding-pnl)

- [x] 2.1 `portfolio.rs`：`HoldingInput` 增加 `cost_price`，校验 > 0，手动金额持仓带了成本价时拒绝；验证：单元测试覆盖填写、留空、清空、0 和负数被拒、手动持仓被拒
- [x] 2.2 `market.rs`：`value_holding` 和 `refresh_holdings` 写金额时一并写原币现价 `price`；有份额但 `price` 为空的持仓不受"每小时一次"限制；验证：用构造的报价测试刷新后 `price` 已写入、缺 `price` 时强制刷新
- [x] 2.3 `Holding` 返回 `cost_price`、`currency`、`cost`、`pnl`、`pnl_pct`(按 design 第 1 条的公式)，新增 `portfolio::pnl_summary`；验证：用例覆盖美股盈利(10 股 × 500→550 美元，汇率 7.2 → +3600 元、+10%)、A 股亏损、汇率变化不计入盈亏、无成本价、无 `price`、部分未计入、全部未计入
- [x] 2.4 快照：新增 `snapshot_today`，在刷新成功后以及持仓增、改、删后调用；新增 `period_pnl(from, to)`，返回区间盈亏和起算日期；验证：用例覆盖同日覆盖、今年以来、快照不足一年、加仓不算盈利、没有快照
- [x] 2.5 `pnl_summary` 加上今年以来盈亏，`lib.rs` 暴露 `pnl_summary` 命令，`add_holding` / `update_holding` 透传 `cost_price`；验证：`cargo test` 全部通过
- [x] 2.6 前端：按份额估值的持仓表单增加"成本价"输入，并按查到的证券显示币种(美元 / 港元 / 元)；份额变化时提示更新成本价；持仓列表加盈亏、收益率列，组合页显示盈亏汇总和今年以来盈亏；`portfolioText.ts` 补格式化函数和测试；验证：`pnpm test` 通过，`pnpm tauri dev` 中给 VOO 填美元成本价后盈亏折合人民币显示正确

## 3. 涨跌过大提醒(price-alerts)

- [x] 3.1 引入 `tauri-plugin-notification`(Cargo 与 npm 两端)，`lib.rs` 注册插件，capabilities 加通知权限；验证：`pnpm tauri build` 成功
- [x] 3.2 新建 `alerts.rs`：读写 `alert_enabled` / `alert_pct` 设置(阈值 0 < x ≤ 50)，计算提醒对象(自选 ∪ 场内持仓，去重，排除 `OF.`)，暴露 get / save 设置命令；验证：单元测试覆盖默认值、阈值越界被拒、去重、排除场外基金
- [x] 3.3 `alerts::evaluate(quotes, pct, sent)` 纯函数，按行情日去重，没有现价时跳过；`alert_sent` 写入并清理 7 天前的记录；验证：单元测试覆盖上涨超阈值、下跌超阈值、未达阈值、同日不重复、隔日再提醒、美股跨午夜、无现价
- [x] 3.4 `setup` 中启动每小时一次的后台线程(启动后延迟几分钟再跑第一轮)：关闭或没有对象时不联网，取报价失败跳过本轮，触发后发通知，未授权时写 `notify_denied`；验证：临时把阈值调到 0.01、间隔调短，在 `pnpm tauri dev` 中最小化窗口后收到通知；断网时不报错
- [x] 3.5 前端：行情页增加"涨跌过大提醒"开关和阈值输入，首次打开开关时请求通知授权，未授权时显示开启指引；验证：`pnpm test` 通过，界面上能开关、改阈值，非法阈值有提示

## 4. 助手与复盘(investment-assistant)

- [x] 4.1 `agent.rs` 注册只读工具 `investment_performance`(明细含原币成本价、现价、币种 + 汇总 + 今年以来，金额换成元，没有成本时返回说明)；验证：仿照 `analytics_tools_are_exposed_and_dispatched` 写用例，`no_destructive_tools_exposed` 仍然通过
- [x] 4.2 `review.rs`：`Facts` 增加可选的 `investment`，`facts_markdown` / `facts_for_model` 渲染投资一节；验证：用例覆盖已结束月份、进行中月份、当月没有快照、没有成本(省略这一节)
- [x] 4.3 本地模型冒烟测试：新增 `live_investment`(`#[ignore]`)，询问"今年投资赚了多少""哪只拖后腿最多"；验证：`cargo test live_investment -- --ignored --nocapture` 的回答引用了正确的数字

## 5. 集成检查

- [x] 5.1 `cargo test`、`pnpm test` 全部通过，`pnpm tauri build` 成功
- [x] 5.2 在打包后的 Ledger.app 上实测：旧数据库升级后持仓自动补上原币现价；给 VOO、QQQM 填美元成本价后盈亏正确；提醒在窗口后台时弹出系统通知(未签名应用的授权流程要确认能走通)；助手和月度复盘能讲出投资盈亏
- [x] 5.3 README 的功能说明补充成本价与盈亏、涨跌过大提醒(包括通知授权)；验证：按 README 的说明能完成操作

## 6. 实测反馈

- [x] 6.1 登记了持仓的投资账户余额跟随持仓合计(`portfolio::holdings_changed` / `sync_account_balances`)，估值刷新后前端重新加载；验证：`account_balance_follows_its_holdings` 覆盖刷新、现金持仓、换账户、删最后一条，`coverage_flags_a_gap_outside_95_to_105_percent` 按新规则通过
- [x] 6.2 界面：涨跌提醒一行的复选框、文字、输入框垂直对齐；持仓弹窗里日期、金额不折行；验证：在打包后的应用里截图确认

## Workflow follow-up

- 验收后用 `/opsx:archive` 归档本变更。
