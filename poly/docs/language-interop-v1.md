# 双向同步语言级互操作 v1

## Summary

实现一个同进程、同线程的双向同步互操作版本：

- TypeScript/JavaScript 可直接静态或动态导入 `.py`。
- Python 可通过 `js.import_module()` 自动使用 Bun resolver 加载任意 JS/TS 模块。
- `poly app.py` 也先启动 Bun/JSC，因此 Python 入口具备同样的 JS 导入能力。
- v1 只传递 JSON 兼容值和同步 callable；不引入 subprocess、RPC、Promise/coroutine 或跨 GC 对象句柄。

## Public APIs and semantics

- TypeScript 原生识别：

  ```ts
  import { add, PI } from "./math.py";
  import { add as checkedAdd } from "./math.py" with { type: "python" };
  ```

  `.py` 无属性即可工作；`type: "python"` 可选。`.py` 搭配其他 type、或非 `.py` 搭配 `python` 都明确报错。

- 新增内建 `"poly"` 模块，保留显式低层入口：

  ```ts
  import { python } from "poly";
  python.call("./math.py", "add", [20, 22]); // 同步返回
  ```

- Python 自动加载 JS：

  ```py
  import js

  math = js.import_module("./math.ts")  # 相对当前 Python 文件解析
  result = math.add(20, 22)

  from js.some_package.tools import run
  # js.some_package.tools -> Bun bare specifier "some_package/tools"
  ```

  `js.import_module(specifier)` 支持相对路径、绝对路径、包名和 Bun 内建 specifier；`js.x.y` 是 bare specifier `x/y` 的语法糖。

- Python 导出优先使用 `__all__`，否则考虑所有非 `_` 开头的顶层名称。callable 变成同步 JS 函数，JSON 值成为导入时快照。`__all__` 中的不支持值导致导入失败；自动发现时不支持的公开对象不导出。

- 值域限定为 `None/null`、boolean、有限安全整数/浮点数、UTF-8 字符串、list/array、字符串键 dict/object。超出 JS safe integer、NaN/Infinity、bytes、BigInt、undefined、循环结构和自定义对象均抛出带值路径的类型错误。

## Implementation changes

- 在 `poly_python` 中把当前每次 `runpy.run_path` 重做为持久模块注册表：按规范化绝对路径加载一次、写入 `sys.modules`、保留模块状态，并提供版本化的 `load/describe/call/run_file` 请求协议。
- 嵌入原生 `js` package、meta-path finder 和 `import_module()`。Python→JS 通过一次调用期间安装的线程局部 host callback 回到当前 JSC global；没有 JSC 上下文时明确报错。
- 在 Bun ESM fetch/transpile 路径中先于未知扩展处理 `.py`：加载并描述 Python 模块，生成只包含静态 named exports、JSON 常量及 callable wrapper 的 ESM source。Python 顶层代码只执行一次，stdout 在模块初始化时转发一次，异常以完整 traceback 拒绝导入。
- 新建 VM 所有的 `PolyInteropState`，用 JSC `Strong` 根保存 Python 已加载的 JS module namespace；销毁跟随 VM 且发生在 JS 线程。JS 调用通过现有 `JSValue::call`，参数和结果在边界处 JSON 转换。
- Python→JS 模块加载走 Bun 现有同步 resolver/require 路径，并以发起导入的 Python 文件作为 referrer；沿用 Bun 当前 package、alias、builtin 和 auto-install 配置。包含 top-level await 或返回 Promise 的模块/函数在同步 v1 中明确拒绝。
- 用状态机限制嵌套调用：允许 `JS → Python → JS`；当 JS callback 再次进入 Python 时抛出 `ERR_POLY_REENTRANT_PYTHON_CALL`，防止循环依赖破坏 RustPython/JSC 状态。
- 移除 `bun_bin` 在 JSC 启动前直接运行 `.py` 的早期分派。在 RunCommand 中把 Python 入口改写为内部虚拟 bootstrap 模块，在完整 Bun 生命周期和 API lock 内调用 `run_file`，同时保持 `sys.argv`、退出码和脚本参数兼容。
- 错误映射固定为：Python import/call 错误在 JS 侧包含稳定错误码和 traceback；JS 异常在 Python 侧成为 `js.JavaScriptError`，保留 JS name、message、stack；Python coroutine、JS Promise 和跨运行时循环重入分别使用独立错误码。
- 更新 README、技术设计、路线图和验证记录，明确同步 v1、缓存语义、值域限制及尚未实现的能力。

## Test plan

- `poly_python` 单元测试覆盖：模块只执行一次、状态保留、`__all__`、公开项回退、参数/返回值校验、异常 traceback、fake JS host、无 host 错误和重入保护。
- Poly 运行时集成测试覆盖：
  - `.py` 静态导入及可选 import attribute；
  - callable、常量、默认/缺失 export、动态 `import()`；
  - 两次导入共享 Python 模块状态；
  - Python 异常映射到 JS；
  - `poly app.py` 导入相对 TS 文件；
  - `js.import_module()` 加载 bare package、Bun builtin 和缓存模块；
  - JS 异常映射到 Python；
  - top-level await、Promise、非 JSON 值及跨运行时循环重入被拒绝；
  - Windows 路径、反斜杠、Unicode 路径和 Linux 路径。
- 先运行 `cargo fmt --check`、`cargo clippy -p poly_python --all-targets -- -D warnings`、`cargo test -p poly_python`，再用 Poly debug build 执行定向 Bun 测试，最后用 Release `poly.exe/poly` 做完整 smoke。
- 扩展 Bun integration workflow，在 Windows x64 和 Linux x64 完整构建后执行双向 smoke；macOS 继续运行隔离 bridge 测试，并运行跨目标 Rust check。
- 保留并扩展无 subprocess contract，确认新 finder、bootstrap 和 callback 不引入进程、socket、channel 或 worker。

## Assumptions and exclusions

- 本期不支持 Promise/coroutine、异步调度、取消/超时、Python→JS→Python 循环、跨运行时对象身份、GC handle、class instance、bytes/BigInt 或热重载。
- `require("./x.py")` 不纳入 v1；`.py` 使用 ESM `import`/`import()`。
- `Bun.build`、`poly build` 和把 Python 模块收入 standalone archive 属于后续 bundle 阶段。
- Python 模块缓存持续到进程结束；v1 不提供 reload API。
- 实施前从干净 `main` 创建符合仓库规则的 `claude/poly-language-interop` 分支，并保留所有非 Poly 的 Bun loader 行为不变。
