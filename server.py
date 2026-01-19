#!/usr/bin/env python3
"""
MCP сервер для индексации и поиска по Android проекту.

Предоставляет быстрый поиск:
- Файлов по имени
- Символов (классы, функции, интерфейсы)
- Модулей и их зависимостей
"""

import os
import sys
import time
from pathlib import Path
from typing import Optional

# Добавляем текущую директорию в путь для импортов
SCRIPT_DIR = Path(__file__).parent
sys.path.insert(0, str(SCRIPT_DIR))

from fastmcp import FastMCP
from db.database import Database
from indexer.file_indexer import FileIndexer
from indexer.module_indexer import ModuleIndexer
from indexer.symbol_indexer import SymbolIndexer

# Конфигурация
# По умолчанию: 2 уровня вверх от .claude/mcp-index
DEFAULT_PROJECT_ROOT = str(SCRIPT_DIR.parent.parent)
PROJECT_ROOT = os.environ.get("KOTLIN_INDEX_PROJECT_ROOT", DEFAULT_PROJECT_ROOT)
DB_PATH = os.environ.get("KOTLIN_INDEX_DB_PATH", str(SCRIPT_DIR / "index.db"))

# Инициализация
mcp = FastMCP("kotlin-index")
db = Database(DB_PATH)
db.connect()

file_indexer = FileIndexer(db, PROJECT_ROOT)
module_indexer = ModuleIndexer(db, PROJECT_ROOT)
symbol_indexer = SymbolIndexer(db, PROJECT_ROOT)


# === Инструменты поиска файлов ===

@mcp.tool()
def find_file(query: str, limit: int = 20) -> str:
    """
    Поиск файлов по имени или части пути.

    Args:
        query: Строка поиска (имя файла или часть пути)
        limit: Максимальное количество результатов (по умолчанию 20)

    Returns:
        JSON со списком найденных файлов
    """
    results = db.search_files(query, limit)

    if not results:
        return f"Файлы по запросу '{query}' не найдены"

    output = [f"Найдено файлов: {len(results)}"]
    for f in results:
        output.append(f"  {f['path']}")

    return "\n".join(output)


@mcp.tool()
def find_file_exact(name: str) -> str:
    """
    Найти файл по точному имени.

    Args:
        name: Точное имя файла (например, PaymentMethodsFragment.kt)

    Returns:
        Полный путь к файлу или сообщение что не найден
    """
    results = db.execute(
        "SELECT path FROM files WHERE name = ? LIMIT 10",
        (name,)
    ).fetchall()

    if not results:
        return f"Файл '{name}' не найден"

    if len(results) == 1:
        return results[0]["path"]

    output = [f"Найдено {len(results)} файлов с именем '{name}':"]
    for r in results:
        output.append(f"  {r['path']}")
    return "\n".join(output)


# === Инструменты поиска символов ===

@mcp.tool()
def find_symbol(query: str, symbol_type: Optional[str] = None, limit: int = 20) -> str:
    """
    Поиск символов (классов, интерфейсов, функций) по имени.

    Args:
        query: Имя символа или его часть
        symbol_type: Тип символа (class, interface, object, function, property, enum). Опционально.
        limit: Максимальное количество результатов

    Returns:
        JSON со списком найденных символов
    """
    results = db.search_symbols(query, symbol_type, limit)

    if not results:
        type_str = f" типа '{symbol_type}'" if symbol_type else ""
        return f"Символы{type_str} по запросу '{query}' не найдены"

    output = [f"Найдено символов: {len(results)}"]
    for s in results:
        sig = f" {s['signature']}" if s.get("signature") else ""
        output.append(f"  [{s['type']}] {s['name']}{sig}")
        output.append(f"    {s['file_path']}:{s['line']}")

    return "\n".join(output)


@mcp.tool()
def find_class(name: str) -> str:
    """
    Найти класс/интерфейс по имени.

    Args:
        name: Имя класса или интерфейса

    Returns:
        Путь к файлу и строка, где объявлен класс
    """
    results = db.search_symbols(name, symbol_type=None, limit=10)
    # Фильтруем только классы и интерфейсы
    results = [r for r in results if r["type"] in ("class", "interface", "object", "enum")]

    if not results:
        return f"Класс/интерфейс '{name}' не найден"

    output = []
    for s in results:
        output.append(f"[{s['type']}] {s['name']}: {s['file_path']}:{s['line']}")

    return "\n".join(output)


@mcp.tool()
def get_file_outline(file_path: str) -> str:
    """
    Получить структуру файла (список классов, функций и т.д.).

    Args:
        file_path: Путь к файлу

    Returns:
        Структура файла с символами
    """
    symbols = db.get_file_symbols(file_path)

    if not symbols:
        return f"Символы в файле '{file_path}' не найдены (возможно файл не проиндексирован)"

    output = [f"Структура файла {file_path}:"]

    for s in symbols:
        indent = "  " if s.get("parent_symbol_id") else ""
        sig = f" {s['signature']}" if s.get("signature") else ""
        output.append(f"{indent}[{s['type']}] {s['name']}{sig} (строка {s['line']})")

    return "\n".join(output)


# === Инструменты работы с модулями ===

@mcp.tool()
def find_module(query: str, limit: int = 20) -> str:
    """
    Поиск модулей по имени.

    Args:
        query: Имя модуля или его часть
        limit: Максимальное количество результатов

    Returns:
        Список найденных модулей
    """
    results = db.search_modules(query, limit)

    if not results:
        return f"Модули по запросу '{query}' не найдены"

    output = [f"Найдено модулей: {len(results)}"]
    for m in results:
        output.append(f"  [{m['type']}] {m['name']}")
        output.append(f"    {m['path']}")

    return "\n".join(output)


@mcp.tool()
def get_module_deps(module_name: str) -> str:
    """
    Получить зависимости модуля.

    Args:
        module_name: Имя модуля (например, features.payments.api)

    Returns:
        Список зависимостей модуля
    """
    deps = db.get_module_deps(module_name)

    if not deps:
        return f"Зависимости модуля '{module_name}' не найдены"

    output = [f"Зависимости модуля {module_name}:"]

    # Группируем по типу
    by_type = {}
    for d in deps:
        dep_type = d["dep_type"]
        if dep_type not in by_type:
            by_type[dep_type] = []
        by_type[dep_type].append(d["dep_module_name"])

    for dep_type, modules in sorted(by_type.items()):
        output.append(f"\n  {dep_type}:")
        for m in sorted(modules):
            output.append(f"    - {m}")

    return "\n".join(output)


@mcp.tool()
def get_module_dependents(module_name: str) -> str:
    """
    Получить модули, которые зависят от данного.

    Args:
        module_name: Имя модуля

    Returns:
        Список модулей-зависимых
    """
    deps = db.get_module_dependents(module_name)

    if not deps:
        return f"От модуля '{module_name}' никто не зависит"

    output = [f"От модуля {module_name} зависят:"]
    for d in deps:
        output.append(f"  [{d['dep_type']}] {d['module_name']}")

    return "\n".join(output)


# === Инструменты индексации ===

@mcp.tool()
def rebuild_index(index_type: str = "all") -> str:
    """
    Пересобрать индекс.

    Args:
        index_type: Тип индекса для пересборки:
            - "files" - только файлы
            - "modules" - только модули
            - "symbols" - только символы
            - "all" - всё (по умолчанию)

    Returns:
        Статистика индексации
    """
    results = {}

    if index_type in ("all", "files"):
        results["files"] = file_indexer.index_all()

    if index_type in ("all", "modules"):
        results["modules"] = module_indexer.index_all()

    if index_type in ("all", "symbols"):
        results["symbols"] = symbol_indexer.index_all()

    # Обновляем метаданные
    db.set_meta("last_indexed", time.strftime("%Y-%m-%d %H:%M:%S"))
    db.commit()

    output = ["Индексация завершена:"]
    for idx_type, stats in results.items():
        output.append(f"\n  {idx_type}:")
        for key, value in stats.items():
            output.append(f"    {key}: {value}")

    return "\n".join(output)


@mcp.tool()
def update_index() -> str:
    """
    Инкрементальное обновление индекса (только изменённые файлы).

    Returns:
        Статистика обновления
    """
    results = symbol_indexer.index_incremental()

    db.set_meta("last_indexed", time.strftime("%Y-%m-%d %H:%M:%S"))
    db.commit()

    if results["files"] == 0:
        return f"Нет изменённых файлов (пропущено: {results['skipped']})"

    output = ["Инкрементальное обновление завершено:"]
    output.append(f"  Обновлено файлов: {results['files']}")
    output.append(f"  Пропущено: {results['skipped']}")
    output.append(f"  Символов: {results['symbols']}")
    output.append(f"  Наследований: {results['inheritance']}")
    output.append(f"  Использований: {results['references']}")

    return "\n".join(output)


@mcp.tool()
def find_usages(symbol_name: str, limit: int = 50) -> str:
    """
    Найти все использования символа в проекте.

    Args:
        symbol_name: Имя символа (класс, функция, переменная)
        limit: Максимальное количество результатов

    Returns:
        Список мест использования символа
    """
    refs = db.get_references(symbol_name, limit)

    if not refs:
        return f"Использования '{symbol_name}' не найдены"

    output = [f"Использования '{symbol_name}' ({len(refs)}):"]

    # Группируем по файлам
    by_file = {}
    for ref in refs:
        path = ref["file_path"]
        if path not in by_file:
            by_file[path] = []
        by_file[path].append(ref)

    for path, file_refs in sorted(by_file.items()):
        output.append(f"\n  {path}:")
        for ref in file_refs:
            ctx = f" ({ref['context']})" if ref.get("context") else ""
            output.append(f"    строка {ref['line']}{ctx}")

    return "\n".join(output)


@mcp.tool()
def find_implementations(interface_name: str) -> str:
    """
    Найти все реализации интерфейса или наследники класса.

    Args:
        interface_name: Имя интерфейса или класса

    Returns:
        Список классов, реализующих интерфейс или наследующих класс
    """
    impls = db.get_implementations(interface_name)

    if not impls:
        return f"Реализации/наследники '{interface_name}' не найдены"

    output = [f"Реализации/наследники '{interface_name}' ({len(impls)}):"]

    for impl in impls:
        rel = "implements" if impl["inheritance_type"] == "implements" else "extends"
        output.append(f"  [{impl['type']}] {impl['name']} ({rel})")
        output.append(f"    {impl['file_path']}:{impl['line']}")

    return "\n".join(output)


@mcp.tool()
def get_index_stats() -> str:
    """
    Получить статистику индекса.

    Returns:
        Статистика индекса (количество файлов, модулей, символов)
    """
    stats = db.get_stats()

    output = [
        "Статистика индекса:",
        f"  Файлов: {stats['files']}",
        f"  Модулей: {stats['modules']}",
        f"  Символов: {stats['symbols']}",
        f"  Зависимостей: {stats['dependencies']}",
        f"  Связей наследования: {stats['inheritance']}",
        f"  Использований символов: {stats['references']}",
        f"  Последняя индексация: {stats['last_indexed'] or 'никогда'}",
    ]

    return "\n".join(output)


# === Комбинированный поиск ===

@mcp.tool()
def search(query: str, limit: int = 10) -> str:
    """
    Универсальный поиск по файлам, символам и модулям.

    Args:
        query: Строка поиска
        limit: Максимальное количество результатов каждого типа

    Returns:
        Результаты поиска по всем категориям
    """
    output = [f"Результаты поиска '{query}':"]

    # Поиск файлов (LIKE для substring matching)
    files = db.search_files(query, limit)
    if files:
        output.append(f"\n--- Файлы ({len(files)}) ---")
        for f in files[:5]:
            output.append(f"  {f['path']}")
        if len(files) > 5:
            output.append(f"  ... и ещё {len(files) - 5}")

    # Поиск символов (FTS для multi-word, LIKE для single-word)
    # FTS5 не подходит для substring matching (UserRepo не найдёт по "Repo")
    if " " in query:
        try:
            symbols = db.search_symbols_fts(query, limit=limit)
        except Exception:
            symbols = []
        if not symbols:
            symbols = db.search_symbols(query, limit=limit)
    else:
        symbols = db.search_symbols(query, limit=limit)
    if symbols:
        output.append(f"\n--- Символы ({len(symbols)}) ---")
        for s in symbols[:5]:
            output.append(f"  [{s['type']}] {s['name']} - {s['file_path']}:{s['line']}")
        if len(symbols) > 5:
            output.append(f"  ... и ещё {len(symbols) - 5}")

    # Поиск модулей
    modules = db.search_modules(query, limit)
    if modules:
        output.append(f"\n--- Модули ({len(modules)}) ---")
        for m in modules[:5]:
            output.append(f"  [{m['type']}] {m['name']}")
        if len(modules) > 5:
            output.append(f"  ... и ещё {len(modules) - 5}")

    if len(output) == 1:
        return f"По запросу '{query}' ничего не найдено"

    return "\n".join(output)


if __name__ == "__main__":
    mcp.run()
