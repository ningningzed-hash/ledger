## Why

记账与账户余额彼此独立:用招行卡花了一笔钱,卡里的余额和净资产都不变,只能靠手动更新估值,净资产因此长期不准。助手记账时也无法指明用的是哪个账户。

## What Changes

- 账户余额改为"最新估值 + 估值之后录入的收支"自动计算;删除账目会自动回退余额。
- 手动更新估值仍可用,并作为新的基准。
- 负债账户(如信用卡)的方向相反:支出使欠款增加,收入使欠款减少。
- 房产账户不接受收支记录,只能更新估值。
- 助手记账可指定账户,并在回复中给出记账后的账户余额。
- 账目列表显示所属账户。
- 估值与账目带录入时间戳;从旧版本导入的数据视为已体现在余额里,导入后各账户余额保持不变(见 `migrate-to-mysql`)。

## Capabilities

### New Capabilities
- `account-balance`: 账户余额与收支、估值之间的联动规则。

### Modified Capabilities
<!-- 助手相关的账户参数并入 account-balance,避免依赖尚未归档的 chat-agent 规格 -->

## Impact

- Rust:`valuation`、`txn` 增加 `created_at`;`list_accounts` 改为计算余额;`add_txn` 校验账户;助手 `add_transaction` 增加 `account` 参数与提示词规则。
- 前端:账目行显示账户;更新估值改为应用内弹窗(替代 `window.prompt`)。
