# ast-index v3.5.0 - Code Search for Mobile Projects

Fast native Rust CLI for code search in Android/Kotlin/Java and iOS/Swift/ObjC projects using SQLite + FTS5.

## Prerequisites

Install the CLI:
```bash
brew tap defendend/ast-index
brew install ast-index
```

Initialize index in project root:
```bash
cd /path/to/project
ast-index rebuild
```

## Supported Projects

| Platform | Languages | Module System |
|----------|-----------|---------------|
| Android | Kotlin, Java | Gradle (build.gradle.kts) |
| iOS | Swift, Objective-C | SPM (Package.swift) |
| Mixed | All above | Both |

Project type is auto-detected by marker files.

## Available Commands (41 total)

### Search Commands

**Universal search** (files + symbols + modules):
```bash
ast-index search "PaymentMethod"
```

**Find files by name**:
```bash
ast-index file "Fragment.kt"
ast-index file "ViewController.swift"
```

**Find symbols** (classes, interfaces, functions):
```bash
ast-index symbol "PaymentInteractor"
ast-index symbol "AppDelegate"
```

**Find class/interface/protocol**:
```bash
ast-index class "BaseFragment"
ast-index class "UIApplicationDelegate"  # Swift protocol
```

**Find usages** of a symbol (~8ms indexed):
```bash
ast-index usages "PaymentRepository"
```

**Find implementations** (subclasses/implementors/protocol conformance):
```bash
ast-index implementations "BasePresenter"
ast-index implementations "Codable"  # Swift protocol
```

**Class hierarchy** (parents + children):
```bash
ast-index hierarchy "BaseFragment"
```

**Find callers** of a function:
```bash
ast-index callers "onClick"
ast-index callers "viewDidLoad"
```

**File imports**:
```bash
ast-index imports "path/to/File.kt"
ast-index imports "path/to/File.swift"
```

### DI Commands (Dagger - Android only)

**Find @Provides/@Binds** for a type:
```bash
ast-index provides "UserRepository"
```

**Find @Inject** points for a type:
```bash
ast-index inject "UserInteractor"
```

**Find classes with annotation**:
```bash
ast-index annotations "Module"
ast-index annotations "Inject"
```

### Audit Commands

**Find TODO/FIXME/HACK**:
```bash
ast-index todo
ast-index todo "FIXME"
```

**Find @Deprecated** items:
```bash
ast-index deprecated
```

**Find @Suppress** annotations:
```bash
ast-index suppress
ast-index suppress "UNCHECKED_CAST"
```

**Find extension functions** (Kotlin) / **extensions** (Swift):
```bash
ast-index extensions "String"
ast-index extensions "View"
```

**Show public API** of a module:
```bash
ast-index api "features/payments/api"
```

**Find deeplinks**:
```bash
ast-index deeplinks
ast-index deeplinks "payment"
```

**Show changed symbols** (git diff):
```bash
ast-index changed
ast-index changed --base "origin/main"
```

### Compose Commands (Android)

**Find @Composable functions**:
```bash
ast-index composables
ast-index composables "Button"
```

**Find @Preview functions**:
```bash
ast-index previews
```

### Coroutines Commands (Kotlin)

**Find suspend functions**:
```bash
ast-index suspend
ast-index suspend "fetch"
```

**Find Flow/StateFlow/SharedFlow**:
```bash
ast-index flows
ast-index flows "user"
```

### Module Commands

**Find modules** (Gradle or SPM):
```bash
ast-index module "payments"
ast-index module "NetworkKit"
```

**Module dependencies**:
```bash
ast-index deps "features.payments.impl"
```

**Modules depending on this module**:
```bash
ast-index dependents "features.payments.api"
```

**Find unused dependencies** (with transitive, XML, resource checks):
```bash
ast-index unused-deps "features.payments.impl"
ast-index unused-deps "features.payments.impl" --verbose
ast-index unused-deps "features.payments.impl" --strict  # only direct imports
```

### XML & Resource Commands (Android only)

**Find class usages in XML layouts**:
```bash
ast-index xml-usages "PaymentIconView"
ast-index xml-usages "ImageView" --module "features.payments.impl"
```

**Find resource usages**:
```bash
ast-index resource-usages "@drawable/ic_payment"
ast-index resource-usages "R.string.payment_title"
```

**Find unused resources in module**:
```bash
ast-index resource-usages --unused --module "features.payments.impl"
```

### iOS-Specific Commands (v3.4.0)

**Find class usages in storyboards/xibs**:
```bash
ast-index storyboard-usages "MyViewController"
ast-index storyboard-usages "TableViewCell" --module "Features"
```

**Find iOS asset usages** (xcassets):
```bash
ast-index asset-usages "AppIcon"
ast-index asset-usages --unused --module "MainApp"  # find unused assets
```

**Find SwiftUI state properties**:
```bash
ast-index swiftui                    # all @State/@Binding/@Published
ast-index swiftui "State"            # filter by type
ast-index swiftui "userName"         # filter by name
```

**Find async functions** (Swift):
```bash
ast-index async-funcs
ast-index async-funcs "fetch"
```

**Find Combine publishers**:
```bash
ast-index publishers                 # PassthroughSubject, CurrentValueSubject, AnyPublisher
ast-index publishers "state"
```

**Find @MainActor usages**:
```bash
ast-index main-actor
ast-index main-actor "ViewModel"
```

### File Structure

**File outline** (classes, functions in file):
```bash
ast-index outline "path/to/File.kt"
ast-index outline "path/to/File.swift"
```

### Index Management

**Initialize index** (create empty database):
```bash
ast-index init
```

**Rebuild index** (includes module dependencies, XML, resources by default):
```bash
ast-index rebuild
ast-index rebuild --no-deps  # skip module dependencies
```

**Update index** (incremental):
```bash
ast-index update
```

**Index statistics**:
```bash
ast-index stats
```

**Version info**:
```bash
ast-index version
```

## Swift/ObjC Support (v3.3.0+)

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

### iOS UI & Assets (v3.4.0)
- **Storyboards/XIBs**: customClass references, storyboard identifiers
- **xcassets**: imageset, colorset, appiconset, dataset
- **Asset usages**: UIImage(named:), Image(), Color()

### Module Detection
**SPM** - Parses `Package.swift`:
- `.target(name: "...")`, `.testTarget(name: "...")`, `.binaryTarget(name: "...")`

**CocoaPods** - Parses `Podfile` and `Podfile.lock`

**Carthage** - Parses `Cartfile` and `Cartfile.resolved`

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
| storyboard-usages | ~1ms |
| asset-usages | ~1ms |
| swiftui | ~0.9s (grep) |
| async-funcs | ~0.9s (grep) |
| publishers | ~0.9s (grep) |
| main-actor | ~0.9s (grep) |
| todo | ~0.8s (grep) |

## Index Location

Database stored at: `~/.cache/ast-index/<project-hash>/index.db`
