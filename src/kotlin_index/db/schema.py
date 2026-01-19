SCHEMA = """
-- Файлы проекта
CREATE TABLE IF NOT EXISTS files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT UNIQUE NOT NULL,
    name TEXT NOT NULL,
    extension TEXT,
    module TEXT,
    modified_at REAL,
    indexed_at REAL
);

CREATE INDEX IF NOT EXISTS idx_files_name ON files(name);
CREATE INDEX IF NOT EXISTS idx_files_module ON files(module);
CREATE INDEX IF NOT EXISTS idx_files_extension ON files(extension);

-- Модули проекта
CREATE TABLE IF NOT EXISTS modules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT UNIQUE NOT NULL,
    path TEXT NOT NULL,
    type TEXT  -- api, impl, stub, app, lib
);

CREATE INDEX IF NOT EXISTS idx_modules_name ON modules(name);

-- Зависимости между модулями
CREATE TABLE IF NOT EXISTS module_deps (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    module_id INTEGER NOT NULL,
    dep_module_name TEXT NOT NULL,
    dep_type TEXT NOT NULL,  -- api, implementation, testImplementation
    FOREIGN KEY (module_id) REFERENCES modules(id),
    UNIQUE(module_id, dep_module_name, dep_type)
);

CREATE INDEX IF NOT EXISTS idx_module_deps_module ON module_deps(module_id);
CREATE INDEX IF NOT EXISTS idx_module_deps_dep ON module_deps(dep_module_name);

-- Символы (классы, интерфейсы, функции)
CREATE TABLE IF NOT EXISTS symbols (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    type TEXT NOT NULL,  -- class, interface, object, function, property, enum
    file_id INTEGER NOT NULL,
    line INTEGER,
    end_line INTEGER,
    signature TEXT,
    parent_symbol_id INTEGER,
    visibility TEXT,  -- public, private, internal, protected
    FOREIGN KEY (file_id) REFERENCES files(id),
    FOREIGN KEY (parent_symbol_id) REFERENCES symbols(id)
);

CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
CREATE INDEX IF NOT EXISTS idx_symbols_type ON symbols(type);
CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_id);

-- Наследование и реализация интерфейсов
CREATE TABLE IF NOT EXISTS inheritance (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    symbol_id INTEGER NOT NULL,           -- класс/интерфейс который наследует/реализует
    parent_name TEXT NOT NULL,            -- имя родительского класса/интерфейса
    inheritance_type TEXT NOT NULL,       -- extends, implements
    FOREIGN KEY (symbol_id) REFERENCES symbols(id),
    UNIQUE(symbol_id, parent_name)
);

CREATE INDEX IF NOT EXISTS idx_inheritance_symbol ON inheritance(symbol_id);
CREATE INDEX IF NOT EXISTS idx_inheritance_parent ON inheritance(parent_name);

-- Использования символов (references)
CREATE TABLE IF NOT EXISTS symbol_references (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    symbol_name TEXT NOT NULL,            -- имя используемого символа
    file_id INTEGER NOT NULL,             -- файл где используется
    line INTEGER NOT NULL,                -- строка использования
    context TEXT,                         -- контекст (вызов, присваивание и т.д.)
    FOREIGN KEY (file_id) REFERENCES files(id)
);

CREATE INDEX IF NOT EXISTS idx_refs_symbol ON symbol_references(symbol_name);
CREATE INDEX IF NOT EXISTS idx_refs_file ON symbol_references(file_id);

-- Полнотекстовый поиск по именам
CREATE VIRTUAL TABLE IF NOT EXISTS files_fts USING fts5(name, path, module);
CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(name, signature);

-- Метаданные индекса
CREATE TABLE IF NOT EXISTS index_meta (
    key TEXT PRIMARY KEY,
    value TEXT
);
"""


def init_schema(conn):
    """Инициализация схемы БД."""
    conn.executescript(SCHEMA)
    conn.commit()
