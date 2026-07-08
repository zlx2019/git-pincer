# git-peace

[![CI](https://github.com/zlx2019/git-peace/actions/workflows/ci.yml/badge.svg)](https://github.com/zlx2019/git-peace/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.96.0%2B-orange.svg)](https://www.rust-lang.org)

> A simple and compact Git CLI, mainly used for resolving conflicts.

终端里的 Git 冲突解决工具:IDEA 风格的三栏合并 TUI(本地 | 结果 | 远端),可直接发起 `merge / rebase / pull / cherry-pick / revert` 并接管随后的全部冲突解决流程。

## Features

- **三栏冲突解决 TUI**:块级色带按改动类型着色(IDEA 语义:蓝=修改、绿=新增、灰=删除、红=冲突),`h/l` 取用本地/远端(冲突两侧先后取用即「两者都要」),`x` 忽略、`u` 撤销、`e` 调 `$EDITOR` 手动编辑
- **现代终端视觉**:圆角边框面板 + IDEA 式 gutter 操作符号(`»«✓✗`)+ delta 式词级差异高亮 + syntect 语法着色(Maple 主题,按扩展名,超大文件自动降级);深浅双主题,`--theme <auto|dark|light>` 指定(auto 经 `COLORFGBG` 自动检测)
- **接管完整流程**:解决全部文件后自动 `git add` + `--continue`,rebase 多轮冲突自动循环,直到仓库回到干净状态;`stash pop` / `checkout -m` 等无 `--continue` 的冲突同样可接管
- **交互式操作菜单**:仓库干净时裸 `git-peace` 弹出菜单,pull 直接执行,merge / rebase 二级选择分支,cherry-pick / revert 二级选择提交,撞冲突无缝进入解决界面
- **diff3 三方合并算法**:两次 2-way diff 分块归组,保守碰撞策略(宁多报冲突,不静默错合)
- **原生 git CLI 交互**:shell out 执行(与 lazygit / IDEA 同路),认证、hooks、merge 策略、rerere 全部继承用户配置
- **二进制冲突降级**:整文件二选一;免 git 的单文件模式可直接解析带 `<<<<<<<` 标记的文件

## 用法

```bash
git-peace                      # 有冲突现场直接接管;仓库干净时弹出操作菜单(选操作 → 选分支/提交 → 执行)
git-peace merge <branch>       # 执行 git merge 并接管冲突解决
git-peace rebase <branch>      # 执行 git rebase,多轮冲突自动循环
git-peace pull origin main     # 参数透传给 git pull
git-peace cherry-pick <commit> # 执行 git cherry-pick(多提交多轮循环)
git-peace revert <commit>      # 执行 git revert 并接管冲突解决
git-peace file conflict.txt    # 免 git:解析带冲突标记的单个文件,解决后写回
git-peace abort                # 中止进行中的合并操作(merge / rebase / cherry-pick / revert / am)
```

TUI 内按 `?` 查看全部按键。试玩:`cp fixtures/conflict.txt /tmp/ && cargo run -- file /tmp/conflict.txt`

## 快速开始

### 1. 安装开发工具

项目通过 `rust-toolchain.toml` 锁定 Rust 版本，进入目录后 rustup 会自动安装对应工具链。另需安装以下工具（与 CI 检查保持一致）：

```bash
cargo install --locked cargo-deny     # 依赖安全 / 许可证审计
cargo install cargo-nextest --locked  # 测试运行器
cargo install typos-cli               # 拼写检查
cargo install git-cliff               # Changelog 生成
pip install pre-commit                # Git 提交前检查
```

### 2. 启用 pre-commit 钩子

```bash
pre-commit install
```

启用后每次 `git commit` 会自动运行格式化、Lint、测试等检查，全部通过才会提交成功。

### 3. 构建与运行

```bash
cargo run -- --help                 # 查看全部子命令与参数
cargo install --path .              # 安装为全局命令 git-peace
```

## 开发

常用命令：

```bash
cargo nextest run    # 运行测试
cargo clippy         # 静态检查
cargo fmt            # 格式化
```

提交规范与完整开发流程见 [CONTRIBUTING.md](./CONTRIBUTING.md)。

## 项目结构

```text
src/
├── main.rs           程序入口:解析参数并分发
├── lib.rs            模块导出(bin+lib 拆分,供集成测试使用)
├── cli.rs            命令行接口定义(clap)与子命令注册
├── commands/         子命令编排(merge/rebase/pull/resolve/file/abort)
├── merge.rs          diff3 三方合并核心与冲突标记解析(纯逻辑)
├── git.rs            原生 git CLI 薄封装(shell out)
├── app.rs            冲突解决会话状态机(纯逻辑)
└── ui/               ratatui 渲染层:mod 事件循环 / theme 配色 / rows 行构建
                      / highlight 词级+语法高亮缓存 / panes 三栏面板 / chrome 状态栏等
tests/git_flow.rs     集成测试:临时真实 git 仓库验证全流程
```

新增子命令:在 `src/commands/` 下新建模块,并在 `src/cli.rs` 的 `Commands` 枚举中注册对应变体。

## License

本项目采用 [MIT](./LICENSE) 许可证。
