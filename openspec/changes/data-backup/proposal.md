## Why

全部数据只存在一个本地 SQLite 文件里,没有任何备份或导出手段。文件损坏、误删或换电脑都会造成不可挽回的损失。

## What Changes

- 新增"数据"页:导出完整备份、导出账目 CSV、从备份恢复,并列出自动备份。
- 每天首次启动自动备份一次,保留最近 7 份。
- 备份为 JSON 文件(与数据库引擎无关)。恢复前校验文件有效性,并自动保留一份当前数据的安全备份(保留最近 3 份)。
- 恢复在单个事务内完成,失败整体回滚;恢复后无需重启。
- 恢复默认提示词、恢复备份的确认改用应用内系统对话框(替代 `window.confirm`)。

## Capabilities

### New Capabilities
- `data-backup`: 备份、恢复与导出。

### Modified Capabilities

## Impact

- Rust:新增 `backup.rs`;引入 `tauri-plugin-dialog`;新增 4 个命令。数据库已改为 MySQL(见 `migrate-to-mysql`)。
- 前端:新增"数据"页;窗口权限增加 `dialog:default` 与 `dialog:allow-ask`。
