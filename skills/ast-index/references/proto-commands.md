# Protocol Buffers Commands Reference

ast-index supports parsing and indexing Protocol Buffers files (`.proto`), including both proto2 and proto3 syntax.

## Supported Proto Elements

| Proto Element | Symbol Kind | Example |
|--------------|-------------|---------|
| `message` | Class | `message UserRequest` → Class |
| Nested `message` | Class | `TRequest.TItem` → Class with parent |
| `service` | Interface | `service UserService` → Interface |
| `rpc` | Function | `rpc GetUser(...)` → Function |
| `enum` | Enum | `enum Status` → Enum |
| `package` | Package | `package api.v1` → Package |
| `option java_package` | Property | Cross-reference with Java |

## Core Commands for Proto Files

### Search Messages

Find proto message definitions (indexed as classes):

```bash
ast-index class "UserRequest"           # Find message by name
ast-index class "TChangeAgency"         # Find messages starting with T
```

### Search Services

Find service definitions (indexed as interfaces):

```bash
ast-index search "Service"              # Find all services
ast-index class "CampaignService"       # Find specific service
```

### Search RPC Methods

Find RPC method definitions (indexed as functions):

```bash
ast-index symbol "GetCampaign"          # Find RPC by name
ast-index callers "GetCampaign"         # Find where RPC is called
```

### Search Enums

Find enum definitions:

```bash
ast-index search "Status"               # Find status enums
ast-index class "EChangeAgencyResult"   # Find specific enum
```

### Find Usages

Find all places where a proto message is used:

```bash
ast-index usages "UserRequest"          # Find message usages
ast-index usages "CampaignService"      # Find service usages
```

## Proto2 vs Proto3

ast-index supports both proto2 and proto3 syntax:

**proto2 features:**
- `optional`, `required`, `repeated` field modifiers
- `extensions` and `extend` declarations
- Default values with `default = value`

**proto3 features:**
- `syntax = "proto3";` declaration
- Implicit `optional` (no `required`)
- Built-in JSON mapping annotations
- Google API annotations (`google.api.http`)

## Nested Messages

Nested messages are indexed with their full path:

```protobuf
message TRequest {
    message TItem {       // Indexed as "TRequest.TItem"
        uint64 id = 1;
    }
}
```

```bash
# Search for nested message
ast-index class "TRequest.TItem"

# Or search by short name (will find if unique)
ast-index class "TItem"
```

## Java Package Cross-Reference

Proto files often specify Java package for code generation:

```protobuf
option java_package = "ru.yandex.direct.api";
```

These are indexed and can be searched:

```bash
ast-index search "java_package:ru.yandex.direct"
```

## Example Workflow

```bash
# 1. Index proto directory
cd /path/to/proto/files
ast-index rebuild

# 2. Find all messages
ast-index class ""

# 3. Find specific message
ast-index class "TChangeAgencyRequest"

# 4. Find nested messages
ast-index search "TChangeAgencyRequest"

# 5. Check file structure
ast-index outline "agency_change_request.proto"

# 6. Find all services
ast-index search "Service"
```

## Performance

| Operation | Time |
|-----------|------|
| Rebuild (100 proto files) | ~200ms |
| Search message | ~2ms |
| Find usages | ~10ms |

## Limitations

Current implementation:
- Does not parse field definitions (only message/service/rpc/enum)
- Does not resolve imports across files
- Does not validate proto syntax
- Does not index comments/documentation

## File Structure

Proto files are detected by `.proto` extension. No special project markers required.

Typical directory structure:
```
project/
├── api/v1/
│   ├── user.proto
│   └── campaign.proto
└── internal/
    └── types.proto
```
