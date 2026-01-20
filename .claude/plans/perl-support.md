# Plan: Add Perl Support to ast-index

## Overview
Add comprehensive Perl language support to ast-index for searching in Perl codebases (Yandex Direct, etc.)

## Perl Constructs to Index

### Symbols (in `symbols` table)
| Perl Construct | Kind | Example |
|----------------|------|---------|
| `package Name;` | package | `package DoCmd;` |
| `sub name { }` | function | `sub do_direct_cmd { }` |
| `use constant NAME =>` | constant | `use constant ORDER_ID_OFFSET => 100_000_000;` |
| `our $VAR` | variable | `our $VERSION = '0.01';` |
| `our @ARRAY` | variable | `our @EXPORT = qw(...)` |
| `our %HASH` | variable | `our %cmds = (...)` |

### Inheritance (in `inheritance` table)
| Perl Construct | Parent |
|----------------|--------|
| `use base qw/Parent/` | Parent |
| `use parent qw/Parent/` | Parent |
| `our @ISA = qw(Parent)` | Parent |

### Imports (for `imports` command)
| Perl Construct | Example |
|----------------|---------|
| `use Module;` | `use Settings;` |
| `use Module qw(...)` | `use Common qw(:globals :subs);` |
| `require Module;` | `require Exporter;` |

### File Extensions
- `.pm` - Perl modules
- `.pl` - Perl scripts
- `.t` - Perl test files

## Implementation Tasks

### Phase 1: Core Indexing (src/indexer.rs) ✅ DONE
- [x] Add `parse_perl_symbols()` function
- [x] Parse `package` declarations
- [x] Parse `sub` definitions (with signatures)
- [x] Parse `use constant` definitions
- [x] Parse `our` variable declarations
- [x] Parse inheritance (`use base`, `use parent`, `@ISA`)
- [x] Add Perl to `parse_file()` dispatch
- [x] Add `.pm`, `.pl`, `.t` to file walker
- [x] Add `Package` and `Constant` to SymbolKind enum

### Phase 2: Grep Commands (src/main.rs) ✅ DONE
- [x] Add `["pm", "pl", "t"]` to `cmd_todo`
- [x] Add `["pm", "pl", "t"]` to `cmd_callers` (with Perl patterns)
- [x] Add `["pm", "pl", "t"]` to `cmd_deprecated` (with Perl patterns)
- [x] Add `["pm", "pl", "t"]` to `cmd_annotations` (Perl attributes)
- [x] Skip `cmd_extensions` (N/A for Perl)
- [x] Update `cmd_changed` for Perl files

### Phase 3: Perl-Specific Commands ✅ DONE
- [x] `perl-exports` - Find exported functions (`@EXPORT`, `@EXPORT_OK`)
- [x] `perl-subs` - Find all subroutines
- [ ] Consider: POD documentation search (future)

### Phase 4: Module Detection ✅ DONE
- [x] Detect Perl project by presence of `Makefile.PL`/`Build.PL`/`cpanfile`
- [x] Add `ProjectType::Perl` variant

### Phase 5: Documentation & Skill ✅ DONE
- [x] Add `references/perl-commands.md` to skill
- [x] Update SKILL.md with Perl mention
- [x] Update README.md with Perl support
- [x] Update Cargo.toml version to 3.6.0

### Phase 6: Testing
- [ ] Test on `arcadia/direct/perl` directory
- [ ] Verify all commands work with Perl files
- [ ] Check performance

## Regex Patterns (Implemented)

### Package
```rust
r"^\s*package\s+([A-Za-z_][A-Za-z0-9_:]*)\s*;"
```

### Subroutine
```rust
r"^\s*sub\s+([A-Za-z_][A-Za-z0-9_]*)\s*[\{(]?"
```

### Constant
```rust
r"^\s*use\s+constant\s+([A-Z_][A-Z0-9_]*)\s*=>"
```

### Our Variable
```rust
r"^\s*our\s+([\$@%][A-Za-z_][A-Za-z0-9_]*)"
```

### Inheritance
```rust
r#"use\s+(?:base|parent)\s+(?:qw[/(]([^)/\\]+)[)/\\]|['"]([^'"]+)['"])"#
r#"our\s+@ISA\s*=\s*(?:qw[/(]([^)/\\]+)[)/\\]|\(([^)]+)\))"#
```

## Version
Version 3.6.0 - "Perl Support"

## Files Modified
1. `src/db.rs` - Added `Package` and `Constant` to SymbolKind
2. `src/indexer.rs` - Added Perl parsing, project detection
3. `src/main.rs` - Added Perl to grep commands, new perl-exports/perl-subs commands
4. `skills/ast-index/SKILL.md` - Added Perl mention
5. `skills/ast-index/references/perl-commands.md` - New file
6. `README.md` - Added Perl to supported languages
7. `Cargo.toml` - Bumped version to 3.6.0
