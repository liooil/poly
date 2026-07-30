# Polyglot Runtime

一个单进程的 Bun + RustPython 多语言运行时原型。

最终可执行文件由 Bun 主线源码构建，内部同时链接：

- Bun 的 JavaScriptCore runtime、TypeScript 转译、bundler 和包管理；
- RustPython 0.5.0 与冻结的 Python 标准库；
- `Bun.polyPythonCall()` 原生 JSC host function；
- Rust 实现的持久 Python VM 和 JSON 值桥。

首版没有 Bun sidecar、Python sidecar或 stdio RPC。TypeScript 调用 Python 时，控制流始终留在同一个 OS 进程：

```text
TypeScript
  -> Bun.polyPythonCall(requestJson)
  -> Rust JSC host function
  -> thread-local persistent RustPython VM
  -> Python function
  -> JSON response
  -> TypeScript
```

上述链路全部在 Bun/JSC 的调用者线程同步完成，没有 worker thread 或跨线程消息。

## 仓库结构

```text
crates/poly-python/            RustPython 嵌入与调用桥
patches/bun-in-process.patch   对固定 Bun 提交的最小集成补丁
scripts/bootstrap-bun.ps1      拉取、打补丁并构建最终二进制
sdk/typescript/poly.ts         TypeScript 用户 API
examples/                      双语言示例
docs/                          技术方案与路线图
```

Bun 固定在提交 `e7ddfeb19e8bc714f6137aa2b1cd5a7bb56b93d7`。固定提交是必要的，因为 `bun_runtime` 和 `bun_bin` 目前仍是 Bun 内部实现，不是稳定 embedding API。

## 构建

Bun 官方说明完整开发环境需要约 10 GB 空间，并可能花费 10–30 分钟。Windows 还需要满足 Bun 自身的源码构建要求。

```powershell
.\scripts\bootstrap-bun.ps1
```

脚本会：

1. 把固定 Bun 提交拉到 `.poly/bun-src`；
2. 把 `crates/poly-python` 放入 Bun workspace；
3. 应用同进程集成补丁；
4. 调用 Bun 官方构建流程；
5. 输出 `dist/poly.exe`。

## 运行

直接运行 Python：

```powershell
.\dist\poly.exe examples\python_main.py -- first second
```

运行 TypeScript，并在同一进程调用 Python：

```powershell
.\dist\poly.exe examples\main.ts
```

TypeScript API：

```ts
import { python } from "./sdk/typescript/poly.ts";

const result = await python.call("./math_tools.py", "add", [20, 22]);
```

## 当前边界

首版跨语言值限定为 JSON 数据。Python VM 由 Bun/JSC runtime 线程本地持有；调用目前是同步的，Python 执行期间 Bun 事件循环会暂停。函数、Promise、coroutine、类实例、循环引用、共享对象身份和双向 import 尚未实现。

`uv` 只计划用来解析、锁定和物化 Python 依赖。uv 的官方 Python 实现支持列表没有 RustPython，因此不能让 uv 把 RustPython 当成受支持解释器，也不能假定 uv 能安装的 wheel 一定能在 RustPython 中运行。

## 文档

- [技术方案](docs/technical-design.md)
- [实施路线图](docs/roadmap.md)
- [验证记录](docs/validation.md)
- [可复现构建与二进制发布方案](docs/release-build.md)

## 持续集成

- `CI` 在 push、PR 与 merge queue 上运行 Linux、macOS、Windows 三平台
  RustPython bridge 测试，并检查格式、Clippy、TypeScript bundle、同进程
  边界和 Bun 补丁适用性。
- `Bun integration build` 在相关 PR 或手动触发时，按照 Bun 上游工具链
  约束完整构建 Windows x64 与 Linux x64 Release 可执行文件，然后运行
  Python 入口和 TS→Python smoke test。
- `Release` 只接受 `v*` tag，在两平台构建与 smoke test 全部成功后打包
  `zip`/`tar.gz`、生成 `BUILDINFO.json` 与 `SHA256SUMS`，最后创建 GitHub
  Release。任一平台失败都不会发布。

## 许可证

本项目自身代码使用 [MIT License](LICENSE)。最终 `poly` 二进制还会
包含 Bun、JavaScriptCore/WebKit、RustPython 等第三方组件，它们继续
适用各自的许可证；初步清单及正式发布要求见
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

公开构建并不免除二进制发行义务。正式版本计划由 Git tag 触发 GitHub
Actions 构建，作为 GitHub Release Assets 发布；GitHub Pages 只提供
下载和校验脚本，不直接保存二进制。

## 上游依据

- [Bun 主线 Rust workspace](https://github.com/oven-sh/bun/blob/main/Cargo.toml)
- [Bun 源码构建说明](https://github.com/oven-sh/bun/blob/main/CONTRIBUTING.md)
- [RustPython 0.5.0 嵌入 API](https://docs.rs/rustpython/0.5.0/rustpython/)
- [uv 支持的 Python 实现](https://docs.astral.sh/uv/reference/policies/python/)
