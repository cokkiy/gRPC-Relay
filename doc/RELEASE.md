# Release Process

This document describes the **hybrid release process** for gRPC-Relay, combining local control with automated publishing.
The flow works **with a protected master branch** — the version bump goes through a pull request instead of
a direct push.

## Overview

```
┌──────────────────────────────────────────────────────────────────────────┐
│                       Hybrid Release Flow                                 │
│                   (protected-branch compatible)                            │
└──────────────────────────────────────────────────────────────────────────┘

   LOCAL                              GITHUB
  ┌──────────────────────┐         ┌──────────────────────────────────────┐
  │ 1. prepare-          │         │ 3. Code review + PR merge            │
  │    release.sh        │         │    • Review version bump PR          │
  │    • Create branch   │  ─────► │    • Merge to master                 │
  │    • Update Cargo    │         └──────────────┬───────────────────────┘
  │      .toml           │                        │
  │    • cargo check     │                        ▼
  │    • Commit          │         ┌──────────────────────────────────────┐
  │    • Push branch     │         │ 4. create-release.yml (manual)       │
  │    • Open PR         │  ─────► │    • Verify version                 │
  └──────────────────────┘         │    • Run tests                      │
                                   │    • Create tag                     │
   LOCAL (PR review)               │    • Create GitHub release          │
  ┌──────────────────────┐         └──────────────┬───────────────────────┘
  │ 2. Merge PR          │                        │ triggers
  │    via GitHub UI      │  ─────►                ▼
  └──────────────────────┘         ┌──────────────────────────────────────┐
                                   │ 5. release.yml (auto)                │
                                   │    • Publish relay-proto             │
                                   │    • Publish device-sdk              │
                                   │    • Publish controller-sdk          │
                                   │    • Build & push Docker image       │
                                   └──────────────────────────────────────┘
```

## Why Hybrid + PR?

- **Branch Protection** — master is protected; the version bump goes through a PR that CI validates
- **Local Control** — review the diff locally before a single line touches the remote
- **Automated Publishing** — once the PR is merged, a single workflow trigger handles tags, releases, crates.io, and Docker
- **Safety** — version mismatch between `Cargo.toml` and workflow input fails fast; no direct push to master
- **Audit Trail** — PR review history + git history + Actions logs all tied to the same version

## Prerequisites

### One-Time Setup

1. **Local tools**:
   - `bash` (already on macOS/Linux)
   - `git`
   - `gh` (GitHub CLI) — install from https://cli.github.com/
   - `rust` toolchain (`rustup`)

2. **gh authentication**:
   ```bash
   gh auth login
   ```

3. **GitHub secrets** (configured by repo admin):
   - `CARGO_REGISTRY_TOKEN` — For publishing to crates.io
   - `GITHUB_TOKEN` — Auto-provided by GitHub

4. **Permissions**:
   - Push access to the repository
   - Ability to create pull requests
   - Ability to trigger GitHub Actions workflows

5. **Branch protection** (already configured):
   - Direct push to `master` is blocked
   - PRs require CI checks to pass before merge
   - Tags are **not** protected (GitHub Actions can push them)

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
- Validate the version format (SemVer)
- Check for uncommitted changes
- Detect the default branch
- Confirm the tag doesn't already exist
- Create a branch `chore/release-v<version>`
- Update `Cargo.toml` workspace version
- Run `cargo check` to verify the workspace builds
- Update `Cargo.lock`
- Show the diff for review
- Commit the changes on the branch
- Push the branch to origin
- Open a pull request targeting the default branch

**Sample output:**
```
ℹ Running pre-flight checks...
ℹ Default branch: master
ℹ Current version: 1.0.0-alpha
ℹ New version:     1.0.0

This will:
  1. Create branch:            chore/release-v1.0.0
  2. Update Cargo.toml version: 1.0.0-alpha → 1.0.0
  3. Run cargo check to verify the workspace builds
  4. Commit the changes on the branch
  5. Push the branch to origin
  6. Open a pull request targeting master

After PR merge:
  • Trigger the 'Create Release' GitHub workflow with version=1.0.0
  • The workflow creates tag v1.0.0 and publishes artifacts

Proceed? [y/N] y

ℹ Creating branch 'chore/release-v1.0.0' from master...
✓ Created branch 'chore/release-v1.0.0'
ℹ Updating Cargo.toml...
✓ Updated Cargo.toml
ℹ Updating Cargo.lock...
✓ Cargo.lock updated

ℹ Changes to commit:
 Cargo.toml | 2 +-
 Cargo.lock | 2 +-
 2 files changed

Commit these changes? [y/N] y
✓ Committed
ℹ Pushing branch 'chore/release-v1.0.0'...
✓ Pushed
ℹ Creating pull request...
✓ Pull request created: https://github.com/cokkiy/gRPC-Relay/pull/18

✓ Release preparation complete!
```

### Step 2: Review and Merge the PR

1. CI will run automatically on the PR (format, clippy, tests, build)
2. Review the version bump diff
3. Once all checks pass, **merge the PR** (via GitHub UI or CLI)

```bash
# Merge via CLI (if you have permission)
gh pr merge --squash --delete-branch

# Or open in browser
gh pr view --web
```

### Step 3: Trigger the Release Workflow

**Wait until the PR is merged to master**, then trigger:

#### Option A — GitHub Web UI

1. Navigate to: `https://github.com/cokkiy/gRPC-Relay/actions`
2. Click **Create Release** workflow on the left sidebar
3. Click **Run workflow** dropdown (top right)
4. Fill in the form:
   - **Branch**: `master` (the default branch)
   - **Version**: `1.0.0` (without the `v` prefix, must match Cargo.toml)
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
1. Validate version format
2. Verify `Cargo.toml` version matches input
3. Run formatting, linting, and tests
4. Build release binary
5. Verify `relay --version` matches
6. Create git tag `v1.0.0`
7. Create GitHub release with auto-generated notes

Once the release is created, the **Release** workflow automatically triggers and:
1. Publishes `relay-proto` to crates.io
2. Waits for crates.io index propagation
3. Publishes `device-sdk` to crates.io
4. Publishes `controller-sdk` to crates.io
5. Builds Docker image
6. Pushes to GitHub Container Registry (GHCR)

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

### Branch Naming

The script auto-generates branch names:

| Version | Branch |
|---------|--------|
| `1.0.0` | `chore/release-v1.0.0` |
| `1.0.0-rc1` | `chore/release-v1.0.0-rc1` |
| `1.0.1` | `chore/release-v1.0.1` |

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

### During Script Execution (Step 1)
```
Aborted by user. Reverting changes...
```
The script automatically reverts files and checks out the detected default branch.

### After PR Created, Before Merge (Step 2)
```bash
# Close the PR
gh pr close <pr-number>

# Delete the remote branch
git push origin --delete chore/release-v1.0.0

# Delete the local branch
git branch -D chore/release-v1.0.0

# Revert local Cargo.toml if still dirty
git checkout master
git checkout -- Cargo.toml Cargo.lock
```

### After PR Merged, Before Workflow Triggered (Step 3)
```bash
# Create a revert branch from master
git checkout -b revert-release-v1.0.0 origin/master

# Revert the version-bump commit
git revert <merge-commit-hash>

# Push and open a revert PR (master is protected)
git push -u origin revert-release-v1.0.0
gh pr create --title "revert: release bump v1.0.0" --body "Rollback release bump" --base master

# Delete the chore branch if still present
git push origin --delete chore/release-v1.0.0
```

### After Tag Created, Before Crates Published
```bash
# Delete the tag
git tag -d v1.0.0
git push origin :refs/tags/v1.0.0

# Delete the GitHub release
gh release delete v1.0.0 --yes

# Revert the version bump (new PR or direct revert)
```

### After Crates Published (⚠️ Limited Options)

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

### "gh CLI not found" Error

**Cause**: GitHub CLI is not installed.

**Solution**:
```bash
# macOS
brew install gh

# Linux
# See: https://github.com/cli/cli/blob/trunk/docs/install_linux.md

# Authenticate
gh auth login
```

### "gh not authenticated" Error

**Cause**: You haven't logged in to the GitHub CLI.

**Solution**:
```bash
gh auth login
# Follow the prompts (HTTPS + browser is easiest)
```

### "Branch already exists" Error

**Cause**: A previous release attempt left the branch behind.

**Solution**: The script will ask if you want to delete and recreate. Answer `y`.
Or manually:
```bash
git branch -D chore/release-v1.0.0
git push origin --delete chore/release-v1.0.0
```

### "Version mismatch" Error (in workflow)

**Cause**: The version in `Cargo.toml` doesn't match the workflow input.

**Solution**: Did you merge the PR before triggering the workflow?
```bash
# Check what version is actually on master
git fetch origin master
git show origin/master:Cargo.toml | grep '^version = '

# If the PR hasn't been merged yet, merge it first.
# If it has, use the exact version from Cargo.toml as the workflow input.
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
3. **Dependency not yet on crates.io**: Wait and retry (the workflow handles this for relay-proto)

### Binary Version Mismatch

**Cause**: The compiled binary doesn't report the expected version.

**Solution**: Make sure `main.rs` has the version annotation:
```rust
#[command(version = env!("CARGO_PKG_VERSION"))]
```

### Master is Protected — Can the Workflow Still Push Tags?

**Yes.** Branch protection rules apply to branches, not tags.
The `create-release.yml` workflow only pushes a **tag** (`v1.0.0`), not commits to master.
Tags are unprotected by default in GitHub. If you later protect tags, grant the
`GITHUB_TOKEN` (or a PAT) permission via the repo's tag protection rules.

## Release Checklist

Use this checklist for each release:

### Pre-Release
- [ ] All planned features merged to `master`
- [ ] All tests passing on `master` (CI green)
- [ ] Documentation updated
- [ ] `CHANGELOG.md` updated (if maintained)
- [ ] Decided on version number (SemVer)

### Release Process
- [ ] Run `./scripts/prepare-release.sh <version>` locally
- [ ] Review the generated PR description
- [ ] CI passes on the PR (fmt, clippy, tests, build)
- [ ] Merge the PR to master
- [ ] Delete the release branch (auto if squash-merge)
- [ ] Trigger **Create Release** workflow with the same version
- [ ] Wait for **Create Release** workflow to complete
- [ ] Verify **Release** workflow also completes

### Post-Release
- [ ] Verify GitHub release page looks correct
- [ ] Verify crates published to crates.io
- [ ] Verify Docker image on GHCR
- [ ] Test installation via `cargo install ...` (if applicable)
- [ ] Announce the release (Slack, Discord, blog, etc.)
- [ ] Update documentation site (if applicable)

## Examples

### First Release (v1.0.0)

```bash
# 1. Prepare (creates branch, commits, pushes, opens PR)
./scripts/prepare-release.sh 1.0.0

# 2. Review and merge the PR
gh pr view --web

# 3. After merge, trigger release
gh workflow run create-release.yml -f version=1.0.0

# 4. Watch
gh run watch
```

### Release Candidate

```bash
# 1. Prepare
./scripts/prepare-release.sh 1.0.0-rc1

# 2. Merge PR (via GitHub UI or CLI)
gh pr merge --squash --delete-branch

# 3. Trigger as pre-release
gh workflow run create-release.yml \
  -f version=1.0.0-rc1 \
  -f prerelease=true
```

### Patch Release

```bash
# 1. Ensure the fix is on master (via a separate feature PR)

# 2. Prepare the version bump
./scripts/prepare-release.sh 1.0.1

# 3. Merge PR
gh pr merge --squash --delete-branch

# 4. Trigger release
gh workflow run create-release.yml -f version=1.0.1
```

## FAQ

**Q: Why can't I push directly to master?**
A: Branch protection is enabled on master to prevent accidental changes. The release
script creates a PR so CI validates the version bump before it lands.

**Q: Can I skip the script and create the PR manually?**
A: Yes. Create a branch, bump the version in `Cargo.toml`, run `cargo check`, commit,
push, and open a PR. The script just automates these steps with validation.

**Q: What happens if I trigger the workflow before merging the PR?**
A: The workflow will fail with "Version mismatch" because `Cargo.toml` on master
still has the old version. Merge the PR first, then trigger.

**Q: Can I release from a branch other than the default?**
A: The script always branches from the default branch. If you need to release from
another branch, do it manually — but this is uncommon.

**Q: How do I add release notes manually?**
A: Use the `draft: true` option when triggering `create-release.yml`, then edit the
release notes on GitHub before publishing.

**Q: What if crates.io is down?**
A: The `release.yml` workflow will fail at the publish step. Re-run it once crates.io
recovers. The git tag and GitHub release already exist and are fine.

**Q: Can I release multiple versions in a day?**
A: Yes, but be careful with crates.io rate limits. Wait a few minutes between releases.

**Q: What if another feature PR is open while I do a release PR?**
A: The release PR is just a version bump — it won't conflict with feature work
unless both touch `Cargo.toml`. Best practice: do the release PR last, after
all feature PRs for that version are merged.

---

**Last Updated**: 2026-05-14
**Maintainers**: gRPC-Relay Contributors
