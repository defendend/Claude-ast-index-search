# C/C++ Commands Reference

ast-index supports parsing and indexing C and C++ source files, with focus on JNI bindings and modern C++ (C++11/14/17).

## Supported Elements

| C++ Element | Symbol Kind | Example |
|-------------|-------------|---------|
| `class ClassName` | Class | `TJavaException` → Class |
| `struct StructName` | Class | `TData` → Class |
| `template<...> class` | Class | `TJniReference<T>` → Class |
| `JNIEXPORT ... JNICALL Java_...` | Function | `analyze2` → Function |
| `template<...> func()` | Function | `jniWrapExceptions` → Function |
| `namespace name` | Package | `NDirect` → Package |
| `enum class Name` | Enum | `Color` → Enum |
| `typedef ... Name` | TypeAlias | `StringType` → TypeAlias |
| `using Name = ...` | TypeAlias | `Callback` → TypeAlias |
| `#define MACRO(...)` | Constant | `MAX_SIZE` → Constant |

## JNI Support

JNI (Java Native Interface) functions are automatically detected:

```cpp
JNIEXPORT jobject JNICALL Java_ru_yandex_direct_textprocessing_TextProcessing_analyze2
  (JNIEnv *, jclass, jstring, jint, jboolean);
```

Indexed as: `analyze2 [function]`

The parser extracts the method name from the full JNI signature.

## Core Commands

### Search Classes

Find class and struct definitions:

```bash
ast-index class "TJavaException"      # Find specific class
ast-index class "Reference"           # Find classes containing "Reference"
ast-index search "Converter"          # Find all converters
```

### Search Functions

Find function definitions including JNI exports:

```bash
ast-index symbol "analyze"            # Find analyze functions
ast-index callers "jniWrapExceptions" # Find callers
```

### Search Namespaces

Find namespace definitions:

```bash
ast-index search "NDirect"            # Find NDirect namespace
```

### Search with Inheritance

Find classes with inheritance:

```bash
ast-index class "TJniClass"           # Shows inheritance from TJniReference
ast-index implementations "TNonCopyable" # Find all implementations
```

## Example Workflow

```bash
# 1. Index C++ directory
cd /path/to/cpp/files
ast-index rebuild

# 2. Check index statistics
ast-index stats

# 3. Find JNI functions
ast-index symbol "Java_"

# 4. Find specific class
ast-index class "TJniReference"

# 5. Show file structure
ast-index outline "util.h"

# 6. Find usages
ast-index usages "TJavaException"
```

## Yandex C++ Patterns

### RAII Wrapper Classes

```cpp
template<class T>
class TJniReference : public TNonCopyable {
    JNIEnv* env_;
    T value_;
public:
    TJniReference(JNIEnv* env, T value);
    T Get() const;
    void Reset();
};
```

Indexed as: `TJniReference [class]` with parent `TNonCopyable`

### Exception Wrapper

```cpp
template<class Func>
inline auto jniWrapExceptions(JNIEnv* env, Func&& func) {
    try { return func(); }
    catch (const std::exception& e) { ... }
}
```

Indexed as: `jniWrapExceptions [function]`

### JNI Class Helper

```cpp
class TJniClass : public TJniReference<jclass> {
public:
    TJniClass(JNIEnv* env, const char* name);
    jmethodID GetMethodID(const char* name, const char* sig);
};
```

Indexed as: `TJniClass [class]` with parent `TJniReference`

## File Extensions

Supported extensions:
- `.cpp` - C++ source
- `.cc` - C++ source (alternative)
- `.c` - C source
- `.h` - C/C++ header
- `.hpp` - C++ header

## Performance

| Operation | Time |
|-----------|------|
| Rebuild (10 C++ files) | ~60ms |
| Search class | ~2ms |
| Find usages | ~10ms |

## Limitations

Current implementation focuses on simple patterns:

**Supported:**
- Basic class/struct definitions
- Template class declarations
- JNI function exports
- Template functions
- Single inheritance
- Namespaces
- Enums (including enum class)
- Typedefs and using aliases
- Function-like macros

**Not supported:**
- Complex template metaprogramming
- Multiple inheritance tracking
- Preprocessor conditionals (#ifdef blocks)
- Operator overloading detection
- Nested class detection
- Lambda expressions as symbols

## Forward Declarations

Forward declarations are automatically skipped:

```cpp
class Foo;      // Skipped (forward declaration)
struct Bar;     // Skipped (forward declaration)

class Foo {     // Indexed
    // ...
};
```
