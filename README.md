# clean-my-mac

一个谨慎的 macOS 磁盘占用扫描器和清理工具，同时提供原有 CLI/TUI 和极简 Tauri 桌面 App。先只读统计，用户逐项选择，再经过二次确认才会删除。发布产物仅支持 Apple Silicon（arm64）。

## 桌面 App

需要 macOS 12+、Rust stable、Node.js 22+ 和 Xcode Command Line Tools（`xcode-select --install`）。

```bash
npm ci
npm run dev
```

界面使用系统字体、纯色背景和紧凑单行列表，默认展示名称、状态与容量，点击“详情”展开描述、路径和读取错误。列表独立滚动，底部清理操作始终可见。支持磁盘容量、白名单目录扫描、逐项选择、永久删除确认、清理结果与权限错误提示。所有项目默认不勾选；重新扫描会清空选择。权限检查独立于目录扫描；启动后先展示全部候选项目，每扫描完一个目录就更新对应行和已发现容量，并显示当前目录与完成项数。扫描中可查看详情和打开权限设置，完整扫描成功后才能选择并清理。扫描和清理在后台执行，不阻塞窗口。

构建 Apple Silicon 安装包：

```bash
rustup target add aarch64-apple-darwin
npm run build -- --target aarch64-apple-darwin --ci -- --locked
```

产物位于 `target/aarch64-apple-darwin/release/bundle/`，包含 `.app` 和 `.dmg`。将 App 拖入“应用程序”后运行。当前使用 ad-hoc 签名，未配置 Apple Developer 证书和公证；下载版本可能被 Gatekeeper 拦截，需要在确认来源后通过系统“隐私与安全性”允许打开。

桌面版需要为 **Clean My Mac 本身**开启完全磁盘访问。点击“打开权限设置”，添加 `/Applications/Clean My Mac.app`，授权后完全退出并重新打开 App。开发模式下请按系统提示授权开发二进制或宿主终端；正式安装版的授权独立于终端。

桌面端与 CLI 共用 `src/lib.rs` 的扫描和清理逻辑；桌面 IPC 仅接受白名单 ID，不接受文件路径。CLI 是默认 Cargo 入口，普通 `cargo run --release` 仍启动终端界面。Tauri 在独立 workspace member 中，CLI 不编译桌面运行时；前端为本地 HTML/CSS/JavaScript，无需前端构建服务。

## 自动发布

[GitHub Actions 工作流](.github/workflows/release.yml) 在每次分支或标签 **push** 时运行：

1. 执行 Rust 格式检查、Clippy、Rust 测试和界面交互测试。
2. 仅构建 macOS **arm64** 的桌面 App 和原有 CLI。
3. 上传 `.dmg`、App 压缩包、CLI 压缩包和 `SHA256SUMS.txt` 到 GitHub Release。

普通分支 push 会创建独立的 `build-<run_number>-<run_attempt>` 预发布版本，不替换稳定版；标签 push 使用该标签创建正式 Release。Release 先以草稿创建，产物上传成功后发布。PR 只构建和测试，不发布；也支持在 Actions 中手动触发。

工作流使用仓库自带的 `GITHUB_TOKEN`，发布 job 请求 `contents: write`，无需额外密钥。仓库或组织策略需要允许 Actions 写入 Release。发布是由提交这份工作流后的 push 触发，修改本地代码不会自动发布。

验证命令：

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
npm test
```

测试中的删除操作只针对临时目录。桌面交互测试模拟 IPC，覆盖确认、取消、重复提交、失败恢复和不可读目录。

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
- 剪贴板归档不限制文件时间；没有任何进程通过 `lsof` 占用时即可清理，否则拒绝清理。
- `~/bej48_local` 不参与扫描且不在 TUI 中显示；清理 API 对该目录及其所有子目录设有硬性拒绝规则。
- 程序先展示预计可释放空间；选择后还必须在确认窗口按 `y`。

清理完成后，APFS 可用空间应立即增加；macOS“系统设置 → 存储空间”的分类数字可能需要一段时间才会刷新。
