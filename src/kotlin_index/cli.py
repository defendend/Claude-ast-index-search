#!/usr/bin/env python3
"""
CLI for kotlin-index - fast code search for Android/Kotlin/Java projects.
"""

import os
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

    db_path = os.environ.get(
        "KOTLIN_INDEX_DB_PATH",
        str(Path.home() / ".cache" / "kotlin-index" / "index.db")
    )

    # Ensure db directory exists
    Path(db_path).parent.mkdir(parents=True, exist_ok=True)

    return project_root, db_path


def get_db() -> Database:
    """Get database connection."""
    _, db_path = get_config()
    db = Database(db_path)
    db.connect()
    return db


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
    file_path: str = typer.Argument(..., help="Path to file"),
):
    """Show file structure (classes, functions, etc.)."""
    db = get_db()
    symbols = db.get_file_symbols(file_path)
    db.close()

    if not symbols:
        console.print(f"[yellow]No symbols found in '{file_path}'[/yellow]")
        return

    console.print(f"[bold]Structure of {file_path}:[/bold]")
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
    import subprocess

    root, _ = get_config()

    # Remove @ if present
    annotation = annotation.lstrip("@")

    # Use grep to find files with annotation
    try:
        result = subprocess.run(
            ["grep", "-rl", f"@{annotation}", "--include=*.kt", "--include=*.java", root],
            capture_output=True,
            text=True,
            timeout=30,
        )
        files = result.stdout.strip().split("\n") if result.stdout.strip() else []
    except Exception as e:
        console.print(f"[red]Error searching: {e}[/red]")
        return

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
