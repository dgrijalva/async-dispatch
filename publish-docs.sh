#!/bin/bash
set -euo pipefail

# This project won't build in docs.rs, so we are going to publish
# them as part of the repo. After tagging a version, this script
# can be used to automatically generate and push the docs to the gh-pages branch

VERSION="${1:-}"

if [ -z "$VERSION" ]; then
    echo "Usage: $0 <version>"
    echo "Example: $0 v0.2.0"
    exit 1
fi

# Validate version format
if [[ ! "$VERSION" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "Error: Version must be in format v0.0.0"
    exit 1
fi

REPO_ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$REPO_ROOT"

# Ensure we're on a clean working tree
if [ -n "$(git status --porcelain)" ]; then
    echo "Error: Working tree is not clean. Commit or stash changes first."
    exit 1
fi

CURRENT_BRANCH="$(git branch --show-current)"

# Build the docs. Clean first, so we don't incorporate a bunch of noise
echo "Building documentation..."
cargo clean
cargo doc --no-deps

echo "Switching to gh-pages branch..."
if git show-ref --verify --quiet refs/heads/gh-pages; then
    git checkout gh-pages
else
    # Create orphan gh-pages branch if it doesn't exist
    git checkout --orphan gh-pages
    git rm -rf .
    git commit --allow-empty -m "Initialize gh-pages branch"
fi

# Pull latest if remote exists
if git ls-remote --exit-code --heads origin gh-pages >/dev/null 2>&1; then
    git pull origin gh-pages --rebase
fi

echo "Copying docs to $VERSION/..."
rm -rf "$VERSION"
cp -r target/doc "$VERSION"

# Create/update root index.html with redirect to latest version
cat > index.html << EOF
<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta http-equiv="refresh" content="0; url=$VERSION/async_dispatch/">
    <title>async-dispatch documentation</title>
</head>
<body>
    <p>Redirecting to <a href="$VERSION/async_dispatch/">$VERSION documentation</a>...</p>
</body>
</html>
EOF

echo "Committing..."
git add "$VERSION" index.html
git commit -m "Add documentation for $VERSION"

echo "Pushing to origin..."
git push origin gh-pages

echo "Switching back to $CURRENT_BRANCH..."
git checkout "$CURRENT_BRANCH"

echo ""
echo "Done! Documentation published to:"
echo "  https://dgrijalva.github.io/async-dispatch/async_dispatch/"
echo "  https://dgrijalva.github.io/async-dispatch/$VERSION/async_dispatch/"
