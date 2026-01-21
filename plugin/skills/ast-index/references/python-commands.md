# Python Commands Reference

ast-index supports parsing and indexing Python source files (`.py`).

## Supported Elements

| Python Element | Symbol Kind | Example |
|----------------|-------------|---------|
| `class ClassName` | Class | `UserService` → Class |
| `def function_name` | Function | `process_data` → Function |
| `async def function_name` | Function | `fetch_user` → Function |
| `@decorator` | Decorator | `@dataclass` → Decorator |
| `import module` | Import | `import os` → Import |
| `from module import name` | Import | `from typing import List` → Import |

## Core Commands

### Search Classes

Find Python class definitions:

```bash
ast-index class "Service"           # Find service classes
ast-index class "Handler"           # Find handler classes
ast-index search "Repository"       # Find repositories
```

### Search Functions

Find functions and async functions:

```bash
ast-index symbol "process"          # Find functions containing "process"
ast-index symbol "fetch"            # Find fetch functions
ast-index callers "handle_request"  # Find callers of handle_request
```

### File Analysis

Show file structure:

```bash
ast-index outline "service.py"      # Show classes and functions
ast-index imports "handler.py"      # Show all imports (including from X import Y)
```

## Example Workflow

```bash
# 1. Index Python project
cd /path/to/python/project
ast-index rebuild

# 2. Check index statistics
ast-index stats

# 3. Find all classes
ast-index search "class"

# 4. Find async functions
ast-index symbol "async"

# 5. Show file structure
ast-index outline "main.py"

# 6. Find usages
ast-index usages "UserService"
```

## Indexed Python Patterns

### Class Definition

```python
class UserService:
    def __init__(self, db: Database):
        self.db = db

    async def get_user(self, user_id: int) -> User:
        return await self.db.fetch_user(user_id)
```

Indexed as:
- `UserService` [class]
- `__init__` [function]
- `get_user` [function]

### Decorators

```python
@dataclass
class Config:
    host: str
    port: int

@router.get("/users")
async def get_users():
    pass
```

Indexed as:
- `@dataclass` [decorator]
- `Config` [class]
- `@router.get` [decorator]
- `get_users` [function]

## Import Handling

Both import styles are tracked:

```python
import os
import sys
from typing import List, Optional
from fastapi import FastAPI, Depends
```

Use `ast-index imports "file.py"` to see all imports with line numbers.

## Performance

| Operation | Time |
|-----------|------|
| Rebuild (500 Python files) | ~500ms |
| Search class | ~1ms |
| Find usages | ~5ms |
| File outline | ~1ms |
