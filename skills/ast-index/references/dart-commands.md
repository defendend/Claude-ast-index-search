# Dart/Flutter Commands Reference

ast-index supports parsing and indexing Dart source files (`.dart`).

## Supported Elements

| Dart Element | Symbol Kind | Example |
|--------------|-------------|---------|
| `class ClassName` | Class | `MyWidget`, `AppState` |
| `abstract class` | Class | `BaseRepository` |
| `sealed class` | Class | `Result` |
| `final class` | Class | `ImmutableConfig` |
| `base class` | Class | `BaseService` |
| `interface class` | Interface | `AppScope` |
| `mixin MixinName` | Interface | `ScrollMixin` |
| `extension ExtName on Type` | Object | `StringExtension` |
| `extension type Name(Type _)` | Class | `UserId` |
| `enum EnumName` | Enum | `Status`, `Theme` |
| `typedef Name = ...` | TypeAlias | `JsonMap` |
| `ReturnType funcName(params)` | Function | `build`, `initState` |
| `ClassName(params)` | Function | Constructor |
| `factory ClassName.named()` | Function | Named constructor |
| `get propertyName` | Function | Getter |
| `set propertyName` | Function | Setter |
| `final/const/late/var` | Property | Top-level variables |
| `import '...'` | Import | Package imports |
| `export '...'` | Import | Re-exports |

## Dart 3 Class Modifiers

All Dart 3 class modifiers are supported:

```dart
abstract class Animal {}           // Class
sealed class Shape {}              // Class
final class Config {}              // Class
base class BaseWidget {}           // Class
interface class Disposable {}      // Interface
mixin class Loggable {}            // Class
abstract interface class Scope {}  // Interface
abstract base class Entity {}      // Class
```

## Inheritance Tracking

Parent relationships are extracted from `extends`, `with`, and `implements`:

```dart
class MyWidget extends StatefulWidget with TickerProviderMixin implements Disposable {}
```

Indexed parents:
- `StatefulWidget` (extends)
- `TickerProviderMixin` (with)
- `Disposable` (implements)

Mixin parents from `on` and `implements`:

```dart
mixin ScrollMixin on State implements TickerProvider {}
```

Indexed parents:
- `State` (on)
- `TickerProvider` (implements)

## Core Commands

### Search Classes

```bash
ast-index class "Widget"            # Find widget classes
ast-index class "State"             # Find State subclasses
ast-index class "Provider"          # Find provider classes
ast-index search "Repository"       # Find repositories
```

### Search Mixins

```bash
ast-index search "mixin"            # Find all mixins
ast-index class "Mixin"             # Find mixin classes
```

### Find Implementations

```bash
ast-index implementations "StatelessWidget"  # Find all stateless widgets
ast-index implementations "ChangeNotifier"   # Find all notifiers
ast-index implementations "Bloc"             # Find all BLoC classes
```

### Class Hierarchy

```bash
ast-index hierarchy "Widget"        # Show widget inheritance tree
ast-index hierarchy "State"         # Show state hierarchy
```

### Find Usages

```bash
ast-index usages "BuildContext"     # Find BuildContext usages
ast-index usages "Navigator"        # Find navigation calls
ast-index usages "Provider"         # Find provider usages
```

### Search Functions

```bash
ast-index symbol "build"            # Find build methods
ast-index symbol "initState"        # Find initState overrides
ast-index callers "dispose"         # Find dispose callers
```

### File Analysis

```bash
ast-index outline "main.dart"       # Show file structure
ast-index imports "app.dart"        # Show import statements
```

## Example Workflow

```bash
# 1. Index Flutter project
cd /path/to/flutter/app
ast-index rebuild

# 2. Check index statistics
ast-index stats

# 3. Find all widgets
ast-index class "Widget"

# 4. Find all BLoC implementations
ast-index implementations "Bloc"

# 5. Find usages of a class
ast-index usages "UserRepository"

# 6. Show widget structure
ast-index outline "lib/src/widgets/custom_button.dart"

# 7. Find all screens
ast-index class "Screen"
```

## Indexed Dart Patterns

### StatefulWidget

```dart
class CounterWidget extends StatefulWidget {
  const CounterWidget({super.key});

  @override
  State<CounterWidget> createState() => _CounterWidgetState();
}

class _CounterWidgetState extends State<CounterWidget> {
  int _count = 0;

  @override
  Widget build(BuildContext context) {
    return Text('$_count');
  }
}
```

Indexed as:
- `CounterWidget` [class] extends `StatefulWidget`
- `_CounterWidgetState` [class] extends `State`
- `createState` [function]
- `build` [function]

### Enhanced Enum

```dart
enum Status implements Comparable<Status> {
  active,
  inactive;

  @override
  int compareTo(Status other) => index - other.index;
}
```

Indexed as:
- `Status` [enum] implements `Comparable`
- `compareTo` [function]

### Extension

```dart
extension StringExtension on String {
  String capitalize() => '${this[0].toUpperCase()}${substring(1)}';
}
```

Indexed as:
- `StringExtension` [object] on `String`
- `capitalize` [function]

### Extension Type (Dart 3.3)

```dart
extension type UserId(int id) {
  UserId.fromString(String s) : id = int.parse(s);
}
```

Indexed as:
- `UserId` [class]
- `UserId.fromString` [function]

### Mixin

```dart
mixin Loggable on Object {
  void log(String message) => print('[$runtimeType] $message');
}
```

Indexed as:
- `Loggable` [interface] on `Object`
- `log` [function]

## Import Handling

Both `import` and `export` statements are tracked:

```dart
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../models/user.dart';
export 'src/widgets.dart';
```

Use `ast-index imports "file.dart"` to see all imports.

## Performance

| Operation | Time |
|-----------|------|
| Rebuild (2700 Dart files) | ~3s |
| Search class | ~1ms |
| Find usages | ~5ms |
| File outline | ~1ms |
