# Perl Commands Reference

ast-index provides comprehensive support for Perl codebases, including indexing and searching.

## Supported File Types

| Extension | Description |
|-----------|-------------|
| `.pm` | Perl modules |
| `.pl` | Perl scripts |
| `.t` | Perl test files |
| `.pod` | POD documentation files |

## Indexed Constructs

### Symbols (in `symbols` table)

| Perl Construct | Kind | Example |
|----------------|------|---------|
| `package Name;` | package | `package DoCmd;` |
| `sub name { }` | function | `sub do_direct_cmd { }` |
| `use constant NAME =>` | constant | `use constant ORDER_ID_OFFSET => 100_000_000;` |
| `our $VAR` | property | `our $VERSION = '0.01';` |
| `our @ARRAY` | property | `our @EXPORT = qw(...)` |
| `our %HASH` | property | `our %cmds = (...)` |

### Inheritance (in `inheritance` table)

| Perl Construct | Parent |
|----------------|--------|
| `use base qw/Parent/` | Parent |
| `use parent qw/Parent/` | Parent |
| `our @ISA = qw(Parent)` | Parent |

### Modules (in `modules` table)

Perl packages are indexed as modules. Each `package Name;` declaration in `.pm` files creates a module entry.

## Perl-Specific Commands

### perl-exports

Find @EXPORT and @EXPORT_OK definitions in Perl modules.

```bash
ast-index perl-exports                    # Find all exports
ast-index perl-exports "function_name"    # Filter by function name
ast-index perl-exports -l 100             # Limit to 100 results
```

**What it searches:**
- `our @EXPORT = qw(...)`
- `our @EXPORT_OK = qw(...)`
- `@EXPORT = ...`
- `@EXPORT_OK = ...`

### perl-subs

Find all subroutine definitions in Perl files.

```bash
ast-index perl-subs                       # Find all subroutines
ast-index perl-subs "validate"            # Find subs containing "validate"
ast-index perl-subs -l 50                 # Limit to 50 results
```

### perl-pod

Find POD documentation sections in Perl files.

```bash
ast-index perl-pod                        # Find all POD sections
ast-index perl-pod "SYNOPSIS"             # Find SYNOPSIS sections
ast-index perl-pod "METHODS"              # Find METHODS sections
```

**What it searches:**
- `=head1`, `=head2`, `=head3`, `=head4` - Section headings
- `=item` - List items
- `=over`, `=back` - List delimiters
- `=pod`, `=cut` - POD block markers
- `=begin`, `=end` - Format blocks
- `=for` - Formatter-specific paragraphs

### perl-tests

Find Test::More and Test::Simple assertions.

```bash
ast-index perl-tests                      # Find all test assertions
ast-index perl-tests "validate"           # Filter by test description
ast-index perl-tests -l 100               # Limit results
```

**What it searches:**
- `ok()` - Basic test
- `is()`, `isnt()` - Equality tests
- `like()`, `unlike()` - Regex tests
- `cmp_ok()` - Comparison tests
- `is_deeply()` - Deep comparison
- `diag()` - Diagnostic output
- `pass()`, `fail()` - Explicit pass/fail
- `subtest` - Subtest blocks
- `plan`, `done_testing` - Test plans
- `SKIP`, `TODO` - Skip/todo blocks

### perl-imports

Find use/require statements in Perl files.

```bash
ast-index perl-imports                    # Find all imports
ast-index perl-imports "DBI"              # Find DBI imports
ast-index perl-imports "Test"             # Find Test::* imports
```

**Note:** Skips pragmas like `use strict`, `use warnings`, `use utf8`, `use base`, `use parent`, `use constant`.

## Core Commands with Perl Support

### imports

Show imports in a specific file. Works with Perl files.

```bash
ast-index imports "path/to/Module.pm"     # Show use/require in Perl file
```

Extracts `use Module;` and `require Module;` statements (skipping pragmas).

### module

Find modules by name. Includes Perl packages.

```bash
ast-index module "DoCmd"                  # Find DoCmd package
ast-index module "Direct"                 # Find packages containing "Direct"
```

### symbol

Find symbols by name (packages, subs, constants, variables).

```bash
ast-index symbol "DoCmd"                  # Find DoCmd package or sub
ast-index symbol "VERSION"                # Find $VERSION declarations
```

### class (package)

Find package definitions.

```bash
ast-index class "DoCmd"                   # Find DoCmd package
```

### implementations

Find packages that inherit from a parent.

```bash
ast-index implementations "Exporter"      # Find packages using Exporter
ast-index implementations "Base::Class"
```

### hierarchy

Show inheritance hierarchy.

```bash
ast-index hierarchy "MyPackage"           # Show parent/child packages
```

### usages

Find symbol usages.

```bash
ast-index usages "process_data"           # Find where process_data is used
```

### outline

Show symbols in a file.

```bash
ast-index outline "path/to/Module.pm"     # Show packages, subs, constants, variables
```

### todo

Find TODO/FIXME/HACK comments.

```bash
ast-index todo                            # Searches Perl comments too
```

### callers

Find function call sites.

```bash
ast-index callers "process_data"          # Find calls in Perl files
```

Patterns: `->func()`, `func()`, `&func()`

### deprecated

Find deprecated markers.

```bash
ast-index deprecated                      # Find DEPRECATED markers
```

Searches `# DEPRECATED` comments and `=head DEPRECATED` POD.

### changed

Show symbols changed in git diff.

```bash
ast-index changed                         # Shows changed .pm/.pl/.t files
```

## Performance

| Command | Time | Notes |
|---------|------|-------|
| perl-exports | ~0.8s | Grep-based |
| perl-subs | ~0.8s | Grep-based |
| perl-pod | ~0.8s | Grep-based |
| perl-tests | ~0.8s | Grep-based |
| perl-imports | ~0.8s | Grep-based |
| symbol (indexed) | ~1ms | Index lookup |
| usages (indexed) | ~8ms | Reference search |
| module | ~1ms | Index lookup |

## Example Workflow

```bash
# 1. Initialize index in Perl project
cd /path/to/perl/project
ast-index rebuild

# 2. Explore modules
ast-index module "Controller"

# 3. Find exported functions
ast-index perl-exports

# 4. Find all subs in specific area
ast-index perl-subs "validate"

# 5. Read documentation
ast-index perl-pod "SYNOPSIS"

# 6. Find tests
ast-index perl-tests

# 7. Check imports in a file
ast-index imports "lib/MyApp/Controller.pm"

# 8. Find inheritance
ast-index implementations "Exporter"

# 9. Check for TODOs
ast-index todo
```

## Tips

1. **Use `symbol` for indexed search** - much faster than grep-based commands
2. **Use `perl-subs` for exploring** - grep-based but finds all subs
3. **Use `module` to find packages** - indexed, instant results
4. **Check inheritance** - `implementations` works with Perl packages
5. **Find exports** - `perl-exports` shows module public API
6. **Read POD** - `perl-pod` finds documentation sections
7. **Find tests** - `perl-tests` locates Test::More assertions
8. **Check imports** - `perl-imports` or `imports` for specific file
