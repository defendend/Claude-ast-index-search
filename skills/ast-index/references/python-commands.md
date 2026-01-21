# Python Commands Reference

ast-index supports parsing and indexing Python source files (`.py`), with focus on backend services patterns.

## Supported Elements

| Python Element | Symbol Kind | Example |
|----------------|-------------|---------|
| `class ClassName:` | Class | `Context` → Class |
| `class Child(Parent):` | Class | `ChildClass` → Class (with parent) |
| `def function_name():` | Function | `handle` → Function |
| `async def handler():` | Function | `async_handler` → Function |
| `import module` | Import | `logging` → Import |
| `from X import Y` | Import | `db` → Import |
| `@decorator` | Annotation | `@pytest.fixture` → Annotation |
| `CONSTANT = value` | Constant | `MAX_SIZE` → Constant |
| `TypeName = Union[...]` | TypeAlias | `ResponseType` → TypeAlias |

## Core Commands

### Search Classes

Find class definitions:

```bash
ast-index class "Context"           # Find specific class
ast-index class "Handler"           # Find all handlers
ast-index search "Service"          # Find services
```

### Search Functions

Find function definitions including async handlers:

```bash
ast-index symbol "handle"           # Find handle functions
ast-index search "async"            # Find async functions
ast-index callers "process"         # Find callers of process
```

### Search Imports

Find imports and module usage:

```bash
ast-index search "logging"          # Find logging usage
ast-index usages "db"               # Find db module usages
```

## Example Workflow

```bash
# 1. Index Python service
cd /path/to/python/service
ast-index rebuild

# 2. Check index statistics
ast-index stats

# 3. Find all handlers
ast-index symbol "handle"

# 4. Find specific class
ast-index class "Context"

# 5. Show file structure
ast-index outline "api/handler.py"

# 6. Find usages
ast-index usages "Context"
```

## Yandex Python Patterns

### Taxi Backend Service Handler

```python
async def handle(
    request: requests.AdminGet,
    context: web_context.Context,
) -> responses.ADMIN_GET_RESPONSES:
    settings = await db.get_all_settings(context)
    return responses.AdminGet200(data=settings)
```

Indexed as:
- `handle` [function]
- `requests.AdminGet` [import usage]
- `web_context.Context` [import usage]

### Service Configuration

```python
from driver_referrals.common import db
from driver_referrals.generated.service.swagger import requests

logger = logging.getLogger(__name__)
```

Indexed as:
- `db` [import]
- `requests` [import]
- `driver_referrals.common` [import]

## File Extensions

Supported extensions:
- `.py` - Python source

## Performance

| Operation | Time |
|-----------|------|
| Rebuild (300 Python files) | ~2s |
| Search class | ~3ms |
| Find usages | ~10ms |

## Decorators

The parser tracks significant decorators:

- `@pytest.fixture` - Test fixtures
- `@pytest.mark.*` - Test markers
- `@dataclass` - Data classes
- `@property` - Properties
- `@route` / `@handler` - Web handlers

```bash
ast-index search "@pytest"          # Find test decorators
ast-index search "@dataclass"       # Find dataclasses
```

## Limitations

**Supported:**
- Class definitions with inheritance
- Functions and async functions
- Import statements
- Module-level constants (UPPER_CASE)
- Type aliases
- Significant decorators

**Not supported:**
- Method detection (all `def` indexed as functions)
- Comprehension expressions
- Lambda functions
- Dynamic imports (`__import__`)
- Conditional imports (`if TYPE_CHECKING:`)
