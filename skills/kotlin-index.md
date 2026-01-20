# kotlin-index v3.3.0 - Code Search for Mobile Projects

Fast native Rust CLI for code search in Android/Kotlin/Java and iOS/Swift/ObjC projects using SQLite + FTS5.

## Prerequisites

Install the CLI:
```bash
brew tap defendend/kotlin-index
brew install kotlin-index
```

Initialize index in project root:
```bash
cd /path/to/project
kotlin-index rebuild
```

## Supported Projects

| Platform | Languages | Module System |
|----------|-----------|---------------|
| Android | Kotlin, Java | Gradle (build.gradle.kts) |
| iOS | Swift, Objective-C | SPM (Package.swift) |
| Mixed | All above | Both |

Project type is auto-detected by marker files.

## Available Commands (34 total)

### Search Commands

**Universal search** (files + symbols + modules):
```bash
kotlin-index search "PaymentMethod"
```

**Find files by name**:
```bash
kotlin-index file "Fragment.kt"
kotlin-index file "ViewController.swift"
```

**Find symbols** (classes, interfaces, functions):
```bash
kotlin-index symbol "PaymentInteractor"
kotlin-index symbol "AppDelegate"
```

**Find class/interface/protocol**:
```bash
kotlin-index class "BaseFragment"
kotlin-index class "UIApplicationDelegate"  # Swift protocol
```

**Find usages** of a symbol (~8ms indexed):
```bash
kotlin-index usages "PaymentRepository"
```

**Find implementations** (subclasses/implementors/protocol conformance):
```bash
kotlin-index implementations "BasePresenter"
kotlin-index implementations "Codable"  # Swift protocol
```

**Class hierarchy** (parents + children):
```bash
kotlin-index hierarchy "BaseFragment"
```

**Find callers** of a function:
```bash
kotlin-index callers "onClick"
kotlin-index callers "viewDidLoad"
```

**File imports**:
```bash
kotlin-index imports "path/to/File.kt"
kotlin-index imports "path/to/File.swift"
```

### DI Commands (Dagger - Android only)

**Find @Provides/@Binds** for a type:
```bash
kotlin-index provides "UserRepository"
```

**Find @Inject** points for a type:
```bash
kotlin-index inject "UserInteractor"
```

**Find classes with annotation**:
```bash
kotlin-index annotations "Module"
kotlin-index annotations "Inject"
```

### Audit Commands

**Find TODO/FIXME/HACK**:
```bash
kotlin-index todo
kotlin-index todo "FIXME"
```

**Find @Deprecated** items:
```bash
kotlin-index deprecated
```

**Find @Suppress** annotations:
```bash
kotlin-index suppress
kotlin-index suppress "UNCHECKED_CAST"
```

**Find extension functions** (Kotlin) / **extensions** (Swift):
```bash
kotlin-index extensions "String"
kotlin-index extensions "View"
```

**Show public API** of a module:
```bash
kotlin-index api "features/payments/api"
```

**Find deeplinks**:
```bash
kotlin-index deeplinks
kotlin-index deeplinks "payment"
```

**Show changed symbols** (git diff):
```bash
kotlin-index changed
kotlin-index changed --base "origin/main"
```

### Compose Commands (Android)

**Find @Composable functions**:
```bash
kotlin-index composables
kotlin-index composables "Button"
```

**Find @Preview functions**:
```bash
kotlin-index previews
```

### Coroutines Commands (Kotlin)

**Find suspend functions**:
```bash
kotlin-index suspend
kotlin-index suspend "fetch"
```

**Find Flow/StateFlow/SharedFlow**:
```bash
kotlin-index flows
kotlin-index flows "user"
```

### Module Commands

**Find modules** (Gradle or SPM):
```bash
kotlin-index module "payments"
kotlin-index module "NetworkKit"
```

**Module dependencies**:
```bash
kotlin-index deps "features.payments.impl"
```

**Modules depending on this module**:
```bash
kotlin-index dependents "features.payments.api"
```

**Find unused dependencies** (with transitive, XML, resource checks):
```bash
kotlin-index unused-deps "features.payments.impl"
kotlin-index unused-deps "features.payments.impl" --verbose
kotlin-index unused-deps "features.payments.impl" --strict  # only direct imports
```

### XML & Resource Commands (Android only)

**Find class usages in XML layouts**:
```bash
kotlin-index xml-usages "PaymentIconView"
kotlin-index xml-usages "ImageView" --module "features.payments.impl"
```

**Find resource usages**:
```bash
kotlin-index resource-usages "@drawable/ic_payment"
kotlin-index resource-usages "R.string.payment_title"
```

**Find unused resources in module**:
```bash
kotlin-index resource-usages --unused --module "features.payments.impl"
```

### File Structure

**File outline** (classes, functions in file):
```bash
kotlin-index outline "path/to/File.kt"
kotlin-index outline "path/to/File.swift"
```

### Index Management

**Rebuild index** (includes module dependencies, XML, resources by default):
```bash
kotlin-index rebuild
kotlin-index rebuild --no-deps  # skip module dependencies
```

**Update index** (incremental):
```bash
kotlin-index update
```

**Index statistics**:
```bash
kotlin-index stats
```

## Swift/ObjC Support (v3.3.0)

### Indexed Swift Constructs
- `class`, `struct`, `enum`, `protocol`, `actor`
- `extension` (indexed as `TypeName+Extension`)
- `func`, `init`, `var`, `let`, `typealias`
- Inheritance and protocol conformance

### Indexed ObjC Constructs
- `@interface` with superclass and protocols
- `@protocol` definitions
- `@implementation`
- Methods (`-`/`+`), `@property`, `typedef`
- Categories (indexed as `TypeName+Category`)

### SPM Module Detection
Parses `Package.swift` and extracts:
- `.target(name: "...")`
- `.testTarget(name: "...")`
- `.binaryTarget(name: "...")`

## Performance

| Command | Time |
|---------|------|
| search | ~10ms |
| class | ~1ms |
| usages | ~8ms (indexed) |
| imports | ~0.3ms |
| deps/dependents | ~2ms |
| unused-deps | ~12s |
| xml-usages | ~1ms |
| resource-usages | ~2ms |
| todo | ~0.8s (grep) |

## Index Location

Database stored at: `~/.cache/kotlin-index/<project-hash>/index.db`
