---
name: create-release
description: Cut a versioned release for the lilctx Rust project. Bumps Cargo.toml to the next semver version, refreshes Cargo.lock, prepends changes since the last tag to CHANGELOG.md (Keep-a-Changelog format), commits with "release: vX.Y.Z", tags the commit, and asks before pushing. Use when the user says "cut a release", "release v1.2.3", "tag a new version", "bump the version", "ship a release", or "do a patch/minor/major release".
---

# Create release

Cut a tagged release. The pipeline below is the *single source of truth* — do not skip steps. The whole flow takes one minute when nothing surprising surfaces.

## 0. Preflight

Before any edits:

- `git status` — working tree must be clean. If it isn't, ask whether to commit / stash first.
- `git rev-parse --abbrev-ref HEAD` — should be `main`. If not, confirm with the user that this branch is intentional.
- `git fetch --tags` — make sure local tag knowledge is current.

## 1. Pick the bump kind

If the user already specified one (e.g. "release v0.3.0", "minor bump", "patch release"), use that. Otherwise ask. Use semver:

- **major** (`X.0.0`): breaking changes — config schema renames/removals, removed CLI flags, removed/renamed env vars (`LILCTX_*`), LanceDB schema changes that force a `data_dir` wipe.
- **minor** (`0.X.0`): new features — new MCP tools, new CLI subcommands, new env-override knobs, new providers.
- **patch** (`0.0.X`): bug fixes, doc-only changes, internal refactors.

If you can suggest the right level from `git log <last-tag>..HEAD`, do — but let the user confirm.

## 2. Read the current version

Parse `version = "X.Y.Z"` from the `[package]` section of `Cargo.toml`. Compute the new version:

| bump  | result                          |
| ----- | ------------------------------- |
| major | `X+1.0.0`                       |
| minor | `X.Y+1.0`                       |
| patch | `X.Y.Z+1`                       |

## 3. Update `Cargo.toml`

Edit the `version = "..."` line in `[package]` to the new version. Use the `Edit` tool — do not rewrite the whole file.

## 4. Refresh `Cargo.lock`

```bash
cargo update -p lilctx --offline
```

If that fails (common when offline cache is incomplete), try `cargo build --offline` or fall back to `cargo build` (online). The goal is to bump the `lilctx` entry in `Cargo.lock` to match the new package version *without* updating any other dependency. After it runs, `git diff Cargo.lock` should show only the `lilctx` package version line and its checksum line.

## 5. Generate the changelog entry

Find the previous tag:

```bash
git describe --tags --abbrev=0 2>/dev/null
```

Collect commits since that tag (or all history if there is no prior tag):

```bash
# with previous tag
git log <prev-tag>..HEAD --no-merges --pretty=format:"%s"
# no previous tag (first release)
git log --no-merges --pretty=format:"%s"
```

Categorize each commit by its conventional-commit prefix:

| prefix                                          | section            |
| ----------------------------------------------- | ------------------ |
| `feat:`                                         | Added              |
| `fix:`                                          | Fixed              |
| `docs:`                                         | Documentation      |
| `refactor:`, `chore:`, `perf:`, `style:`, `test:`, `ci:`, `build:` | Changed |
| commit body contains `BREAKING CHANGE` or subject has `!:` | ⚠ Breaking |
| anything else                                   | Changed (default)  |

Skip any commit whose subject starts with `release:` — that's a previous release commit, not a change to ship.

Don't fabricate categories. Don't pad. If a section is empty, drop the heading.

## 6. Update `CHANGELOG.md`

If the file does not exist, create it with this header:

```markdown
# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
```

Prepend a new section *above any existing version sections* (newest first), using `date +%Y-%m-%d` for the date:

```markdown
## [X.Y.Z] — YYYY-MM-DD

### ⚠ Breaking
- ...

### Added
- ...

### Changed
- ...

### Fixed
- ...

### Documentation
- ...
```

If this is the first release on a fresh project (no prior tag), don't dump the whole git log — write a one- or two-bullet summary of what the project is and what's in this release under "Added".

## 7. Commit

Stage exactly these files (don't `git add .`):

```bash
git add Cargo.toml Cargo.lock CHANGELOG.md
```

Commit with:

```
release: vX.Y.Z
```

No body — the changelog is the body. Do not amend an existing commit; always make a fresh one.

## 8. Tag

```bash
git tag -a vX.Y.Z -m "vX.Y.Z"
```

Annotated (not lightweight) — annotated tags carry metadata and behave correctly with `git describe`. If a tag with this name already exists locally or on the remote (`git ls-remote origin refs/tags/vX.Y.Z`), abort and ask the user — do not force-overwrite published tags.

## 9. Push — ASK FIRST

Pushing is a shared-state action. Before pushing, surface to the user:

1. The new version.
2. The `CHANGELOG.md` diff (just the new section).
3. The two commands that will run.

Then ask: **"push commit and tag to origin? (y/n)"**.

If yes:

```bash
git push origin main
git push origin vX.Y.Z
```

Pushing the tag triggers `.github/workflows/release.yml`, which builds the `aarch64-apple-darwin` / `x86_64-apple-darwin` / `x86_64-unknown-linux-gnu` artifacts and attaches them to a GitHub Release with auto-generated notes. After pushing, surface the Actions URL: `https://github.com/<owner>/<repo>/actions`.

## Failure modes worth catching

- **Working tree dirty.** Stop. Ask the user.
- **Branch is not `main`.** Confirm before continuing.
- **Tag already exists.** Stop. Don't force.
- **`cargo update` touches dependencies you didn't intend.** Revert `Cargo.lock` and use `cargo update -p lilctx --offline` more carefully.
- **No commits since last tag.** There is nothing to release. Tell the user and stop.
- **The user reverted mid-flow.** If the user backs out after the commit but before the push, leave the local commit + tag alone but do not push. They can amend or `git tag -d` themselves.

## What this skill deliberately does not do

- Doesn't bump versions in docs prose. Version strings in markdown rot fast and aren't load-bearing.
- Doesn't sign tags. Add `-s` to the `git tag` command if the user has signing configured and asks for it.
- Doesn't open a PR. Releases land on `main` directly per this project's flow.
- Doesn't publish to crates.io. lilctx ships as a binary, not a library.
