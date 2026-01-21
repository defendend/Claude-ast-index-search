# Go Commands Reference

ast-index supports parsing and indexing Go source files (`.go`), with focus on backend services and microservices patterns.

## Supported Elements

| Go Element | Symbol Kind | Example |
|------------|-------------|---------|
| `package name` | Package | `main` → Package |
| `type Name struct` | Class | `DeleteAction` → Class |
| `type Name interface` | Interface | `Repository` → Interface |
| `func Name()` | Function | `NewService` → Function |
| `func (r *T) Method()` | Function | `Do` → Function (with receiver) |
| `import "module"` | Import | `context` → Import |
| `const Name = value` | Constant | `MaxRetries` → Constant |
| `type Name = OtherType` | TypeAlias | `Handler` → TypeAlias |
| `var Name Type` | Property | `Logger` → Property |

## Core Commands

### Search Types

Find struct and interface definitions:

```bash
ast-index class "Service"           # Find service structs
ast-index class "Action"            # Find action structs
ast-index search "Repository"       # Find repositories
```

### Search Interfaces

Find interface definitions:

```bash
ast-index search "interface"        # Find all interfaces
ast-index class "Handler"           # Find handler types
```

### Search Functions

Find functions and methods:

```bash
ast-index symbol "New"              # Find constructor functions
ast-index symbol "Do"               # Find Do methods
ast-index callers "Handle"          # Find Handle callers
```

### Search Methods

Methods are indexed with their receiver type as parent:

```bash
ast-index search "DeleteAction"     # Find DeleteAction and its methods
ast-index symbol "Do"               # Find Do method (shows receiver)
```

## Example Workflow

```bash
# 1. Index Go service
cd /path/to/go/service
ast-index rebuild

# 2. Check index statistics
ast-index stats

# 3. Find all structs
ast-index search "struct"

# 4. Find constructor functions
ast-index symbol "New"

# 5. Show file structure
ast-index outline "internal/handler.go"

# 6. Find usages
ast-index usages "Service"
```

## Yandex Go Patterns

### Action Pattern

```go
type DeleteAction struct {
    avaSrv        AvatarsMDS
    tmpStorageSrv TmpStorage
    filesRepo     FilesRepo
}

// di:new
func NewDeleteAction(
    avaSrv *avatarsmds.Service,
    tmpStorageSrv *tmpstorage.FileUploader,
    storage *repositories.Storage,
) *DeleteAction {
    return &DeleteAction{...}
}

func (a *DeleteAction) Do(ctx context.Context, task *entities.TaskToProcess) error {
    // ...
}
```

Indexed as:
- `DeleteAction` [class]
- `NewDeleteAction` [function]
- `Do` [function] with parent `DeleteAction`

### Interface Definition

```go
type AvatarsMDS interface {
    Delete(ctx context.Context, groupID int, name string) error
    Upload(ctx context.Context, data []byte) (int, error)
}
```

Indexed as: `AvatarsMDS` [interface]

### Repository Pattern

```go
type Storage struct {
    db *sql.DB
}

func NewStorage(db *sql.DB) *Storage {
    return &Storage{db: db}
}

func (s *Storage) FilesRepo() FilesRepository {
    return &filesRepository{db: s.db}
}
```

Indexed as:
- `Storage` [class]
- `NewStorage` [function]
- `FilesRepo` [function] with parent `Storage`

## File Extensions

Supported extensions:
- `.go` - Go source

## Performance

| Operation | Time |
|-----------|------|
| Rebuild (100 Go files) | ~250ms |
| Search class | ~1ms |
| Find usages | ~5ms |

## Import Handling

Imports are tracked with their full path:

```go
import (
    "context"
    "fmt"

    "a.yandex-team.ru/taxi/backend-go/services/eats-files-uploads/internal/entities"
)
```

Indexed as:
- `context` [import] from "context"
- `fmt` [import] from "fmt"
- `entities` [import] from "a.yandex-team.ru/.../entities"

```bash
ast-index search "entities"         # Find entities package usage
ast-index usages "context"          # Find context usage
```

## Limitations

**Supported:**
- Package declarations
- Struct definitions
- Interface definitions
- Functions and methods
- Import statements (single and block)
- Constants (exported only)
- Package-level variables (exported only)
- Type aliases

**Not supported:**
- Embedded structs tracking
- Interface embedding
- Generic type parameters (Go 1.18+)
- Build tags detection
- Test function classification
