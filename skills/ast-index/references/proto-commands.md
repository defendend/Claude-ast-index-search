# Protocol Buffers Commands Reference

ast-index supports parsing and indexing Protocol Buffer files (`.proto`) for both proto2 and proto3 syntax.

## Supported Elements

| Proto Element | Symbol Kind | Example |
|---------------|-------------|---------|
| `message Name` | Class | `UserRequest` → Class |
| `enum Name` | Enum | `Status` → Enum |
| `service Name` | Interface | `UserService` → Interface |
| `rpc Method()` | Function | `GetUser` → Function |
| `import "file.proto"` | Import | `common.proto` → Import |
| `package name` | Package | `api.v1` → Package |
| `field type name = N` | Property | `user_id` → Property |
| `oneof name` | Property | `payload` → Property |

## Core Commands

### Search Messages

Find message definitions:

```bash
ast-index class "Request"           # Find request messages
ast-index class "Response"          # Find response messages
ast-index search "User"             # Find user-related types
```

### Search Services

Find gRPC service definitions:

```bash
ast-index search "Service"          # Find all services
ast-index class "UserService"       # Find specific service
```

### Search RPC Methods

```bash
ast-index symbol "Get"              # Find Get* methods
ast-index symbol "Create"           # Find Create* methods
ast-index usages "GetUser"          # Find RPC usages
```

### Search Enums

```bash
ast-index search "enum"             # Find all enums
ast-index class "Status"            # Find status enum
```

### File Analysis

```bash
ast-index outline "user.proto"      # Show messages, services, enums
ast-index imports "api.proto"       # Show import statements
```

## Example Workflow

```bash
# 1. Index proto files
cd /path/to/proto/project
ast-index rebuild

# 2. Check index statistics
ast-index stats

# 3. Find all messages
ast-index search "message"

# 4. Find service definitions
ast-index search "Service"

# 5. Show proto file structure
ast-index outline "api/v1/user.proto"

# 6. Find usages of message
ast-index usages "UserRequest"
```

## Proto Patterns

### Message Definition (proto3)

```protobuf
syntax = "proto3";

package api.v1;

message UserRequest {
    string user_id = 1;
    repeated string fields = 2;

    oneof filter {
        string name = 3;
        int32 age = 4;
    }
}

message UserResponse {
    User user = 1;
    Status status = 2;
}
```

Indexed as:
- `api.v1` [package]
- `UserRequest` [class]
- `user_id` [property]
- `fields` [property]
- `filter` [property] (oneof)
- `UserResponse` [class]

### Service Definition

```protobuf
service UserService {
    rpc GetUser(UserRequest) returns (UserResponse);
    rpc CreateUser(CreateUserRequest) returns (UserResponse);
    rpc ListUsers(ListUsersRequest) returns (stream UserResponse);
}
```

Indexed as:
- `UserService` [interface]
- `GetUser` [function] with parent `UserService`
- `CreateUser` [function] with parent `UserService`
- `ListUsers` [function] with parent `UserService`

### Enum Definition

```protobuf
enum Status {
    STATUS_UNSPECIFIED = 0;
    STATUS_ACTIVE = 1;
    STATUS_INACTIVE = 2;
}
```

Indexed as:
- `Status` [enum]
- `STATUS_UNSPECIFIED`, `STATUS_ACTIVE`, `STATUS_INACTIVE` [property]

## Import Handling

```protobuf
import "google/protobuf/timestamp.proto";
import "common/types.proto";
import public "shared.proto";
```

```bash
ast-index imports "service.proto"   # Shows all imports
ast-index usages "common/types.proto"  # Find where proto is imported
```

## Performance

| Operation | Time |
|-----------|------|
| Rebuild (50 proto files) | ~100ms |
| Search message | ~1ms |
| Find usages | ~3ms |
| File outline | ~1ms |
