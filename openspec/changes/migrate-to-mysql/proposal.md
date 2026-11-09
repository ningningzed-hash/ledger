## Why

数据原先存放在应用目录下的 SQLite 文件里。用户希望改用独立的 MySQL 数据库保存,而不是本地文件,便于用常规数据库工具查看、备份和管理数据。

## What Changes

- **BREAKING**:数据存储从 SQLite 文件改为本机 MySQL。应用不再创建或读取 `ledger.db`。
- 连接信息保存在应用数据目录的 `database.json`(仅当前用户可读),也可用环境变量覆盖。
- 新增 `scripts/setup-mysql.sh`:建库、建最小权限的专用账号、生成随机密码、写出配置,可重复运行。
- 启动时连不上 MySQL 不再导致应用退出;窗口照常打开,每次操作都会重试并给出明确原因。
- 备份格式从"整库文件"改为 JSON 导出(与数据库引擎无关);恢复在单个事务内完成。
- 新增一次性导入工具,把旧的 SQLite 数据迁入 MySQL。
- 已有的真实数据(聊天记录 6 条)已迁入。

## Capabilities

### New Capabilities
- `data-storage`: 数据的存放位置、连接方式、可用性与迁移。

### Modified Capabilities
<!-- 备份与账户余额两个变更尚未归档,其文档已直接同步修订 -->

## Impact

- Rust:`rusqlite` 换为 `mysql`(同步驱动,无 TLS 依赖);`core`、`agent`、`backup`、`lib` 全部改用连接池;`rusqlite` 仅作为开发依赖供导入工具使用,不进入应用本体。
- 新增 `testdb`:测试在真实 MySQL 上为每个用例建独立临时库,结束即删。
- 前端:数据页改为展示数据库连接(不含密码),备份文件类型改为 `.json`。
- 运行环境:需要运行中的 MySQL 服务(Homebrew 安装,已设为登录自启)。
