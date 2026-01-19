import re
import time
from pathlib import Path
from typing import Optional, Set

from kotlin_index.db.database import Database

# Попробуем импортировать tree-sitter для Kotlin
try:
    import tree_sitter_kotlin as ts_kotlin
    from tree_sitter import Language, Parser
    KOTLIN_AVAILABLE = True
except ImportError:
    KOTLIN_AVAILABLE = False

# Попробуем импортировать tree-sitter для Java
try:
    import tree_sitter_java as ts_java
    JAVA_AVAILABLE = True
except ImportError:
    JAVA_AVAILABLE = False


class SymbolIndexer:
    """Индексатор символов (классы, функции, интерфейсы) для Kotlin и Java."""

    # Regex паттерны для fallback парсинга Kotlin
    KOTLIN_CLASS_PATTERN = re.compile(
        r'^(?:(?:public|private|internal|protected|abstract|open|sealed|data|enum|annotation)\s+)*'
        r'(class|interface|object|enum\s+class)\s+(\w+)(?:\s*:\s*([^{]+))?',
        re.MULTILINE
    )

    KOTLIN_FUNCTION_PATTERN = re.compile(
        r'^(?:\s*)(?:(?:public|private|internal|protected|override|suspend|inline|operator)\s+)*'
        r'fun\s+(?:<[^>]+>\s+)?(\w+)\s*\(([^)]*)\)',
        re.MULTILINE
    )

    # Regex паттерны для fallback парсинга Java
    JAVA_CLASS_PATTERN = re.compile(
        r'^(?:(?:public|private|protected|abstract|final|static)\s+)*'
        r'(class|interface|enum)\s+(\w+)(?:\s+extends\s+(\w+))?(?:\s+implements\s+([^{]+))?',
        re.MULTILINE
    )

    JAVA_METHOD_PATTERN = re.compile(
        r'^(?:\s*)(?:(?:public|private|protected|static|final|synchronized|native|abstract)\s+)*'
        r'(?:<[^>]+>\s+)?(\w+)\s+(\w+)\s*\(([^)]*)\)',
        re.MULTILINE
    )

    def __init__(self, db: Database, project_root: str):
        self.db = db
        self.project_root = Path(project_root)
        self.kotlin_parser = None
        self.java_parser = None

        # Собираем известные символы для поиска использований
        self.known_symbols: Set[str] = set()

        if KOTLIN_AVAILABLE:
            self._init_kotlin_parser()

        if JAVA_AVAILABLE:
            self._init_java_parser()

    def _init_kotlin_parser(self):
        """Инициализация Kotlin парсера."""
        try:
            self.kotlin_parser = Parser(Language(ts_kotlin.language()))
        except Exception as e:
            print(f"Failed to init Kotlin tree-sitter: {e}", file=sys.stderr)
            self.kotlin_parser = None

    def _init_java_parser(self):
        """Инициализация Java парсера."""
        try:
            self.java_parser = Parser(Language(ts_java.language()))
        except Exception as e:
            print(f"Failed to init Java tree-sitter: {e}", file=sys.stderr)
            self.java_parser = None

    def index_all(self, progress_callback: Optional[callable] = None) -> dict:
        """Полная индексация символов во всех Kotlin и Java файлах."""
        start_time = time.time()
        stats = {
            "files": 0,
            "symbols": 0,
            "inheritance": 0,
            "references": 0,
            "errors": 0,
            "kotlin_files": 0,
            "java_files": 0
        }

        # Очищаем таблицы наследования и references
        self.db.clear_inheritance()
        self.db.clear_references()

        # Получаем все Kotlin и Java файлы
        files = self.db.execute(
            "SELECT id, path, extension FROM files WHERE extension IN ('.kt', '.java')"
        ).fetchall()

        # Первый проход: собираем символы и наследование
        for file_row in files:
            file_id = file_row["id"]
            file_path = file_row["path"]
            extension = file_row["extension"]

            try:
                # Удаляем старые данные файла
                self.db.delete_symbols_by_file(file_id)
                self.db.delete_inheritance_by_file(file_id)
                self.db.delete_references_by_file(file_id)

                # Индексируем символы
                if extension == ".kt":
                    symbols_count, inheritance_count = self._index_kotlin_file(file_id, file_path)
                    stats["kotlin_files"] += 1
                else:
                    symbols_count, inheritance_count = self._index_java_file(file_id, file_path)
                    stats["java_files"] += 1

                stats["symbols"] += symbols_count
                stats["inheritance"] += inheritance_count
                stats["files"] += 1

                if progress_callback and stats["files"] % 500 == 0:
                    progress_callback(stats["files"])

            except Exception as e:
                stats["errors"] += 1

        # Собираем известные символы для второго прохода
        self._load_known_symbols()

        # Второй проход: ищем использования
        for file_row in files:
            file_id = file_row["id"]
            file_path = file_row["path"]
            extension = file_row["extension"]

            try:
                refs_count = self._index_references(file_id, file_path, extension)
                stats["references"] += refs_count
            except Exception:
                pass

        # Пересобираем FTS индекс
        self.db.rebuild_symbols_fts()
        self.db.commit()

        stats["elapsed_seconds"] = round(time.time() - start_time, 2)
        return stats

    def index_incremental(self, progress_callback: Optional[callable] = None) -> dict:
        """Инкрементальная индексация только изменённых файлов."""
        start_time = time.time()
        stats = {
            "files": 0,
            "symbols": 0,
            "inheritance": 0,
            "references": 0,
            "skipped": 0,
            "errors": 0
        }

        # Получаем все Kotlin и Java файлы
        files = self.db.execute(
            "SELECT id, path, extension, modified_at, indexed_at FROM files WHERE extension IN ('.kt', '.java')"
        ).fetchall()

        # Загружаем известные символы
        self._load_known_symbols()

        for file_row in files:
            file_id = file_row["id"]
            file_path = file_row["path"]
            extension = file_row["extension"]
            modified_at = file_row["modified_at"]
            indexed_at = file_row["indexed_at"]

            # Проверяем актуальный mtime файла
            try:
                current_mtime = Path(file_path).stat().st_mtime
            except FileNotFoundError:
                continue

            # Пропускаем если файл не изменился
            if indexed_at and current_mtime <= modified_at:
                stats["skipped"] += 1
                continue

            try:
                # Удаляем старые данные файла
                self.db.delete_symbols_by_file(file_id)
                self.db.delete_inheritance_by_file(file_id)
                self.db.delete_references_by_file(file_id)

                # Обновляем mtime в БД
                self.db.execute(
                    "UPDATE files SET modified_at = ?, indexed_at = ? WHERE id = ?",
                    (current_mtime, time.time(), file_id)
                )

                # Индексируем символы
                if extension == ".kt":
                    symbols_count, inheritance_count = self._index_kotlin_file(file_id, file_path)
                else:
                    symbols_count, inheritance_count = self._index_java_file(file_id, file_path)

                # Индексируем использования
                refs_count = self._index_references(file_id, file_path, extension)

                stats["symbols"] += symbols_count
                stats["inheritance"] += inheritance_count
                stats["references"] += refs_count
                stats["files"] += 1

                if progress_callback and stats["files"] % 100 == 0:
                    progress_callback(stats["files"])

            except Exception as e:
                stats["errors"] += 1

        self.db.rebuild_symbols_fts()
        self.db.commit()

        stats["elapsed_seconds"] = round(time.time() - start_time, 2)
        return stats

    def _load_known_symbols(self):
        """Загружает известные символы (классы, интерфейсы) для поиска использований."""
        rows = self.db.execute(
            "SELECT DISTINCT name FROM symbols WHERE type IN ('class', 'interface', 'object', 'enum')"
        ).fetchall()
        self.known_symbols = {row["name"] for row in rows}

    def _index_kotlin_file(self, file_id: int, file_path: str) -> tuple[int, int]:
        """Индексация Kotlin файла. Возвращает (symbols_count, inheritance_count)."""
        try:
            content = Path(file_path).read_text(encoding="utf-8")
        except Exception:
            return 0, 0

        if self.kotlin_parser:
            return self._index_kotlin_with_tree_sitter(file_id, content)
        else:
            return self._index_kotlin_with_regex(file_id, content), 0

    def _index_java_file(self, file_id: int, file_path: str) -> tuple[int, int]:
        """Индексация Java файла. Возвращает (symbols_count, inheritance_count)."""
        try:
            content = Path(file_path).read_text(encoding="utf-8")
        except Exception:
            return 0, 0

        if self.java_parser:
            return self._index_java_with_tree_sitter(file_id, content)
        else:
            return self._index_java_with_regex(file_id, content), 0

    def _index_kotlin_with_tree_sitter(self, file_id: int, content: str) -> tuple[int, int]:
        """Индексация Kotlin с tree-sitter."""
        tree = self.kotlin_parser.parse(bytes(content, "utf-8"))
        symbols_count = 0
        inheritance_count = 0

        def visit_node(node, parent_id=None):
            nonlocal symbols_count, inheritance_count

            symbol_type = None
            name = None
            signature = None

            if node.type == "class_declaration":
                symbol_type = self._detect_kotlin_class_type(node)
                name = self._get_identifier(node, content)
            elif node.type == "object_declaration":
                symbol_type = "object"
                name = self._get_identifier(node, content)
            elif node.type == "function_declaration":
                symbol_type = "function"
                name = self._get_identifier(node, content)
                for child in node.children:
                    if child.type == "function_value_parameters":
                        signature = content[child.start_byte:child.end_byte]
                        break
            elif node.type == "property_declaration":
                symbol_type = "property"
                name = self._get_property_name(node, content)
                signature = self._get_property_type(node, content)

            current_parent_id = parent_id

            if symbol_type and name:
                line = node.start_point[0] + 1
                end_line = node.end_point[0] + 1

                symbol_id = self.db.upsert_symbol(
                    name=name,
                    symbol_type=symbol_type,
                    file_id=file_id,
                    line=line,
                    end_line=end_line,
                    signature=signature,
                    parent_id=parent_id,
                )
                symbols_count += 1
                current_parent_id = symbol_id

                # Парсим наследование для классов/интерфейсов
                if symbol_type in ("class", "interface", "object", "enum"):
                    parents = self._get_kotlin_supertypes(node, content)
                    for parent_name, inh_type in parents:
                        self.db.add_inheritance(symbol_id, parent_name, inh_type)
                        inheritance_count += 1

            for child in node.children:
                visit_node(child, current_parent_id)

        visit_node(tree.root_node)
        return symbols_count, inheritance_count

    def _index_java_with_tree_sitter(self, file_id: int, content: str) -> tuple[int, int]:
        """Индексация Java с tree-sitter."""
        tree = self.java_parser.parse(bytes(content, "utf-8"))
        symbols_count = 0
        inheritance_count = 0

        def visit_node(node, parent_id=None):
            nonlocal symbols_count, inheritance_count

            symbol_type = None
            name = None
            signature = None

            if node.type == "class_declaration":
                symbol_type = "class"
                name = self._get_java_identifier(node, content)
            elif node.type == "interface_declaration":
                symbol_type = "interface"
                name = self._get_java_identifier(node, content)
            elif node.type == "enum_declaration":
                symbol_type = "enum"
                name = self._get_java_identifier(node, content)
            elif node.type == "method_declaration":
                symbol_type = "function"
                name = self._get_java_identifier(node, content)
                # Получаем параметры
                for child in node.children:
                    if child.type == "formal_parameters":
                        signature = content[child.start_byte:child.end_byte]
                        break
            elif node.type == "field_declaration":
                # Поля класса как property
                symbol_type = "property"
                for child in node.children:
                    if child.type == "variable_declarator":
                        for subchild in child.children:
                            if subchild.type == "identifier":
                                name = content[subchild.start_byte:subchild.end_byte]
                                break

            current_parent_id = parent_id

            if symbol_type and name:
                line = node.start_point[0] + 1
                end_line = node.end_point[0] + 1

                symbol_id = self.db.upsert_symbol(
                    name=name,
                    symbol_type=symbol_type,
                    file_id=file_id,
                    line=line,
                    end_line=end_line,
                    signature=signature,
                    parent_id=parent_id,
                )
                symbols_count += 1
                current_parent_id = symbol_id

                # Парсим наследование
                if symbol_type in ("class", "interface", "enum"):
                    parents = self._get_java_supertypes(node, content)
                    for parent_name, inh_type in parents:
                        self.db.add_inheritance(symbol_id, parent_name, inh_type)
                        inheritance_count += 1

            for child in node.children:
                visit_node(child, current_parent_id)

        visit_node(tree.root_node)
        return symbols_count, inheritance_count

    def _detect_kotlin_class_type(self, node) -> str:
        """Определить тип Kotlin class_declaration."""
        for child in node.children:
            if child.type == "interface":
                return "interface"
            elif child.type == "modifiers":
                for mod in child.children:
                    if mod.type == "class_modifier" and mod.children:
                        for m in mod.children:
                            if m.type == "enum":
                                return "enum"
        return "class"

    def _get_identifier(self, node, content: str) -> Optional[str]:
        """Получить identifier из Kotlin узла."""
        for child in node.children:
            if child.type == "identifier":
                return content[child.start_byte:child.end_byte]
        return None

    def _get_java_identifier(self, node, content: str) -> Optional[str]:
        """Получить identifier из Java узла."""
        for child in node.children:
            if child.type == "identifier":
                return content[child.start_byte:child.end_byte]
        return None

    def _get_property_name(self, node, content: str) -> Optional[str]:
        """Получить имя свойства из Kotlin property_declaration."""
        for child in node.children:
            if child.type == "variable_declaration":
                for subchild in child.children:
                    if subchild.type == "identifier":
                        return content[subchild.start_byte:subchild.end_byte]
        return None

    def _get_property_type(self, node, content: str) -> Optional[str]:
        """Получить тип свойства."""
        for child in node.children:
            if child.type == "variable_declaration":
                for subchild in child.children:
                    if subchild.type == "user_type":
                        return content[subchild.start_byte:subchild.end_byte]
        return None

    def _get_kotlin_supertypes(self, node, content: str) -> list[tuple[str, str]]:
        """Получить суперклассы и интерфейсы Kotlin класса."""
        parents = []
        for child in node.children:
            if child.type == "delegation_specifiers":
                for specifier in child.children:
                    if specifier.type in ("delegation_specifier", "user_type", "constructor_invocation"):
                        # Получаем имя типа
                        type_name = self._extract_type_name(specifier, content)
                        if type_name:
                            # В Kotlin и наследование и реализация через ":"
                            parents.append((type_name, "extends"))
        return parents

    def _get_java_supertypes(self, node, content: str) -> list[tuple[str, str]]:
        """Получить суперклассы и интерфейсы Java класса."""
        parents = []
        for child in node.children:
            if child.type == "superclass":
                # extends
                for subchild in child.children:
                    type_name = self._extract_java_type_name(subchild, content)
                    if type_name:
                        parents.append((type_name, "extends"))
            elif child.type == "super_interfaces":
                # implements
                for subchild in child.children:
                    if subchild.type == "type_list":
                        for type_node in subchild.children:
                            type_name = self._extract_java_type_name(type_node, content)
                            if type_name:
                                parents.append((type_name, "implements"))
            elif child.type == "extends_interfaces":
                # interface extends interface
                for subchild in child.children:
                    if subchild.type == "type_list":
                        for type_node in subchild.children:
                            type_name = self._extract_java_type_name(type_node, content)
                            if type_name:
                                parents.append((type_name, "extends"))
        return parents

    def _extract_java_type_name(self, node, content: str) -> Optional[str]:
        """Извлечь имя типа из Java узла (поддержка generics)."""
        if node.type == "type_identifier":
            return content[node.start_byte:node.end_byte]
        elif node.type == "generic_type":
            # Для generic типов ищем type_identifier внутри
            for child in node.children:
                if child.type == "type_identifier":
                    return content[child.start_byte:child.end_byte]
        return None

    def _extract_type_name(self, node, content: str) -> Optional[str]:
        """Извлечь имя типа из узла."""
        if node.type == "user_type":
            for child in node.children:
                if child.type in ("type_identifier", "identifier"):
                    return content[child.start_byte:child.end_byte]
        elif node.type == "constructor_invocation":
            for child in node.children:
                if child.type == "user_type":
                    return self._extract_type_name(child, content)
        elif node.type == "delegation_specifier":
            for child in node.children:
                result = self._extract_type_name(child, content)
                if result:
                    return result
        elif node.type in ("type_identifier", "identifier"):
            return content[node.start_byte:node.end_byte]
        return None

    def _index_references(self, file_id: int, file_path: str, extension: str) -> int:
        """Индексация использований символов в файле."""
        try:
            content = Path(file_path).read_text(encoding="utf-8")
        except Exception:
            return 0

        refs_count = 0
        lines = content.split("\n")

        # Используем regex для поиска идентификаторов
        identifier_pattern = re.compile(r'\b([A-Z][a-zA-Z0-9_]*)\b')

        for line_num, line in enumerate(lines, 1):
            # Пропускаем комментарии и импорты
            stripped = line.strip()
            if stripped.startswith("//") or stripped.startswith("*") or stripped.startswith("import "):
                continue

            for match in identifier_pattern.finditer(line):
                symbol_name = match.group(1)
                if symbol_name in self.known_symbols:
                    # Определяем контекст использования
                    context = self._detect_usage_context(line, match.start())
                    self.db.add_reference(symbol_name, file_id, line_num, context)
                    refs_count += 1

        return refs_count

    def _detect_usage_context(self, line: str, pos: int) -> str:
        """Определить контекст использования символа."""
        # Простая эвристика
        before = line[:pos].rstrip()
        after = line[pos:].lstrip()

        if "(" in after[:20]:
            return "call"
        elif before.endswith(":") or before.endswith("extends") or before.endswith("implements"):
            return "inheritance"
        elif before.endswith("=") or before.endswith("new"):
            return "instantiation"
        elif before.endswith("<") or ">" in after[:10]:
            return "generic"
        else:
            return "reference"

    # === Fallback regex methods ===

    def _index_kotlin_with_regex(self, file_id: int, content: str) -> int:
        """Fallback индексация Kotlin с regex."""
        symbols_count = 0

        for match in self.KOTLIN_CLASS_PATTERN.finditer(content):
            symbol_type_raw = match.group(1)
            name = match.group(2)

            if "interface" in symbol_type_raw:
                symbol_type = "interface"
            elif "object" in symbol_type_raw:
                symbol_type = "object"
            elif "enum" in symbol_type_raw:
                symbol_type = "enum"
            else:
                symbol_type = "class"

            line = content[:match.start()].count("\n") + 1

            self.db.upsert_symbol(
                name=name,
                symbol_type=symbol_type,
                file_id=file_id,
                line=line,
            )
            symbols_count += 1

        for match in self.KOTLIN_FUNCTION_PATTERN.finditer(content):
            name = match.group(1)
            params = match.group(2)
            line = content[:match.start()].count("\n") + 1

            line_start = content.rfind("\n", 0, match.start()) + 1
            indent = len(content[line_start:match.start()]) - len(content[line_start:match.start()].lstrip())

            if indent == 0:
                self.db.upsert_symbol(
                    name=name,
                    symbol_type="function",
                    file_id=file_id,
                    line=line,
                    signature=f"({params})",
                )
                symbols_count += 1

        return symbols_count

    def _index_java_with_regex(self, file_id: int, content: str) -> int:
        """Fallback индексация Java с regex."""
        symbols_count = 0

        for match in self.JAVA_CLASS_PATTERN.finditer(content):
            symbol_type_raw = match.group(1)
            name = match.group(2)

            if symbol_type_raw == "interface":
                symbol_type = "interface"
            elif symbol_type_raw == "enum":
                symbol_type = "enum"
            else:
                symbol_type = "class"

            line = content[:match.start()].count("\n") + 1

            self.db.upsert_symbol(
                name=name,
                symbol_type=symbol_type,
                file_id=file_id,
                line=line,
            )
            symbols_count += 1

        return symbols_count

    # === Public methods for single file ===

    def index_file(self, file_path: str) -> int:
        """Индексация символов одного файла."""
        file_info = self.db.get_file_by_path(file_path)
        if not file_info:
            return 0

        extension = Path(file_path).suffix

        self.db.delete_symbols_by_file(file_info["id"])
        self.db.delete_inheritance_by_file(file_info["id"])
        self.db.delete_references_by_file(file_info["id"])

        if extension == ".kt":
            symbols_count, _ = self._index_kotlin_file(file_info["id"], file_path)
        elif extension == ".java":
            symbols_count, _ = self._index_java_file(file_info["id"], file_path)
        else:
            return 0

        self.db.commit()
        return symbols_count
