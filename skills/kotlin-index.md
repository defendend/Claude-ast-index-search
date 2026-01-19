# kotlin-index - Code Search for Android/Kotlin/Java Projects

Fast code search tool for Android/Kotlin/Java projects using SQLite + FTS5.

## Prerequisites

Install the CLI:
```bash
pip install kotlin-index
```

Initialize index in project root:
```bash
cd /path/to/android/project
kotlin-index init
```

## Available Commands

### Search Commands

**Universal search** (files + symbols + modules):
```bash
kotlin-index search "PaymentMethod"
```

**Find files by name**:
```bash
kotlin-index file "Fragment.kt"
kotlin-index file --exact "PaymentMethodsFragment.kt"
```

**Find symbols** (classes, interfaces, functions):
```bash
kotlin-index symbol "PaymentInteractor"
kotlin-index symbol --type class "Repository"
kotlin-index symbol --type interface "Callback"
kotlin-index symbol --type function "validate"
```

**Find class/interface**:
```bash
kotlin-index class "BaseFragment"
```

**Find usages** of a symbol:
```bash
kotlin-index usages "PaymentRepository"
```

**Find implementations** (subclasses/implementors):
```bash
kotlin-index implementations "BasePresenter"
```

### Module Commands

**Find modules**:
```bash
kotlin-index module "payments"
```

**Module dependencies**:
```bash
kotlin-index deps "features.payments.impl"
```

**Modules depending on this module**:
```bash
kotlin-index dependents "features.payments.api"
```

### File Structure

**File outline** (classes, functions in file):
```bash
kotlin-index outline "/path/to/File.kt"
```

### Index Management

**Rebuild index**:
```bash
kotlin-index rebuild
kotlin-index rebuild --type files    # only files
kotlin-index rebuild --type symbols  # only symbols
kotlin-index rebuild --type modules  # only modules
```

**Index statistics**:
```bash
kotlin-index stats
```

## Environment Variables

- `KOTLIN_INDEX_PROJECT_ROOT` - project root directory (auto-detected by default)
- `KOTLIN_INDEX_DB_PATH` - path to SQLite database (default: `~/.cache/kotlin-index/index.db`)

## Use Cases

1. **Find class definition**: `kotlin-index class "PaymentMethodsFragment"`
2. **Find all implementations of interface**: `kotlin-index implementations "PaymentCallback"`
3. **Find where class is used**: `kotlin-index usages "PaymentRepository"`
4. **Analyze module dependencies**: `kotlin-index deps "features.payments.impl"`
5. **Find files by pattern**: `kotlin-index file "Payment"` (finds all files with "Payment" in name)

## Tips

- Index updates automatically detect changed files
- Use `--exact` flag for precise file name matching
- Symbol types: `class`, `interface`, `object`, `function`, `property`, `enum`
- FTS5 provides fast full-text search across 100k+ files
