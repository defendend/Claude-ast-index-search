# C# Commands Reference

ast-index supports parsing and indexing C# source files (`.cs`).

## Supported Elements

| C# Element | Symbol Kind | Example |
|------------|-------------|---------|
| `namespace Name` | Package | `MyApp.Services` |
| `class ClassName` | Class | `UserService` |
| `interface IName` | Interface | `IRepository<T>` |
| `struct StructName` | Class | `Point` |
| `record RecordName` | Class | `Person` |
| `enum EnumName` | Enum | `Status` |
| `ReturnType Method()` | Function | `GetUserAsync` |
| `Type Property { get; }` | Property | `Name`, `Id` |
| `_fieldName` | Property | `_logger` |
| `const TYPE NAME` | Constant | `MAX_SIZE` |
| `delegate Type Name` | TypeAlias | `EventHandler` |
| `event Type Name` | Property | `OnDataReceived` |
| `using Namespace` | Import | `using System` |
| `[Attribute]` | Annotation | `[ApiController]` |

## Core Commands

### Search Classes and Interfaces

```bash
ast-index class "Service"           # Find service classes
ast-index class "Controller"        # Find controllers
ast-index class "IRepository"       # Find repository interfaces
ast-index search "Record"           # Find record types
```

### Search Methods

```bash
ast-index symbol "GetUser"          # Find GetUser methods
ast-index symbol "Async"            # Find async methods
ast-index callers "ProcessOrder"    # Find method callers
```

### Search Properties and Fields

```bash
ast-index search "{ get;"           # Find properties
ast-index search "_logger"          # Find private fields
```

### File Analysis

```bash
ast-index outline "UserService.cs"  # Show file structure
ast-index imports "Program.cs"      # Show using statements
```

## ASP.NET Core Commands

```bash
ast-index search "[ApiController]"  # Find API controllers
ast-index search "[HttpGet]"        # Find GET endpoints
ast-index search "[HttpPost]"       # Find POST endpoints
ast-index search "[Authorize]"      # Find authorized endpoints
ast-index search "[Route"           # Find route definitions
```

## Unity Commands

```bash
ast-index search "MonoBehaviour"    # Find Unity scripts
ast-index search "ScriptableObject" # Find scriptable objects
ast-index search "[SerializeField]" # Find serialized fields
ast-index search "Start"            # Find Start methods
ast-index search "Update"           # Find Update methods
```

## Test Framework Commands

```bash
# xUnit
ast-index search "[Fact]"           # Find test methods
ast-index search "[Theory]"         # Find parameterized tests

# NUnit
ast-index search "[Test]"           # Find NUnit tests
ast-index search "[TestCase]"       # Find test cases

# MSTest
ast-index search "[TestMethod]"     # Find MSTest methods
```

## Example Workflow

```bash
# 1. Index .NET project
cd /path/to/dotnet/project
ast-index rebuild

# 2. Check index statistics
ast-index stats

# 3. Find all controllers
ast-index class "Controller"

# 4. Find all interfaces
ast-index class "I"

# 5. Find implementations
ast-index implementations "IUserRepository"

# 6. Show file structure
ast-index outline "Services/UserService.cs"

# 7. Find usages
ast-index usages "UserService"
```

## Indexed C# Patterns

### Class with Inheritance

```csharp
public class UserService : BaseService, IUserService
{
    private readonly ILogger _logger;

    public UserService(ILogger logger)
    {
        _logger = logger;
    }

    public async Task<User> GetUserAsync(int id)
    {
        return await _repository.GetByIdAsync(id);
    }
}
```

Indexed as:
- `UserService` [class] extends `BaseService`, implements `IUserService`
- `_logger` [property]
- `GetUserAsync` [function]

### Interface

```csharp
public interface IRepository<T> : IDisposable
{
    T GetById(int id);
    void Save(T entity);
}
```

Indexed as:
- `IRepository` [interface] extends `IDisposable`
- `GetById` [function]
- `Save` [function]

### Record

```csharp
public record Person(string FirstName, string LastName);

public record Employee(string Name, string Department)
    : Person(Name, "");
```

Indexed as:
- `Person` [class]
- `Employee` [class] extends `Person`

### ASP.NET Controller

```csharp
[ApiController]
[Route("api/[controller]")]
public class UsersController : ControllerBase
{
    [HttpGet("{id}")]
    public async Task<ActionResult<User>> GetById(int id)
    {
        return await _service.GetByIdAsync(id);
    }

    [HttpPost]
    [Authorize]
    public async Task<ActionResult<User>> Create(UserDto dto)
    {
        return await _service.CreateAsync(dto);
    }
}
```

Indexed as:
- `[ApiController]` [annotation]
- `UsersController` [class]
- `[HttpGet]` [annotation]
- `GetById` [function]
- `[HttpPost]` [annotation]
- `[Authorize]` [annotation]
- `Create` [function]

### Unity MonoBehaviour

```csharp
public class PlayerController : MonoBehaviour
{
    [SerializeField] private float _speed;
    [Header("Settings")]
    public int health = 100;

    void Start()
    {
        Initialize();
    }

    void Update()
    {
        HandleInput();
    }
}
```

Indexed as:
- `[SerializeField]` [annotation]
- `PlayerController` [class]
- `_speed` [property]
- `Start` [function]
- `Update` [function]

## Namespace Handling

Both namespace styles are supported:

```csharp
// Traditional
namespace MyApp.Services
{
    public class UserService { }
}

// File-scoped (C# 10+)
namespace MyApp.Services;

public class UserService { }
```

## Performance

| Operation | Time |
|-----------|------|
| Rebuild (500 C# files) | ~500ms |
| Search class | ~1ms |
| Find usages | ~5ms |
| Find implementations | ~5ms |
| File outline | ~1ms |
