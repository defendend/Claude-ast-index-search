# План рефакторинга v3.6.1 - Разделение файлов

## Текущее состояние

| Файл | Строк | Проблема |
|------|-------|----------|
| main.rs | 3447 | Все 46 команд в одном файле |
| indexer.rs | 2639 | Парсеры для 4 языков в одном файле |
| db.rs | 668 | OK |

## Целевая структура

```
src/
├── main.rs              # CLI definition + main() (~300 строк)
├── db.rs                # Database operations (без изменений)
├── indexer.rs           # Core indexing logic (~500 строк)
├── commands/
│   ├── mod.rs           # Re-exports
│   ├── grep.rs          # Grep-based: todo, callers, provides, suspend, composables,
│   │                    # deprecated, suppress, inject, annotations, deeplinks,
│   │                    # extensions, flows, previews (~550 строк)
│   ├── index.rs         # Index-based: search, file, symbol, class,
│   │                    # implementations, hierarchy, usages (~350 строк)
│   ├── modules.rs       # Module analysis: module, deps, dependents,
│   │                    # unused_deps, api (~650 строк)
│   ├── files.rs         # File analysis: outline, imports, changed (~250 строк)
│   ├── android.rs       # Android: xml_usages, resource_usages (~250 строк)
│   ├── ios.rs           # iOS: storyboard_usages, asset_usages, swiftui,
│   │                    # async_funcs, publishers, main_actor (~400 строк)
│   ├── perl.rs          # Perl: perl_exports, perl_subs, perl_pod,
│   │                    # perl_tests, perl_imports (~200 строк)
│   └── management.rs    # Management: init, rebuild, update, stats (~250 строк)
└── parsers/
    ├── mod.rs           # Re-exports + common utilities (~150 строк)
    ├── kotlin.rs        # Kotlin/Java parsing (~400 строк)
    ├── swift.rs         # Swift parsing (~250 строк)
    ├── objc.rs          # Objective-C parsing (~200 строк)
    └── perl.rs          # Perl parsing (~200 строк)
```

## Порядок выполнения

### Этап 1: Создание структуры директорий
```bash
mkdir -p src/commands src/parsers
touch src/commands/mod.rs src/parsers/mod.rs
```

### Этап 2: Выделение парсеров (indexer.rs → parsers/)

**2.1 parsers/perl.rs** (~150 строк)
- `parse_perl_symbols()` (строки 952-1091)

**2.2 parsers/objc.rs** (~200 строк)
- `parse_objc_symbols()` (строки 786-950)

**2.3 parsers/swift.rs** (~250 строк)
- `parse_swift_symbols()` (строки 562-762)
- `parse_swift_parents()` (строки 763-784)

**2.4 parsers/kotlin.rs** (~400 строк)
- `parse_symbols()` (строки 436-561) → переименовать в `parse_kotlin_symbols()`
- `collect_class_declaration()` (строки 243-282)
- `extract_parents_from_declaration()` (строки 283-340)
- `parse_parents()` (строки 341-395)

**2.5 parsers/mod.rs**
- `extract_references()` (строки 1108-1183)
- `is_supported_extension()` (строки 1184-1186)
- Re-export всех парсеров

**2.6 Обновление indexer.rs**
- Убрать перенесённые функции
- Добавить `mod parsers;`
- Использовать `parsers::*`

### Этап 3: Выделение команд (main.rs → commands/)

**3.1 commands/grep.rs** (~550 строк)
- `cmd_todo` (672-724)
- `cmd_callers` (725-759)
- `cmd_provides` (760-833)
- `cmd_suspend` (834-867)
- `cmd_composables` (868-911)
- `cmd_deprecated` (912-944)
- `cmd_suppress` (945-975)
- `cmd_inject` (976-1010)
- `cmd_annotations` (1011-1042)
- `cmd_deeplinks` (1043-1076)
- `cmd_extensions` (1077-1117)
- `cmd_flows` (1118-1153)
- `cmd_previews` (1154-1198)

**3.2 commands/index.rs** (~350 строк)
- `cmd_search` (1423-1469)
- `cmd_file` (1470-1499)
- `cmd_symbol` (1500-1535)
- `cmd_class` (1536-1565)
- `cmd_implementations` (1566-1596)
- `cmd_hierarchy` (1597-1652)
- `cmd_usages` (2273-2341)

**3.3 commands/modules.rs** (~650 строк)
- `cmd_module` (1653-1687)
- `cmd_deps` (1688-1752)
- `cmd_dependents` (1753-1817)
- `cmd_unused_deps` (1818-2272)
- `cmd_api` (2504-2545)

**3.4 commands/files.rs** (~250 строк)
- `cmd_outline` (2342-2441)
- `cmd_imports` (2442-2503)
- `cmd_changed` (2546-2619)

**3.5 commands/android.rs** (~250 строк)
- `cmd_xml_usages` (2620-2693)
- `cmd_resource_usages` (2694-2876)

**3.6 commands/ios.rs** (~400 строк)
- `cmd_storyboard_usages` (2877-2939)
- `cmd_asset_usages` (2940-3050)
- `cmd_swiftui` (3051-3111)
- `cmd_async_funcs` (3112-3153)
- `cmd_publishers` (3154-3202)
- `cmd_main_actor` (3203-3242)

**3.7 commands/perl.rs** (~200 строк)
- `cmd_perl_exports` (3243-3280)
- `cmd_perl_subs` (3281-3318)
- `cmd_perl_pod` (3319-3357)
- `cmd_perl_tests` (3358-3397)
- `cmd_perl_imports` (3398-3447)

**3.8 commands/management.rs** (~250 строк)
- `cmd_init` (1199-1216)
- `cmd_rebuild` (1217-1343)
- `cmd_update` (1344-1378)
- `cmd_stats` (1379-1422)

**3.9 commands/mod.rs**
- Re-export всех команд

**3.10 Обновление main.rs**
- Оставить только CLI definition (Commands enum, ~250 строк)
- Оставить main() с match
- Добавить `mod commands;`
- Использовать `commands::*`

### Этап 4: Верификация

1. `cargo build --release` - должен скомпилироваться без ошибок
2. `cargo test` - все тесты должны пройти
3. Ручное тестирование ключевых команд:
   - `ast-index rebuild` (в Perl проекте)
   - `ast-index perl-subs`
   - `ast-index class DoCmd`
   - `ast-index hierarchy DoCmd`

### Этап 5: Финализация

1. Обновить версию в Cargo.toml → 3.6.1
2. Обновить версию в plugin.json → 3.6.1
3. Коммит: "v3.6.1: Refactor - split large files into modules"
4. Тег v3.6.1
5. Собрать бинарь для Homebrew

## Принципы рефакторинга

1. **Логика 1:1** - никаких изменений в поведении
2. **Публичные функции** - все `cmd_*` функции становятся `pub`
3. **Импорты** - каждый модуль импортирует только то, что использует
4. **Минимум зависимостей** - избегать циклических зависимостей

## Ожидаемый результат

| Файл | До | После |
|------|-----|-------|
| main.rs | 3447 | ~300 |
| indexer.rs | 2639 | ~500 |
| Новые файлы | 0 | ~3300 (распределено) |
| Всего строк | 6754 | ~6754 (без изменений) |
