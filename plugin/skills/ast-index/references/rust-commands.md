# Rust Commands Reference

ast-index supports parsing and indexing Rust source files (`.rs`).

## Supported Elements

| Rust Element | Symbol Kind | Example |
|--------------|-------------|---------|
| `struct Name` | Class | `User` |
| `enum Name` | Enum | `Status` |
| `trait Name` | Interface | `Repository` |
| `impl Trait for Type` | Class | Implementation |
| `impl Type` | Class | Self implementation |
| `fn name()` | Function | `process_data` |
| `macro_rules! name` | Function | `vec_of_strings!` |
| `type Name = ...` | TypeAlias | `UserId` |
| `const NAME` | Constant | `MAX_SIZE` |
| `static NAME` | Constant | `GLOBAL_COUNTER` |
| `mod name` | Package | `utils` |
| `use path` | Import | `use std::io` |
| `#[derive(...)]` | Annotation | `#[derive(Debug)]` |
| `#[test]`, etc. | Annotation | Attributes |

## Core Commands

### Search Structs and Enums

```bash
ast-index class "User"              # Find struct User
ast-index class "Service"           # Find service structs
ast-index symbol "Status"           # Find Status enum
ast-index search "Repository"       # Find repositories
```

### Search Traits

```bash
ast-index class "Repository"        # Find Repository trait
ast-index class "Handler"           # Find handler traits
ast-index implementations "Trait"   # Find trait implementations
```

### Search Functions

```bash
ast-index symbol "process"          # Find functions containing "process"
ast-index symbol "new"              # Find constructors
ast-index callers "handle_request"  # Find function callers
```

### Search Implementations

```bash
ast-index search "impl"             # Find all impl blocks
ast-index search "impl Repository"  # Find Repository implementations
ast-index usages "UserService"      # Find usages of UserService
```

### Search Macros

```bash
ast-index search "macro_rules"      # Find all macro definitions
ast-index symbol "vec_of_strings!"  # Find specific macro
```

### File Analysis

```bash
ast-index outline "lib.rs"          # Show file structure
ast-index imports "main.rs"         # Show all use statements
```

## Example Workflow

```bash
# 1. Index Rust project
cd /path/to/rust/project
ast-index rebuild

# 2. Check index statistics
ast-index stats

# 3. Find all structs
ast-index search "struct" --kind class

# 4. Find all traits
ast-index search "trait" --kind interface

# 5. Find implementations of a trait
ast-index implementations "Repository"

# 6. Show file structure
ast-index outline "src/lib.rs"

# 7. Find usages
ast-index usages "UserService"

# 8. Find callers
ast-index callers "process_request"
```

## Indexed Rust Patterns

### Struct Definition

```rust
#[derive(Debug, Clone, Serialize)]
pub struct User {
    pub id: u64,
    pub name: String,
}
```

Indexed as:
- `#[derive(Debug)]` [annotation]
- `#[derive(Clone)]` [annotation]
- `#[derive(Serialize)]` [annotation]
- `User` [class]

### Enum Definition

```rust
pub enum Status {
    Active,
    Inactive,
    Pending,
}
```

Indexed as:
- `Status` [enum]

### Trait Definition

```rust
pub trait Repository {
    fn find(&self, id: u64) -> Option<User>;
    fn save(&mut self, user: User);
}
```

Indexed as:
- `Repository` [interface]
- `find` [function]
- `save` [function]

### Implementation

```rust
impl Repository for SqlUserRepository {
    fn find(&self, id: u64) -> Option<User> {
        // ...
    }
}

impl User {
    pub fn new(name: String) -> Self {
        Self { id: 0, name }
    }
}
```

Indexed as:
- `impl Repository for SqlUserRepository` [class] with parent `Repository`
- `impl User` [class]
- `find` [function]
- `new` [function]

### Functions

```rust
pub fn process_data(data: &[u8]) -> Result<(), Error> {
    Ok(())
}

pub async fn fetch_user(id: u64) -> User {
    todo!()
}

const fn compute() -> i32 {
    42
}
```

All indexed as [function].

### Macros

```rust
#[macro_export]
macro_rules! vec_of_strings {
    ($($x:expr),*) => (vec![$($x.to_string()),*]);
}
```

Indexed as:
- `#[macro_export]` [annotation]
- `vec_of_strings!` [function]

### Modules and Imports

```rust
mod tests;
pub mod utils;

use std::collections::HashMap;
use crate::db::{Database, Connection};
```

Indexed as:
- `tests` [package]
- `utils` [package]
- `std::collections::HashMap` [import]
- `crate::db` [import]

## Attributes Tracked

The following attributes are indexed:
- `#[test]` - Test functions
- `#[bench]` - Benchmark functions
- `#[cfg]` - Conditional compilation
- `#[derive(...)]` - Derive macros (each derive is separate)
- `#[macro_export]` - Exported macros
- `#[inline]`, `#[cold]`, `#[must_use]` - Optimization hints
- `#[tokio]` - Tokio runtime
- `#[async_trait]` - Async trait macro
- `#[serde]` - Serde attributes
- Framework attributes: `#[rocket]`, `#[actix]`, `#[axum]`

## Performance

| Operation | Time |
|-----------|------|
| Rebuild (1000 Rust files) | ~700ms |
| Search struct | ~1ms |
| Find usages | ~5ms |
| Find implementations | ~5ms |
| File outline | ~1ms |
