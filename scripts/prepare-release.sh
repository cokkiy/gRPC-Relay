#!/usr/bin/env bash
#
# prepare-release.sh - Prepare a new release for gRPC-Relay
#
# This script creates a branch, updates the workspace version in Cargo.toml,
# and opens a pull request. After the PR is merged to master, you trigger
# the "Create Release" GitHub workflow to create the tag and release.
#
# Usage:
#   ./scripts/prepare-release.sh <version>
#
# Examples:
#   ./scripts/prepare-release.sh 1.0.0           # Stable release
#   ./scripts/prepare-release.sh 1.0.0-rc1       # Release candidate
#   ./scripts/prepare-release.sh 1.0.0-beta.1    # Beta release
#

set -euo pipefail

# ─── Colors ──────────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
BOLD='\033[1m'
NC='\033[0m' # No Color

# ─── Helpers ─────────────────────────────────────────────────────────────────
info()    { echo -e "${BLUE}ℹ ${NC}$*"; }
success() { echo -e "${GREEN}✓ ${NC}$*"; }
warn()    { echo -e "${YELLOW}⚠ ${NC}$*"; }
error()   { echo -e "${RED}✗ ${NC}$*" >&2; }
die()     { error "$*"; exit 1; }

# ─── Argument Validation ─────────────────────────────────────────────────────
if [ $# -ne 1 ]; then
    echo -e "${BOLD}Usage:${NC} $0 <version>"
    echo ""
    echo -e "${BOLD}Examples:${NC}"
    echo "  $0 1.0.0           # Stable release"
    echo "  $0 1.0.0-rc1       # Release candidate"
    echo "  $0 1.0.0-beta.1    # Beta release"
    echo "  $0 1.1.0-alpha     # Alpha release"
    echo ""
    echo -e "${BOLD}Version format:${NC} MAJOR.MINOR.PATCH[-PRERELEASE]"
    exit 1
fi

VERSION=$1

if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.]+)?$ ]]; then
    die "Invalid version format: '$VERSION'"
    echo ""
    die "Expected: MAJOR.MINOR.PATCH[-PRERELEASE]"
    echo "Example: 1.0.0 or 1.0.0-rc1"
fi

# ─── Locate Repository Root ──────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CARGO_TOML="$REPO_ROOT/Cargo.toml"

if [ ! -f "$CARGO_TOML" ]; then
    die "Cargo.toml not found at $CARGO_TOML"
fi

cd "$REPO_ROOT"

# ─── Pre-flight Checks ───────────────────────────────────────────────────────
info "Running pre-flight checks..."

if ! git rev-parse --git-dir > /dev/null 2>&1; then
    die "Not a git repository"
fi

REMOTE=$(git remote get-url origin 2>/dev/null || true)
if [ -z "$REMOTE" ]; then
    die "No 'origin' remote configured"
fi

# Check for uncommitted changes
if ! git diff-index --quiet HEAD --; then
    warn "You have uncommitted changes:"
    git status --short
    echo ""
    read -rp "Continue anyway? [y/N] " yn
    if [[ ! $yn =~ ^[Yy]$ ]]; then
        die "Aborted by user"
    fi
fi

# Detect default branch
DEFAULT_BRANCH=$(git remote show origin 2>/dev/null \
    | sed -n '/HEAD branch/s/.*: //p' \
    | head -n1)
if [ -z "$DEFAULT_BRANCH" ]; then
    # Fallback: try common names
    for candidate in master main; do
        if git show-ref --verify --quiet "refs/remotes/origin/$candidate"; then
            DEFAULT_BRANCH=$candidate
            break
        fi
    done
fi
if [ -z "$DEFAULT_BRANCH" ]; then
    die "Could not detect default branch. Set it manually: git remote set-head origin master"
fi

# Check gh CLI is installed and authenticated
if ! command -v gh &> /dev/null; then
    die "'gh' CLI not found. Install it: https://cli.github.com/"
fi
if ! gh auth status &>/dev/null; then
    die "gh not authenticated. Run: gh auth login"
fi

info "Default branch: ${BOLD}$DEFAULT_BRANCH${NC}"

# Check if tag already exists
TAG="v$VERSION"
if git rev-parse "$TAG" >/dev/null 2>&1; then
    die "Tag '$TAG' already exists locally"
fi
if git ls-remote --tags origin "refs/tags/$TAG" 2>/dev/null | grep -q "$TAG"; then
    die "Tag '$TAG' already exists on remote"
fi

# Get current version
CURRENT_VERSION=$(grep -E '^version = "' "$CARGO_TOML" | head -n1 | sed -E 's/version = "(.*)"/\1/')
info "Current version: ${BOLD}$CURRENT_VERSION${NC}"
info "New version:     ${BOLD}$VERSION${NC}"

if [ "$CURRENT_VERSION" = "$VERSION" ]; then
    die "Version is already $VERSION"
fi

# ─── Confirmation ────────────────────────────────────────────────────────────
BRANCH="chore/release-v$VERSION"

echo ""
echo -e "${BOLD}This will:${NC}"
echo "  1. Create branch:            $BRANCH"
echo "  2. Update Cargo.toml version: $CURRENT_VERSION → $VERSION"
echo "  3. Run cargo check to verify the workspace builds"
echo "  4. Commit the changes on the branch"
echo "  5. Push the branch to origin"
echo "  6. Open a pull request targeting $DEFAULT_BRANCH"
echo ""
echo -e "${BOLD}After PR merge:${NC}"
echo "  • Trigger the 'Create Release' GitHub workflow with version=$VERSION"
echo "  • The workflow creates tag v$VERSION and publishes artifacts"
echo ""
read -rp "Proceed? [y/N] " yn
if [[ ! $yn =~ ^[Yy]$ ]]; then
    die "Aborted by user"
fi

# ─── Create Branch ───────────────────────────────────────────────────────────
info "Creating branch '$BRANCH' from $DEFAULT_BRANCH..."

# Fetch latest to avoid stale base
git fetch origin "$DEFAULT_BRANCH" 2>/dev/null || true

# Check if branch already exists
if git show-ref --verify --quiet "refs/heads/$BRANCH"; then
    warn "Branch '$BRANCH' already exists locally"
    read -rp "Delete and recreate? [y/N] " yn
    if [[ $yn =~ ^[Yy]$ ]]; then
        git branch -D "$BRANCH"
    else
        die "Aborted by user"
    fi
fi
if git show-ref --verify --quiet "refs/remotes/origin/$BRANCH"; then
    warn "Branch '$BRANCH' already exists on remote"
    read -rp "Delete remote branch and continue? [y/N] " yn
    if [[ $yn =~ ^[Yy]$ ]]; then
        git push origin --delete "$BRANCH" 2>/dev/null || true
    else
        die "Aborted by user"
    fi
fi

git checkout -b "$BRANCH" "origin/$DEFAULT_BRANCH"
success "Created branch '$BRANCH'"

# ─── Update Cargo.toml ───────────────────────────────────────────────────────
info "Updating Cargo.toml..."

TEMP_FILE=$(mktemp)
trap "rm -f $TEMP_FILE" EXIT

awk -v new_version="$VERSION" '
    /^\[workspace\.package\]/ { in_workspace_package = 1 }
    /^\[/ && !/^\[workspace\.package\]/ { in_workspace_package = 0 }
    in_workspace_package && /^version = "/ {
        print "version = \"" new_version "\""
        next
    }
    { print }
' "$CARGO_TOML" > "$TEMP_FILE"

mv "$TEMP_FILE" "$CARGO_TOML"

NEW_VERSION_CHECK=$(grep -E '^version = "' "$CARGO_TOML" | head -n1 | sed -E 's/version = "(.*)"/\1/')
if [ "$NEW_VERSION_CHECK" != "$VERSION" ]; then
    die "Failed to update version. Got: $NEW_VERSION_CHECK"
fi

success "Updated Cargo.toml"

# ─── Update Cargo.lock ───────────────────────────────────────────────────────
info "Updating Cargo.lock..."
if ! cargo check --workspace --quiet 2>&1; then
    error "cargo check failed — reverting changes"
    git checkout -- "$CARGO_TOML"
    exit 1
fi
success "Cargo.lock updated"

# ─── Commit ──────────────────────────────────────────────────────────────────
echo ""
info "Changes to commit:"
echo ""
git --no-pager diff --stat Cargo.toml Cargo.lock
echo ""

read -rp "Commit these changes? [y/N] " yn
if [[ ! $yn =~ ^[Yy]$ ]]; then
    warn "Aborted by user. Reverting changes..."
    git checkout -- Cargo.toml Cargo.lock
    git checkout "$DEFAULT_BRANCH" 2>/dev/null || true
    die "Aborted. Branch '$BRANCH' still exists — delete it manually if needed."
fi

git add Cargo.toml Cargo.lock
git commit -m "chore: bump version to $VERSION"
success "Committed"

# ─── Push ────────────────────────────────────────────────────────────────────
info "Pushing branch '$BRANCH'..."
git push -u origin "$BRANCH"
success "Pushed"

# ─── Create Pull Request ─────────────────────────────────────────────────────
info "Creating pull request..."

PR_TITLE="chore: bump version to $VERSION"
PR_BODY=$(cat <<PRBODY
Version bump prepared by \`scripts/prepare-release.sh\`.

## Changes
- Bump workspace version: \`$CURRENT_VERSION\` → \`$VERSION\`
- Update \`Cargo.lock\`

## After Merge
Trigger the **Create Release** workflow:
\`\`\`
gh workflow run create-release.yml -f version=$VERSION
\`\`\`
Or go to: ${BLUE}Actions → Create Release${NC} → Run workflow

The workflow will:
- Verify \`Cargo.toml\` version matches \`$VERSION\`
- Run the full test suite
- Create tag \`v$VERSION\`
- Create GitHub release
- Trigger crates.io + Docker publishing
PRBODY
)

PR_URL=$(gh pr create \
    --title "$PR_TITLE" \
    --body "$PR_BODY" \
    --base "$DEFAULT_BRANCH" \
    --head "$BRANCH" \
    2>&1) || {
    error "Failed to create PR: $PR_URL"
    echo ""
    echo -e "Create it manually:"
    echo -e "  ${YELLOW}gh pr create --title \"$PR_TITLE\" --base \"$DEFAULT_BRANCH\" --head \"$BRANCH\"${NC}"
    exit 1
}

# Extract URL from gh output (handles both formats)
PR_NUMBER=$(echo "$PR_URL" | grep -oE 'https://github.com/[^ ]+/pull/[0-9]+' | head -n1)
if [ -z "$PR_NUMBER" ]; then
    PR_NUMBER="$PR_URL"
fi

success "Pull request created: $PR_NUMBER"

# ─── Next Steps ──────────────────────────────────────────────────────────────
echo ""
echo -e "${GREEN}${BOLD}✓ Release preparation complete!${NC}"
echo ""
echo -e "${BOLD}Next steps:${NC}"
echo ""
echo -e "  ${BLUE}1.${NC} Review and merge the PR:"
echo -e "     ${YELLOW}$PR_NUMBER${NC}"
echo ""
echo -e "  ${BLUE}2.${NC} After merge, trigger the release workflow:"
echo ""
echo -e "     ${BOLD}Option A — GitHub UI:${NC}"
echo -e "       • Go to: ${BLUE}Actions → Create Release${NC}"
echo -e "       • Click ${BOLD}Run workflow${NC}"
echo -e "       • Select branch: ${BOLD}$DEFAULT_BRANCH${NC}"
echo -e "       • Enter version: ${BOLD}$VERSION${NC}"
echo -e "       • Click ${BOLD}Run workflow${NC}"
echo ""
echo -e "     ${BOLD}Option B — GitHub CLI:${NC}"
echo -e "       ${YELLOW}gh workflow run create-release.yml -f version=$VERSION${NC}"
echo ""
echo -e "  ${BLUE}3.${NC} The workflow will:"
echo "     • Verify version matches Cargo.toml"
echo -e "     • Create git tag ${BOLD}$TAG${NC}"
echo "     • Create GitHub release with auto-generated notes"
echo "     • Trigger the publish workflow (crates.io + Docker)"
echo ""
