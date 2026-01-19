#!/usr/bin/env python3
"""
CLI for kotlin-index - fast code search for Android/Kotlin/Java projects.
"""

import os
import re
import time
from pathlib import Path
from typing import Optional

import typer
from rich.console import Console
from rich.table import Table

from kotlin_index import __version__
from kotlin_index.db.database import Database
from kotlin_index.indexer.file_indexer import FileIndexer
from kotlin_index.indexer.module_indexer import ModuleIndexer
from kotlin_index.indexer.symbol_indexer import SymbolIndexer

app = typer.Typer(
    name="kotlin-index",
    help="Fast code indexer for Android/Kotlin/Java projects",
    add_completion=False,
)
console = Console()


def get_config():
    """Get configuration from environment or defaults."""
    import hashlib

    # Try to auto-detect project root (look for settings.gradle or build.gradle)
    cwd = Path.cwd()
    project_root = os.environ.get("KOTLIN_INDEX_PROJECT_ROOT")

    if not project_root:
        # Try to find project root by looking for gradle files
        for parent in [cwd] + list(cwd.parents):
            if (parent / "settings.gradle").exists() or (parent / "settings.gradle.kts").exists():
                project_root = str(parent)
                break
        else:
            project_root = str(cwd)

    # Use project-specific database (hash of path for uniqueness)
    db_path = os.environ.get("KOTLIN_INDEX_DB_PATH")
    if not db_path:
        # Create unique DB name: project_name-hash.db
        project_name = Path(project_root).name
        path_hash = hashlib.md5(project_root.encode()).hexdigest()[:8]
        db_name = f"{project_name}-{path_hash}.db"
        db_path = str(Path.home() / ".cache" / "kotlin-index" / db_name)

    # Ensure db directory exists
    Path(db_path).parent.mkdir(parents=True, exist_ok=True)

    return project_root, db_path


def get_db() -> Database:
    """Get database connection."""
    _, db_path = get_config()
    db = Database(db_path)
    db.connect()
    return db


def run_search(pattern: str, root: str, context_before: int = 0, context_after: int = 0,
               file_types: list[str] = None, timeout: int = 60, files_only: bool = False) -> str:
    """
    Run fast search using ripgrep (with grep fallback).
    Returns stdout from the search command.

    Args:
        pattern: Regex pattern to search
        root: Directory to search in
        context_before: Lines of context before match (-B)
        context_after: Lines of context after match (-A)
        file_types: File globs to include (default: ["*.kt", "*.java"])
        timeout: Command timeout in seconds
        files_only: If True, return only file paths (no line numbers/content)
    """
    import shutil
    import subprocess

    if file_types is None:
        file_types = ["*.kt", "*.java"]

    # Try ripgrep first (much faster)
    rg_path = shutil.which("rg")
    if rg_path:
        if files_only:
            cmd = [rg_path, "-l", pattern]
        else:
            cmd = [rg_path, "-n", pattern]
        for ft in file_types:
            cmd.extend(["-g", ft])
        if not files_only:
            if context_before > 0:
                cmd.extend(["-B", str(context_before)])
            if context_after > 0:
                cmd.extend(["-A", str(context_after)])
        cmd.append(root)
    else:
        # Fallback to grep
        if files_only:
            cmd = ["grep", "-rl"]
        else:
            cmd = ["grep", "-rn"]
            if context_before > 0:
                cmd.extend([f"-B{context_before}"])
            if context_after > 0:
                cmd.extend([f"-A{context_after}"])
        cmd.extend(["-E", pattern])
        for ft in file_types:
            cmd.extend([f"--include={ft}"])
        cmd.append(root)

    try:
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
        return result.stdout
    except Exception:
        return ""


@app.command()
def version():
    """Show version."""
    console.print(f"kotlin-index v{__version__}")


@app.command()
def init(
    project_root: Optional[str] = typer.Option(None, "--root", "-r", help="Project root directory"),
):
    """Initialize index for current project."""
    root, db_path = get_config()
    if project_root:
        root = project_root

    console.print(f"[bold]Initializing index...[/bold]")
    console.print(f"  Project: {root}")
    console.print(f"  Database: {db_path}")

    db = Database(db_path)
    db.connect()

    # Index everything
    file_indexer = FileIndexer(db, root)
    module_indexer = ModuleIndexer(db, root)
    symbol_indexer = SymbolIndexer(db, root)

    console.print("\n[cyan]Indexing files...[/cyan]")
    file_stats = file_indexer.index_all()
    console.print(f"  Files: {file_stats['total']}")

    console.print("[cyan]Indexing modules...[/cyan]")
    module_stats = module_indexer.index_all()
    console.print(f"  Modules: {module_stats['modules']}")

    console.print("[cyan]Indexing symbols...[/cyan]")
    symbol_stats = symbol_indexer.index_all()
    console.print(f"  Symbols: {symbol_stats['symbols']}")
    console.print(f"  Inheritance: {symbol_stats.get('inheritance', 0)}")

    db.set_meta("last_indexed", time.strftime("%Y-%m-%d %H:%M:%S"))
    db.commit()
    db.close()

    console.print("\n[green]Done![/green]")


@app.command()
def rebuild(
    index_type: str = typer.Option("all", "--type", "-t", help="Index type: files, modules, symbols, all"),
):
    """Rebuild index."""
    root, _ = get_config()
    db = get_db()

    file_indexer = FileIndexer(db, root)
    module_indexer = ModuleIndexer(db, root)
    symbol_indexer = SymbolIndexer(db, root)

    results = {}

    if index_type in ("all", "files"):
        console.print("[cyan]Indexing files...[/cyan]")
        results["files"] = file_indexer.index_all()

    if index_type in ("all", "modules"):
        console.print("[cyan]Indexing modules...[/cyan]")
        results["modules"] = module_indexer.index_all()

    if index_type in ("all", "symbols"):
        console.print("[cyan]Indexing symbols...[/cyan]")
        results["symbols"] = symbol_indexer.index_all()

    db.set_meta("last_indexed", time.strftime("%Y-%m-%d %H:%M:%S"))
    db.commit()
    db.close()

    for idx_type, stats in results.items():
        console.print(f"\n[bold]{idx_type}:[/bold]")
        for key, value in stats.items():
            console.print(f"  {key}: {value}")

    console.print("\n[green]Done![/green]")


@app.command()
def update():
    """Incremental index update (only changed files)."""
    root, _ = get_config()
    db = get_db()

    symbol_indexer = SymbolIndexer(db, root)

    console.print("[cyan]Updating index (incremental)...[/cyan]")
    results = symbol_indexer.index_incremental()

    db.set_meta("last_indexed", time.strftime("%Y-%m-%d %H:%M:%S"))
    db.commit()
    db.close()

    if results["files"] == 0:
        console.print(f"[yellow]No changed files (skipped: {results['skipped']})[/yellow]")
        return

    console.print(f"  Updated files: {results['files']}")
    console.print(f"  Skipped: {results['skipped']}")
    console.print(f"  Symbols: {results['symbols']}")
    console.print(f"  Inheritance: {results['inheritance']}")
    console.print(f"  References: {results['references']}")
    console.print("\n[green]Done![/green]")


@app.command()
def stats():
    """Show index statistics."""
    db = get_db()
    s = db.get_stats()
    db.close()

    table = Table(title="Index Statistics")
    table.add_column("Metric", style="cyan")
    table.add_column("Value", style="green")

    table.add_row("Files", str(s["files"]))
    table.add_row("Modules", str(s["modules"]))
    table.add_row("Symbols", str(s["symbols"]))
    table.add_row("Dependencies", str(s["dependencies"]))
    table.add_row("Inheritance", str(s.get("inheritance", 0)))
    table.add_row("References", str(s.get("references", 0)))
    table.add_row("Last indexed", s["last_indexed"] or "never")

    console.print(table)


@app.command("search")
def search_all(
    query: str = typer.Argument(..., help="Search query"),
    limit: int = typer.Option(10, "--limit", "-l", help="Max results per category"),
):
    """Universal search across files, symbols, and modules."""
    db = get_db()

    # Search files
    files = db.search_files(query, limit)
    if files:
        console.print(f"\n[bold cyan]Files ({len(files)})[/bold cyan]")
        for f in files[:5]:
            console.print(f"  {f['path']}")
        if len(files) > 5:
            console.print(f"  ... and {len(files) - 5} more")

    # Search symbols
    symbols = db.search_symbols(query, limit=limit)
    if symbols:
        console.print(f"\n[bold cyan]Symbols ({len(symbols)})[/bold cyan]")
        for s in symbols[:5]:
            console.print(f"  [{s['type']}] {s['name']} - {s['file_path']}:{s['line']}")
        if len(symbols) > 5:
            console.print(f"  ... and {len(symbols) - 5} more")

    # Search modules
    modules = db.search_modules(query, limit)
    if modules:
        console.print(f"\n[bold cyan]Modules ({len(modules)})[/bold cyan]")
        for m in modules[:5]:
            console.print(f"  [{m['type']}] {m['name']}")
        if len(modules) > 5:
            console.print(f"  ... and {len(modules) - 5} more")

    if not files and not symbols and not modules:
        console.print(f"[yellow]No results for '{query}'[/yellow]")

    db.close()


@app.command("file")
def find_file(
    query: str = typer.Argument(..., help="File name or path pattern"),
    limit: int = typer.Option(20, "--limit", "-l", help="Max results"),
    exact: bool = typer.Option(False, "--exact", "-e", help="Exact name match"),
):
    """Find files by name."""
    db = get_db()

    if exact:
        results = db.execute(
            "SELECT path FROM files WHERE name = ? LIMIT ?",
            (query, limit)
        ).fetchall()
        files = [{"path": r["path"]} for r in results]
    else:
        files = db.search_files(query, limit)

    db.close()

    if not files:
        console.print(f"[yellow]No files found for '{query}'[/yellow]")
        return

    console.print(f"[bold]Found {len(files)} files:[/bold]")
    for f in files:
        console.print(f"  {f['path']}")


@app.command("symbol")
def find_symbol(
    query: str = typer.Argument(..., help="Symbol name"),
    symbol_type: Optional[str] = typer.Option(None, "--type", "-t", help="Symbol type: class, interface, function, property, enum, object"),
    limit: int = typer.Option(20, "--limit", "-l", help="Max results"),
):
    """Find symbols (classes, functions, etc.)."""
    db = get_db()
    symbols = db.search_symbols(query, symbol_type, limit)
    db.close()

    if not symbols:
        type_str = f" of type '{symbol_type}'" if symbol_type else ""
        console.print(f"[yellow]No symbols{type_str} found for '{query}'[/yellow]")
        return

    console.print(f"[bold]Found {len(symbols)} symbols:[/bold]")
    for s in symbols:
        sig = f" {s['signature']}" if s.get("signature") else ""
        console.print(f"  [{s['type']}] {s['name']}{sig}")
        console.print(f"    {s['file_path']}:{s['line']}")


@app.command("class")
def find_class(
    name: str = typer.Argument(..., help="Class or interface name"),
    limit: int = typer.Option(20, "--limit", "-l", help="Max results"),
):
    """Find class/interface by name (contains search)."""
    db = get_db()
    # Search with higher limit since we filter by type after
    results = db.search_symbols(name, symbol_type=None, limit=limit * 5)
    results = [r for r in results if r["type"] in ("class", "interface", "object", "enum")][:limit]
    db.close()

    if not results:
        console.print(f"[yellow]Class/interface '{name}' not found[/yellow]")
        return

    for s in results:
        console.print(f"[{s['type']}] {s['name']}: {s['file_path']}:{s['line']}")


@app.command("outline")
def file_outline(
    file_path: str = typer.Argument(..., help="Path to file (relative or absolute)"),
):
    """Show file structure (classes, functions, etc.)."""
    from pathlib import Path

    root, _ = get_config()

    # Handle relative paths
    if not file_path.startswith("/"):
        file_path = str(Path(root) / file_path)

    db = get_db()
    symbols = db.get_file_symbols(file_path)
    db.close()

    if not symbols:
        # Show relative path in error
        rel_path = file_path.replace(root + "/", "")
        console.print(f"[yellow]No symbols found in '{rel_path}'[/yellow]")
        return

    rel_path = file_path.replace(root + "/", "")
    console.print(f"[bold]Structure of {rel_path}:[/bold]")
    for s in symbols:
        indent = "  " if s.get("parent_symbol_id") else ""
        sig = f" {s['signature']}" if s.get("signature") else ""
        console.print(f"{indent}[{s['type']}] {s['name']}{sig} (line {s['line']})")


@app.command("module")
def find_module(
    query: str = typer.Argument(..., help="Module name"),
    limit: int = typer.Option(20, "--limit", "-l", help="Max results"),
):
    """Find modules by name."""
    db = get_db()
    modules = db.search_modules(query, limit)
    db.close()

    if not modules:
        console.print(f"[yellow]No modules found for '{query}'[/yellow]")
        return

    console.print(f"[bold]Found {len(modules)} modules:[/bold]")
    for m in modules:
        console.print(f"  [{m['type']}] {m['name']}")
        console.print(f"    {m['path']}")


@app.command("deps")
def module_deps(
    module_name: str = typer.Argument(..., help="Module name"),
):
    """Show module dependencies."""
    db = get_db()
    deps = db.get_module_deps(module_name)
    db.close()

    if not deps:
        console.print(f"[yellow]No dependencies found for '{module_name}'[/yellow]")
        return

    console.print(f"[bold]Dependencies of {module_name}:[/bold]")

    by_type = {}
    for d in deps:
        dep_type = d["dep_type"]
        if dep_type not in by_type:
            by_type[dep_type] = []
        by_type[dep_type].append(d["dep_module_name"])

    for dep_type, modules in sorted(by_type.items()):
        console.print(f"\n  [cyan]{dep_type}:[/cyan]")
        for m in sorted(modules):
            console.print(f"    - {m}")


@app.command("dependents")
def module_dependents(
    module_name: str = typer.Argument(..., help="Module name"),
):
    """Show modules that depend on this module."""
    db = get_db()
    deps = db.get_module_dependents(module_name)
    db.close()

    if not deps:
        console.print(f"[yellow]No dependents found for '{module_name}'[/yellow]")
        return

    console.print(f"[bold]Modules depending on {module_name}:[/bold]")
    for d in deps:
        console.print(f"  [{d['dep_type']}] {d['module_name']}")


@app.command("usages")
def find_usages(
    symbol_name: str = typer.Argument(..., help="Symbol name"),
    limit: int = typer.Option(50, "--limit", "-l", help="Max results"),
):
    """Find usages of a symbol."""
    db = get_db()
    refs = db.get_references(symbol_name, limit)
    db.close()

    if not refs:
        console.print(f"[yellow]No usages found for '{symbol_name}'[/yellow]")
        return

    console.print(f"[bold]Usages of '{symbol_name}' ({len(refs)}):[/bold]")

    by_file = {}
    for ref in refs:
        path = ref["file_path"]
        if path not in by_file:
            by_file[path] = []
        by_file[path].append(ref)

    for path, file_refs in sorted(by_file.items()):
        console.print(f"\n  [cyan]{path}:[/cyan]")
        for ref in file_refs:
            ctx = f" ({ref['context']})" if ref.get("context") else ""
            console.print(f"    line {ref['line']}{ctx}")


@app.command("implementations")
def find_implementations(
    interface_name: str = typer.Argument(..., help="Interface or class name"),
):
    """Find implementations of interface or subclasses."""
    db = get_db()
    impls = db.get_implementations(interface_name)
    db.close()

    if not impls:
        console.print(f"[yellow]No implementations found for '{interface_name}'[/yellow]")
        return

    console.print(f"[bold]Implementations of '{interface_name}' ({len(impls)}):[/bold]")
    for impl in impls:
        rel = "implements" if impl["inheritance_type"] == "implements" else "extends"
        console.print(f"  [{impl['type']}] {impl['name']} ({rel})")
        console.print(f"    {impl['file_path']}:{impl['line']}")


@app.command("hierarchy")
def class_hierarchy(
    class_name: str = typer.Argument(..., help="Class or interface name"),
):
    """Show class hierarchy (parents and children)."""
    db = get_db()
    hierarchy = db.get_class_hierarchy(class_name)
    db.close()

    if not hierarchy:
        console.print(f"[yellow]Class/interface '{class_name}' not found[/yellow]")
        return

    symbol = hierarchy["symbol"]
    parents = hierarchy["parents"]
    children = hierarchy["children"]

    # Show hierarchy tree
    console.print(f"\n[bold]Hierarchy of \\[{symbol['type']}] {symbol['name']}[/bold]")
    console.print(f"  {symbol['file_path']}:{symbol['line']}")

    if parents:
        console.print(f"\n[cyan]Parents ({len(parents)}):[/cyan]")
        for p in parents:
            rel = "extends" if p["inheritance_type"] == "extends" else "implements"
            console.print(f"  {rel} {p['parent_name']}")

    if children:
        console.print(f"\n[cyan]Children ({len(children)}):[/cyan]")
        for c in children:
            rel = "extends" if c["inheritance_type"] == "extends" else "implements"
            console.print(f"  [{c['type']}] {c['name']} ({rel})")
            console.print(f"    {c['file_path']}:{c['line']}")

    if not parents and not children:
        console.print("\n[dim]No parents or children found[/dim]")


@app.command("annotations")
def find_annotations(
    annotation: str = typer.Argument(..., help="Annotation name (e.g., @Module, @Inject)"),
    limit: int = typer.Option(30, "--limit", "-l", help="Max results"),
):
    """Find classes with specific annotation."""
    root, _ = get_config()

    # Remove @ if present
    annotation = annotation.lstrip("@")

    # Use run_search to find files with annotation
    output = run_search(f"@{annotation}", root, files_only=True)
    files = output.strip().split("\n") if output.strip() else []

    if not files:
        console.print(f"[yellow]No files with @{annotation} found[/yellow]")
        return

    # Get symbols from these files
    db = get_db()
    symbols = db.get_symbols_by_paths(files[:limit * 2])
    db.close()

    # Filter to classes only
    classes = [s for s in symbols if s["type"] in ("class", "interface", "object")][:limit]

    console.print(f"[bold]Classes with @{annotation} ({len(classes)}):[/bold]")
    for s in classes:
        console.print(f"  [{s['type']}] {s['name']}")
        console.print(f"    {s['file_path']}:{s['line']}")


@app.command("changed")
def show_changed(
    base: str = typer.Option("HEAD", "--base", "-b", help="Base commit/branch to compare"),
):
    """Show symbols in changed files (git diff)."""
    import subprocess

    root, _ = get_config()

    # Get changed files from git
    try:
        result = subprocess.run(
            ["git", "diff", "--name-only", base],
            capture_output=True,
            text=True,
            cwd=root,
            timeout=10,
        )
        changed = set(result.stdout.strip().split("\n")) if result.stdout.strip() else set()

        # Also get untracked/staged files
        result2 = subprocess.run(
            ["git", "status", "--porcelain"],
            capture_output=True,
            text=True,
            cwd=root,
            timeout=10,
        )
        for line in result2.stdout.strip().split("\n"):
            if line and len(line) > 3:
                changed.add(line[3:].strip())

        changed = list(changed)

    except Exception as e:
        console.print(f"[red]Error running git: {e}[/red]")
        return

    # Filter to Kotlin/Java files
    code_files = [f for f in changed if f.endswith((".kt", ".java"))]

    if not code_files:
        console.print("[yellow]No changed Kotlin/Java files[/yellow]")
        return

    # Convert to absolute paths
    from pathlib import Path
    abs_paths = [str(Path(root) / f) for f in code_files]

    # Get symbols
    db = get_db()
    symbols = db.get_symbols_by_paths(abs_paths)
    db.close()

    console.print(f"[bold]Changed files ({len(code_files)}):[/bold]")
    for f in code_files:
        console.print(f"  {f}")

    if symbols:
        console.print(f"\n[bold]Symbols in changed files ({len(symbols)}):[/bold]")

        by_file = {}
        for s in symbols:
            path = s["file_path"]
            if path not in by_file:
                by_file[path] = []
            by_file[path].append(s)

        for path, file_symbols in sorted(by_file.items()):
            rel_path = path.replace(root + "/", "")
            console.print(f"\n  [cyan]{rel_path}:[/cyan]")
            for s in file_symbols:
                console.print(f"    [{s['type']}] {s['name']} (line {s['line']})")


@app.command("callers")
def find_callers(
    function_name: str = typer.Argument(..., help="Function name"),
    limit: int = typer.Option(50, "--limit", "-l", help="Max results"),
):
    """Find where a function is called."""
    root, _ = get_config()

    # Search for function calls: name( or .name(
    pattern = f"[.>]{function_name}\\s*\\(|^\\s*{function_name}\\s*\\("
    output = run_search(pattern, root)
    lines = output.strip().split("\n") if output.strip() else []

    # Filter out function definitions (fun name, def name, void name, etc.)
    calls = []
    for line in lines:
        # Skip definitions
        if re.search(rf'\b(fun|def|void|private|public|protected|override)\s+{function_name}\s*\(', line):
            continue
        calls.append(line)

    if not calls:
        console.print(f"[yellow]No callers found for '{function_name}'[/yellow]")
        return

    # Group by file
    by_file = {}
    for line in calls[:limit]:
        parts = line.split(":", 2)
        if len(parts) >= 3:
            file_path = parts[0].replace(root + "/", "")
            line_num = parts[1]
            content = parts[2].strip()[:70]
            if file_path not in by_file:
                by_file[file_path] = []
            by_file[file_path].append((line_num, content))

    console.print(f"[bold]Callers of '{function_name}' ({len(calls[:limit])}):[/bold]")
    for file_path, items in sorted(by_file.items()):
        console.print(f"\n  [cyan]{file_path}:[/cyan]")
        for line_num, content in items:
            console.print(f"    :{line_num} {content}")


@app.command("imports")
def show_imports(
    file_path: str = typer.Argument(..., help="Path to file"),
):
    """Show imports of a file."""
    from pathlib import Path

    root, _ = get_config()

    # Handle relative paths
    if not file_path.startswith("/"):
        file_path = str(Path(root) / file_path)

    path = Path(file_path)
    if not path.exists():
        console.print(f"[red]File not found: {file_path}[/red]")
        return

    try:
        content = path.read_text()
    except Exception as e:
        console.print(f"[red]Error reading file: {e}[/red]")
        return

    imports = []
    for line in content.split("\n"):
        line = line.strip()
        if line.startswith("import "):
            imports.append(line[7:].rstrip(";"))
        elif line.startswith("package ") or (line and not line.startswith("//") and not line.startswith("/*") and "import " not in line and imports):
            # Stop after imports section
            if imports:
                break

    if not imports:
        console.print(f"[yellow]No imports found in {path.name}[/yellow]")
        return

    console.print(f"[bold]Imports in {path.name} ({len(imports)}):[/bold]")
    for imp in sorted(imports):
        console.print(f"  {imp}")


@app.command("provides")
def find_provides(
    type_name: str = typer.Argument(..., help="Type name to find provider for"),
    limit: int = typer.Option(20, "--limit", "-l", help="Max results"),
):
    """Find @Provides/@Binds methods that provide a type."""
    root, _ = get_config()

    # Search with context around @Provides/@Binds to find the type
    output = run_search("@Provides|@Binds", root, context_after=5)

    # Parse grep output and find blocks containing the type
    matches = []
    current_block = []
    current_file_line = ""

    for line in output.split("\n"):
        if line.startswith("--"):
            # Block separator - check if block contains type
            if current_block and type_name in "\n".join(current_block):
                matches.append((current_file_line, current_block))
            current_block = []
            current_file_line = ""
            continue

        if "@Provides" in line or "@Binds" in line:
            # Start of new provider
            if current_block and type_name in "\n".join(current_block):
                matches.append((current_file_line, current_block))
            current_block = [line]
            current_file_line = line
        elif current_block:
            current_block.append(line)

    # Don't forget last block
    if current_block and type_name in "\n".join(current_block):
        matches.append((current_file_line, current_block))

    if not matches:
        console.print(f"[yellow]No @Provides/@Binds found for '{type_name}'[/yellow]")
        return

    console.print(f"[bold]Providers for '{type_name}' ({len(matches[:limit])}):[/bold]")
    for file_line, block in matches[:limit]:
        parts = file_line.split(":", 2)
        if len(parts) >= 2:
            file_path = parts[0].replace(root + "/", "")
            line_num = parts[1]
            # Find the line with the type (grep uses - for context lines, : for match lines)
            type_line = next((l for l in block if type_name in l), block[0])
            # Extract content after file:line: or file-line- pattern
            match = re.search(r'^[^:]+[:\-]\d+[:\-](.*)$', type_line)
            content = match.group(1).strip()[:80] if match else type_line.strip()[:80]
            console.print(f"  {file_path}:{line_num}")
            console.print(f"    {content}")


@app.command("inject")
def find_inject(
    type_name: str = typer.Argument(..., help="Type name to find injection points"),
    limit: int = typer.Option(30, "--limit", "-l", help="Max results"),
):
    """Find where a type is injected (@Inject constructor/field)."""
    root, _ = get_config()

    # Search for @Inject with the type
    output = run_search("@Inject", root)
    lines = output.strip().split("\n") if output.strip() else []

    # Filter lines that mention the type
    matches = []
    for line in lines:
        if type_name in line:
            matches.append(line)

    # Also search for constructor injection pattern: class Foo @Inject constructor(type: Type)
    output2 = run_search(f"constructor.*{type_name}", root, file_types=["*.kt"])
    for line in output2.strip().split("\n"):
        if line and "@Inject" not in line and line not in matches:
            matches.append(line)

    if not matches:
        console.print(f"[yellow]No injection points found for '{type_name}'[/yellow]")
        return

    console.print(f"[bold]Injection points for '{type_name}' ({len(matches[:limit])}):[/bold]")
    for match in matches[:limit]:
        parts = match.split(":", 2)
        if len(parts) >= 3:
            file_path = parts[0].replace(root + "/", "")
            line_num = parts[1]
            content = parts[2].strip()[:80]
            console.print(f"  {file_path}:{line_num}")
            console.print(f"    {content}")


@app.command("todo")
def find_todos(
    pattern: str = typer.Argument("TODO|FIXME|HACK", help="Pattern to search (regex)"),
    limit: int = typer.Option(50, "--limit", "-l", help="Max results"),
):
    """Find TODO/FIXME/HACK comments in code."""
    root, _ = get_config()

    search_pattern = f"//.*({pattern})|#.*({pattern})"
    output = run_search(search_pattern, root)
    lines = output.strip().split("\n") if output.strip() else []

    if not lines:
        console.print(f"[yellow]No {pattern} comments found[/yellow]")
        return

    # Group by type
    todos = {"TODO": [], "FIXME": [], "HACK": [], "OTHER": []}
    for line in lines[:limit]:
        parts = line.split(":", 2)
        if len(parts) >= 3:
            file_path = parts[0].replace(root + "/", "")
            line_num = parts[1]
            content = parts[2].strip()[:80]

            if "TODO" in content.upper():
                todos["TODO"].append((file_path, line_num, content))
            elif "FIXME" in content.upper():
                todos["FIXME"].append((file_path, line_num, content))
            elif "HACK" in content.upper():
                todos["HACK"].append((file_path, line_num, content))
            else:
                todos["OTHER"].append((file_path, line_num, content))

    total = sum(len(v) for v in todos.values())
    console.print(f"[bold]Found {total} comments:[/bold]")

    for category, items in todos.items():
        if items:
            console.print(f"\n[cyan]{category} ({len(items)}):[/cyan]")
            for file_path, line_num, content in items[:20]:
                console.print(f"  {file_path}:{line_num}")
                console.print(f"    {content}")


@app.command("deprecated")
def find_deprecated(
    limit: int = typer.Option(30, "--limit", "-l", help="Max results"),
):
    """Find @Deprecated classes and functions."""
    root, _ = get_config()

    output = run_search("@Deprecated", root, context_after=1)

    # Parse blocks
    matches = []
    lines = output.split("\n")
    i = 0
    while i < len(lines):
        line = lines[i]
        if "@Deprecated" in line and not line.startswith("--"):
            parts = line.split(":", 2)
            if len(parts) >= 2:
                file_path = parts[0].replace(root + "/", "")
                line_num = parts[1]
                # Get next line for context
                next_line = ""
                if i + 1 < len(lines) and not lines[i + 1].startswith("--"):
                    next_parts = lines[i + 1].split(":", 2)
                    if len(next_parts) >= 3:
                        next_line = next_parts[2].strip()[:60]
                matches.append((file_path, line_num, next_line))
        i += 1

    if not matches:
        console.print("[yellow]No @Deprecated found[/yellow]")
        return

    console.print(f"[bold]Deprecated items ({len(matches[:limit])}):[/bold]")
    for file_path, line_num, context in matches[:limit]:
        console.print(f"  {file_path}:{line_num}")
        if context:
            console.print(f"    {context}")


@app.command("suppress")
def find_suppress(
    warning: Optional[str] = typer.Argument(None, help="Warning name to filter (e.g., UNCHECKED_CAST)"),
    limit: int = typer.Option(30, "--limit", "-l", help="Max results"),
):
    """Find @Suppress annotations (audit suppressed warnings)."""
    root, _ = get_config()

    output = run_search("@Suppress", root)
    lines = output.strip().split("\n") if output.strip() else []

    # Filter by warning if specified
    if warning:
        lines = [l for l in lines if warning.upper() in l.upper()]

    if not lines:
        msg = f" for '{warning}'" if warning else ""
        console.print(f"[yellow]No @Suppress found{msg}[/yellow]")
        return

    # Group by warning type
    by_type = {}
    for line in lines[:limit * 2]:
        parts = line.split(":", 2)
        if len(parts) >= 3:
            file_path = parts[0].replace(root + "/", "")
            line_num = parts[1]
            content = parts[2].strip()

            # Extract warning names from @Suppress("...")
            import re
            warnings = re.findall(r'"([^"]+)"', content)
            for w in warnings:
                if w not in by_type:
                    by_type[w] = []
                by_type[w].append((file_path, line_num))

    console.print(f"[bold]Suppressed warnings ({len(lines[:limit])}):[/bold]")
    for warn_type, items in sorted(by_type.items(), key=lambda x: -len(x[1])):
        console.print(f"\n[cyan]{warn_type} ({len(items)}):[/cyan]")
        for file_path, line_num in items[:10]:
            console.print(f"  {file_path}:{line_num}")
        if len(items) > 10:
            console.print(f"  ... and {len(items) - 10} more")


@app.command("extensions")
def find_extensions(
    receiver_type: str = typer.Argument(..., help="Receiver type (e.g., String, View, Context)"),
    limit: int = typer.Option(30, "--limit", "-l", help="Max results"),
):
    """Find extension functions for a type."""
    root, _ = get_config()

    # Pattern: fun Type.functionName or fun Type?.functionName
    pattern = f"fun\\s+{receiver_type}\\??\\."
    output = run_search(pattern, root, file_types=["*.kt"])
    lines = output.strip().split("\n") if output.strip() else []

    if not lines:
        console.print(f"[yellow]No extension functions found for '{receiver_type}'[/yellow]")
        return

    console.print(f"[bold]Extension functions for '{receiver_type}' ({len(lines[:limit])}):[/bold]")
    for line in lines[:limit]:
        parts = line.split(":", 2)
        if len(parts) >= 3:
            file_path = parts[0].replace(root + "/", "")
            line_num = parts[1]
            content = parts[2].strip()[:80]
            # Extract function name
            match = re.search(rf'{receiver_type}\??\.([\w]+)', content)
            func_name = match.group(1) if match else "?"
            console.print(f"  [cyan]{func_name}[/cyan]: {file_path}:{line_num}")
            console.print(f"    {content}")


@app.command("api")
def show_api(
    module_name: str = typer.Argument(..., help="Module path or name (features/payments/api or features.payments.api)"),
):
    """Show public API of a module (public classes and functions)."""
    from pathlib import Path

    root, _ = get_config()

    # Support both formats: features/payments/api and features.payments.api
    module_path = module_name.replace(".", "/")
    module_dir = Path(root) / module_path

    if not module_dir.exists():
        console.print(f"[red]Module not found: {module_name}[/red]")
        return

    # Find all Kotlin/Java files in src/main
    src_dir = module_dir / "src" / "main"
    if not src_dir.exists():
        src_dir = module_dir  # Try module root

    files = list(src_dir.rglob("*.kt")) + list(src_dir.rglob("*.java"))

    if not files:
        console.print(f"[yellow]No source files found in {module_path}[/yellow]")
        return

    # Get symbols from index
    db = get_db()
    file_paths = [str(f) for f in files]
    symbols = db.get_symbols_by_paths(file_paths)
    db.close()

    # Filter to public API (top-level classes, interfaces, functions)
    api_symbols = [s for s in symbols if s.get("parent_symbol_id") is None]

    # Group by type
    classes = [s for s in api_symbols if s["type"] in ("class", "interface", "object", "enum")]
    functions = [s for s in api_symbols if s["type"] == "function"]

    console.print(f"[bold]Public API of {module_path}:[/bold]")

    if classes:
        console.print(f"\n[cyan]Classes/Interfaces ({len(classes)}):[/cyan]")
        for s in sorted(classes, key=lambda x: x["name"]):
            console.print(f"  [{s['type']}] {s['name']}")
            rel_path = s['file_path'].replace(root + "/", "")
            console.print(f"    {rel_path}:{s['line']}")

    if functions:
        console.print(f"\n[cyan]Top-level Functions ({len(functions)}):[/cyan]")
        for s in sorted(functions, key=lambda x: x["name"]):
            sig = f" {s['signature']}" if s.get('signature') else ""
            console.print(f"  {s['name']}{sig}")
            rel_path = s['file_path'].replace(root + "/", "")
            console.print(f"    {rel_path}:{s['line']}")

    if not classes and not functions:
        console.print("[yellow]No public API symbols found[/yellow]")


@app.command("deeplinks")
def find_deeplinks(
    query: Optional[str] = typer.Argument(None, help="Filter deeplinks by pattern"),
    limit: int = typer.Option(30, "--limit", "-l", help="Max results"),
):
    """Find deeplinks and intent-filters in AndroidManifest files."""
    root, _ = get_config()

    # Search in AndroidManifest.xml files
    output1 = run_search("android:(scheme|host|path|pathPrefix|pathPattern)=", root,
                         file_types=["AndroidManifest.xml"])
    lines = output1.strip().split("\n") if output1.strip() else []

    # Also search for @DeepLink annotations
    output2 = run_search("@DeepLink|@DeepLinkHandler", root)
    if output2.strip():
        lines.extend(output2.strip().split("\n"))

    if query:
        lines = [l for l in lines if query.lower() in l.lower()]

    if not lines:
        msg = f" matching '{query}'" if query else ""
        console.print(f"[yellow]No deeplinks found{msg}[/yellow]")
        return

    # Parse and group by manifest/file
    by_file = {}
    for line in lines[:limit]:
        parts = line.split(":", 2)
        if len(parts) >= 3:
            file_path = parts[0].replace(root + "/", "")
            line_num = parts[1]
            content = parts[2].strip()

            if file_path not in by_file:
                by_file[file_path] = []
            by_file[file_path].append((line_num, content))

    console.print(f"[bold]Deeplinks ({len(lines[:limit])}):[/bold]")
    for file_path, items in sorted(by_file.items()):
        console.print(f"\n[cyan]{file_path}:[/cyan]")
        for line_num, content in items:
            # Clean up XML content
            content = content.strip()[:70]
            console.print(f"  :{line_num} {content}")


@app.command("composables")
def find_composables(
    query: Optional[str] = typer.Argument(None, help="Filter by function name"),
    limit: int = typer.Option(50, "--limit", "-l", help="Max results"),
):
    """Find @Composable functions."""
    root, _ = get_config()

    output = run_search("@Composable", root, context_after=1, file_types=["*.kt"])

    # Parse to extract function names
    composables = []
    lines = output.split("\n")
    i = 0
    while i < len(lines):
        line = lines[i]
        if "@Composable" in line and not line.startswith("--"):
            parts = line.split(":", 2)
            if len(parts) >= 2:
                file_path = parts[0].replace(root + "/", "")
                line_num = parts[1]
                # Get next line for function name
                func_name = ""
                if i + 1 < len(lines) and not lines[i + 1].startswith("--"):
                    next_line = lines[i + 1]
                    # Extract function name from "fun FunctionName(" pattern
                    match = re.search(r'fun\s+(\w+)\s*\(', next_line)
                    if match:
                        func_name = match.group(1)
                if func_name:
                    composables.append((func_name, file_path, line_num))
        i += 1

    # Filter by query if provided
    if query:
        composables = [(n, f, l) for n, f, l in composables if query.lower() in n.lower()]

    if not composables:
        msg = f" matching '{query}'" if query else ""
        console.print(f"[yellow]No @Composable functions found{msg}[/yellow]")
        return

    console.print(f"[bold]@Composable functions ({len(composables[:limit])}):[/bold]")
    for func_name, file_path, line_num in composables[:limit]:
        console.print(f"  [cyan]{func_name}[/cyan]: {file_path}:{line_num}")


@app.command("previews")
def find_previews(
    limit: int = typer.Option(30, "--limit", "-l", help="Max results"),
):
    """Find @Preview functions (Compose previews)."""
    root, _ = get_config()

    output = run_search("@Preview", root, context_before=1, context_after=1, file_types=["*.kt"])

    # Parse to extract preview info
    previews = []
    lines = output.split("\n")
    i = 0
    while i < len(lines):
        line = lines[i]
        if "@Preview" in line and not line.startswith("--"):
            parts = line.split(":", 2)
            if len(parts) >= 2:
                file_path = parts[0].replace(root + "/", "")
                line_num = parts[1]
                # Extract preview params if any
                preview_params = ""
                if "(" in line:
                    match = re.search(r'@Preview\(([^)]*)\)', line)
                    if match:
                        preview_params = match.group(1)[:40]
                # Look for function name in next lines
                func_name = ""
                for j in range(1, 4):
                    if i + j < len(lines) and not lines[i + j].startswith("--"):
                        next_line = lines[i + j]
                        match = re.search(r'fun\s+(\w+)\s*\(', next_line)
                        if match:
                            func_name = match.group(1)
                            break
                if func_name:
                    previews.append((func_name, preview_params, file_path, line_num))
        i += 1

    if not previews:
        console.print("[yellow]No @Preview functions found[/yellow]")
        return

    console.print(f"[bold]@Preview functions ({len(previews[:limit])}):[/bold]")
    for func_name, params, file_path, line_num in previews[:limit]:
        param_str = f" ({params})" if params else ""
        console.print(f"  [cyan]{func_name}[/cyan]{param_str}")
        console.print(f"    {file_path}:{line_num}")


@app.command("suspend")
def find_suspend(
    query: Optional[str] = typer.Argument(None, help="Filter by function name"),
    limit: int = typer.Option(50, "--limit", "-l", help="Max results"),
):
    """Find suspend functions."""
    root, _ = get_config()

    output = run_search(r"suspend\s+fun\s+", root, file_types=["*.kt"])
    lines = output.strip().split("\n") if output.strip() else []

    # Parse and extract function names
    suspends = []
    for line in lines:
        parts = line.split(":", 2)
        if len(parts) >= 3:
            file_path = parts[0].replace(root + "/", "")
            line_num = parts[1]
            content = parts[2].strip()
            # Extract function name
            match = re.search(r'suspend\s+fun\s+(\w+)', content)
            if match:
                func_name = match.group(1)
                suspends.append((func_name, file_path, line_num, content[:60]))

    # Filter by query if provided
    if query:
        suspends = [(n, f, l, c) for n, f, l, c in suspends if query.lower() in n.lower()]

    if not suspends:
        msg = f" matching '{query}'" if query else ""
        console.print(f"[yellow]No suspend functions found{msg}[/yellow]")
        return

    console.print(f"[bold]Suspend functions ({len(suspends[:limit])}):[/bold]")
    for func_name, file_path, line_num, content in suspends[:limit]:
        console.print(f"  [cyan]{func_name}[/cyan]: {file_path}:{line_num}")


@app.command("flows")
def find_flows(
    query: Optional[str] = typer.Argument(None, help="Filter by pattern"),
    limit: int = typer.Option(50, "--limit", "-l", help="Max results"),
):
    """Find Flow, StateFlow, SharedFlow usage."""
    root, _ = get_config()

    # Search for Flow types
    pattern = r"(Flow<|StateFlow<|SharedFlow<|MutableStateFlow|MutableSharedFlow|flowOf|asFlow|channelFlow)"
    output = run_search(pattern, root, file_types=["*.kt"])
    lines = output.strip().split("\n") if output.strip() else []

    # Filter by query if provided
    if query:
        lines = [l for l in lines if query.lower() in l.lower()]

    if not lines:
        msg = f" matching '{query}'" if query else ""
        console.print(f"[yellow]No Flow usage found{msg}[/yellow]")
        return

    # Group by type
    by_type = {"StateFlow": [], "SharedFlow": [], "Flow": [], "Other": []}
    for line in lines[:limit * 2]:
        parts = line.split(":", 2)
        if len(parts) >= 3:
            file_path = parts[0].replace(root + "/", "")
            line_num = parts[1]
            content = parts[2].strip()[:70]

            if "StateFlow" in content or "MutableStateFlow" in content:
                by_type["StateFlow"].append((file_path, line_num, content))
            elif "SharedFlow" in content or "MutableSharedFlow" in content:
                by_type["SharedFlow"].append((file_path, line_num, content))
            elif "Flow<" in content or "flowOf" in content or "asFlow" in content or "channelFlow" in content:
                by_type["Flow"].append((file_path, line_num, content))
            else:
                by_type["Other"].append((file_path, line_num, content))

    total = sum(len(v) for v in by_type.values())
    console.print(f"[bold]Flow usage ({min(total, limit)}):[/bold]")

    shown = 0
    for flow_type, items in by_type.items():
        if items and shown < limit:
            console.print(f"\n[cyan]{flow_type} ({len(items)}):[/cyan]")
            for file_path, line_num, content in items[:limit - shown]:
                console.print(f"  {file_path}:{line_num}")
                console.print(f"    {content}")
                shown += 1
                if shown >= limit:
                    break


@app.command("mcp")
def run_mcp():
    """Start MCP server (requires kotlin-index[mcp])."""
    try:
        from kotlin_index.server import mcp
        console.print("[cyan]Starting MCP server...[/cyan]")
        mcp.run()
    except ImportError:
        console.print("[red]MCP dependencies not installed.[/red]")
        console.print("Install with: pip install kotlin-index\\[mcp]")
        raise typer.Exit(1)


if __name__ == "__main__":
    app()
