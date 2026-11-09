#!/usr/bin/env bash
# 初始化 Ledger 使用的 MySQL:建库、建专用账号、生成随机密码,并写出应用的连接配置。
# 可重复运行:已有配置时沿用其中的密码,只补齐缺失的库/账号/权限。
#
# 用法:scripts/setup-mysql.sh
# 环境变量:
#   MYSQL_ROOT_ARGS  连接 MySQL 管理员的参数,默认 "-uroot"(Homebrew 安装的 root 无密码)
#                    例:MYSQL_ROOT_ARGS="-uroot -pSECRET"
set -euo pipefail

DB_NAME=ledger
DB_USER=ledger
IDENTIFIER=com.maker.ledger

case "$(uname -s)" in
  Darwin) DATA_DIR="$HOME/Library/Application Support/$IDENTIFIER" ;;
  *)      DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/$IDENTIFIER" ;;
esac
CONFIG="$DATA_DIR/database.json"

MYSQL_BIN="$(command -v mysql || true)"
[ -z "$MYSQL_BIN" ] && [ -x /opt/homebrew/opt/mysql/bin/mysql ] && MYSQL_BIN=/opt/homebrew/opt/mysql/bin/mysql
[ -z "$MYSQL_BIN" ] && { echo "找不到 mysql 客户端,请先安装 MySQL(brew install mysql)" >&2; exit 1; }

# shellcheck disable=SC2086
admin() { "$MYSQL_BIN" ${MYSQL_ROOT_ARGS:--uroot} "$@"; }

admin -e "SELECT 1" >/dev/null 2>&1 || {
  echo "无法以管理员身份连接 MySQL。请确认服务已启动(brew services start mysql),或设置 MYSQL_ROOT_ARGS。" >&2
  exit 1
}

mkdir -p "$DATA_DIR"
if [ -f "$CONFIG" ]; then
  PASSWORD="$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["password"])' "$CONFIG")"
  echo "沿用已有配置:$CONFIG"
else
  PASSWORD="$(openssl rand -hex 16)"
fi

# 密码只含十六进制字符,可安全拼进 SQL
admin <<SQL
CREATE DATABASE IF NOT EXISTS \`$DB_NAME\` CHARACTER SET utf8mb4;
CREATE USER IF NOT EXISTS '$DB_USER'@'localhost'  IDENTIFIED BY '$PASSWORD';
CREATE USER IF NOT EXISTS '$DB_USER'@'127.0.0.1' IDENTIFIED BY '$PASSWORD';
ALTER USER '$DB_USER'@'localhost'  IDENTIFIED BY '$PASSWORD';
ALTER USER '$DB_USER'@'127.0.0.1' IDENTIFIED BY '$PASSWORD';
GRANT ALL ON \`$DB_NAME\`.* TO '$DB_USER'@'localhost';
GRANT ALL ON \`$DB_NAME\`.* TO '$DB_USER'@'127.0.0.1';
-- 每位后续用户使用一个独立的 ledger_u_* 数据库。
GRANT ALL ON \`ledger\_u\_%\`.* TO '$DB_USER'@'localhost';
GRANT ALL ON \`ledger\_u\_%\`.* TO '$DB_USER'@'127.0.0.1';
-- 单元测试会创建并删除 ledger_test_* 临时库,只授予这个前缀
GRANT ALL ON \`ledger\_test\_%\`.* TO '$DB_USER'@'localhost';
GRANT ALL ON \`ledger\_test\_%\`.* TO '$DB_USER'@'127.0.0.1';
SQL

if [ ! -f "$CONFIG" ]; then
  umask 077
  cat > "$CONFIG" <<JSON
{
  "host": "127.0.0.1",
  "port": 3306,
  "user": "$DB_USER",
  "password": "$PASSWORD",
  "database": "$DB_NAME"
}
JSON
  echo "已写入配置:$CONFIG(仅当前用户可读)"
fi
chmod 600 "$CONFIG"

echo "完成。应用将连接 mysql://$DB_USER@127.0.0.1:3306/$DB_NAME"
