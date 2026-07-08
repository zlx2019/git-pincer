# git-pincer

[![CI](https://github.com/zlx2019/git-pincer/actions/workflows/ci.yml/badge.svg)](https://github.com/zlx2019/git-pincer/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.96.0%2B-orange.svg)](https://www.rust-lang.org)

[English](./README.md) | **简体中文**

> 终端里的 IDEA 风格三栏 Git 冲突解决工具。

`git-pincer` 让合并冲突不再痛苦:通过它发起 `merge / rebase / pull / cherry-pick / revert`(或在任何冲突发生后再调用),在三栏 TUI 中逐块解决冲突,随后由它驱动 `git add` + `--continue` 循环,直到仓库回到干净状态 —— 多提交 cherry-pick 与多轮 rebase 也不例外。

<!-- TODO: 替换为真实截图 / GIF -->

```text
 ⚑ MERGE ▏█████░░░░░ 2/5 文件 ▏src/app.rs ▏✗ 2 冲突待解决
╭ LOCAL · feature ────╮╭ RESULT ───────╮╭ REMOTE · main ─────╮
│  8 » fn new() {     ││  8 ⋯ 待解决 ⋯ ││  8 « fn make() {   │
│    »   init()       ││               ││    «   setup()     │
│  ✓ 已解决的块色带消失,gutter 保留 ✓                       │
│ ────── ⋯ 12 行未改动 (z 展开) ⋯ ──────                    │
╰─────────────────────╯╰───────────────╯╰────────────────────╯
  h 取本地 · l 取远端 · x 忽略 · e 编辑 · ? 帮助
```

## 特性

- **三栏合并界面** — 本地 | 结果 | 远端,块级色带按改动类型着色,沿用 IDEA 语义:蓝 = 修改、绿 = 新增、灰 = 删除、红 = 冲突;块解决后色带随之消失。
- **精细的差异呈现** — 改动块内 delta 式词级高亮,配合语法着色(syntect 的 Maple 主题,按扩展名匹配,超大文件自动降级)。
- **接管完整流程** — 全部文件解决后自动执行 `git add` 与对应的 `--continue`,重新探测并循环,直到仓库干净;多提交摘取、多轮变基开箱即用。
- **交互式操作菜单** — 在干净仓库中裸运行 `git-pincer` 会弹出菜单:先选操作,再选分支(merge / rebase)或提交(cherry-pick / revert);执行失败会弹框展示原因并回到菜单,而非退出程序。
- **广泛的冲突来源支持** — merge、rebase、pull、cherry-pick、revert、`git am`,以及 `stash pop`、`checkout -m`、`apply --3way` 这类没有 `--continue` 的场景。
- **原生 git,无黑魔法** — 全部 shell out 调用你的 git 二进制(与 lazygit / IDEA 同路),认证、hooks、合并策略、rerere 完全继承现有配置;参数以数组传递不经过 shell(构造上杜绝注入),并清除宿主 `GIT_DIR` 类环境变量,防止从钩子中被调起时嵌套 git 劫持到错误仓库。
- **终端自适应主题** — 深色(Tokyo Night)/ 浅色(Maple Light)双主题,`--theme <auto|dark|light>` 指定,`auto` 经 `COLORFGBG` 检测;不支持真彩的终端自动量化为 xterm-256 色。
- **周全的兜底** — 二进制冲突降级为整文件二选一;免 git 的 `file` 模式直接解析冲突标记;非 TTY 环境给出可读报错而非 panic。

## 安装

需要 `PATH` 中有 `git`;源码构建需要 Rust 1.96+。

```bash
# 从仓库安装
cargo install --git https://github.com/zlx2019/git-pincer

# 或从本地克隆安装
cargo install --path .
```

主流平台的预编译二进制会随打 tag 的版本附在 [GitHub Releases](https://github.com/zlx2019/git-pincer/releases);crates.io 发布在计划中。

## 用法

```bash
git-pincer                      # 有冲突现场:直接接管解决
                                # 仓库干净:弹出交互式操作菜单
git-pincer merge <branch>       # 执行 git merge,撞冲突则接管
git-pincer rebase <branch>      # 执行 git rebase,多轮冲突自动循环
git-pincer pull origin main     # 参数原样透传给 git pull
git-pincer cherry-pick <commit> # 多提交 / 选项均可透传
git-pincer revert <commit>      # 执行 git revert 并接管冲突
git-pincer file conflict.txt    # 免 git:解析带冲突标记的文件,解决后写回
git-pincer abort                # 中止进行中的合并操作(有确认)
```

全局选项:

| 选项 | 说明 |
| ---- | ---- |
| `-C, --repo <PATH>` | 操作指定路径的仓库(默认当前目录) |
| `--theme <auto\|dark\|light>` | 界面主题;`auto` 读取 `COLORFGBG`,检测不到用深色 |
| `-v, --verbose` | 回显执行的每条 git 命令 |

不需要 git 仓库也能试玩 TUI:

```bash
cp fixtures/conflict.txt /tmp/ && git-pincer file /tmp/conflict.txt
```

### 按键

| 按键 | 动作 |
| ---- | ---- |
| `h` / `←` | 取用本地侧(冲突两侧先后取用 = 两者都要) |
| `l` / `→` | 取用远端侧 |
| `x` | 忽略当前块剩余未处理的侧(保留 base) |
| `u` | 撤销当前块的全部决定 |
| `e` | 用 `$EDITOR` 编辑当前块 |
| `a` | 一键应用所有非冲突改动 |
| `j` / `k` | 移动到下一个 / 上一个改动块 |
| `n` / `p` | 跳到下一个 / 上一个未解决冲突 |
| `y` / `Y` | 复制当前块结果 / 整个文件结果 |
| `H` / `L` | 复制当前块的本地侧 / 远端侧 |
| `w` | 写盘(自动应用剩余非冲突改动,随后 `git add`) |
| `Tab` | 切换到下一个文件 |
| `z` | 折叠 / 展开未改动区域 |
| `q` | 退出(未完成时需按两次;现场保留) |
| `?` | 查看完整按键说明 |

### 支持的冲突来源

| 来源 | 探测依据 | 收尾方式 |
| ---- | -------- | -------- |
| `git merge` / `git pull` | `MERGE_HEAD` | `git merge --continue` |
| `git rebase` | `rebase-merge` / `rebase-apply` | `git rebase --continue`(多轮) |
| `git cherry-pick` | `CHERRY_PICK_HEAD` | `git cherry-pick --continue`(多轮) |
| `git revert` | `REVERT_HEAD` | `git revert --continue` |
| `git am -3` | `rebase-apply/applying` | `git am --continue` |
| `stash pop` / `checkout -m` / `apply --3way` | 仅 index 中的 unmerged 条目 | 无需 continue,解完即可 |

## 工作原理

- **diff3 内核** — 两次 2-way diff(base→ours、base→theirs,Myers 算法,500 ms 超时保护),按 base 行区间碰撞归组为块:稳定、单侧、双方一致或冲突。归组策略刻意保守:宁可多报一个冲突,也不静默合错。
- **纯逻辑会话** — 每个块的每一侧处于待处理 / 已取用 / 已忽略三态;取用顺序决定内容拼接方式,`$EDITOR` 编辑整块覆写;含 NUL 字节的文件降级为整文件二选一。
- **git 薄封装** — 冲突内容读自 index 的 stage 1/2/3,写回经 `git add`;仓库状态(merge / rebase / cherry-pick / revert / am)从 git 目录探测,保证收尾命令永远正确。

## 开发

```bash
cargo nextest run --all-features --no-tests pass   # 测试
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

工具链准备、pre-commit 钩子与提交规范见 [CONTRIBUTING.md](./CONTRIBUTING.md)。

## 致谢

基于 [ratatui](https://github.com/ratatui/ratatui)、[similar](https://github.com/mitsuhiko/similar)、[syntect](https://github.com/trishume/syntect) 与 [clap](https://github.com/clap-rs/clap) 构建;视觉设计受 IntelliJ IDEA 合并工具、[delta](https://github.com/dandavison/delta) 与 [lazygit](https://github.com/jesseduffield/lazygit) 启发。

## License

本项目采用 [MIT](./LICENSE) 许可证分发。
