<!--
  This is the Pull Request template; it is filled into the description box automatically when a PR is created.
  Please complete each section and tick the checklist; delete sections that do not apply, but do not empty the whole template.
-->

> [简体中文](./PULL_REQUEST_TEMPLATE.md) | English

## Type of Change

- [ ] New feature (`feat`)
- [ ] Bug fix (`fix`)
- [ ] Refactor (`refactor`)
- [ ] Performance improvement (`perf`)
- [ ] Tests (`test`)
- [ ] Documentation (`docs`)
- [ ] Build / dependencies (`chore`)

## Background and Motivation

<!-- What problem does this solve? Related issue: Closes #xxx -->

## What Changed

<!-- Briefly describe what changed and why it was done this way -->

## Scope

- [ ] Packaging tool (`cli`)
- [ ] Launcher (`launcher`)
- [ ] Dependency manager (`depend`)
- [ ] Build flow (`build.rs` / `post_build`)
- [ ] Common library (`payload` / `dwarfs` / `dwarfs_enc` / `win`)
- [ ] Documentation

## Compatibility

<!-- If any of the following is affected, describe the impact and migration plan here. See "Changes Needing Extra Care" in CONTRIBUTING.md -->

- [ ] None of the sensitive changes below are involved
- [ ] Involves the `.payload` section layout
- [ ] Involves MessagePack struct fields (`PayloadData` / `Payload` / `LauncherPayload`)
- [ ] Involves command-line arguments or the configuration file format
- [ ] Involves `include` / features in `Cargo.toml`

## How to Verify

<!-- How can the change be verified? Paste the key commands and their output -->

```text

```

## Checklist

Please confirm each item before submitting:

- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy -r --all-targets --all-features` reports zero warnings
- [ ] `cargo test -r --all-features` passes completely
- [ ] `cargo test -r` passes on non-Windows (if common modules were changed)
- [ ] New / modified public methods have `///` doc comments covering parameters and return values
- [ ] New logic comes with unit tests covering the normal path and edge cases
- [ ] Related documentation is updated (README / CONTRIBUTING)
- [ ] Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/)
