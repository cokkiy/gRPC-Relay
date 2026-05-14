# Release Process

This document describes the **hybrid release process** for gRPC-Relay, combining local control with automated publishing.

## Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                        Hybrid Release Flow                          │
└─────────────────────────────────────────────────────────────────────┘

   LOCAL                          GITHUB
  ┌──────────────────┐         ┌──────────────────────────────────┐
  │ 1. prepare-      │         │ 3. create-release.yml            │
  │    release.sh    │         │    • Verify version              │
  │    • Update      │  ─────► │    • Run tests                   │
  │      Cargo.toml  │         │    • Create tag                  │
  │    • Run check   │         │    • Create release              │
  │    • Show diff   │         └──────────┬───────────────────────┘
  └────────┬─────────┘                    │
           │                              │ triggers
           ▼                              ▼
  ┌──────────────────┐         ┌──────────────────────────────────┐
  │ 2. Commit & push │         │ 4. release.yml                   │
  │    • Review      │         │    • Publish relay-proto         │
  │    • Commit      │         │    • Publish device-sdk          │
  │    • Push        │         │    • Publish controller-sdk     │
  └──────────────────┘         │    • Build & push Docker image   │
                               └──────────────────────────────────┘
```

## Why Hybrid?

This approach combines the best of both worlds:

- **Local Control** — Review changes before committing, work offline
- **Automated Publishing** — Consistent, reliable, no manual mistakes
- **Safety** — Multiple checkpoints prevent accidental releases
- **Audit Trail** — Both git history and GitHub Actions logs

## Prerequisites

### One-Time Setup

1. **Local tools**:
   - `bash` (already on macOS/Linux)
   - `git`
   - `rust` toolchain (`rustup`)
   - Optional: `gh` (GitHub CLI) for triggering workflows from terminal

2. **GitHub secrets** (configured by repo admin):
   - `CARGO_REGISTRY_TOKEN` — For publishing to crates.io
   - `GITHUB_TOKEN` — Auto-provided by GitHub

3. **Permissions**:
   - Push access to the repository
   - Ability to trigger GitHub Actions workflows

## Release Steps

### Step 1: Prepare the Release (Local)

Run the preparation script with the target version:

```bash
# Stable release
./scripts/prepare-release.sh 1.0.0

# Release candidate
./scripts/prepare-release.sh 1.0.0-rc1

# Beta release
./scripts/prepare-release.sh 1.0.0-beta.1

# Alpha release
./scripts/prepare-release.sh 1.1.0-alpha
```

The script will:
- ✅ Validate the version format (SemVer)
- ✅ Check for uncommitted changes
- ✅ Verify you're on `master`/`main` branch
- ✅ Confirm the tag doesn't already exist
- ✅ Update `Cargo.toml` workspace version
- ✅ Run `cargo check` to verify it builds
- ✅ Update `Cargo.lock`
- ✅ Show you the diff for review

**Sample output:**
```
ℹ Running pre-flight checks...
ℹ Current version: 1.0.0-alpha
ℹ New version:     1.0.0

This will:
  1. Update Cargo.toml version: 1.0.0-alpha → 1.0.0
  2. Run cargo check to verify the workspace builds
  3. Show you the diff for review

It will NOT:
  • Commit the changes (you do that manually)
  • Create the tag (GitHub workflow does that)
  • Push anything

Proceed? [y/N] y

ℹ Updating Cargo.toml...
✓ Updated Cargo.toml
ℹ Updating Cargo.lock...
✓ Cargo.lock updated

ℹ Changes to be committed:

diff --git a/Cargo.toml b/Cargo.toml
-version = "1.0.0-alpha"
+version = "1.0.0"

✓ Release preparation complete!
```

### Step 2: Commit and Push (Local)

Review the changes, then commit and push:

```bash
# Review the diff one more time
git diff Cargo.toml Cargo.lock

# Commit
git add Cargo.toml Cargo.lock
git commit -m "chore: bump version to 1.0.0"

# Push to remote
git push origin master
```

### Step 3: Trigger the Release Workflow (GitHub)

You have two options to trigger the release:

#### Option A — GitHub Web UI

1. Navigate to: `https://github.com/cokkiy/gRPC-Relay/actions`
2. Click **Create Release** workflow on the left sidebar
3. Click **Run workflow** dropdown (top right)
4. Fill in the form:
   - **Branch**: `master` (or your release branch)
   - **Version**: `1.0.0` (without the `v` prefix)
   - **Mark as pre-release**: Check if RC/beta/alpha
   - **Create as draft**: Check if you want to review before publishing
5. Click **Run workflow**

#### Option B — GitHub CLI

```bash
# Stable release
gh workflow run create-release.yml -f version=1.0.0

# Pre-release
gh workflow run create-release.yml \
  -f version=1.0.0-rc1 \
  -f prerelease=true

# Draft release (review before publishing)
gh workflow run create-release.yml \
  -f version=1.0.0 \
  -f draft=true
```

### Step 4: Monitor the Workflow

Watch the workflow progress:

```bash
# Watch with GitHub CLI
gh run watch

# Or check status
gh run list --workflow=create-release.yml
```

The **Create Release** workflow will:
1. ✅ Validate version format
2. ✅ Verify `Cargo.toml` version matches input
3. ✅ Run formatting, linting, and tests
4. ✅ Build release binary
5. ✅ Verify `relay --version` matches
6. ✅ Create git tag `v1.0.0`
7. ✅ Create GitHub release with auto-generated notes

Once the release is created, the **Release** workflow automatically triggers and:
1. ✅ Publishes `relay-proto` to crates.io
2. ✅ Waits for crates.io index propagation
3. ✅ Publishes `device-sdk` to crates.io
4. ✅ Publishes `controller-sdk` to crates.io
5. ✅ Builds Docker image
6. ✅ Pushes to GitHub Container Registry (GHCR)

### Step 5: Verify the Release

After both workflows complete:

```bash
# Check the GitHub release
gh release view v1.0.0

# Check crates.io
curl -s https://crates.io/api/v1/crates/relay-proto | jq '.crate.max_version'
curl -s https://crates.io/api/v1/crates/device-sdk | jq '.crate.max_version'
curl -s https://crates.io/api/v1/crates/controller-sdk | jq '.crate.max_version'

# Check Docker image
docker pull ghcr.io/cokkiy/grpc-relay:v1.0.0
docker run --rm ghcr.io/cokkiy/grpc-relay:v1.0.0 --version
```

## Version Numbering

We follow [Semantic Versioning 2.0.0](https://semver.org/):

```
MAJOR.MINOR.PATCH[-PRERELEASE]

Examples:
  1.0.0           Stable release
  1.0.1           Patch (bug fixes)
  1.1.0           Minor (new features, backwards compatible)
  2.0.0           Major (breaking changes)
  1.0.0-alpha     Alpha (early testing)
  1.0.0-beta.1    Beta (feature complete, testing)
  1.0.0-rc1       Release candidate
```

### When to Bump

| Change Type | Version Bump | Example |
|-------------|--------------|---------|
| Bug fix | PATCH | `1.0.0` → `1.0.1` |
| New feature (compatible) | MINOR | `1.0.0` → `1.1.0` |
| Breaking change | MAJOR | `1.0.0` → `2.0.0` |
| Pre-release testing | PRERELEASE | `1.0.0` → `1.0.0-rc1` |

## Pre-Release vs Stable

### Pre-Release (alpha, beta, rc)
- Mark as `prerelease=true` in workflow
- Won't be marked as "Latest" on GitHub
- Still publishes to crates.io (with pre-release tag)
- Used for: testing, gathering feedback, RC validation

### Stable
- Mark as `prerelease=false` (default)
- Becomes the "Latest" release on GitHub
- Published to crates.io as the new latest version
- Used for: production releases

## Rollback Procedures

### If Something Goes Wrong

#### Before Pushing (Step 1-2)
```bash
# Revert local changes
git checkout -- Cargo.toml Cargo.lock
```

#### After Pushing, Before Workflow (Step 2-3)
```bash
# Revert the version bump commit
git revert HEAD
git push origin master
```

#### After Tag Created, Before Crates Published
```bash
# Delete the tag
git tag -d v1.0.0
git push origin :refs/tags/v1.0.0

# Delete the GitHub release
gh release delete v1.0.0 --yes

# Revert the version bump
git revert <commit-hash>
git push origin master
```

#### After Crates Published (⚠️ Limited Options)

**crates.io does NOT allow deleting versions.** You can only:

1. **Yank** the version (prevents new dependencies on it):
   ```bash
   cargo yank --version 1.0.0 --package relay-proto
   cargo yank --version 1.0.0 --package device-sdk
   cargo yank --version 1.0.0 --package controller-sdk
   ```

2. **Release a patch version** with the fix:
   ```bash
   ./scripts/prepare-release.sh 1.0.1
   ```

3. **Delete the Docker image** from GHCR (via web UI or `gh api`)

## Troubleshooting

### "Version mismatch" Error

**Cause**: The version in `Cargo.toml` doesn't match the workflow input.

**Solution**:
```bash
# Make sure you ran the script and committed:
./scripts/prepare-release.sh 1.0.0
git add Cargo.toml Cargo.lock
git commit -m "chore: bump version to 1.0.0"
git push
# Then trigger the workflow with version=1.0.0
```

### "Tag already exists" Error

**Cause**: Someone (or a previous run) already created this tag.

**Solution**:
```bash
# Check existing tags
git tag -l "v*"

# Use a different version or delete the existing tag (only if no release was created)
git tag -d v1.0.0
git push origin :refs/tags/v1.0.0
```

### "cargo publish" Fails

**Common causes**:
1. **Token expired**: Renew `CARGO_REGISTRY_TOKEN` in repo secrets
2. **Version already published**: Bump to next patch version
3. **Dependency not yet on crates.io**: Wait and retry (script handles this for relay-proto)

### Binary Version Mismatch

**Cause**: The compiled binary doesn't report the expected version.

**Solution**: Make sure `main.rs` has the version annotation:
```rust
#[command(version = env!("CARGO_PKG_VERSION"))]
```

## Release Checklist

Use this checklist for each release:

### Pre-Release
- [ ] All planned features merged to `master`
- [ ] All tests passing on `master`
- [ ] Documentation updated
- [ ] `CHANGELOG.md` updated (if maintained)
- [ ] Decided on version number (SemVer)

### Release Process
- [ ] Run `./scripts/prepare-release.sh <version>`
- [ ] Review the diff
- [ ] Commit and push the version bump
- [ ] Trigger **Create Release** workflow
- [ ] Wait for workflow to complete successfully
- [ ] Verify **Release** workflow also completes

### Post-Release
- [ ] Verify GitHub release page looks correct
- [ ] Verify crates published to crates.io
- [ ] Verify Docker image on GHCR
- [ ] Test installation: `cargo install ...` (if applicable)
- [ ] Announce the release (Slack, Discord, blog, etc.)
- [ ] Update documentation site (if applicable)

## Examples

### First Release (v1.0.0)

```bash
# 1. Prepare
./scripts/prepare-release.sh 1.0.0

# 2. Commit
git add Cargo.toml Cargo.lock
git commit -m "chore: bump version to 1.0.0"
git push origin master

# 3. Trigger workflow
gh workflow run create-release.yml -f version=1.0.0

# 4. Watch
gh run watch
```

### Release Candidate

```bash
# 1. Prepare
./scripts/prepare-release.sh 1.0.0-rc1

# 2. Commit
git add Cargo.toml Cargo.lock
git commit -m "chore: bump version to 1.0.0-rc1"
git push origin master

# 3. Trigger as pre-release
gh workflow run create-release.yml \
  -f version=1.0.0-rc1 \
  -f prerelease=true
```

### Patch Release

```bash
# 1. Cherry-pick or merge the fix
git cherry-pick <commit-hash>

# 2. Prepare
./scripts/prepare-release.sh 1.0.1

# 3. Commit and push
git add Cargo.toml Cargo.lock
git commit -m "chore: bump version to 1.0.1"
git push origin master

# 4. Trigger
gh workflow run create-release.yml -f version=1.0.1
```

## FAQ

**Q: Can I skip the local script and just bump the version in Cargo.toml manually?**
A: Yes, but the script provides validation and safety checks. The workflow will still verify the version matches.

**Q: What if I want to release without publishing to crates.io?**
A: Modify the `release.yml` workflow to add a `skip-publish` input, or disable the workflow temporarily.

**Q: Can I release from a branch other than master?**
A: Yes, but the script will warn you. Make sure your release branch has all necessary changes.

**Q: How do I add release notes manually?**
A: Use the `draft: true` option, then edit the release on GitHub before publishing.

**Q: What if crates.io is down?**
A: The workflow will fail. Retry after crates.io recovers. The git tag and GitHub release will remain.

**Q: Can I release multiple versions in a day?**
A: Yes, but be careful with crates.io rate limits. Wait a few minutes between releases.

---

**Last Updated**: 2026-05-14
**Maintainers**: gRPC-Relay Contributors
