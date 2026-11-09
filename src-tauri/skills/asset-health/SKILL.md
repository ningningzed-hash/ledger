---
name: asset-health
description: 应急金、负债和持仓集中度体检
---
# 资产健康体检
1. 调用 get_overview、list_accounts、get_portfolio、get_portfolio_advice、investment_performance，读取实际资产与风险问卷。
2. 依工具结果检查应急金覆盖、负债、单一市场和资产集中度、估值日期、目标资金缺口。
3. 区分原币与人民币、收益率与浮动盈亏。缺少成本或风险问卷时不编造收益率及目标比例。
4. 输出「已知事实、需要关注、可执行步骤」。任何估值修改先生成确认卡片。不得发起交易或预测保本收益。
