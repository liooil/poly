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

`.py` entries use the current Poly routing path and execute with RustPython;
`.js`/`.ts` entries use the Bun CLI and runtime. Cross-language module loading
uses the persistent bridge VM; standalone file execution remains script-scoped.

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

每次 bridge 请求：

1. JSC host function 在当前 runtime 线程调用 Rust bridge；
2. bridge 规范化模块路径；
3. persistent module registry 先查询已加载状态；
4. 首次加载时在持久 RustPython VM 中执行模块并保存状态；
5. 调用指定同步函数；
6. 捕获 Python stdout 与 traceback；
7. 向同一 JSC 调用栈返回 JSON response。

bridge 当前按规范化路径复用自己的模块 registry。Python 标准
`sys.modules` 与 bridge registry 是否已经在所有 TS/Python 导入路径中共享
同一个 module identity，仍由跨边界 conformance 测试确认。reload API 不属于
当前 v1。

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

`poly_uv` 使用 uv 自己的 `Lock::from_toml` 与 lock traversal 验证
`uv.lock`、选择当前 workspace member 的传递依赖，并求值 Python、平台、
实现类型 marker 与 resolution fork。当前保守边界只接受包含通用 Python 3
tag、`none` ABI 与 `any` platform 的 wheel。物化后的静态扫描始终执行；
RustPython smoke import 由显式的 `--check` 开启。

`poly sync` 在进程内组合 uv 的 workspace lowering、registry client、universal
resolver 与 lock 数据结构。默认模式不沿用旧锁中的版本偏好，而是刷新 index、
选择当前约束下可获取的最高版本，再原子更新 `uv.lock`；`--frozen` 跳过解析并
严格消费现有锁。源码构建被关闭，因此这两种模式都不会发现或启动外部 Python。

物化阶段使用 uv 的 HTTP client 在进程内下载 lock 指定的 wheel，
按 SHA-256 建立项目级持久缓存。物化阶段按 lock 中的文件名与 SHA-256
校验 wheel，再用 `uv_extract` 同进程解包，并给 `uv_install_wheel` 提供 Poly
自己的 `Layout`，把 `purelib`/`platlib` 直接指向临时的 `.poly/python`
环境。Poly 根据每个 distribution 的 `RECORD` 推导安全的 import 候选，并拒绝
原生动态库和没有对应源码的 CPython bytecode；没有源码 import 候选的包标记为
`unverified`，不会伪报兼容。默认 `poly sync` 到此为止，不执行依赖代码。

`poly sync --check` 会在同一进程的新 RustPython VM 中逐个导入这些候选。VM 的
搜索路径只包含 frozen 标准库和 staging 环境，不读取用户 site、环境变量或
`site.py`，也不写 bytecode。这个隔离限制的是 Python import 环境，并不是 OS
安全沙箱：包的顶层代码仍会执行，因此 smoke import 必须保持显式启用。

静态扫描或 smoke import 失败发生在 commit 之前，原有 `uv.lock` 与
`.poly/python` 都保持不变。验证通过后才用目录 rename 激活新环境并原子写入
新 lock；若 lock 写入失败，环境事务会恢复旧目录。下载、校验、staging 和
兼容性失败同样不会替换旧环境。这条路径不需要发现、启动或伪造 CPython
virtualenv，也不启动 Python 或 uv 子进程。

Python 文件入口、REPL 和 TS→Python module bridge 会从项目祖先中发现
`.poly/python` 并加入 RustPython 的 `sys.path`。

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
