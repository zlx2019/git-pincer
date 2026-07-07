# 贡献指南

感谢你的贡献。提交代码前请阅读以下规范。

## 开发环境准备

### Rust 工具链

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

项目通过 `rust-toolchain.toml` 锁定版本，进入目录后 rustup 会自动安装对应的工具链与组件。

### 开发工具

```bash
cargo install --locked cargo-deny     # 依赖安全 / 许可证审计
cargo install typos-cli               # 拼写检查
cargo install git-cliff               # Changelog 生成
cargo install cargo-nextest --locked  # 增强版测试运行器
pip install pre-commit                # Git 提交前检查
```

### 启用 pre-commit

```bash
pre-commit install
```

此后每次 `git commit` 会自动运行 fmt / deny / typos / check / clippy / test 检查，全部通过才会提交成功。

## 本地检查

提交前请确保以下命令全部通过：

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run --all-features --no-tests pass
cargo deny check
typos
```

## 提交规范

提交信息遵循 [Conventional Commits](https://www.conventionalcommits.org/)：

| 类型 | 说明 |
|------|------|
| `feat:` | 新功能 |
| `fix:` | 缺陷修复 |
| `docs:` | 文档变更 |
| `refactor:` | 重构（非功能、非修复） |
| `perf:` | 性能优化 |
| `test:` | 测试相关 |
| `chore:` | 构建 / 工具链 / 杂项 |

Changelog 由 git-cliff 依据提交记录自动生成，请保持提交信息规范。

## 分支与 PR

- 从 `main` 切出特性分支进行开发。
- 一个 PR 聚焦单一主题，便于审查。
- PR 需通过 CI（lint / test / deny / typos）方可合并。
