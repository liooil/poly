# 验证记录

## 2026-08-24：staging 兼容性报告与 `poly sync --check`

- `cargo fmt --all --check`
- `cargo test --locked -p poly_python`
  - 18 项单元测试与 1 项 interop 测试通过；
  - 新 RustPython VM 只从 staging 环境导入，并返回逐模块 traceback。
- `cargo test --locked -p poly_uv`
  - 30 项通过，0 项失败；
  - `RECORD` 扫描覆盖原生动态库、sourceless CPython bytecode、普通包、
    namespace package 与 data-only distribution；
  - 本地 registry 覆盖 smoke import 成功和失败，不访问公共网络；
  - 静态或动态兼容性失败时，原有 `uv.lock` 和 `.poly/python` 均保持不变；
  - lock commit 失败时使用的环境 rollback 路径可恢复旧目录。
- `cargo clippy --locked -p poly_python -p poly_uv --all-targets -- -D warnings`
- `cargo check --locked -p bun_runtime --tests`
- `USE_SYSTEM_BUN=1 bun test test/cli/bun.test.ts -t "--check reports staged smoke-import validation"`
  - 系统 Bun 按预期失败，证明新回归测试依赖本分支实现。
- `bun bd test test/cli/bun.test.ts`
  - 55 项通过，2 项既有 `todo`，0 项失败；
  - 覆盖 `--check` 解析、帮助、输出和 Bash、Zsh、Fish 补全。
- `bun run rust:check-all`
  - 本机 `x86_64-unknown-linux-gnu` 通过；
  - 其余 11 个目标缺少对应 cross compiler、平台 SDK 或归档工具，未形成有效
    检查结果。

## 2026-08-23：resolver-driven Python sync

- `cargo fmt --all --check`
- `cargo test --locked -p poly_uv`
  - 23 项通过，0 项失败；
  - 本地 registry 覆盖最高兼容版本选择、标准 `uv.lock` 生成与 frozen 复用；
  - 带 Basic Auth 的私有 index 覆盖解析与 wheel 下载，锁文件不包含密码；
  - CPython 专用 wheel、平台 wheel 与 source-only 解析不会进入安装环境；
  - 解析失败时原有 `uv.lock` 保持不变。
- `cargo clippy --locked -p poly_uv --all-targets -- -D warnings`
- `cargo test --locked -p poly_python`
  - 17 项单元测试与 1 项 interop 测试通过。
- `cargo check --locked -p bun_runtime --tests`
- `USE_SYSTEM_BUN=1 bun test test/cli/bun.test.ts -t "resolves an empty project and writes uv.lock"`
  - 系统 Bun 按预期失败，证明 CLI 回归测试依赖本分支实现。
- `bun bd test test/cli/bun.test.ts`
  - 54 项通过，2 项既有 `todo`，0 项失败。
- `bun run rust:check-all`
  - 本机 `x86_64-unknown-linux-gnu` 通过；
  - 其余 11 个目标缺少对应 cross compiler、平台 SDK 或归档工具，未形成有效检查结果。

## 2026-08-23：frozen Python sync

- `cargo test --locked -p poly_uv`
  - uv 负责解析 lock、求值 marker 与遍历传递依赖；
  - 本地 HTTP 与 `file:` wheel 下载全程不访问公共网络；
  - SHA-256 校验、损坏缓存替换、失败时保留旧环境；
  - 安装后的包可由 RustPython 文件入口导入。
- `cargo clippy --locked -p poly_uv --all-targets -- -D warnings`
- `cargo test --locked -p poly_python`
  - 文件入口与持久 module bridge 可发现 `.poly/python`；
  - 第三方包路径位于标准库之后。
- `cargo clippy --locked -p poly_python --all-targets -- -D warnings`
- `cargo test --locked -p bun_options_types`
- `cargo check --locked -p bun_runtime --tests`
- `bun bd test test/cli/bun.test.ts`
  - 54 项通过，2 项既有 `todo`，0 项失败；
  - 覆盖 `sync` CLI 边界以及 Bash、Fish、Zsh 的公共命令补全。
- `bun run rust:check-all`
  - 本机 `x86_64-unknown-linux-gnu` 通过；
  - 其余 11 个目标在本机缺少对应 C/C++ cross toolchain、平台 headers 或 SDK，
    尚未形成有效的目标平台检查结果。
---

验证日期：2026-07-31

## 已通过

- Poly `main` 通过双亲 merge commit 保留原 Poly 历史，并接入 Bun
  `e7ddfeb19e8bc714f6137aa2b1cd5a7bb56b93d7` 的完整历史。
- 随后使用普通 merge（不再使用 `--allow-unrelated-histories`）同步到 Bun
  upstream `cbe3e18a769f446222d5b8009b5a76943153d1ba`。
- `cargo fmt -p poly_python -- --check`
- `cargo test -p poly_python`
  - 在一个调用者线程中初始化 RustPython；
  - 调用 `poly/examples/math_tools.py::add(20, 22)`；
  - 返回 JSON 值 `42`；
  - 捕获 Python stdout。
- 从仓库根目录执行
  `.\poly\scripts\build.ps1 -Configuration Release`，成功生成
  `dist/poly.exe`；构建图不含 `.poly/bun-src` 路径。
- Poly 修改已直接进入 Bun fork，不再动态拉取另一份 Bun 源码或应用
  `patches/bun-in-process.patch`。
- 使用发行版 Bun 对 `poly/examples/main.ts` 与 TypeScript SDK 做 bundle 检查
- 搜索确认运行时源码不含 `Command::new`、`Bun.spawn`、`child_process` 或 `mpsc`
- 新生成的 Windows x64 Release 二进制通过：
  - `poly.exe --version`，输出 `1.4.0`；
  - Python 入口 `poly.exe poly/examples/python_main.py -- first second`；
  - JavaScript 入口 `poly.exe poly/examples/js_smoke.ts`；
  - `poly.exe poly/examples/main.ts` 的 TypeScript → RustPython 调用，返回 `42`。
  - 文件大小 `99,396,096` bytes，SHA-256
    `09D65C1F76EC4A81D027ACB4E6033AC0A96E63D5A80BC325B230BCD37C6761B3`。
- GitHub Actions CI run `30621951052` 通过：
  - Windows、Linux、macOS 的 RustPython bridge；
  - Linux 上的格式和 Clippy 检查；
  - TypeScript bundle、无子进程边界与直接 fork 集成 contract。
- GitHub Pages run `30621171683` 成功部署 `poly/website`。

## 尚待 CI 验证

- Linux x64 完整构建和 smoke test；
- GitHub Release 打包、校验和与发布。

Windows 首次冷构建约 44 分钟。最终链接有 `LNK4217` 本地符号 imported
警告，但构建成功，且上述运行测试均通过。
