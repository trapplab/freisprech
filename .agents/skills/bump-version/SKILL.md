---
name: bump-version
description: Bump the version of Freisprech and release it. Sets the version the user names in Cargo.toml and Cargo.lock, verifies the build, commits, pushes main and pushes the matching tag, which starts the GitHub release workflow. Use when the user asks to release, bump the version, or cut/tag a version like 0.2.0.
---

# Bump the version

The user names the version, e.g. `0.2.0` or `0.2.0-rc1`. Never pick or guess one yourself;
if none was given, ask. Tags have **no `v` prefix**. Pushing the tag starts
`.github/workflows/release.yml`, which builds the Linux and Windows binaries and publishes
the GitHub release. It fails if the tag does not equal `version` in `Cargo.toml`.

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
```

- The tree isn't clean: stop and name the changed files. Don't stash or commit them.
- The tag already exists, e.g. from a failed release: stop and tell the user. Only move
  or delete a tag when they say so explicitly.
- The version is not higher than the current one: ask before going on.

## 2. Set the version

```sh
sed -i "0,/^version = \".*\"/s//version = \"$VERSION\"/" Cargo.toml
cargo update --workspace             # moves only our own version in Cargo.lock
git diff --stat                      # exactly Cargo.toml and Cargo.lock, 1 line each
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

Give the old and new version, the commit hash, and the links:

- Workflow run: https://github.com/trapplab/freisprech/actions
- Release, once both builds are done (about 10–15 min): https://github.com/trapplab/freisprech/releases/tag/$VERSION

A suffix such as `-rc1` makes it a pre-release.
