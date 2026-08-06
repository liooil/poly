# PolyMIR：Rust MIR 解释执行（已记录，暂不实现）

> 状态：**设计冻结，未进入实施顺序**。本文记录 2026-08 评估结论与技术验证结果，
> 供未来决定是否启动时使用。不构成版本承诺，不成为任何 Release 门禁。

## 目标

让 `poly run a.rs` 成为可能：Rust 作为 Poly 第三个同进程语言，与 JS、Python 平级。

```text
poly run a.rs
    │
    ├─ rustc frontend
    │   parse → macro expand → type check → monomorphize → MIR
    │
    └─ Poly MIR interpreter
        ├─ virtual memory
        ├─ call stack
        ├─ ownership / drop semantics
        ├─ std shims
        └─ JS / Python host bridge
```

流程止于 MIR：**不生成目标文件、机器码或临时可执行程序**，不进入 LLVM、
Cranelift 或 linker。使用 rustc 前端是因为完整 Rust 的宏展开、trait 求解、
借用检查、泛型单态化不适合重新实现。

## 核心架构

### 复用 `rustc_const_eval::interpret`，不嵌入 MiriMachine

Rust 编译器内部已有通用 MIR interpreter（`rustc_const_eval::interpret`），CTFE
和 Miri 都建立在它上面。它通过 `Machine` trait 把"纯 MIR 执行"与具体运行环境
分离：遇到 CTFE 不允许的操作时，由具体 Machine 决定如何处理。

Miri 的目标是检测未定义行为，不是日常程序运行；其机器层包含 provenance/
别名检查、Stacked/Tree Borrows、数据竞争与弱内存模拟、确定性虚拟 OS、大量
诊断状态，且解释器本身单线程运行。Poly 应复用 MIR evaluator、内存表示、
layout/ABI 处理、drop/unwind、intrinsic 框架和部分 std shims，但去掉或默认
关闭 UB 检测器、borrow tracker、weak-memory simulation、Miri isolation 与
测试诊断状态，实现面向应用运行的 `PolyMachine`。

```rust
struct PolyMachine<'tcx> {
    js_runtime: *mut JSGlobalObject,
    python_runtime: ...,
    filesystem: ...,
    scheduler: ...,
}
```

### 已验证的技术事实（2026-08-06，基于 rust-lang/miri HEAD）

- `miri/src/eval.rs` 公开 `entry_fn`（line 40）、`create_ecx`（line 327）、
  `eval_entry`（line 515），注释明确标注 "Public because this is used by
  Priroda"。Priroda（MIR 单步调试器）位于 `miri/priroda/`，是独立消费者，
  直接创建 Miri 执行上下文并逐步驱动解释器。
- `miri/src/lib.rs:102`：`pub use rustc_const_eval::interpret::{self, AllocMap, Provenance as _};`
- `miri/src/bin/miri.rs` 的 `MiriCompilerCalls` 是"止于 MIR"的官方模板：
  `config.make_codegen_backend = DummyCodegenBackend`，`after_analysis` 拦截后
  直接 `eval_entry`。
- Miri 的嵌入是默认特性：`[features] default = ["stack-cache", "native-lib"]`，
  含 `native_lib` 模块（`init_sv`/`register_retcode_sv`）。其 libffi/libloading/
  capstone 依赖均为 `cfg(unix)`，Windows 上走 stub，嵌入时 Windows 依赖更少。
- crates.io 上的 `miri` 是 0.0.0 占位包（防抢注），**不能作为 crates.io 依赖**，
  只能 git 依赖 `https://github.com/rust-lang/miri` 并锁定 `rev = "..."`。

### 目录规划（未实施）

```text
src/poly_rust/
├── compiler.rs       # rustc_interface，源码到 MIR
├── machine.rs        # PolyMachine
├── eval.rs           # main / function 执行循环
├── memory.rs         # Poly 特有内存及句柄
├── shims/
│   ├── std.rs
│   ├── os.rs
│   ├── js.rs
│   └── python.rs
└── diagnostics.rs
```

## 运行入口

与 `.py` 入口一致，不在 `bun_bin` 启动前截获：

```text
RunCommand
  └─ a.rs
      └─ 启动 JSC VM
          └─ synthetic poly:rust-main
              └─ Bun.polyRustRun(path, argv)
                  └─ poly_rust::run_file()
```

`.py` 入口已采用"先建立 JSC VM，再由 synthetic bootstrap 调用嵌入式解释器"
的方式，以便 Python 反向调用 JavaScript；`.rs` 延续该原则，从第一天起 Rust
就是同进程语言而非孤立的 CLI 功能。

## 第一阶段范围

### 支持

- 完整语法和类型系统；泛型和 trait；enum、match、closure；core / alloc
- `Vec`、`String`、`HashMap`；panic 和 drop
- 参数、环境变量、stdout；受控文件系统；纯 Rust 依赖

```rust
fn fib(n: u64) -> u64 {
    match n {
        0 | 1 => n,
        _ => fib(n - 1) + fib(n - 2),
    }
}

fn main() {
    println!("{}", fib(20));
}
```

### 明确不支持

- 任意 C FFI、inline assembly、原生动态库
- 依赖本机系统调用细节的 crate、build.rs
- procedural macro（标准 rustc 会把 proc macro 编译为宿主动态库再加载，
  破坏"完全不生成和执行本地代码"的约束；首版只支持内置 derive 或禁止
  第三方 proc macro）

## Rust 与 JS/Python 的互操作

内置 Rust crate：

```rust
use poly::{js, python};

fn main() {
    let lodash = js::import("lodash");
    let result = lodash.call("add", &[20.into(), 22.into()]);

    let numpy = python::import("numpy");
    println!("{result}");
}
```

`poly` crate 的函数是特殊 extern symbol：

```rust
pub fn import(specifier: &str) -> Module {
    unsafe { __poly_js_import(specifier) }
}
```

`PolyMachine` 遇到这些 foreign item 时不查找真实动态符号，直接调用当前
JSC / RustPython runtime。不需要 C ABI，不生成动态库，不离开进程。

```text
Rust MIR
   ↓ special foreign item
PolyMachine
   ├─ JSC JSValue
   └─ RustPython PyObject
```

反向（JS 静态导入 `.rs`，`Loader::Rs` 生成 ESM proxy）属第二阶段，先完成
`.rs` entrypoint 与 Rust → JS/Python 调用。

## 未决风险与前置条件

- **`rustc-dev` 组件缺失**：rustup 的 miri component 只分发预编译二进制，
  不提供 `rustc_private` rlib。自行编译嵌入 miri crate 需
  `rustup component add rustc-dev` 并加入 `rust-toolchain.toml` components。
- **双重版本锁定**：miri 每个 commit 绑定一个 nightly。仓库固定
  `nightly-2026-07-20`，需找到 miri 仓库恰好对应那一天的 commit 并以
  `rev = "..."` 锁定；工具链一升级，miri 依赖必须同步更换。
- **dependency-free 边界**：PolyMachine 内核可完全静态链接，但 std 的 MIR
  表示（sysroot）是运行期数据。`println!`/`Vec`/`String` 都要解释 std 的
  MIR；Miri 的机制是运行时从 `rust-src` 构建（`MIRI_SYSROOT`）。单文件移植
  只有一条路：构建时生成 sysroot 并作为资源嵌入 exe，代价是体积 +100–200MB
  （与 std rlib 同量级）。否则必须在目标机现场构建，违反 dependency-free。
- **`create_ecx` 返回 `InterpCx<'tcx, MiriMachine<'tcx>>`，类型固定为
  MiriMachine**：PolyMachine 需直接调 `InterpCx::new` 并自实现 `Machine`
  trait，这是解释器工作量的主体（memory ops、foreign item 分派、layout、
  drop/unwind、intrinsic），没有现成捷径。
- **Windows 系统 API shim 覆盖不足**：Miri 官方建议 Windows 上使用
  `--target x86_64-unknown-linux-gnu` 以获得更好支持；Windows 原生语义覆盖
  不全。

## 启动建议（若未来实施）

1. `rustup component add rustc-dev`
2. workspace 建 `src/poly_rust/` 空 crate，git 依赖 miri
   `rev = <nightly-2026-07-20 对应 commit>`
3. 最小验证：hello-world `.rs` → 前端到 MIR → `eval_entry`（先直接用
   MiriMachine）跑通 `fib(20)` 打印结果
4. 跑通后再谈 `PolyMachine` 定制与 `poly::{js, python}` bridge
