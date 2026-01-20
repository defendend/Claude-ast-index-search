# Android-Specific Commands

Commands for Android/Kotlin/Java projects.

## DI Commands (Dagger)

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

## Compose Commands

**Find @Composable functions**:
```bash
ast-index composables
ast-index composables "Button"
```

**Find @Preview functions**:
```bash
ast-index previews
```

## Coroutines Commands

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

## XML & Resource Commands

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

## Indexed Kotlin/Java Constructs

- `class`, `interface`, `object`, `enum`
- `fun`, `val`, `var`
- Inheritance and interface implementation
- Annotations (@Inject, @Provides, @Composable, etc.)
