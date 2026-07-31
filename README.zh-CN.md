# Poly

[English](README.md) | **简体中文**

**一个同时属于 TypeScript 和 Python 的运行时。**

运行任一种语言，跨越两个生态导入模块，最终交付一个可执行文件。

[![CI](https://github.com/liooil/poly/actions/workflows/ci.yml/badge.svg)](https://github.com/liooil/poly/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

Poly 是一个面向 TypeScript 与 Python 混合应用的实验性运行时和工具链。
Bun 负责 JavaScript 与 TypeScript，RustPython 负责 Python，Rust host
把它们组织在同一个可执行文件和同一个 OS 进程中。

Poly 的目标不是把一种语言藏在 RPC 服务后面，而是让两种语言共同成为
同一个项目、模块图、开发流程和发布产物中的一等成员。

> [!IMPORTANT]
> Poly 仍处于工程原型阶段。目前已经具备共享运行时和底层同进程调用桥；
> 跨语言 import、统一依赖管理和应用打包仍属于路线图能力。

## 目标体验

Python 文件应该是模块，而不是服务：

```ts
import { add } from "./math_tools.py" with { type: "python" };

console.log(add(20, 22)); // 42
```

需要显式访问 Python 能力时，Poly 还将提供运行时内置模块：

```ts
import { python } from "poly";
```

常用项目生命周期应该由一个 CLI 完成：

```text
poly run app.ts        # 运行 TypeScript 入口
poly run tool.py       # 运行 Python 入口
poly sync              # 解析两个生态的依赖
poly build app.ts      # 生成独立应用
```

`poly run` 的入口分派属于当前原型。静态跨语言 import、`poly sync` 和
`poly build` 尚未实现。

## 一个项目，两个生态

Poly 项目将保留两个生态各自原生的工具和元数据：

```text
my-app/
├── package.json       # JavaScript 与 TypeScript 依赖
├── bun.lock
├── pyproject.toml     # Python 依赖
├── uv.lock
├── poly.toml          # 入口、运行时与互操作配置
└── src/
    ├── main.ts
    └── model.py
```

| 层次 | 方向 |
|---|---|
| 运行时 | Bun 与内嵌 RustPython 位于同一个可执行文件中 |
| 模块 | TypeScript 与 Python 之间直接 import |
| 依赖 | Bun 管理 npm 包，uv 负责 Python 依赖解析与锁定 |
| 构建 | 收集两种语言的模块图、依赖和资源 |
| 分发 | 一个原生可执行文件，不需要 Python sidecar |

Poly 负责两个生态之间的兼容性边界。uv 用于依赖解析、锁定和物化，但
RustPython 不是 uv 官方支持的解释器，也无法加载任意 CPython 原生 wheel。
因此 Poly 需要提供自己的兼容性检查和报告。

## 运行时如何组合

当前 M0 原型把 RustPython 直接集成进固定版本的 Bun 源码树：

```text
poly executable
│
├── Bun / JavaScriptCore
│   ├── JavaScript and TypeScript entry points
│   └── Bun.polyPythonCall()
│
├── Rust host bridge
│   └── JSON value and error envelope
│
└── RustPython
    ├── Python entry points
    └── frozen Python standard library
```

当前 TypeScript → Python 调用始终留在 Bun/JSC 调用者线程，不会启动
子进程、sidecar、worker thread、socket 或 stdio RPC。Python 调用同步
执行，并在运行期间阻塞 Bun 事件循环。

JSON bridge 是一个启动层：它证明两个运行时可以共存于同一进程，同时
保持各自垃圾回收对象的边界。路线图将用 import、callable proxy、tagged
value 和明确的生命周期语义取代这个底层接口。

## 项目状态

| 能力 | 状态 |
|---|---|
| Bun 与 RustPython 源码集成 | 已实现 |
| `.js` / `.ts` 与 `.py` 入口分派 | 已通过 Windows Release 完整构建验证 |
| TypeScript → Python JSON bridge | 已通过实际链接的 JSC 与 RustPython runtime 验证 |
| Python 模块缓存与 reload 语义 | 计划中 |
| 静态跨语言 import 与 callable proxy | 计划中 |
| 协作式异步执行 | 计划中 |
| 带 uv 兼容性检查的 `poly sync` | 计划中 |
| 生成独立应用的 `poly build` | 计划中 |

准确的已验证边界见[验证记录](poly/docs/validation.md)，后续计划见
[实施路线图](poly/docs/roadmap.md)。

## 体验当前原型

### 验证 RustPython bridge

这是最快的检查方式，不需要编译完整 Bun：

```powershell
cargo test -p poly_python
```

该测试会在一个调用者线程中初始化 RustPython，调用
`poly/examples/math_tools.py::add(20, 22)`，检查返回值并验证 stdout 捕获。
它不会生成最终的 `poly` 可执行文件。

### 构建完整运行时

完整构建直接编译当前 fork。仓库本身已经包含 Bun 的源码和历史，构建时
不会再拉取另一份 Bun 工作树，也不会应用下游 patch。预计需要约 10 GB
磁盘空间和 10–30 分钟。

前置条件包括 Git、PowerShell 7、Bun 1.3.2、Rust，以及 Bun 所需的原生
构建工具链。完整的 Windows 和 Linux 环境可参考
[Bun integration 工作流](.github/workflows/bun-integration.yml)。

这个 fork 从 Bun 提交
[`e7ddfeb19e8bc714f6137aa2b1cd5a7bb56b93d7`](https://github.com/oven-sh/bun/commit/e7ddfeb19e8bc714f6137aa2b1cd5a7bb56b93d7)，
开始建立，后续上游更新通过显式 merge commit 同步。

#### Windows

```powershell
.\poly\scripts\build.ps1 -Configuration Release
.\dist\poly.exe poly\examples\python_main.py -- first second
.\dist\poly.exe poly\examples\main.ts
```

#### Linux

```bash
pwsh ./poly/scripts/build.ps1 -Configuration Release
./dist/poly poly/examples/python_main.py -- first second
./dist/poly poly/examples/main.ts
```

省略 `-Configuration Release` 时默认构建 Debug 版本。

当前 TypeScript 示例仍通过 [`poly/examples/main.ts`](poly/examples/main.ts) 使用
过渡性的底层 SDK。它是集成验证入口，不是计划中的最终开发体验。

## 仓库结构

```text
src/                           Bun runtime 与 Poly 直接集成源码
src/poly_python/               RustPython 嵌入与调用桥
poly/scripts/build.ps1         fork 的直接构建入口
poly/sdk/                      过渡性的底层 TypeScript SDK
poly/examples/                 TypeScript 与 Python 集成示例
poly/docs/                     技术方案、路线图和验证记录
poly/poly.toml                 未来的 Poly 项目清单
```

## 同步 Bun 上游

克隆本仓库后，可以把 Bun 配置为 upstream remote 并正常合并：

```bash
git remote add upstream https://github.com/oven-sh/bun.git
git fetch upstream
git merge upstream/main
```

第一次 unrelated-history merge 已经进入 `main`；后续同步不再需要
`--allow-unrelated-histories`，也不会重新生成 Poly patch。

## 文档

- [技术方案](poly/docs/technical-design.md)
- [实施路线图](poly/docs/roadmap.md)
- [验证记录](poly/docs/validation.md)
- [可复现构建与二进制发布方案](poly/docs/release-build.md)

## 项目网站

项目网站发布在 [liooil.github.io/poly](https://liooil.github.io/poly/)，
源文件位于 [`poly/website/`](poly/website/)。`main` 分支中的网站改动会通过
[GitHub Pages 工作流](.github/workflows/pages.yml)自动部署。

## 持续集成与发布

- `CI` 在 Linux、macOS 和 Windows 上运行 RustPython bridge 测试，并检查
  格式、Clippy、TypeScript 集成样例、无子进程边界和直接源码集成。
- `Bun integration build` 在相关 PR 或手动触发时构建 Windows x64 与
  Linux x64 可执行文件，然后验证两种语言的入口和
  TypeScript → Python 调用链。
- `Release` 只接受 `v*` tag，并且只在两平台构建与 smoke test 全部成功
  后发布资产。

## 许可证

本项目自身代码使用 [MIT License](LICENSE)。最终 `poly` 可执行文件还会
包含 Bun、JavaScriptCore/WebKit、RustPython 等第三方组件，它们继续
适用各自的许可证。初步清单及正式发布要求见
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

正式版本计划由 Git tag 触发 GitHub Actions 构建，并作为带有构建元数据
和校验和的 GitHub Release Assets 发布。

## 上游依据

- [Bun 主线 Rust workspace](https://github.com/oven-sh/bun/blob/main/Cargo.toml)
- [Bun 源码构建说明](https://github.com/oven-sh/bun/blob/main/CONTRIBUTING.md)
- [RustPython 0.5.0 嵌入 API](https://docs.rs/rustpython/0.5.0/rustpython/)
- [uv 支持的 Python 实现](https://docs.astral.sh/uv/reference/policies/python/)
