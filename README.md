# clean-my-mac

一个谨慎的 macOS 磁盘占用扫描器和交互式 TUI 清理工具。它针对本机这次“系统数据 391 GB”问题编写：先只读统计，用户逐项选择，再经过二次确认才会删除。

## 本机排查结论（2026-08-13）

Data 卷实际占用约 938 GB。macOS 显示的约 391 GB“系统数据”主要由以下内容组成：

| 占用 | 位置 | 结论 |
| ---: | --- | --- |
| 278 GiB（约 299 GB） | `~/Library/Group Containers/group.com.apple.coreservices.useractivityd/shared-pasteboard/archives` | macOS 通用剪贴板/接力留下的 4 个归档文件，大小约 195.8 GB、72.9 GB、28.0 GB 和 1.95 GB |
| 60 GiB（约 64 GB） | `~/Library/Caches` | 用户缓存；其中 `Adobe Camera Raw 2` 单独占约 50 GiB |
| 约 28 GB | 其他系统支持目录 | 正常的系统数据、日志和应用支持文件 |

`~/bej48_local` 是主要用户数据，不属于异常占用。本程序不会扫描或显示它；底层清理函数也会拒绝该目录及其所有子目录。

已排除 Time Machine 本地快照、Apple Content Cache，以及“文件已经删除但仍被进程打开”的超大文件。

## 运行

需要 Rust 工具链：

```bash
cd ~/projects/clean-my-mac
cargo run --release
```

首次启动会先统计白名单目录；不会扫描 `bej48_local`。

TUI 快捷键：

- `↑` / `↓` 或 `j` / `k`：移动
- `Space`：勾选或取消
- `c`：进入清理二次确认
- `y`：在确认窗口执行永久清理
- `n` / `Esc`：取消确认
- `r`：重新统计
- `o`：打开 macOS“完全磁盘访问”设置
- `q`：退出

所有项目默认均不勾选。删除不会进入废纸篓。

界面中的容量全部统一使用 GiB，避免在 MiB、GiB 和 TiB 之间切换导致比较困难。

当前白名单覆盖：通用剪贴板归档、Chrome 缓存/本地 AI 模型/更新缓存/站点缓存、DaVinci 渲染缓存、Codex runtime 与应用缓存、Adobe/Lightroom 缓存和日志、Cargo/npm/pnpm/Homebrew 包缓存、Downloads 顶层安装包以及废纸篓。

## macOS 权限

macOS 的 TCC 隐私机制独立于 Unix 的 root 权限，因此 `sudo dust /` 仍会对 Mail、Messages、Group Containers、Spotlight 等目录打印大量 `Operation not permitted`。`sudo` 并不能绕过“完全磁盘访问”。

本程序不要求 root，也不会尝试清理系统目录。请给**启动本程序的宿主应用**开启完全磁盘访问：

1. 打开“系统设置 → 隐私与安全性 → 完全磁盘访问”。
2. 如果从 Terminal、iTerm、Warp 或 VS Code 终端运行，就授权对应终端应用。
3. 如果从 ChatGPT/Codex 运行，就授权 ChatGPT。
4. 完全退出并重新启动被授权的应用，再运行程序。

程序会尝试读取受 TCC 保护的已知目录来检测权限。若检测失败，TUI 会显示“未授权”；按 `o` 可以直接打开设置页。扫描中遇到权限错误的项目不会允许选择。

## 安全边界

- 只扫描和清理源码中定义的白名单路径，不接受命令行传入任意删除路径。
- 只清理当前用户主目录内的目标；拒绝主目录本身、主目录外路径和符号链接目标。
- 清理缓存时只删除目录内容，保留目录本身。
- Downloads 选项只删除顶层、超过 1 小时的 `.dmg`/`.pkg` 文件；不会遍历子目录，也不会删除其他下载文件。
- 通用剪贴板仅处理 `archives` 子目录，不碰当前剪贴板元数据。
- 剪贴板归档必须超过 1 小时且没有任何进程通过 `lsof` 占用，否则拒绝清理。
- `~/bej48_local` 不参与扫描且不在 TUI 中显示；清理 API 对该目录及其所有子目录设有硬性拒绝规则。
- 程序先展示预计可释放空间；选择后还必须在确认窗口按 `y`。

清理完成后，APFS 可用空间应立即增加；macOS“系统设置 → 存储空间”的分类数字可能需要一段时间才会刷新。
