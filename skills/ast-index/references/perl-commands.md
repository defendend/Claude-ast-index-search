# Perl Commands Reference

Commands for Perl projects (.pm, .pl, .t, .pod files).

## Perl-Specific Commands

### Find Exports

```bash
ast-index perl-exports              # Find all @EXPORT/@EXPORT_OK
ast-index perl-exports "function"   # Filter by name
```

### Find Subroutines

```bash
ast-index perl-subs                 # Find all subroutine definitions
ast-index perl-subs "process"       # Filter by name
```

### Find POD Documentation

```bash
ast-index perl-pod                  # Find =head1, =item, etc.
ast-index perl-pod "SYNOPSIS"       # Filter by section
```

### Find Tests

```bash
ast-index perl-tests                # Find Test::More assertions
ast-index perl-tests "ok"           # Find specific assertion type
```

### Find Imports

```bash
ast-index perl-imports              # Find use/require statements
ast-index perl-imports "DBI"        # Find specific module usage
```

## Indexed Perl Constructs

- `package` declarations
- `sub` definitions
- `use constant` constants
- `our` variables
- Inheritance: `use base`, `use parent`, `@ISA`

## Module Detection

Perl packages are indexed as modules for `module` command:

```bash
ast-index module "MyApp::Utils"     # Find Perl module
ast-index deps "MyApp::Utils"       # Module dependencies
```

## Project Detection

Auto-detected by marker files:
- `Makefile.PL`
- `Build.PL`
- `cpanfile`
