import os
import time
from pathlib import Path
from typing import Optional

from kotlin_index.db.database import Database


class FileIndexer:
    """Индексатор файлов проекта."""

    # Расширения для индексации
    EXTENSIONS = {".kt", ".java", ".xml", ".gradle", ".kts", ".properties"}

    # Директории для игнорирования
    IGNORE_DIRS = {
        "build", ".gradle", ".idea", ".git", "node_modules",
        "__pycache__", ".pytest_cache", "venv", ".venv"
    }

    def __init__(self, db: Database, project_root: str):
        self.db = db
        self.project_root = Path(project_root)

    def index_all(self, progress_callback: Optional[callable] = None) -> dict:
        """Полная индексация всех файлов."""
        start_time = time.time()
        stats = {"added": 0, "updated": 0, "removed": 0, "total": 0}

        # Получаем существующие файлы в индексе
        existing_paths = self.db.get_all_file_paths()
        found_paths = set()

        # Сканируем файловую систему
        for file_path in self._scan_files():
            found_paths.add(file_path)
            stats["total"] += 1

            path_obj = Path(file_path)
            modified_at = path_obj.stat().st_mtime

            # Проверяем, нужно ли обновление
            existing = self.db.get_file_by_path(file_path)
            if existing and existing["modified_at"] == modified_at:
                continue  # Файл не изменился

            # Определяем модуль
            module = self._detect_module(file_path)

            # Добавляем/обновляем файл
            self.db.upsert_file(
                path=file_path,
                name=path_obj.name,
                extension=path_obj.suffix,
                module=module,
                modified_at=modified_at,
            )

            if existing:
                stats["updated"] += 1
            else:
                stats["added"] += 1

            if progress_callback and stats["total"] % 1000 == 0:
                progress_callback(stats["total"])

        # Удаляем отсутствующие файлы
        removed_paths = existing_paths - found_paths
        if removed_paths:
            self.db.delete_files_by_paths(list(removed_paths))
            stats["removed"] = len(removed_paths)

        # Пересобираем FTS индекс
        self.db.rebuild_files_fts()
        self.db.commit()

        stats["elapsed_seconds"] = round(time.time() - start_time, 2)
        return stats

    def index_file(self, file_path: str) -> bool:
        """Индексация одного файла."""
        path_obj = Path(file_path)

        if not path_obj.exists():
            return False

        if path_obj.suffix not in self.EXTENSIONS:
            return False

        module = self._detect_module(file_path)

        self.db.upsert_file(
            path=file_path,
            name=path_obj.name,
            extension=path_obj.suffix,
            module=module,
            modified_at=path_obj.stat().st_mtime,
        )
        self.db.commit()
        return True

    def _scan_files(self):
        """Сканирование файлов проекта."""
        for root, dirs, files in os.walk(self.project_root):
            # Фильтруем игнорируемые директории
            dirs[:] = [d for d in dirs if d not in self.IGNORE_DIRS]

            for filename in files:
                ext = Path(filename).suffix
                if ext in self.EXTENSIONS:
                    yield os.path.join(root, filename)

    def _detect_module(self, file_path: str) -> str:
        """Определение модуля по пути файла."""
        rel_path = Path(file_path).relative_to(self.project_root)
        parts = rel_path.parts

        # Ищем build.gradle.kts в родительских директориях
        # Типичная структура: features/payments/api/src/main/kotlin/...
        # Модуль: features.payments.api

        module_parts = []
        current_path = self.project_root

        for i, part in enumerate(parts[:-1]):  # Исключаем имя файла
            current_path = current_path / part

            # Проверяем наличие build.gradle.kts
            if (current_path / "build.gradle.kts").exists():
                # Это корень модуля
                module_parts = list(parts[:i + 1])

                # Фильтруем служебные директории
                module_parts = [p for p in module_parts if p not in {"src", "main", "kotlin", "java", "res", "test", "androidTest"}]

        if module_parts:
            return ".".join(module_parts)

        # Fallback: первые 2-3 части пути
        significant_parts = [p for p in parts[:3] if p not in {"src", "main", "kotlin", "java"}]
        return ".".join(significant_parts) if significant_parts else "root"
