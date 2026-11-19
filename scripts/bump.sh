#!/usr/bin/env bash
set -euo pipefail

# Usage: ./scripts/bump.sh 3.32.0
# Updates version in all files, commits, tags, and pushes.
# Write changelog in README.md BEFORE running this script.

VERSION="${1:-}"

if [ -z "$VERSION" ]; then
    echo "Usage: $0 <version>"
    echo "Example: $0 3.32.0"
    echo ""
    echo "Write changelog in README.md first, then run this script."
    exit 1
fi

# Validate version format
if ! echo "$VERSION" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$'; then
    echo "Error: version must be in format X.Y.Z (got: $VERSION)"
    exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# Detect current version from Cargo.toml
CURRENT=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')
echo "Bumping $CURRENT → $VERSION"

# 1. Cargo.toml
sed -i '' "s/^version = \"$CURRENT\"/version = \"$VERSION\"/" Cargo.toml
echo "  ✓ Cargo.toml"

# 2. README.md title
sed -i '' "s/# ast-index v$CURRENT/# ast-index v$VERSION/" README.md
echo "  ✓ README.md"

# 3. plugin/.claude-plugin/plugin.json
sed -i '' "s/\"version\": \"$CURRENT\"/\"version\": \"$VERSION\"/" plugin/.claude-plugin/plugin.json
echo "  ✓ plugin/.claude-plugin/plugin.json"

# 4. plugin/.codex-plugin/plugin.json
sed -i '' "s/\"version\": \"$CURRENT\"/\"version\": \"$VERSION\"/" plugin/.codex-plugin/plugin.json
echo "  ✓ plugin/.codex-plugin/plugin.json"

# 5. plugin/.cursor-plugin/plugin.json
sed -i '' "s/\"version\": \"$CURRENT\"/\"version\": \"$VERSION\"/" plugin/.cursor-plugin/plugin.json
echo "  ✓ plugin/.cursor-plugin/plugin.json"

# 6. .claude-plugin/plugin.json
sed -i '' "s/\"version\": \"$CURRENT\"/\"version\": \"$VERSION\"/" .claude-plugin/plugin.json
echo "  ✓ .claude-plugin/plugin.json"

# 7. .claude-plugin/marketplace.json
sed -i '' "s/\"version\": \"$CURRENT\"/\"version\": \"$VERSION\"/" .claude-plugin/marketplace.json
echo "  ✓ .claude-plugin/marketplace.json"

# 7b. Grok Build plugin manifest and marketplace
sed -i '' "s/\"version\": \"$CURRENT\"/\"version\": \"$VERSION\"/" plugin/.grok-plugin/plugin.json
echo "  ✓ plugin/.grok-plugin/plugin.json"
sed -i '' "s/\"version\": \"$CURRENT\"/\"version\": \"$VERSION\"/" .grok-plugin/marketplace.json
echo "  ✓ .grok-plugin/marketplace.json"

# 8. npm/package.json (version + optionalDependencies)
if [ -f npm/package.json ]; then
    sed -i '' "s/$CURRENT/$VERSION/g" npm/package.json
    echo "  ✓ npm/package.json"
fi

# 9. npm platform packages
for pkg in npm/platforms/*/package.json; do
    if [ -f "$pkg" ]; then
        sed -i '' "s/$CURRENT/$VERSION/g" "$pkg"
        echo "  ✓ $pkg"
    fi
done

# 8. Build and test
echo ""
echo "Building release..."
cargo build --release
echo ""
echo "Running tests..."
cargo test --quiet

# Verify version
BUILT_VERSION=$(./target/release/ast-index version 2>&1)
echo ""
echo "Built: $BUILT_VERSION"

# 9. Commit, tag, push
echo ""
echo "Committing..."
git add Cargo.toml Cargo.lock README.md \
    plugin/.claude-plugin/plugin.json \
    plugin/.codex-plugin/plugin.json \
    plugin/.cursor-plugin/plugin.json \
    .claude-plugin/plugin.json \
    .claude-plugin/marketplace.json \
    plugin/.grok-plugin/plugin.json \
    .grok-plugin/marketplace.json \
    npm/package.json npm/platforms/*/package.json 2>/dev/null || true
git commit -m "Bump to v$VERSION"
git tag "v$VERSION"
git push origin main --tags

echo ""
echo "Done! v$VERSION released."
echo "GitHub Actions will build and publish the release."
