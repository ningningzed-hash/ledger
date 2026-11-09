# Ledger

记账 + 资产管理 + 本地大模型助手。Tauri 2 + React + Rust,数据存放在本机 MySQL,助手默认使用本机 Ollama,也可在助手设置里填写地址、接口路径和 API Key 接入 DeepSeek 等 OpenAI 兼容的外部模型。

## 首次运行

```bash
brew install mysql && brew services start mysql   # 安装并启动 MySQL(已装可跳过)
scripts/setup-mysql.sh                            # 建库、专用账号、随机密码,并写出连接配置
ollama pull qwen3:14b                             # 助手使用的模型
pnpm install && pnpm tauri dev
```

连接配置在应用数据目录的 `database.json`(macOS:`~/Library/Application Support/com.maker.ledger/`),
也可用环境变量 `LEDGER_DB_URL=mysql://user:pass@127.0.0.1:3306/ledger` 覆盖。

## 打包安装

```bash
pnpm tauri build    # 产物在 src-tauri/target/release/bundle/:macos/Ledger.app 与 dmg/Ledger_<版本>_aarch64.dmg
```

打开 dmg,把 Ledger 拖进「应用程序」即可。安装包不含 MySQL 和 Ollama,目标电脑需按上面「首次运行」先装好并运行 `scripts/setup-mysql.sh`。
应用未做苹果签名:第一次打开时在「应用程序」里右键 Ledger →「打开」放行一次(或到「系统设置 → 隐私与安全性」点「仍要打开」)。
数据和连接配置按 identifier `com.maker.ledger` 存放,和开发模式共用同一份,不要改 identifier。
图标源文件是 `src-tauri/icons/app-icon.png`(由 `python3 scripts/make-icon.py src-tauri/icons/app-icon.png` 生成),改完运行 `pnpm tauri icon src-tauri/icons/app-icon.png -o src-tauri/icons` 重新生成各尺寸。

## 投资盈亏与涨跌提醒

- **成本价与盈亏**:在「组合建议 → 持仓」里,按份额估值的持仓可以填成本价。成本价按证券的交易币种填(VOO、QQQM 填美元,A 股填人民币),程序按 成本价 × 份额 算成本,盈亏按原币价格计算、不含汇率变化,再折成人民币显示。加仓或减仓后,把份额和成本价一起改成新的数值(平均成本)。手动填金额的持仓不算盈亏。
- **今年以来盈亏**:每天自动记一份持仓快照,从升级到这个版本那天开始积累,之前的盈亏无法补算;不满一年时会注明从哪天开始统计。月度复盘里的「投资」一节也用这些快照。
- **涨跌过大提醒**:在「行情」页自选列表下方打开开关、设阈值(默认 5%)。自选和场内持仓当日涨跌超过阈值时发系统通知,每只每天最多一次;应用开着(包括在后台)时每小时检查一次,应用退出后不提醒。第一次打开开关时 macOS 会请求通知权限;没收到通知的话,到「系统设置 → 通知 → Ledger」允许通知。

## 用户登录与账本隔离

- 首次使用需注册用户名和密码。用户名为 3–32 位字母、数字、下划线或短横线（不区分大小写），密码至少 8 个字符，最多 128 字节。
- 升级已有账本时，首个注册用户需要勾选确认接管旧账本；账目、资产、聊天、持仓和模型配置原样保留。后续用户从独立的空账本开始。
- 每个用户的账目、资产、预算、目标、订阅、持仓、报表、聊天、模型密钥及个人设置均隔离。用户名目录保存在原数据库的 `auth_user` 表，首位用户继续使用原库，后续用户使用 `<原库名>_u_<随机标识>` 数据库。
- 旧安装请先重新运行 `scripts/setup-mysql.sh` 补齐 `ledger_u_*` 建库权限；脚本沿用已有数据库密码和账本。使用自定义 MySQL 配置时，需要为相应 `<原库名>_u_*` 前缀授予建库与读写权限。
- 密码使用带随机盐的 Argon2id 哈希保存。连续 5 次登录失败会暂停尝试 30 秒；会话有效期为 12 小时，退出、切换或修改密码后旧会话立即失效。应用退出后需重新登录；页面刷新可恢复尚有效的会话。
- 自动记账、估值与涨跌提醒仅针对当前登录用户。切换后，已发出的后台请求仍绑定原用户账本，其结果不会写入新用户账本或显示旧用户通知。
- 登录时按用户做每日备份，文件放在应用数据目录的 `backups/user-<用户ID>/`，各自保留最近 7 份。旧备份目录保留。账本 JSON 不包含用户目录、密码哈希和 API Key；整机迁移仍需保留原数据库中的 `auth_user` 及对应用户数据库。

## 个人中心与聊天窗口

侧栏「理财」下方的「个人中心」可保存昵称、邮箱、简介，并设置浅色 / 深色 / 跟随系统外观和下次启动的默认首页。助手设置也可从这里进入。个人中心的「登录与安全」可修改密码、切换用户或退出登录。每位用户的资料与偏好独立保存。

财务助手默认宽度为 560px。拖动左边缘可调整宽度，双击边缘恢复默认，顶部展开按钮可铺满窗口并还原。宽度记在当前设备，缩小应用窗口时自动适配；聚焦左边缘后也可用左右方向键调整，Home 恢复默认，End 展开。

## 助手能力市场

侧栏「能力市场」提供搜索、MCP / Skill 筛选、按用户启停、连接测试、来源与流程详情，以及「立即使用」。

- **AKTools 0.1.4**（[MIT](https://github.com/akfamily/aktools)）：AKShare 国内财经接口搜索、参数查询及只读数据查询。
- **yfinance MCP / yfmcp 0.14.0**（[MIT](https://github.com/narumiruna/yfinance-mcp)）：海外股票 / ETF 基本信息、基金持仓、价格历史、新闻和证券搜索。数据使用需遵循 Yahoo Finance 的条款。
- **四个内置 Skill**：月度复盘、资产健康体检、ETF 对比、账单核对。流程源文件在 `src-tauri/skills/*/SKILL.md`，编译进应用；不是另外下载的第三方 Skill。修改文件后需重新编译。它们使用现有账本工具或已启用 MCP，模型仍需在助手设置中配置。

首次安装 MCP（Python 3.12+，macOS 可先 `brew install python@3.12`）：

```bash
sh scripts/setup-mcp.sh
```

两个服务分别安装在 `~/.local/share/ledger/mcp/aktools` 与 `yfinance` 的独立 Python 环境，不改系统 Python。可通过 `LEDGER_MCP_HOME` 指定运行目录（安装脚本与启动应用需保持一致）。首次下载可能需要几分钟；安装后在能力市场点击「测试连接」，通过后「启用」。新用户的 MCP 默认关闭，Skill 默认开启。

助手使用官方 Rust MCP SDK，经 stdio 启动服务、握手并发现允许的只读工具，每轮结束关闭进程。连接与调用有超时，结果有长度上限。连接测试成功只表示 MCP 协议可用，具体行情仍可能因数据源限流或网络失败；错误会进入执行记录与助手上下文，不会生成替代行情。账本工具使用当前用户数据库，配置和连接测试记录也按用户隔离；账本明细不主动发送给财经 MCP，查询参数由助手根据任务生成。模型服务的数据传输沿用助手设置。

从市场「立即使用」会明确选择能力；普通对话也会按意图加载已启用 Skill。聊天中可展开执行记录查看加载、调用和错误。预算、估值、记账操作都先生成卡片，只有点击确认才修改账本。执行记录与未确认卡片仅保留在本次页面会话，刷新后历史保留回复文字。

集成测试（需要上述运行时，实际数据测试还需联网）：

```bash
cd src-tauri
cargo test installed_servers_handshake -- --ignored --nocapture
cargo test live_data_queries -- --ignored --nocapture
```

## 测试

```bash
cd src-tauri && cargo test          # 在真实 MySQL 上为每个用例建临时库 ledger_test_*,结束即删
cargo test live_ -- --ignored --nocapture --test-threads=1   # 对本机 Ollama 的真实模型冒烟测试(含 live_investment)
pnpm test                           # 前端
```

## 从旧版本(SQLite)迁移数据

```bash
cd src-tauri && cargo run --example import_sqlite -- "<旧的 ledger.db 路径>"
```

只读打开旧文件;仅当 MySQL 里还没有业务数据时才导入。

## 变更记录

设计与需求变更记录在 `openspec/changes/`。
