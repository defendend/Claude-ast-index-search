# Contributing to ast-index

Thank you for your interest in contributing to ast-index!

## Prerequisites

- Rust 1.75+ (edition 2021)
- `cargo` build tool

## Getting Started

```bash
git clone https://github.com/defendend/Claude-ast-index-search.git
cd Claude-ast-index-search
cargo build
```

## Project Structure

```
src/
  main.rs                          # CLI entry point (clap)
  db.rs                            # SQLite schema, SymbolKind enum
  indexer.rs                       # File discovery, parallel indexing (rayon)
  parsers/
    mod.rs                         # ParsedSymbol, LanguageParser trait
    treesitter/
      mod.rs                       # Tree-sitter helpers (parse_tree, node_text, etc.)
      typescript.rs, python.rs,    # Tree-sitter parsers (one per language)
      kotlin.rs, java.rs, ...
      queries/
        typescript.scm, python.scm, ...  # Tree-sitter query patterns
    typescript.rs                  # Regex-based fallback parser for TS/JS
    perl.rs                        # Regex-based Perl parser
    wsdl.rs                        # WSDL/XSD parser
  commands/
    mod.rs                         # Command dispatch, grep-based search
    index.rs                       # rebuild, update, init
    files.rs                       # file, outline, imports, changed
    analysis.rs                    # search, class, symbol, usages, hierarchy
    modules.rs                     # module, deps, dependents, unused-deps
    android.rs                     # xml-usages, resource-usages
    ios.rs                         # storyboard-usages, asset-usages, swiftui
    perl.rs                        # perl-exports, perl-subs, perl-pod
    grep.rs                        # grep-based commands (todo, callers, etc.)
    management.rs                  # stats, version, install-claude-plugin
    project_info.rs                # map, conventions
    watch.rs                       # watch (filesystem watcher)
```

## Adding a New Language Parser

1. Add tree-sitter dependency to `Cargo.toml`
2. Create query file `src/parsers/treesitter/queries/<lang>.scm`
3. Create parser `src/parsers/treesitter/<lang>.rs` implementing `LanguageParser`
4. Register in `src/parsers/treesitter/mod.rs`
5. Add file extensions in `src/indexer.rs`
6. Add tests (see existing parsers for examples)

### Parser Pattern

Each tree-sitter parser follows the same pattern:

```rust
use crate::db::SymbolKind;
use crate::parsers::ParsedSymbol;
use super::{LanguageParser, parse_tree, node_text, node_line, line_text};

pub struct MyLangParser;

impl LanguageParser for MyLangParser {
    fn parse_symbols(&self, content: &str) -> Result<Vec<ParsedSymbol>> {
        let tree = parse_tree(content, &MY_LANG)?;
        // ... match tree-sitter captures, emit ParsedSymbol
    }
}
```

### SymbolKind Values

`Class`, `Interface`, `Enum`, `Function`, `Property`, `Constant`, `TypeAlias`,
`Package`, `Import`, `Annotation`, `Trait`, `Macro`

## Code Style

- **Comments & docstrings**: English within code files, concise
- **Helper docstrings**: `/// Check if ...` style (see existing examples in parsers)
- **Tests**: use raw strings `r#"..."#` for multi-line test fixtures
- **Match blocks**: extract helper functions when 3+ blocks share the same structure
- **Formatting**: `cargo fmt`
- **Linting**: `cargo clippy -- -D warnings` (zero new warnings)

## Testing

```bash
# Run all tests
cargo test

# Run tests for a specific parser
cargo test parsers::treesitter::typescript

# Run clippy
cargo clippy -- -D warnings
```

`.cargo/config.toml` sets `AST_INDEX_CACHE_DIR` to `target/ast-index-cache`
for everything cargo starts, so tests (and `cargo run`) never touch your own
index cache. Remove that directory to reset it; an `AST_INDEX_CACHE_DIR`
exported in your shell takes precedence.

### Test on Real Projects

After building, test on a real codebase:

```bash
cargo build --release
./target/release/ast-index rebuild    # in a project directory
./target/release/ast-index stats
./target/release/ast-index search "ClassName"
```

## Pull Requests

1. Fork and create a feature branch
2. Make changes with tests
3. Ensure `cargo test` passes (all tests, not just yours)
4. Ensure `cargo clippy -- -D warnings` has no new warnings
5. Open a PR with a clear description of what and why

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
