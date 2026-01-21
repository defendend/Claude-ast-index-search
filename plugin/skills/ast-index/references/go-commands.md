# Go Commands Reference

ast-index supports parsing and indexing Go source files (`.go`).

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

### File Analysis

```bash
ast-index outline "handler.go"      # Show structs, interfaces, functions
ast-index imports "service.go"      # Show imports (including import blocks)
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

## Go Patterns

### Action Pattern

```go
type DeleteAction struct {
    avaSrv        AvatarsMDS
    tmpStorageSrv TmpStorage
    filesRepo     FilesRepo
}

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

## Import Handling

Imports are tracked with their full path:

```go
import (
    "context"
    "fmt"

    "a.yandex-team.ru/taxi/backend-go/services/eats-files-uploads/internal/entities"
)
```

```bash
ast-index imports "handler.go"      # Shows all imports with aliases
ast-index usages "context"          # Find context usage
```

## Performance

| Operation | Time |
|-----------|------|
| Rebuild (100 Go files) | ~250ms |
| Search class | ~1ms |
| Find usages | ~5ms |
