## Why

记账仍要一笔笔手输;助手记账直接写库,模型听错了只能事后去删;助手只能在固定的右栏里用,也不知道用户正在看哪一页。

## What Changes

- 账单导入:选择或拖入微信(XLSX/CSV)、支付宝(CSV,GBK)导出的账单 → 自动识别来源、跳过不计收支/关闭/退款、部分退款按净额 → 按"学到的规则 → 平台分类 → 商户关键词"映射分类 → 预览(疑似与手动账目重复的默认不勾选)→ 一键入账。交易单号去重,支付方式可关联账户;用户改过的分类、账户会记成规则下次自动套用。
- 记账确认卡片:助手的 add_transaction 只生成卡片(金额/分类/账户/日期/备注可改),用户点确认才入账。
- 一键复盘:程序算出收支、预算、分类对比、最大支出、订阅、目标的表格,模型只写解读(模型不可用时仍给出数字);可保存到报表页。
- 总览洞察卡:纯规则——预算超支/消耗超前、3 天内订阅扣费、近 7 天大额支出;每条可「问助手」。
- 全局助手抽屉:⌘K / Ctrl+K 或右下角悬浮球唤起,附带当前页面上下文。
- 查账工具沿用已有的 query_transactions、compare_periods,未新增。
- 应用锁:用户决定不做。

## Impact

- Rust:新增 `bill_import.rs`、`review.rs`、`insights.rs`;表 `import_ref`、`import_rule`、`review`(均已加入备份);`Agent::reply` 返回 `Reply { text, drafts }` 并接收页面上下文;新增 `agent::complete`(无工具单次调用);依赖 calamine、encoding_rs。
- 前端:新增 `chat.tsx`(抽屉 + 卡片)、`importBill.tsx`、`insights.tsx`、`reviews.tsx`;布局从三栏改为两栏 + 抽屉。
