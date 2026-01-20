# Module Analysis Commands

Commands for analyzing module structure and dependencies. Works for both Android (Gradle) and iOS (SPM) projects.

## Find Modules

**Find modules by name** (Gradle or SPM):
```bash
ast-index module "payments"
ast-index module "NetworkKit"
```

## Module Dependencies

**Get module dependencies**:
```bash
ast-index deps "features.payments.impl"
```

**Find modules depending on this module**:
```bash
ast-index dependents "features.payments.api"
```

## Unused Dependencies

**Find unused dependencies** (with transitive, XML, resource checks):
```bash
ast-index unused-deps "features.payments.impl"
ast-index unused-deps "features.payments.impl" --verbose
ast-index unused-deps "features.payments.impl" --strict  # only direct imports
```

Available flags:
- `--verbose` — show detailed analysis
- `--strict` — only check direct imports (skip transitive)
- `--no-transitive` — skip transitive dependency checking
- `--no-xml` — skip XML layout usage checking (Android)
- `--no-resources` — skip resource usage checking (Android)

## Public API

**Show public API of a module**:
```bash
ast-index api "features/payments/api"
```

## Performance

| Command | Time |
|---------|------|
| module | ~10ms |
| deps | ~2ms |
| dependents | ~2ms |
| unused-deps | ~12s |
| api | ~30ms |
