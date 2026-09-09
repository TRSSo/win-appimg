<!--
  这是 Pull Request 模板，创建 PR 时会自动填充到描述框。
  请填写各章节并勾选检查清单；不适用的章节可删除，但请勿清空整个模板。
-->

> [English](./PULL_REQUEST_TEMPLATE_en.md) | 简体中文

## 改动类型

- [ ] 新功能（`feat`）
- [ ] 缺陷修复（`fix`）
- [ ] 重构（`refactor`）
- [ ] 性能优化（`perf`）
- [ ] 测试（`test`）
- [ ] 文档（`docs`）
- [ ] 构建 / 依赖（`chore`）

## 背景与动机

<!-- 解决什么问题？关联的 Issue：Closes #xxx -->

## 改动内容

<!-- 简述改了什么、为什么这样改 -->

## 影响范围

- [ ] 打包工具（`cli`）
- [ ] 启动器（`launcher`）
- [ ] 依赖管理器（`depend`）
- [ ] 构建流程（`build.rs` / `post_build`）
- [ ] 公共库（`payload` / `dwarfs` / `dwarfs_enc` / `win`）
- [ ] 文档

## 兼容性

<!-- 若触及以下几项，请在此说明影响与迁移方案，详见 CONTRIBUTING.md 的「需要特别留意的改动」 -->

- [ ] 未涉及下列敏感改动
- [ ] 涉及 `.payload` 段布局
- [ ] 涉及 MessagePack 结构体字段（`PayloadData` / `Payload` / `LauncherPayload`）
- [ ] 涉及命令行参数或配置文件格式
- [ ] 涉及 `Cargo.toml` 的 `include` / features

## 验证方式

<!-- 如何验证改动有效？贴出关键命令与输出 -->

```text

```

## 检查清单

提交前请逐项确认：

- [ ] `cargo fmt --check` 通过
- [ ] `cargo clippy -r --all-targets --all-features` 零警告
- [ ] `cargo test -r --all-features` 全部通过
- [ ] 非 Windows 下 `cargo test -r` 通过（若改动了通用模块）
- [ ] 新增 / 修改的公开方法有 `///` 文档注释，含参数与返回值说明
- [ ] 新增逻辑补充了单元测试，覆盖了正常路径与边界条件
- [ ] 更新了相关文档（README / CONTRIBUTING）
- [ ] 提交信息遵循[约定式提交](https://www.conventionalcommits.org/zh-hans/)
