#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

python3 - <<'PY'
import json
import pathlib
import re

root = pathlib.Path.cwd()
errors = []


def load_json(path: str):
    try:
        with (root / path).open("r", encoding="utf-8") as fh:
            return json.load(fh)
    except Exception as exc:
        errors.append(f"{path}: invalid JSON: {exc}")
        return {}


def require_file(path: str):
    if not (root / path).is_file():
        errors.append(f"{path}: missing file")


def require_dir(path: str):
    if not (root / path).is_dir():
        errors.append(f"{path}: missing directory")


def require_text(path: str) -> str:
    try:
        return (root / path).read_text(encoding="utf-8")
    except Exception as exc:
        errors.append(f"{path}: unreadable text file: {exc}")
        return ""


cargo_toml = (root / "Cargo.toml").read_text(encoding="utf-8")
match = re.search(r'(?m)^version\s*=\s*"([^"]+)"', cargo_toml)
if not match:
    errors.append("Cargo.toml: could not find package version")
    cargo_version = ""
else:
    cargo_version = match.group(1)

plugin_name_re = re.compile(r"^[a-z0-9]([a-z0-9.-]*[a-z0-9])?$")
cursor_allowed = {
    "name",
    "displayName",
    "description",
    "version",
    "author",
    "publisher",
    "homepage",
    "repository",
    "license",
    "logo",
    "keywords",
    "category",
    "tags",
    "commands",
    "agents",
    "skills",
    "rules",
    "hooks",
    "mcpServers",
}

shared_plugin_files = [
    "plugin/.claude-plugin/plugin.json",
    "plugin/.codex-plugin/plugin.json",
    "plugin/.cursor-plugin/plugin.json",
]

for path in shared_plugin_files:
    manifest = load_json(path)
    if manifest.get("name") != "ast-index":
        errors.append(f"{path}: name must be ast-index")
    if manifest.get("version") != cargo_version:
        errors.append(f"{path}: version {manifest.get('version')!r} != Cargo.toml {cargo_version!r}")
    if not plugin_name_re.match(manifest.get("name", "")):
        errors.append(f"{path}: invalid plugin name")

cursor_manifest = load_json("plugin/.cursor-plugin/plugin.json")
extra = sorted(set(cursor_manifest) - cursor_allowed)
if extra:
    errors.append(f"plugin/.cursor-plugin/plugin.json: unsupported fields: {', '.join(extra)}")

for path in [
    "plugin/commands/initialize.md",
    "plugin/commands/initialize-android.md",
    "plugin/commands/initialize-csharp.md",
    "plugin/commands/initialize-ios.md",
    "plugin/commands/initialize-ruby.md",
    "plugin/commands/initialize-rust.md",
    "plugin/commands/initialize-web.md",
    "plugin/skills/ast-index/SKILL.md",
    "plugin/rules/ast-index.mdc",
    "plugin/commands-cursor/initialize-ast-index.md",
]:
    require_file(path)

require_dir("plugin/commands")
require_dir("plugin/skills")

codex_marketplace = load_json(".agents/plugins/marketplace.json")
codex_plugins = codex_marketplace.get("plugins", [])
if len(codex_plugins) != 1:
    errors.append(".agents/plugins/marketplace.json: expected exactly one plugin entry")
else:
    entry = codex_plugins[0]
    if entry.get("name") != "ast-index":
        errors.append(".agents/plugins/marketplace.json: plugin name must be ast-index")
    source = entry.get("source", {})
    if source.get("source") != "local" or source.get("path") != "./plugin":
        errors.append(".agents/plugins/marketplace.json: source must be local ./plugin")
    policy = entry.get("policy", {})
    for key in ("installation", "authentication"):
        if key not in policy:
            errors.append(f".agents/plugins/marketplace.json: missing policy.{key}")
    if "category" not in entry:
        errors.append(".agents/plugins/marketplace.json: missing category")

cursor_marketplace = load_json(".cursor-plugin/marketplace.json")
cursor_plugins = cursor_marketplace.get("plugins", [])
if len(cursor_plugins) != 1:
    errors.append(".cursor-plugin/marketplace.json: expected exactly one plugin entry")
else:
    entry = cursor_plugins[0]
    if entry.get("name") != "ast-index" or entry.get("source") != "plugin":
        errors.append(".cursor-plugin/marketplace.json: plugin entry must point ast-index to plugin")

initialize_md = require_text("plugin/commands/initialize.md")
if initialize_md:
    required_snippets = {
        "frontmatter name": "name: initialize",
        "mentions KMP": "Kotlin Multiplatform (KMP)",
        "mentions polyglot": "polyglot/monorepo",
        "settings path": ".claude/settings.json",
        "rules path": ".claude/rules/ast-index.md",
        "rebuild step": "ast-index rebuild",
        "stats step": "ast-index stats",
        "android source command": "plugin/commands/initialize-android.md",
        "ios source command": "plugin/commands/initialize-ios.md",
        "web source command": "plugin/commands/initialize-web.md",
        "rust source command": "plugin/commands/initialize-rust.md",
        "csharp source command": "plugin/commands/initialize-csharp.md",
        "ruby source command": "plugin/commands/initialize-ruby.md",
        "python fallback": "Python/Go/Dart/PHP/Scala",
        "kmp marker commonMain": "commonMain",
        "kmp marker androidMain": "androidMain",
        "kmp marker iosMain": "iosMain",
    }
    for label, snippet in required_snippets.items():
        if snippet not in initialize_md:
            errors.append(f"plugin/commands/initialize.md: missing {label} snippet {snippet!r}")

    if "set of stacks" not in initialize_md or "not a single label" not in initialize_md:
        errors.append(
            "plugin/commands/initialize.md: must instruct Claude to detect a set of stacks, not a single label"
        )

manual_override_files = [
    "plugin/commands/initialize-android.md",
    "plugin/commands/initialize-csharp.md",
    "plugin/commands/initialize-ios.md",
    "plugin/commands/initialize-ruby.md",
    "plugin/commands/initialize-rust.md",
    "plugin/commands/initialize-web.md",
]
for path in manual_override_files:
    content = require_text(path)
    if content and "Manual override:" not in content:
        errors.append(f"{path}: description must be marked as Manual override")

readme = require_text("README.md")
if readme and "/initialize" not in readme:
    errors.append("README.md: must mention /initialize as the default Claude setup command")

plugin_readme = require_text("plugin/README.md")
if plugin_readme and "/initialize" not in plugin_readme:
    errors.append("plugin/README.md: must mention /initialize for Claude plugin testing")

if errors:
    for error in errors:
        print(f"ERROR: {error}")
    raise SystemExit(1)

print("agent plugin validation ok")
PY
