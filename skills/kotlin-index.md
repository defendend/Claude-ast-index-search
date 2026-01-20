# kotlin-index v3.2.0 - Code Search for Android/Kotlin/Java Projects

Fast native Rust CLI for code search in Android/Kotlin/Java projects using SQLite + FTS5.

## Prerequisites

Install the CLI:
```bash
brew tap defendend/kotlin-index
brew install kotlin-index
```

Initialize index in project root:
```bash
cd /path/to/android/project
kotlin-index rebuild
```

## Available Commands (36 total)

### Search Commands

**Universal search** (files + symbols + modules):
```bash
kotlin-index search "PaymentMethod"
```

**Find files by name**:
```bash
kotlin-index file "Fragment.kt"
```

**Find symbols** (classes, interfaces, functions):
```bash
kotlin-index symbol "PaymentInteractor"
```

**Find class/interface**:
```bash
kotlin-index class "BaseFragment"
```

**Find usages** of a symbol (~8ms indexed):
```bash
kotlin-index usages "PaymentRepository"
```

**Find implementations** (subclasses/implementors):
```bash
kotlin-index implementations "BasePresenter"
```

**Class hierarchy** (parents + children):
```bash
kotlin-index hierarchy "BaseFragment"
```

**Find callers** of a function:
```bash
kotlin-index callers "onClick"
```

**File imports**:
```bash
kotlin-index imports "path/to/File.kt"
```

### DI Commands (Dagger)

**Find @Provides/@Binds** for a type (supports Java + Kotlin):
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

**Find extension functions**:
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

### Compose Commands

**Find @Composable functions**:
```bash
kotlin-index composables
kotlin-index composables "Button"
```

**Find @Preview functions**:
```bash
kotlin-index previews
```

### Coroutines Commands

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

**Find modules**:
```bash
kotlin-index module "payments"
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

### XML & Resource Commands (new in v3.2)

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
