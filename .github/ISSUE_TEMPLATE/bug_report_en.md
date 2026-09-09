---
name: Bug report
about: Report abnormal behavior when packing or running
title: "[Bug] "
labels: bug
assignees: ""
---

## Problem Description

<!-- Describe the problem concisely -->

## Category

- [ ] Packing failed (`cli`)
- [ ] Startup failed (the packaged exe)
- [ ] Dependency install / uninstall failed (`depend` or the first-run wizard)
- [ ] Writable layer anomaly (`--write` / `--write-temp` / `--write-memory`)
- [ ] Failed to build from source
- [ ] Other

## Environment

| Item | Value |
| :--- | :--- |
| WinAppImg version | <!-- `win-appimg-cli --version` --> |
| How obtained | <!-- Prebuilt artifact / built from source (features enabled: ) --> |
| Windows version | <!-- `winver`, e.g. 22H2 (19045.3803) --> |
| WinFsp version | <!-- e.g. 2.2.26215 --> |
| DwarFS version | <!-- `dwarfs.exe --version` --> |
| Other component versions | <!-- if other components are involved --> |

## Steps to Reproduce

1.
2.
3.

## Full Command and Output

<!--
- Packing issues: paste the complete cli command line and all of its output
- Runtime issues: repackage with --show-console to enable the runtime log
- Dependency install failures: attach <temp directory>\install.log
-->

```text

```

## Expected Behavior

<!-- What do you think the correct result is? -->

## Actual Behavior

<!-- What actually happened; for dialog errors please include the full text of the dialog -->

## Additional Information

<!-- Input directory structure, screenshots, related issue links, etc. -->

## Confirmation

- [ ] I have searched the existing issues and this is not a duplicate
- [ ] I am using the latest version
