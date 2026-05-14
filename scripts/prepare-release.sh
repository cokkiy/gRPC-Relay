#!/usr/bin/env bash
#
# prepare-release.sh - Prepare a new release for gRPC-Relay
#
# This script updates the workspace version in Cargo.toml and prepares
# everything for a release. It does NOT push or create tags - that's
# done by the GitHub workflow after you push the version bump commit.
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
info() {
    echo -e "${BLUE}ℹ ${NC}$*"
}

success() {
    echo -e "${GREEN}✓ ${NC}$*"
}

warn() {
    echo -e "${YELLOW}⚠ ${NC}$*"
}

error() {
    echo -e "${RED}✗ ${NC}$*" >&2
}

die() {
    error "$*"
    exit 1
}

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

# Validate version format (SemVer compliant)
if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.]+)?$ ]]; then
    die "Invalid version format: '$VERSION'\n  Expected: MAJOR.MINOR.PATCH[-PRERELEASE]\n  Example: 1.0.0 or 1.0.0-rc1"
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

# Check if we're in a git repository
if ! git rev-parse --git-dir > /dev/null 2>&1; then
    die "Not a git repository"
fi

# Check for uncommitted changes
if ! git diff-index --quiet HEAD --; then
    warn "You have uncommitted changes:"
    git status --short
    echo ""
    read -p "Continue anyway? [y/N] " -n 1 -r
    echo ""
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        die "Aborted by user"
    fi
fi

# Check current branch
CURRENT_BRANCH=$(git rev-parse --abbrev-ref HEAD)
if [ "$CURRENT_BRANCH" != "master" ] && [ "$CURRENT_BRANCH" != "main" ]; then
    warn "You are on branch '$CURRENT_BRANCH' (not master/main)"
    read -p "Continue anyway? [y/N] " -n 1 -r
    echo ""
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        die "Aborted by user"
    fi
fi

# Check if tag already exists
TAG="v$VERSION"
if git rev-parse "$TAG" >/dev/null 2>&1; then
    die "Tag '$TAG' already exists"
fi

# Get current version
CURRENT_VERSION=$(grep -E '^version = "' "$CARGO_TOML" | head -n1 | sed -E 's/version = "(.*)"/\1/')
info "Current version: ${BOLD}$CURRENT_VERSION${NC}"
info "New version:     ${BOLD}$VERSION${NC}"

if [ "$CURRENT_VERSION" = "$VERSION" ]; then
    die "Version is already $VERSION"
fi

# ─── Confirmation ────────────────────────────────────────────────────────────
echo ""
echo -e "${BOLD}This will:${NC}"
echo "  1. Update Cargo.toml version: $CURRENT_VERSION → $VERSION"
echo "  2. Run cargo check to verify the workspace builds"
echo "  3. Show you the diff for review"
echo ""
echo -e "${BOLD}It will NOT:${NC}"
echo "  • Commit the changes (you do that manually)"
echo "  • Create the tag (GitHub workflow does that)"
echo "  • Push anything"
echo ""
read -p "Proceed? [y/N] " -n 1 -r
echo ""
if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    die "Aborted by user"
fi

# ─── Update Cargo.toml ───────────────────────────────────────────────────────
info "Updating Cargo.toml..."

# Use a temporary file for atomic update
TEMP_FILE=$(mktemp)
trap "rm -f $TEMP_FILE" EXIT

# Update only the workspace.package version (first occurrence after [workspace.package])
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

# Verify the change
NEW_VERSION_CHECK=$(grep -E '^version = "' "$CARGO_TOML" | head -n1 | sed -E 's/version = "(.*)"/\1/')
if [ "$NEW_VERSION_CHECK" != "$VERSION" ]; then
    die "Failed to update version. Got: $NEW_VERSION_CHECK"
fi

success "Updated Cargo.toml"

# ─── Update Cargo.lock ───────────────────────────────────────────────────────
info "Updating Cargo.lock..."
if ! cargo check --workspace --quiet 2>&1; then
    error "cargo check failed - reverting changes"
    git checkout -- "$CARGO_TOML"
    exit 1
fi
success "Cargo.lock updated"

# ─── Show Diff ───────────────────────────────────────────────────────────────
echo ""
info "Changes to be committed:"
echo ""
git --no-pager diff Cargo.toml Cargo.lock
echo ""

# ─── Next Steps ──────────────────────────────────────────────────────────────
echo ""
echo -e "${GREEN}${BOLD}✓ Release preparation complete!${NC}"
echo ""
echo -e "${BOLD}Next steps:${NC}"
echo ""
echo -e "  ${BLUE}1.${NC} Review the changes above"
echo ""
echo -e "  ${BLUE}2.${NC} Commit the version bump:"
echo -e "     ${YELLOW}git add Cargo.toml Cargo.lock${NC}"
echo -e "     ${YELLOW}git commit -m \"chore: bump version to $VERSION\"${NC}"
echo ""
echo -e "  ${BLUE}3.${NC} Push to remote:"
echo -e "     ${YELLOW}git push origin $CURRENT_BRANCH${NC}"
echo ""
echo -e "  ${BLUE}4.${NC} Trigger the release workflow:"
echo ""
echo -e "     ${BOLD}Option A — GitHub UI:${NC}"
echo -e "       • Go to: ${BLUE}Actions → Create Release${NC}"
echo -e "       • Click ${BOLD}Run workflow${NC}"
echo -e "       • Enter version: ${BOLD}$VERSION${NC}"
echo -e "       • Click ${BOLD}Run workflow${NC}"
echo ""
echo -e "     ${BOLD}Option B — GitHub CLI:${NC}"
echo -e "       ${YELLOW}gh workflow run create-release.yml -f version=$VERSION${NC}"
echo ""
echo -e "  ${BLUE}5.${NC} The workflow will:"
echo "     • Verify version matches Cargo.toml"
echo -e "     • Create git tag ${BOLD}$TAG${NC}"
echo "     • Create GitHub release with auto-generated notes"
echo "     • Trigger the publish workflow (crates.io + Docker)"
echo ""
echo -e "${BOLD}To abort:${NC}"
echo -e "  ${YELLOW}git checkout -- Cargo.toml Cargo.lock${NC}"
echo ""
