# Release Process

This document outlines the steps to create a new release.

## Prerequisites

- Write access to the repository
- Git configured with your credentials
- Review our [contributing guideline](https://github.com/awslabs/iam-policy-autopilot/blob/main/CONTRIBUTING.md)

## Quick Reference

```bash
# Check existing tags
git tag -l --sort=-v:refname

# Check if this is first release
git tag | wc -l  # Returns 0 if no tags exist
```

## Release Steps

### 1. Create a Release Branch

Create a new branch from `main` for the release:

```bash
# Fetch latest changes
git checkout main
git pull origin main

# Create release branch (replace X.Y.Z with version number)
git checkout -b release/X.Y.Z
```
*Note: It's recommended to create the release branch directly in the awslabs/iam-policy-autopilot rather than in a forked repo.*

#### Optional: Cherry-pick Specific Commits

If you need to include only specific commits instead of all changes from `main`:

```bash
# Create release branch from a specific base (e.g., last release tag)
git checkout -b release/X.Y.Z 0.1.0

# Cherry-pick specific commits
git cherry-pick <commit-hash-1>
git cherry-pick <commit-hash-2>

# Or cherry-pick a range of commits
git cherry-pick <start-commit>..<end-commit>

# If conflicts occur, resolve them and continue
git cherry-pick --continue

# To abort cherry-pick if needed
git cherry-pick --abort
```

**Finding commits to cherry-pick:**
```bash
# View commits since last release
git log 0.1.0..main --oneline

# View commits by author
git log --author="username" --oneline

# View commits with specific prefix
git log --grep="^fix:" --oneline
```

### 2. Update Version

Update the [version](https://doc.rust-lang.org/cargo/reference/semver.html) in both `Cargo.toml` and `pyproject.toml`:

```bash
# Edit Cargo.toml - update version field under [workspace.package]
# Change: version = "0.1.0"
# To:     version = "X.Y.Z"

# Edit pyproject.toml - update version field under [project]
# Change: version = "0.1.0"
# To:     version = "X.Y.Z"
```

For pre-release versions, use SemVer format in Cargo.toml (e.g., `X.Y.Z-rc.1`) and PEP 440 format in pyproject.toml (e.g., `X.Y.Zrc1`). Maturin normalizes the Cargo version to PEP 440 automatically. PyPI will not serve pre-releases to users running `pip install iam-policy-autopilot` unless they explicitly opt in with `--pre` or pin the exact version.

Verify the version is correct:

```bash
cargo build
./target/debug/iam-policy-autopilot --version
```

### 3. Finalize Changelog

Rename the `[Unreleased]` section to the new version and date, then add a fresh empty `[Unreleased]` heading at the top.

### 4. Commit and Push Changes

Commit the version and changelog updates:

```bash
# Stage changes
git add Cargo.toml pyproject.toml CHANGELOG.md

# Commit with descriptive message
git commit -m "chore: bump version to X.Y.Z"

# Push the release branch
git push origin release/X.Y.Z
```

### 5. Create Pull Request

Create a PR from the release branch to `main`:

```bash
# Using GitHub CLI (if installed)
gh pr create --base main --head release/X.Y.Z \
  --title "Release X.Y.Z" \
  --body "Release version X.Y.Z

## Changes
- Updated version to X.Y.Z
- Updated CHANGELOG.md

## Checklist
- [ ] Version updated in Cargo.toml and pyproject.toml
- [ ] Changelog updated
- [ ] All tests passing
- [ ] Ready for release"
```

Or manually create the PR through the GitHub web interface.

### 6. Merge and Create Release

#### Using the GitHub Web Interface

It's recommended to create the new release and tag directly via the GitHub web interface, where you can automatically generate release notes, create a tag, and draft a release before publishing it.

Notes:
- The new tag should be the same as the version to be released
- Make sure to select the correct release branch as the target when creating the tag
  - The main branch can be used if it's identical to the release branch (i.e., no cherry-picked commits in the release branch)
- Be sure to `Save draft` and review it once before publishing the release.


### Automated Build and Publish 
   
Once a release is published, the GitHub Actions workflow (`build_and_publish.yml`) will automatically:
- Build wheels for all supported platforms (Linux, Windows, macOS)
- Test the wheels on each platform
- Verify version matches the release tag
- Publish to PyPI (if tests pass)

Monitor the workflow progress at: `https://github.com/awslabs/iam-policy-autopilot/actions`

## Post-Release

1. Verify the release on PyPI: `https://pypi.org/project/iam-policy-autopilot/`
2. Test installation: `pip install iam-policy-autopilot==X.Y.Z`

## Troubleshooting

### Build Failures

If the automated build fails:
- Check the GitHub Actions logs
- Ensure all tests pass locally: `cargo test --workspace`
- Verify version consistency in Cargo.toml and pyproject.toml

### PyPI Publishing Issues

If PyPI publishing fails:
- Verify the `Release` environment is configured in GitHub repository settings
- Check that trusted publishing is set up correctly
- Ensure version doesn't already exist on PyPI

### Version Mismatch

If version verification fails:
- Ensure both `Cargo.toml` and `pyproject.toml` versions match the git tag exactly
- Rebuild and verify: `cargo build && ./target/debug/iam-policy-autopilot --version`

### Checking Existing Tags

To view existing tags in your repository:

```bash
# List all tags
git tag

# List tags with dates (sorted by version)
git tag -l --sort=-v:refname

# Show tag details
git show <tag-name>

# List tags with commit messages
git tag -n

# Fetch tags from remote
git fetch --tags
```

### Syncing Local Tags with Remote

To sync your local tags with remote (fetch new tags and remove deleted ones):

```bash
# Fetch all tags from remote
git fetch --tags

# Remove local tags that don't exist on remote (prune)
git fetch --prune --prune-tags origin

# Or combine both operations
git fetch --tags --prune --prune-tags origin

# Compare local vs remote tags
git ls-remote --tags origin
```
