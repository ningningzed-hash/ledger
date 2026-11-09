---
name: monthly-review
description: 基于当前用户账本进行月度复盘
---
# 月度复盘
1. 确认月份，未指定用本月。调用 get_overview、category_summary、list_subscriptions、list_goals 获取事实。
2. 使用 analytics 工具比较上月支出；统计必须引用工具结果，不自行猜测或把截断明细当全量。
3. 按「收支与预算、主要变化、固定支出、目标进度、下月行动」输出，标注月份、币种和数据范围。
4. 数据为空时说明缺少什么。预算调整只能调用 set_monthly_budget 生成待确认卡片，不声称已经写入。
