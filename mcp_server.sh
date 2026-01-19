#!/bin/bash

# Определяем директорию скрипта
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Определяем корень проекта (на 2 уровня выше от .claude/mcp-index)
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Путь к БД
DB_PATH="$HOME/.cache/kotlin-index/index.db"

# Создаём директорию для БД
mkdir -p "$(dirname "$DB_PATH")"

# Активируем venv если есть
if [ -d "$SCRIPT_DIR/.venv" ]; then
    source "$SCRIPT_DIR/.venv/bin/activate"
fi

# Экспортируем переменные
export KOTLIN_INDEX_PROJECT_ROOT="$PROJECT_ROOT"
export KOTLIN_INDEX_DB_PATH="$DB_PATH"

# Запускаем сервер
exec python3 "$SCRIPT_DIR/server.py"
