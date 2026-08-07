# 实施路线图

## M0：同进程原型

- [x] 固定 Bun 主线提交
- [x] RustPython 独立嵌入 crate
- [x] 冻结 Python 标准库
- [x] Bun workspace 集成补丁
- [x] `.py` 入口在 `bun_bin` 内分派
- [x] `Bun.polyPythonCall` JSC host function
- [x] TypeScript SDK 不使用子进程
- [x] Python stdout 与 traceback 响应
- [x] RustPython bridge 真实调用测试
- [x] Bun/JSC 与 RustPython 同线程执行模型
- [x] Bun 补丁 apply/reverse-apply 检查
- [ ] 完整 Bun Windows debug build
- [ ] 运行 `.py` 入口验证
- [ ] 运行 TS→Python 示例验证

## M1：可开发的双语言项目

- [ ] `poly init`
- [ ] `poly.toml` 默认入口
- [ ] Python module cache 与 reload 语义
- [ ] 协议版本握手
- [ ] bytes、BigInt 与 non-finite number 标签
- [ ] JS event loop 与 Python 协作式任务调度
- [ ] 取消、超时和退出语义
- [ ] Windows / Linux / macOS CI

## M2：uv 与依赖兼容性

- [ ] `poly sync`
- [ ] uv lock 与 staging
- [ ] wheel / sdist 检查
- [ ] 原生扩展拒绝策略
- [ ] RustPython smoke imports
- [ ] 兼容性报告
- [ ] 依赖缓存

## M3：Bundle

- [ ] Bun 生成 JS bundle
- [ ] Python 模块图
- [ ] Python 依赖与资源收集
- [ ] poly archive + footer
- [ ] `poly build`
- [ ] 当前平台单文件输出
- [ ] 可复现构建
- [ ] Git tag 驱动的多平台 GitHub Actions
- [ ] GitHub Release Assets、SHA-256 与 provenance
- [ ] SPDX SBOM、第三方许可证和 LGPL 重新链接材料
- [ ] GitHub Pages 安装脚本与干净环境安装验证

### Browser build target 调研

浏览器只作为 `poly build` 的输出目标，不属于 Poly runtime 的受支持宿主平台，
也不进入 Native Profile 的平台矩阵或 JIT 兼容承诺。

- [ ] `poly build --target browser` 输出 ESM、资源清单和可部署静态文件
- [ ] 支持 TS/JS 入口、动态 import、DOM、Fetch、WebSocket 和 Web Worker
- [ ] 使用浏览器原生 `WebAssembly` 支持 Core Wasm 模块与 JS interop
- [ ] 输出 Bun/Node API、filesystem、network、process 和 native extension 的
      browser target 兼容性报告
- [ ] 构建时拒绝依赖 native Python、LuaJIT、CoreCLR、Shell 或本机 SQL driver
      的 required capability
- [ ] 不静默替换 runtime、改用远程服务或把 AOT/interpreter 宣传为 native JIT
- [ ] 通过 feature detection 和构建产物 conformance 覆盖主流浏览器

## M4：语言级 interop

- [x] TS `import x from "./x.py"` loader（v1：ESM 合成，JSON 常量 + callable wrapper）
- [x] Python `import js.x` finder/loader（v1：路径 specifier；裸包名待 resolver）
- [x] callable proxy（v1：双向同步，按 specifier+name 无句柄）
- [ ] Promise / coroutine 映射
- [ ] Python→JS 安全重入
- [ ] GC、弱引用与释放协议

## M5：降低 fork 成本

- [ ] Bun 上游变更监测
- [ ] Bun callback integration tests
- [ ] 评估上游 embedding API
- [ ] 后端 conformance suite
- [ ] capability 权限模型

## 统一交互式 REPL

任何新增的通用语言或内置领域语言，只有提供可持续使用的 REPL，才算成为 Poly
的一等语言能力。统一入口暂定为 `poly repl <language>`，覆盖 `js`、`python`、
`lua`、`shell` 和 `sql`；语言自己的快捷命令可以后续增加，但不能形成另一套会话
语义。

### 通用会话契约

- [ ] 为 JS/TS 复用 Bun REPL，为 Python 接入 RustPython interactive compiler
- [ ] 持久保存语言上下文；变量、模块缓存、对象 handle 和工作目录可跨命令使用
- [ ] 统一 history、multiline、语法高亮、补全、粘贴、错误展示和终端宽度处理
- [ ] `Ctrl+C` 中断当前求值但不终止会话，`Ctrl+D` 在输入为空时退出
- [ ] 对支持异步的语言定义 top-level await、coroutine、取消和 event loop 语义
- [ ] REPL 中创建的跨语言 proxy 可继续调用，并遵守 GC、异常和释放协议
- [ ] 按语言隔离 history，并对 URL、连接串、token 和 password 做持久化脱敏
- [ ] 提供适合脚本和工具调用的 `-e`、print/JSON 输出模式，不依赖伪终端
- [ ] 在 Windows、Linux、macOS 的交互终端和重定向输入下建立 conformance suite

实现基线参考 [Bun REPL](https://bun.com/docs/runtime/repl)；Lua、Shell 和 SQL 的
专有会话语义在各自路线中定义。

## 平台与发行 Profile

Poly 不用一张模糊的“可构建平台”列表代表正式支持。每个平台必须归入明确的
发行 Profile；只有该 Profile 要求的运行时、JIT、入口、互操作、GC、取消和
bundle conformance 全部通过，才可以晋升为正式支持。

### Native Profile：完整同进程 JIT 与互操作

Native Profile 保持 Bun/JSC、RustPython、LuaJIT 以及未来通过准入门的 runtime
在一个本机进程中运行。首个发布门禁仍先完成现有 Windows x64 与 Linux x64
glibc 目标；之后按下列顺序扩展并逐个平台晋升：

- [x] Windows x64：完整 Poly Release、JS、Python 与 TS→Python smoke
- [ ] Linux x64 glibc：完整 Poly Release 与 smoke
- [ ] macOS ARM64：完整构建、签名、JIT 与互操作 conformance
- [ ] Linux ARM64 glibc：完整构建、JIT 与互操作 conformance
- [ ] Windows ARM64：完整构建、JIT 与互操作 conformance
- [ ] macOS x64：作为 Intel Mac 兼容目标评估需求和维护期限
- [ ] Linux x64/ARM64 musl：作为 preview 目标验证，不阻塞 glibc Release
- [ ] 为 x64 baseline 与 modern CPU 产物定义命名、自动选择和测试策略

一个 Native Profile 平台只有在每个已启用语言的入口、所有成对双向 interop、
真实 JIT、异常、重入、GC、资源限额和单文件 bundle 都通过后，才可标为正式
支持；某个 bridge crate 能编译或单独测试通过不等于完整平台支持。

### Android Native Profile：战略平台

Android 的目标不是把应用预编译成 WebAssembly，而是将 Poly 作为 native
library 嵌入应用进程，并尽可能保持与桌面 Native Profile 相同的 JIT 和互操作
语义。首选 `arm64-v8a` 作为设备发行目标，`x86_64` 仅用于模拟器和 CI。

#### 可行性门

- [ ] 为 Android ARM64 构建 Bun/JavaScriptCore，验证 JSC JIT、event loop、
      filesystem、network、TLS 和 package resolution
- [ ] 生成可由 Kotlin/Java 通过 JNI 启动和关闭的 `libpoly.so`
- [ ] 在同一 Android app process 中验证 JSC、RustPython 与 LuaJIT 共存、
      线程附着、回调和异常边界
- [ ] 在 ARM64 真机证明 Lua 源码产生 JIT trace，而不是解释器回退
- [ ] 定义 Activity、后台暂停、低内存通知、应用退出与 runtime 生命周期
- [ ] 建立 Kotlin/Java、JS、Python 和 Lua 之间的 Android host bridge
- [ ] 验证 APK/AAB 体积、启动时间、峰值内存、耗电和崩溃诊断

#### 分发与安全

- [ ] Play Store Profile 默认把源码、assembly 和 native library 收入已签名
      APK/AAB，不在运行时静默下载可执行代码
- [ ] 仅从 app internal storage 加载开发期代码，并在执行前验证来源和完整性
- [ ] 将远程热更新、JIT、FFI、filesystem 和 network 分别纳入 capability 策略
- [ ] 输出 Android 与桌面 Native Profile 的 API 和运行语义差异报告
- [ ] 只有通过真机 release conformance 后，才把 Android 从 research 提升为
      preview 或正式支持

### 当前不进入支持路线

- iOS 上要求 native JIT 的完整 Profile
- 32 位 Windows、Linux 和 Android
- FreeBSD、OpenBSD、NetBSD
- RISC-V、PPC、S390x 等尚未形成全部运行时交集的架构

调研基线参考 [Bun 支持平台](https://bun.com/docs/installation)、
[LuaJIT 平台状态](https://luajit.org/status.html)、
[Android 动态代码加载安全指南](https://developer.android.com/privacy-and-security/risks/dynamic-code-loading)。

## 第三语言：JIT 与同进程互操作

第三语言必须能在 Poly 进程内动态加载源码或中间表示，并由运行时 JIT 为本机
机器码；同时必须提供稳定的原生嵌入 API，支持双向函数调用、对象句柄、异常和
GC 生命周期互操作。子进程、仅 AOT 动态库以及先编译为 Wasm/WASI 的方案不计入
这条路线。LuaJIT 是当前唯一进入实施顺序的第三语言候选；CoreCLR/C# 暂缓，
不阻塞 Lua、Shell、SQL 及其 REPL。

### LuaJIT：优先候选

#### 嵌入与发行基线

- [ ] 固定 LuaJIT v2.1 production 分支提交，记录 rolling release 与 ABI 策略
- [ ] 在每个 Native Profile 候选目标验证构建、JIT 启用和解释器回退
- [ ] 将 LuaJIT 作为 vendored native runtime 链入 Poly，不依赖外部 Lua 安装
- [ ] 建立 `lua_State` 的创建、关闭、内存限额、指令限额和取消原语
- [ ] 验证 `.lua` 源码在 Poly 进程内产生并执行 JIT trace，而不是只通过解释器
- [ ] 评估可执行内存、代码签名和 hardened runtime 对各平台发行的影响

#### 语言入口与模块

- [ ] `.lua` 入口分派、模块缓存和 reload 语义
- [ ] `poly.toml` 的 Lua 入口、module path、JIT 和资源限额配置
- [ ] TS/JS 静态导入 `.lua` 模块
- [ ] Lua 通过受控的 `poly.js` 与 `poly.python` 模块访问其他运行时
- [ ] 将 Lua 源码、模块图和资源纳入 `poly build` 单文件 archive

#### REPL

- [ ] `poly repl lua` 在整个会话中复用同一个 `lua_State`
- [ ] 自动打印 expression 结果，同时支持 statement、multiline chunk 和粘贴输入
- [ ] 使用 Lua parser 判断 incomplete chunk，不用定时等待或行尾启发式猜测
- [ ] 补全 global、table field 和已加载 module，并保存独立的 Lua history
- [ ] `Ctrl+C` 安全中断当前 chunk，`Ctrl+D` 退出且完整释放会话资源
- [ ] 显示 Lua traceback 和 JIT 状态，证明 REPL 输入也能产生并执行 JIT trace
- [ ] 允许 REPL 中导入和调用 JS/Python，跨命令保存的 proxy 遵守 GC 与释放协议

#### 双向 interop

- [ ] 为 nil、boolean、number、整数、字符串、bytes、array 和 record 定义
      Lua `PolyValue` 映射
- [ ] JS/Python/Lua callable proxy 与同步安全重入
- [ ] Promise、Python coroutine 与 Lua coroutine 的调度和取消映射
- [ ] Lua error、JS exception 与 Python exception 的 traceback 保真转换
- [ ] 用 userdata、registry handle 和 finalizer 定义跨 GC 所有权与释放协议
- [ ] 禁止或 capability-gate LuaJIT FFI、`loadlib`、filesystem、network 和 debug API
- [ ] 将 LuaJIT 加入统一 interop、GC、重入、取消和资源限额 conformance suite

### C / 内嵌 TinyCC：已入实施顺序的便宜语言

C 经 Poly 内嵌的 TinyCC（bun:ffi `cc`，进程内编译到本机机器码、无子进程）是
当前成本最低的第三语言候选，已实现 v1 入口分派：`.c` 入口编译后在进程内
调用 `int main(void)`，其返回值成为进程退出码。

- [x] `.c` 入口分派（`poly app.c`；要求入口定义 `int main(void)`）
- [x] `poly repl` 内 `.c` 模式（v2：真正的 REPL 语义 —— 无分号输入按表达式求值并回显
      值；`;`/`}` 结尾输入按声明（文件作用域，持久）或语句（单次执行，副作用不重复）
      分类；函数定义/递归/调用可用。限制：TinyCC 每次输入整体重编译，赋值语句对
      文件作用域变量的修改不跨输入持久（有明确警告）；指针/结构体返回值暂不导出）
- [x] v1 双向 interop：C REPL 编译的函数按名字发现（正则解析签名）并暴露为
      `globalThis.__polyC[name]`，JS 直接调用；Python 经 `poly.c(name, ...)` 调用
- [ ] C 回调 JS/Python、GC 生命周期与释放协议、指针/结构体返回值与复杂参数
- [ ] 与 LuaJIT 相同的准入评估：内存/指令限额、取消、可执行内存与代码签名
      影响、各 Native Profile 平台验证
- [ ] 只有通过完整 interop、重入、GC 与资源限额 conformance，才计入
      Native Profile 门禁；v1 入口分派不构成完整平台支持

### CoreCLR / C#：暂缓研究

C# 暂不进入实施顺序，也不作为当前 Native Profile、Android 或统一 REPL 的发布
门禁。其 JIT 本身可行，但源码编译、可嵌入的交互体验、发行体积、runtime 卸载和
多 GC 互操作还不足以形成与 LuaJIT 同等明确的路线。

- [ ] 等 LuaJIT、Shell 和 SQL 的 REPL 与 interop conformance 稳定后再重新评估
- [ ] 比较 Roslyn scripting 与内存 assembly 编译的 multiline、补全、history、
      top-level await、诊断和真实 CoreCLR JIT 行为
- [ ] 重新验证 `hostfxr`、collectible `AssemblyLoadContext`、`GCHandle`、`Task`
      bridge、单进程隔离、发行体积和各 Native Profile 平台成本
- [ ] 只有可嵌入 REPL 与双向 interop 原型同时通过准入门，才恢复实施里程碑

## 内置领域语言：Shell 与 SQL

Shell 和 SQL 是 Poly 的一等领域语言，但不是满足“第三语言”JIT 条件的通用
runtime。它们复用 Bun 已有能力，并通过同一 Poly host bridge 向 JS、Python 和
Lua 暴露；这不会把子进程命令或数据库服务端执行宣传为同进程 JIT。

### Bun Shell：bash-like Shell

Bun Shell 拥有自己的 lexer、parser 和 interpreter，提供跨平台的 bash-like
语法，但不宣称完整 Bash 兼容。内建命令可在进程内执行；外部命令仍按 Shell
本身的语义创建子进程。

- [x] 当前 Poly 可执行文件可从 JS 通过 `import { $ } from "bun"` 调用内建命令
- [x] `.sh` 入口分派（原生 Bun Shell 解释器，JSC 初始化前运行；`poly app.sh`，exit code 透传）
- [x] `poly repl` 内 `.sh` 模式（每输入经 Bun Shell 进程内求值，返回 exit code）
- [ ] `.sh` 模块图与 bundle 纳入；REPL 的 cwd/env/shell variables 持久化、multiline、补全与 history
- [ ] `poly repl shell` 持久保存 cwd、environment 和 shell variables
- [ ] 为 pipe、redirect、command substitution 和 incomplete construct 实现
      multiline 输入、补全、history 与结构化错误
- [x] 同进程 `poly` shell builtin（REPL Shell 模式内：`poly js <expr>` / `poly python <expr>` / `poly sql <query>`；
      注意：`.sh` 入口在 JSC 初始化前运行，无 VM，builtin 不可用）
- [ ] 定义 Python/Lua 的 `poly.shell` API，以及 stdout、stderr、exit code、bytes、
      lines、streaming 和 backpressure 映射
- [ ] 统一取消、timeout、signal、外部进程清理和 runtime 退出行为
- [ ] capability-gate 外部命令、filesystem、environment 和 network 访问
- [ ] 建立 Bash 兼容矩阵，明确不支持或语义不同的 syntax、builtin 和 job control
- [x] 同进程 `poly` shell builtin，用于调用 JS、Python、Lua module 或 callable
      （REPL Shell 模式内，见上）
- [ ] 在各 Native Profile 平台验证相同脚本的 quoting、path 和 exit semantics

实现基线参考 [Bun Shell](https://bun.com/docs/runtime/shell) 和
[Shell loader](https://bun.com/docs/bundler/loaders#shell)。

### SQL：会话、连接与跨语言查询

SQL 以 Bun SQL 的 PostgreSQL、MySQL 和 SQLite 统一 API 为宿主基线；REPL 的
连接、事务和结果游标属于持久会话状态，而不是每条语句重新启动 runtime。

- [x] 当前 Poly 可执行文件可用 `Bun.SQL(":memory:")` 执行 SQLite 查询
- [x] `.sql` 入口（`poly app.sql`，内存 SQLite via `bun:sqlite` exec，多语句脚本）
- [x] `poly repl` 内 `.sql` 模式（会话持久内存 SQLite，结果 JSON 输出；`.editor` 提供多行输入）
- [ ] `poly repl sql --database <name>`、poly.toml 连接声明与统一连接池
- [ ] 在 `poly.toml` 声明 SQLite/PostgreSQL/MySQL 连接；secret 只从环境或
      secret provider 注入，不写入 archive、history 或诊断
- [x] REPL 会话数据库以 `globalThis.__polySql` 暴露给 JS；Python 经内嵌 `poly` 包
      调用（`poly.sql(query)` / `poly.sqlexec(script)`）
- [ ] 由 Poly host 统一拥有 connection pool，并向 JS、Python、Lua 暴露
      `poly.sql`，避免每个 runtime 重复建池；命名连接与 poly.toml 声明
- [ ] 定义参数协议，禁止跨语言调用退化为字符串拼接 SQL
- [ ] 映射 null、int64、decimal、float、text、blob、date/time、JSON 和 UUID
- [ ] 为 row、cursor、streaming、backpressure、取消、timeout 和数据库错误建立
      `PolyValue` 与异常协议
- [ ] 在 REPL 中持久保存当前连接、transaction/savepoint 和输出模式
- [ ] 支持 table、JSON、CSV 输出；为 `.tables`、`.schema`、`.mode` 等 metadata
      命令定义 dialect-aware 行为
- [ ] SQLite filesystem 与 PostgreSQL/MySQL network 分别进入 capability 策略
- [ ] SQLite 先达到正式支持，再让 PostgreSQL 和 MySQL 通过独立 conformance

实现基线参考 [Bun SQL](https://bun.com/docs/runtime/sql)。

## 关注列表：尚未进入实施顺序

关注列表用于记录方向匹配、但嵌入边界或维护成本尚未达到 Poly 准入条件的项目。
进入此列表不代表版本承诺，也不成为 Native Profile 或 Release 的门禁。

### Nushell：等待稳定嵌入条件

Nushell 的结构化 `Value`、`PipelineData` 和交互体验与 Poly 的 Shell、SQL 及
`PolyValue` stream 很契合。当前主要障碍不是功能成熟度，而是 `nu-engine` 等
crate 仍被标为内部接口，以及 Nushell 自己的值系统、REPL、配置和插件生命周期
需要与 Poly host 合并，而不能并排形成第二套控制面。

- [ ] 上游提供或长期稳定一组公共 embedding API，覆盖 parser、engine、eval 和 REPL
- [ ] 验证同进程初始化、重复求值、`Ctrl+C` 中断和退出，不接管 Poly 的 signal、
      terminal 或 process 生命周期
- [ ] 定义 `Nu Value` / `PipelineData` 与 `PolyValue` 的映射，包括 record、table、
      bytes、lazy stream、error、custom value、resource handle 和 drop notification
- [ ] 接入 `poly repl nu` 与 `.nu` 入口，复用 Poly 的 history、secrets、取消和权限契约
- [ ] 将 JS、Python、Lua callable 注册为同进程 Nu command；进程外 plugin protocol
      不作为主要 interop 路径
- [ ] capability-gate external command、filesystem、environment、network、config 和 plugin
- [ ] 测量裁剪后的可执行体积、启动时间、常驻内存、编译时间和上游升级成本
- [ ] 只有公共 API 与跨平台 conformance 连续稳定后，才晋升为内置领域语言

关注基线参考 [Nushell pipeline](https://www.nushell.sh/book/pipelines.html)、
[`nu-engine`](https://docs.rs/nu-engine/latest/nu_engine/)、
[`nu-cli` REPL](https://docs.rs/nu-cli/latest/nu_cli/fn.evaluate_repl.html) 和
[plugin protocol](https://www.nushell.sh/contributor-book/plugins.html)。

### Rust-native Haskell-like JIT runtime：长期研发候选

这条路线不是在 Poly 中嵌入 GHC，也不是用 Rust 重写一个兼容 GHC、Hackage 和
全部语言扩展的实现。目标是从零设计一门 embedding-first 的 Haskell-like 惰性
函数式语言：compiler、GC 和 runtime 以 Rust 实现，源码在 Poly 进程内编译为
本机机器码，并从第一天使用 `PolyValue` ABI 和统一 REPL。它不经过子进程、GHC
RTS 或 Wasm/WASI。

在兼容边界和正式名称确定前，不占用 `.hs` 扩展名，也不宣称 Haskell 兼容；
以下入口使用 `poly repl hs-like` 作为路线图占位名。

#### 语言与语义边界

- [ ] 实现 layout-aware parser、renamer、module/import 和可保留 source span 的 Core IR
- [ ] 先支持 lambda、application、`let`、`letrec`、algebraic data type 和
      pattern matching
- [ ] 实现 Hindley–Milner 类型推断、parametric polymorphism 和明确的诊断格式
- [ ] 用 thunk、closure、update、sharing 和 blackhole 定义 call-by-need 语义
- [ ] 定义 integer、float、boolean、char、text、bytes、list、tuple 和 record 基线
- [ ] `IO` 只能经 Poly host capability 访问 filesystem、network、clock、random
      和 process，不另建一套不受控系统 API
- [ ] 把 typeclass、higher-rank type、GADT、type family、STM、async exception 和
      Template Haskell 留在独立晋升门后，不阻塞最小语言闭环
- [ ] 明确不兼容 GHC ABI、`.hi`、GHC package database 和现有 Hackage binary

#### 惰性 runtime 与本机 JIT

- [ ] 建立可执行语义的 Core interpreter，作为测试 oracle 而不是正式性能后端
- [ ] 使用 Cranelift 将 Core function 在 Poly 进程内增量编译为本机机器码
- [ ] 让 REPL expression、加载的 module 和递归 closure 都走 native code，并提供
      可验证的 JIT event/trace，禁止静默退回 interpreter
- [ ] 建立 thunk heap、closure layout、constructor tag、indirection 和 update frame
- [ ] 第一阶段使用 non-moving heap 或 stable handle，避免向 JIT 暴露可移动裸指针
- [ ] 定义 GC root、safepoint、stack map、write barrier、finalizer 和跨 runtime
      引用环处理
- [ ] 建立 executable memory、code cache、symbol resolution、失效、回收和 W^X 策略
- [ ] 在 baseline JIT 稳定后再评估 hot counter、specialization、guard、deoptimization
      和 interpreter-to-JIT tiering
- [ ] 在 Windows x64、Linux x64 和后续 Native Profile 平台验证 JIT 与 GC conformance

#### Poly interop 与 REPL

- [ ] 定义 lazy value 穿过 `PolyValue` 边界时的 force 规则和显式 thunk handle
- [ ] 映射 algebraic data、list、record、multiple-result、closure 和 typed callable
- [ ] JS、Python、Lua 可调用编译后的函数，hs-like code 也可调用同进程 callable
- [ ] 统一 exception、bottom、取消、安全重入、Promise/coroutine 和 `IO` 调度语义
- [ ] `poly repl hs-like` 持久保存 module、binding、type environment 和 JIT code cache
- [ ] REPL 支持 multiline、补全、history、自动打印 inferred type、`:type`、中断和
      native-code 状态检查
- [ ] 单文件 bundle 收集源码、Core/cache metadata 和资源；是否携带机器码由
      reproducibility 与目标平台策略决定

#### 原型与晋升门

- [ ] 跑通 `letrec + lazy list + pattern matching` → Cranelift native code →
      调用 JS function → 返回 `PolyValue` 的同进程端到端原型
- [ ] 用 differential/property tests 对照 Core interpreter 与 JIT 的值、异常和求值顺序
- [ ] 测量 REPL 编译延迟、代码体积、启动时间、峰值内存、GC pause 和跨语言开销
- [ ] fuzz parser、type checker、Core lowering、GC handle 和 JIT/native ABI 边界
- [ ] 只有上述闭环在至少 Windows x64 与 Linux x64 稳定后，才进入第三语言实施顺序

研究基线参考 [Lambdachine](https://github.com/nominolo/lambdachine)、
[Gluon](https://github.com/gluon-lang/gluon)、
[HVM2](https://github.com/HigherOrderCO/HVM2)、
[Bend](https://github.com/HigherOrderCO/Bend) 和
[Cranelift](https://github.com/bytecodealliance/wasmtime/tree/main/cranelift)。

## Wasm / WASI：复用 Bun 与 JavaScriptCore 的现有引擎

这条路线把 Wasm 视为可由多种语言生成的执行格式和模块后端，而不是第三门
源码语言。默认复用 Bun 已经链接的 JavaScriptCore WebAssembly 引擎，避免
在同一个 Poly 可执行文件中重复嵌入 Wasmtime、Wasmer 等第二套 Wasm 引擎。

### 已验证基线

- [x] JavaScriptCore `WebAssembly` Core API 随 Bun 一起进入 Poly 可执行文件
- [x] 当前 Windows `dist/poly.exe` 可直接运行 `.wasm` 入口
- [x] 使用 Bun `node:wasi` 运行 `wasi_snapshot_preview1` hello-world smoke test
- [ ] 固定并记录当前 JSC 支持的 Core Wasm proposal 矩阵
- [ ] 建立 Bun `node:wasi` Preview 1 syscall 支持与限制矩阵
- [ ] 在 Windows、Linux、macOS CI 重复 `.wasm` 与 WASI Preview 1 smoke test

### 一等项目能力

- [ ] 在 `poly.toml` 声明 `.wasm` 默认入口及其参数、环境变量和 preopen 目录
- [ ] `poly run` 显式分派 Core Wasm 与 WASI Preview 1 入口
- [ ] 将 `.wasm` 文件纳入模块图、资源收集、缓存和变更监测
- [ ] `poly build` 把 Wasm 模块及其清单收进单文件 archive
- [ ] 输出实际启用的 Wasm、WASI 和宿主 capability 兼容性报告
- [ ] 默认最小权限，并显式配置 filesystem、environment、stdio、clock、
      random、network 等 WASI capability

### 跨运行时 interop

- [ ] TS/JS 静态导入 `.wasm`，生成类型稳定的 export wrapper
- [ ] 定义字符串、bytes、整数、浮点数、list、record 与错误的 Core Wasm ABI
- [ ] 定义资源句柄的所有权、释放、取消、超时和实例生命周期
- [ ] Python 通过 Poly host bridge 调用同一个 JSC Wasm 实例，不在 RustPython
      内再嵌入一套 Wasm 引擎
- [ ] 明确 Wasm 调用与 JS event loop、Python coroutine 的调度及安全重入语义
- [ ] 将 Wasm 后端加入统一 conformance suite

### Component Model / WASI Preview 2 调研门

- [ ] 调研 Component Model、WIT、Canonical ABI 与 WASI Preview 2 的稳定程度
- [ ] 验证 Bun/JSC 是否能加载 component，而不只是在 Core Wasm 上运行
      `wasi_snapshot_preview1`
- [ ] 评估 WIT 是否适合作为 TS、Python 与 Wasm 的统一接口描述
- [ ] 仅在 Bun/JSC 无法满足 Component Model、WASI Preview 2、资源限额或 AOT
      需求时，再评估可选 Wasmtime 后端及其体积、许可证和调度成本
