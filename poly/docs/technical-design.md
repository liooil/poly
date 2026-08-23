# 第一版技术方案：同进程 Bun + RustPython

## 1. 硬约束

M0 必须满足：

1. 最终只有一个可执行文件和一个 OS 进程；
2. JS/TS 由 Bun runtime 执行，不换成 Boa、QuickJS 或独立 JavaScriptCore wrapper；
3. Python 由内嵌 RustPython 执行；
4. JS→Python 调用不经过子进程、socket 或 stdio RPC；
5. Bun、RustPython、interop 和 bundle 最终由同一个 Rust 主线工程组织。

## 2. 为什么采用 Bun 下游源码集成

Bun 主线已经是 Cargo workspace，并包含 `bun_runtime` 与 `bun_bin`。但目前：

- `bun_runtime` 是内部 crate，依赖 Bun 几乎完整的 crate graph 和生成代码；
- `bun_bin` 产物是供 Bun 自己最终链接的 staticlib；
- CLI 入口以进程生命周期为中心，会调用全局退出逻辑；
- JavaScriptCore 绑定依赖 Bun 的 C++、代码生成和定制 WebKit/JSC 构建。

因此，外部 crate 直接写 `bun_runtime = { git = ... }` 不是稳定或完整的集成方式。M0 固定一个 Bun 提交，在 Bun workspace 内加入 RustPython crate。以后若 Bun 提供稳定 embedding API，再把补丁缩减为公开依赖。

RustPython 0.5.0 依赖 malachite 0.9，而 `pymath 0.2` 的约束允许 Cargo 在大型工作区另选 malachite 0.10，造成两个不兼容的 `BigInt` 类型。bootstrap 会把该依赖统一锁定到 0.9.1；这属于当前上游依赖约束的兼容补丁。

## 3. 运行时结构

```text
poly executable
|
+-- bun_bin process entry
|   +-- .py entry -> poly_python::run_file
|   +-- .js/.ts   -> Bun CLI and runtime
|
+-- bun_runtime
|   +-- Bun.polyPythonCall JSC host function
|       +-- poly_python::call_json
|           +-- thread-local persistent RustPython VM
|               +-- executes on the Bun/JSC runtime thread
|
+-- JavaScriptCore
+-- RustPython frozen stdlib
```

直接执行 `.py` 时，`bun_bin` 在进入 Bun CLI 分派前把文件交给 RustPython。执行 JS/TS 时走 Bun 原本路径。

## 4. 同进程 interop

### 4.1 JSC 入口

补丁把 `polyPythonCall` 加入 BunObject 的 LUT 和 callback export 表。JSC 调用 Rust host function：

```text
Bun.polyPythonCall(string) -> string
```

内部字符串是版本 0 的 JSON envelope。保持底层为字符串有两个目的：

- M0 不把 JSC 与 RustPython 的 GC 对象互相持有；
- 上层 SDK 可以保持稳定，后续 transport 换成直接值转换或对象代理。

### 4.2 Python VM

`poly_python::call_json` 在当前 Bun/JSC runtime 线程上惰性创建并复用 thread-local RustPython VM。没有 worker、channel 或跨线程值搬运。Bun 的 Windows 主线程链接栈已经设为 `0x1200000`（18 MiB），足以避开 RustPython 在普通小栈测试线程上的初始化溢出；crate 测试会显式创建 16 MiB 测试线程，但 Python 调用仍发生在该调用者线程本身。

每次调用：

1. JSC host function 在当前 runtime 线程调用 Rust bridge；
2. bridge 校验并规范化模块路径；
3. 把请求字符串写入 Python scope；
4. 通过 `runpy.run_path` 加载模块；
5. 调用指定同步函数；
6. 捕获 Python stdout 与 traceback；
7. 直接向同一 JSC 调用栈返回 JSON response。

当前每次会重新执行目标 `.py` 文件。模块缓存和显式 reload 语义留到 M1。

### 4.3 类型与错误

支持：

- `null` / `None`
- boolean
- JSON number
- UTF-8 string
- array / list
- string-key object / dict

Python 异常被转换成 `{ ok: false, error, traceback, stdout }`。Rust/JSC 层初始化失败则抛出 JavaScript exception。

## 5. CLI

同一个产物支持：

```text
poly app.py
poly run app.py
poly app.ts
poly run app.ts
```

M0 的 Python 路由只识别第一个入口参数或 `run` 后的直接入口，不处理 Bun 所有 watch/hot/debug 参数组合。

## 6. uv 设计

保留：

```text
pyproject.toml
uv.lock
package.json
bun.lock
```

`poly.toml` 始终可选，不参与依赖解析或锁定。

Poly 直接链接固定 revision 的 uv Rust crates，并通过 `poly_uv` 适配层
同进程调用。不得启动 `uv` 可执行文件，也不得调用 uv 面向独立进程的
`unsafe main(args)` 入口。uv 的内部 API 只允许出现在 `poly_uv` 内，避免
其不稳定类型扩散到运行时其余部分。

`uv` 的职责限定为：

- 依赖解析与 lock；
- 下载 wheel；
- 把包物化到 bundle staging 目录；
- 提供缓存。

Poly 的职责：

- 禁止或隔离原生 CPython wheel；
- 扫描传递依赖；
- 用 RustPython 做 smoke import；
- 输出兼容性报告；
- 决定哪些文件进入 bundle。

首个依赖管理切片只接受无需构建的纯 Python wheel。sdist 构建和原生
CPython wheel 均不能通过外部 Python、构建后端或其他子进程完成；在有
同进程实现之前应明确拒绝。

`poly_uv` 使用 uv 自己的 `Lock::from_toml` 验证 `uv.lock`，并使用 uv 的
wheel filename/tag 类型生成 RustPython 安装计划。当前保守边界只接受
包含通用 Python 3 tag、`none` ABI 与 `any` platform 的 wheel；具体包仍
必须在物化后通过 RustPython smoke import。

物化阶段先按 lock 中的文件名与 SHA-256 校验本地 wheel，再用
`uv_extract` 同进程解包，并给 `uv_install_wheel` 提供 Poly 自己的
`Layout`，把 `purelib`/`platlib` 直接指向 `.poly/python`。这条路径不需要
发现、启动或伪造 CPython virtualenv。

## 7. Bundle

最终产物本身已经同时链接 Bun 与 RustPython。下一阶段的 bundle 只需解决应用资源：

```text
poly build main.ts
  +-- Bun bundler -> JS bundle
  +-- Python module/package collection
  +-- manifest + resources archive
  +-- append to poly launcher
  -> app.exe
```

首版格式建议为 launcher + archive + fixed footer，避免一开始分别实现 PE、ELF、Mach-O 资源写入。

## 8. 已知风险

| 风险 | 当前处理 |
|---|---|
| Bun 内部 API 快速变化 | 固定 commit，补丁做最小化 |
| Bun 完整源码构建很重 | bootstrap 自动化，CI 缓存 |
| RustPython 不是 uv 支持实现 | uv 不负责解释器发现 |
| Python 阻塞 JS event loop | M0 明确采用同线程同步调用，M1 引入协作式任务调度 |
| VM/模块状态长期驻留 | thread-local 单 owner，后续加 reset |
| JSON 丢失对象身份与大整数精度 | 协议后续增加 tagged values |
| Python→JS 嵌套重入破坏 VM 状态 | M0 不开放，M1 增加显式重入状态机 |
