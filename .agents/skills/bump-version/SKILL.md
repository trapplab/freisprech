---
name: bump-version
description: Bump the version of Freisprech and release it. Sets the version the user names in Cargo.toml and Cargo.lock, turns the Unreleased section of CHANGELOG.md into that version, verifies the build, commits, pushes main and pushes the matching tag, which starts the GitHub release workflow. Use when the user asks to release, bump the version, or cut/tag a version like 0.2.0.
---

# Bump the version

The user names the version, e.g. `0.2.0` or `0.2.0-rc1`. Never pick or guess one yourself;
if none was given, ask. Tags have **no `v` prefix**. Pushing the tag starts
`.github/workflows/release.yml`, which builds the Linux and Windows binaries and publishes
the GitHub release, with the version's section of `CHANGELOG.md` as release notes. It fails
if the tag does not equal `version` in `Cargo.toml` or the changelog has no section for it.

Invoking this skill with a version is the go-ahead for all steps below, including the
pushes. Run them in order from the repo root and **stop at the first failure or surprise**:
report what happened and don't try to repair it on your own.

## 1. Check the preconditions

```sh
VERSION=<version from the user>
echo "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$'  # valid, no "v"
git branch --show-current            # must be: main
git status --porcelain               # must be empty
git fetch origin --tags
git status -sb | head -1             # must not say "behind"
git tag -l "$VERSION"                # must be empty
git ls-remote --tags origin "$VERSION"  # must be empty
grep -m1 '^version' Cargo.toml       # current version, for the report
scripts/changelog-section.sh Unreleased  # what goes into the release
```

- The tree isn't clean: stop and name the changed files. Don't stash or commit them.
- The tag already exists, e.g. from a failed release: stop and tell the user. Only move
  or delete a tag when they say so explicitly.
- The version is not higher than the current one: ask before going on.
- `Unreleased` in `CHANGELOG.md` has no entries: stop and ask the user what changed. Don't
  write entries from the commit log on your own.

## 2. Set the version

```sh
sed -i "0,/^version = \".*\"/s//version = \"$VERSION\"/" Cargo.toml
cargo update --workspace             # moves only our own version in Cargo.lock

# CHANGELOG.md: "Unreleased" becomes the version, above it a new empty "Unreleased";
# the compare links at the bottom move along.
sed -i "s/^## \[Unreleased\]\$/## [Unreleased]\n\n## [$VERSION] - $(date +%F)/" CHANGELOG.md
sed -i -E "s#^\[Unreleased\]: (.*)/compare/(.*)\.\.\.HEAD\$#[Unreleased]: \1/compare/$VERSION...HEAD\n[$VERSION]: \1/compare/\2...$VERSION#" CHANGELOG.md
scripts/changelog-section.sh "$VERSION"  # the release notes: must match step 1

git diff --stat                      # Cargo.toml and Cargo.lock 1 line each, plus CHANGELOG.md
```

## 3. Verify the build

As required by `AGENTS.md`:

```sh
cargo check --locked
cargo check --locked --target x86_64-pc-windows-msvc
cargo clippy --locked
```

## 4. Commit, push, tag

```sh
git commit -am "release $VERSION"
git push origin main
git tag "$VERSION"
git push origin "$VERSION"
```

## 5. Report

Give the old and new version, the commit hash, the release notes from `CHANGELOG.md`,
and the links:

- Workflow run: https://github.com/trapplab/freisprech/actions
- Release, once both builds are done (about 10–15 min): https://github.com/trapplab/freisprech/releases/tag/$VERSION

A suffix such as `-rc1` makes it a pre-release.
