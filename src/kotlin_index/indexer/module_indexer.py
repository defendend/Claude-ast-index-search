import os
import re
import time
from pathlib import Path
from typing import Optional

from kotlin_index.db.database import Database


class ModuleIndexer:
    """Индексатор модулей и зависимостей проекта."""

    # Паттерн для projects.xxx.yyy.zzz зависимостей
    PROJECTS_DEP_PATTERN = re.compile(
        r'(api|implementation|testImplementation|androidTestImplementation)\s*\(\s*'
        r'projects\.([a-zA-Z0-9_.]+)\s*\)',
        re.MULTILINE
    )

    # Паттерн для modules { } блока (многострочный)
    MODULES_BLOCK_PATTERN = re.compile(
        r'modules\s*\{([\s\S]*?)\n\s*\}',
        re.MULTILINE
    )

    # Паттерн для зависимостей внутри modules блока
    MODULES_DEP_PATTERN = re.compile(
        r'(api|implementation|testImplementation)\s*\(\s*projects\.([a-zA-Z0-9_.]+)\s*\)'
    )

    def __init__(self, db: Database, project_root: str):
        self.db = db
        self.project_root = Path(project_root)

    def index_all(self, progress_callback: Optional[callable] = None) -> dict:
        """Полная индексация всех модулей."""
        start_time = time.time()
        stats = {"modules": 0, "dependencies": 0}

        # Очищаем старые данные
        self.db.clear_modules()

        # Находим все build.gradle и build.gradle.kts файлы
        gradle_files = list(self.project_root.rglob("build.gradle.kts"))
        gradle_files.extend(self.project_root.rglob("build.gradle"))

        for gradle_file in gradle_files:
            # Пропускаем buildSrc и .gradle директории (кэш Gradle)
            gradle_path = str(gradle_file)
            if "buildSrc" in gradle_path or "/.gradle/" in gradle_path or "/build/" in gradle_path:
                continue

            module_path = gradle_file.parent
            module_name = self._path_to_module_name(module_path)

            if not module_name:
                continue

            # Определяем тип модуля
            module_type = self._detect_module_type(module_name, gradle_file)

            # Добавляем модуль
            self.db.upsert_module(module_name, str(module_path), module_type)
            stats["modules"] += 1

            if progress_callback and stats["modules"] % 50 == 0:
                progress_callback(stats["modules"])

        self.db.commit()

        # Индексируем зависимости (второй проход)
        for gradle_file in gradle_files:
            gradle_path = str(gradle_file)
            if "buildSrc" in gradle_path or "/.gradle/" in gradle_path or "/build/" in gradle_path:
                continue

            module_path = gradle_file.parent
            module_name = self._path_to_module_name(module_path)

            if not module_name:
                continue

            module = self.db.get_module_by_name(module_name)
            if not module:
                continue

            # Парсим зависимости
            deps = self._parse_dependencies(gradle_file)
            for dep_name, dep_type in deps:
                self.db.add_module_dep(module["id"], dep_name, dep_type)
                stats["dependencies"] += 1

        self.db.commit()

        stats["elapsed_seconds"] = round(time.time() - start_time, 2)
        return stats

    def _path_to_module_name(self, module_path: Path) -> Optional[str]:
        """Конвертация пути в имя модуля."""
        try:
            rel_path = module_path.relative_to(self.project_root)
            parts = rel_path.parts

            # Фильтруем пустые и служебные
            if not parts or parts[0] in {".gradle", "buildSrc", "gradle"}:
                return None

            return ".".join(parts)
        except ValueError:
            return None

    def _detect_module_type(self, module_name: str, gradle_file: Path) -> str:
        """Определение типа модуля."""
        name_lower = module_name.lower()

        if name_lower.endswith(".api"):
            return "api"
        elif name_lower.endswith(".impl"):
            return "impl"
        elif name_lower.endswith(".stub"):
            return "stub"
        elif name_lower.startswith("apps."):
            return "app"
        elif name_lower.startswith("libs.") or name_lower.startswith("internallibs."):
            return "lib"

        # Проверяем содержимое build.gradle.kts
        try:
            content = gradle_file.read_text()
            if "com.android.application" in content:
                return "app"
            elif "com.android.library" in content:
                return "lib"
        except Exception:
            pass

        return "module"

    def _parse_dependencies(self, gradle_file: Path) -> list[tuple[str, str]]:
        """Парсинг зависимостей из build.gradle / build.gradle.kts."""
        deps = []
        seen = set()

        try:
            content = gradle_file.read_text()

            # Ищем блок modules { }
            modules_match = self.MODULES_BLOCK_PATTERN.search(content)
            if modules_match:
                modules_content = modules_match.group(1)
                for match in self.MODULES_DEP_PATTERN.finditer(modules_content):
                    dep_type = match.group(1)
                    dep_name = self._normalize_module_name(match.group(2))
                    key = (dep_name, dep_type)
                    if key not in seen:
                        seen.add(key)
                        deps.append(key)

            # Также ищем прямые зависимости projects.xxx вне modules блока
            for match in self.PROJECTS_DEP_PATTERN.finditer(content):
                dep_type = match.group(1)
                dep_name = self._normalize_module_name(match.group(2))
                key = (dep_name, dep_type)
                if key not in seen:
                    seen.add(key)
                    deps.append(key)

        except Exception:
            pass

        return deps

    def _normalize_module_name(self, name: str) -> str:
        """Нормализация имени модуля из camelCase в точечную нотацию."""
        # projects.features.tappablePoi.api -> features.tappablePoi.api
        # Просто убираем camelCase, сохраняя структуру
        return name

    def get_module_tree(self) -> dict:
        """Получить дерево модулей."""
        modules = self.db.get_all_modules()
        tree = {}

        for module in modules:
            parts = module["name"].split(".")
            current = tree

            for part in parts[:-1]:
                if part not in current:
                    current[part] = {"_children": {}}
                current = current[part]["_children"]

            current[parts[-1]] = {
                "_info": module,
                "_children": {}
            }

        return tree
