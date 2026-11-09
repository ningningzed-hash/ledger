# Design

## Context

- `holding` 表记 `amount`(人民币，分)、`shares`(份额 × 10000)、`secid`；按份额估值的持仓由 `market::refresh_holdings` 更新金额，触发时机是启动、窗口获得焦点、进入组合建议，并且每小时最多联网一次。原币价格只在 `market::Price` 里经过，没有存下来。目前没有任何后台定时任务。
- 报价走 `market::fetch_quotes`(东方财富主站 → 备用站 → 新浪兜底)，一次能取多只；港股、美股按 `133.HKDCNH` / `133.USDCNH` 换算成人民币(`fetch_exchange` / `to_price`)。
- 助手工具定义在 `agent::tool_defs`，各模块用 `tool_defs()` / 执行函数扩展；复盘由 `review::facts` 产出结构化数字，再由 `facts_markdown` 和 `facts_for_model` 分别渲染。
- 备份按 `backup::TABLES` 逐表导出，`OPTIONAL` 中的表在旧备份里缺失时视为空；后加的列通过 `db::ADDED_COLUMNS` 迁移。
- 应用状态 `AppState` 持有懒连接的 MySQL 池；目前只有 opener、dialog 两个插件。

## Goals / Non-Goals

**Goals:**
- 成本、盈亏的计算全部放在后端，前端、助手和复盘用同一份结果，三处口径一致。
- 用户只填原币成本价，汇率换算完全由程序负责。
- 提醒检查不依赖窗口在前台。

**Non-Goals:**
- 不记逐笔交易，不自动摊薄成本：加仓、减仓时由用户自己改份额和成本价。
- 不单独计算汇率损益。
- 快照不记单只持仓，只记总额。

## Decisions

**1. 存原币成本价 `holding.cost_price`(DOUBLE)和原币现价 `holding.price`(DOUBLE)。**
用户按交易币种填写成本价(VOO 填美元)。`refresh_holdings` 和 `value_holding` 写入金额的同时，把原币现价也写进 `price`。
盈亏全部从已存数据推出，不需要另外存汇率：
- 收益率 = `price / cost_price − 1`
- 盈亏(人民币) = `amount × (1 − cost_price / price)`，等价于 (现价 − 成本价) × 份额 × 最近一次估值使用的汇率
- 成本(人民币) = `amount − 盈亏`

这样算出的是股价本身的盈亏，不受买入后汇率波动影响，符合"用美元买入"的情况。
备选方案"成本折成人民币总投入"会把汇率波动算进盈亏，而且需要用户自己换算或记住买入时的汇率，所以不采用。
`price` 为空(升级后尚未刷新)时盈亏为空，下一次刷新后自然补上。币种由 `market::currency_of(secid)` 推断，前端表单据此显示"美元 / 港元 / 元"。

**2. 盈亏在 `list_holdings` 返回时算好，另加 `portfolio::pnl_summary`。**
`Holding` 增加 `cost_price`、`currency`、`cost`、`pnl`、`pnl_pct`(都可空)。`pnl_summary` 汇总能算出盈亏的持仓，返回总成本、总市值、总盈亏、收益率、未计入数量，以及今年以来盈亏(含起算日期)。前端持仓页、助手工具、复盘都调用它。

**3. 快照表 `holding_snapshot(date PK, value, cost)`，用 `INSERT … ON DUPLICATE KEY UPDATE` 写入当天。**
写入点：`refresh_holdings` 成功更新后、`add_holding` / `update_holding` / `delete_holding` 之后。持仓变动后立即写快照，是"加仓不算盈利"能够成立的原因：成本与市值在同一天一起变化。
区间盈亏 = 区间末的 (value − cost) − 区间开始前最后一份快照的 (value − cost)；区间末若是今天，用实时汇总。
备选方案"按交易流水推算"需要逐笔交易记录，属于非目标。

**4. 提醒只有全局开关与阈值，存在 `setting` 表(`alert_enabled`、`alert_pct`)；已提醒记录存在 `alert_sent(secid, quote_day)`。**
提醒对象在每轮检查时现场计算：自选 ∪ 有 secid 且不是 `OF.` 开头的持仓，按 secid 去重。不需要单独维护提醒列表。
`alert_sent` 以报价时间的日期(行情日)去重，美股跨北京时间午夜也不会重复提醒。超过 7 天的记录在检查时顺手清掉。它是运行状态，不放进备份。

**5. 后台检查放在 Rust 线程，每小时一次。**
`setup` 中启动线程：开关关闭或没有提醒对象就直接跳过；否则调用一次 `fetch_quotes`，用纯函数 `alerts::evaluate(quotes, pct, sent)` 选出要通知的证券，写入 `alert_sent` 并通过 `tauri-plugin-notification` 发通知。启动后先等几分钟再跑第一轮，避免和启动时的估值刷新挤在一起。
前端定时器在窗口隐藏时会被 WebView 节流，无法保证后台提醒，所以不用前端定时器。数据库连不上或取报价失败时，本轮跳过并记录日志。

**6. 通知授权。**
用户在界面上首次打开开关时，由前端调用插件请求授权。后台线程发通知前检查权限；未授权就只写 `alert_sent`，并在 `setting` 写 `notify_denied`，提醒设置处读到后显示开启指引。

**7. 助手工具 `investment_performance`(只读)，复盘新增 `Facts.investment: Option<InvestFacts>`。**
工具直接序列化 `pnl_summary` 加上每只持仓的明细(含原币成本价、现价和币种)，金额换成元。复盘的投资一节：已结束的月份用"月末及之前"的最后一份快照，进行中的月份用实时汇总；本月盈亏 = 该值 − 上月末及之前的最后一份快照。最好、最差持仓按当前收益率选取。

**8. 投资账户余额跟随持仓(实测后追加)。**
原先账户余额和持仓各记各的，只在偏差超过 5% 时提示对账，用户反馈资产页的余额不会跟着持仓变。现在由 `portfolio::holdings_changed` 统一处理：持仓增、改、删和估值刷新之后，先 `sync_account_balances`(登记了持仓的投资账户写当天估值 = 持仓合计，相等时不写)，再写盈亏快照。账户里的现金由用户登记成一条现金类手动持仓。对账提示因此只针对没登记持仓的投资账户。

## Risks / Trade-offs

- [快照依赖应用被打开过] 某个月一次都没打开应用，那个月就没有快照 → 区间计算向前找最近一份快照并标出起算日期；复盘在数据不足时如实说明。
- [成本价需要用户手动维护] 加仓后忘记改成本价，盈亏会失真 → 编辑持仓时如果份额变了，表单提示"份额变了，记得更新成本价(加仓后的平均成本)"。
- [每小时检查会错过盘中的短时异动] 盘中跌到 −6% 又在一小时内收回 → 用户接受这个精度，以通知次数少为优先。
- [券商账户里有未登记的现金] 余额会被改成只含持仓的数值 → 持仓弹窗和资产页都说明了规则，现金登记成现金类持仓即可。
- [macOS 未签名应用的通知] 未签名应用可能不弹授权框 → 实现阶段在打包后的 Ledger.app 上实测；如果不行，在提示里引导用户到「系统设置 → 通知」手动开启。
- [原币现价缺失] 升级后到下一次估值刷新之间看不到盈亏 → 升级后首次启动时，估值刷新照常执行(不受"每小时一次"限制，因为还没有 `price`)。

## Migration Plan

- `db::migrate`：`ADDED_COLUMNS` 增加 `holding.cost_price DOUBLE NULL`、`holding.price DOUBLE NULL`；`STATEMENTS` 增加 `holding_snapshot`、`alert_sent` 两张表。
- 备份：`TABLES` 的 holding 列表加上 `cost_price`、`price`；新增 `holding_snapshot` 并放入 `OPTIONAL`(`alert_sent` 不备份)。
- 升级后，有份额但缺 `price` 的持仓触发一次强制刷新；第一次刷新或修改持仓时生成第一份快照。
- 回滚：旧版本会忽略多出来的列和表，可以直接换回旧的 Ledger.app。
