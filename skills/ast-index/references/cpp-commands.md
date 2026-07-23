# C/C++ Commands Reference

ast-index supports parsing and indexing C/C++ source files (`.cpp`, `.c`, `.h`, `.hpp`, `.cc`, `.cxx`).

## Supported Elements

| C/C++ Element | Symbol Kind | Example |
|---------------|-------------|---------|
| `class ClassName` | Class | `FileHandler` → Class |
| `struct Name` | Class | `RequestData` → Class |
| `namespace Name` | Namespace | `utils` → Namespace |
| `void function()` | Function | `ProcessRequest` → Function |
| `void Class::method()` | Function | `Handle` → Function (with parent) |
| `#include "header"` | Import | `handler.h` → Import |
| `typedef` | TypeAlias | `Handler` → TypeAlias |
| `enum Name` | Enum | `Status` → Enum |
| `#define MACRO` | Macro | `MAX_SIZE` → Macro |

## Core Commands

### Search Classes and Structs

Find class and struct definitions:

```bash
ast-index class "Handler"           # Find handler classes
ast-index class "Request"           # Find request structs
ast-index search "Service"          # Find service classes
```

### Search Functions

Find functions and methods:

```bash
ast-index symbol "Process"          # Find process functions
ast-index symbol "Handle"           # Find handle methods
ast-index callers "Init"            # Find Init callers
```

### Search Namespaces

```bash
ast-index search "namespace"        # Find all namespaces
ast-index symbol "utils"            # Find utils namespace
```

### File Analysis

```bash
ast-index outline "handler.cpp"     # Show classes, functions, structs
ast-index imports "service.cpp"     # Show #include statements
```

## Example Workflow

```bash
# 1. Index C++ project
cd /path/to/cpp/project
ast-index rebuild

# 2. Check index statistics
ast-index stats

# 3. Find all classes
ast-index search "class"

# 4. Find header usages
ast-index usages "RequestHandler"

# 5. Show file structure
ast-index outline "src/handler.cpp"
```

## C++ Patterns

### Class Definition

```cpp
namespace myapp {

class RequestHandler {
public:
    RequestHandler(Database* db);
    ~RequestHandler();

    bool Handle(const Request& req, Response* resp);

private:
    Database* db_;
};

}  // namespace myapp
```

Indexed as:
- `myapp` [namespace]
- `RequestHandler` [class]
- `RequestHandler` [function] (constructor)
- `~RequestHandler` [function] (destructor)
- `Handle` [function] with parent `RequestHandler`

### JNI Functions

```cpp
extern "C" JNIEXPORT jstring JNICALL
Java_com_example_MyClass_nativeMethod(JNIEnv* env, jobject obj) {
    return env->NewStringUTF("Hello from JNI");
}
```

Indexed as: `Java_com_example_MyClass_nativeMethod` [function]

Find JNI functions:

```bash
ast-index symbol "Java_"            # Find all JNI functions
ast-index search "JNICALL"          # Find JNI exports
```

## Include Handling

Both include styles are tracked:

```cpp
#include <iostream>
#include <vector>
#include "myheader.h"
#include "utils/helpers.h"
```

```bash
ast-index imports "main.cpp"        # Shows all includes
ast-index usages "myheader.h"       # Find where header is included
```

## Performance

| Operation | Time |
|-----------|------|
| Rebuild (200 C++ files) | ~400ms |
| Search class | ~1ms |
| Find usages | ~5ms |
| File outline | ~1ms |
