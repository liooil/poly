# Poly

[English](README.md) | **简体中文**

在同一个可执行文件中运行 TypeScript 和 Python，并让两个生态直接互相导入。

[![CI](https://github.com/liooil/poly/actions/workflows/ci.yml/badge.svg)](https://github.com/liooil/poly/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

## 从一个文件开始

不需要项目初始化。

```ts
// hello.ts
console.log("hello from TypeScript");
```

```bash
poly hello.ts
```

Python 也一样：

```python
# hello.py
print("hello from Python")
```

```bash
poly hello.py
```

`package.json`、`pyproject.toml`、`poly.toml` 和锁文件都是可选的。只有在需要
依赖、锁定或 Poly 专属配置时再添加它们。

## 在 TypeScript 中导入 Python

```python
# math_tools.py

def add(a, b):
    return a + b
```

```ts
// main.ts
import { add } from "./math_tools.py";

console.log(add(20, 22)); // 42
```

`.py` 模块在进程内加载。调用是同步的，值使用 JSON 兼容类型，也可以导出同步
callable proxy。

需要显式使用底层 bridge 时：

```ts
import { python } from "poly";

console.log(python.call("./math_tools.py", "add", [20, 22]));
```

## 在 Python 中导入 TypeScript

```ts
// app_numbers.ts
export function double(value: number) {
  return value * 2;
}
```

```python
# main.py
from app_numbers import double

print(double(21))  # 42
```

优先使用 Python 标准 `import`。Python 自身的 builtin、标准库、本地模块和已安装包
优先；Python 无法解析时，Poly 才交给 Bun resolver。`js.import_module()` 用于
相对路径、绝对路径或运行时才知道的 specifier：

```python
import js

numbers = js.import_module("./app_numbers.ts")
print(numbers.double(21))  # 42
```

## 逐步发展为项目

先添加源码文件，需要依赖时再添加各自的原生元数据：

```text
my-app/
├── main.ts
├── math_tools.py
├── package.json       # 需要 JavaScript 依赖时添加
├── pyproject.toml     # 需要 Python 依赖时添加
├── bun.lock           # 可选的 Bun 锁文件
├── uv.lock            # 可选的 Python 锁文件
└── poly.toml          # 可选的 Poly 配置
```

Python 依赖使用进程内 uv 集成：

```bash
poly sync
poly sync --check
poly sync --frozen
```

没有 `pyproject.toml` 时，`poly sync` 会提示没有 Python 项目且不修改任何文件。
不需要先执行 `poly init`。

## REPL

```bash
poly repl
poly repl python
poly repl shell
poly repl sql
poly repl c
```

REPL 共享 history，并提供各语言专属语法高亮。

## 当前边界

已实现：

- 一个 Poly 可执行文件中的 `.ts` / `.py` 入口；
- 同步的 TS↔Python import 和 callable proxy；
- JSON 兼容 bridge 值；
- Poly bridge registry 的规范化路径缓存；
- 进程内 uv 解析、wheel staging、兼容性扫描和可选 RustPython smoke import；
- 无配置单文件运行。

尚未实现或尚未完整验证：

- reload API；
- 所有 bridge 路径与 Python `sys.modules` 共享同一个 module identity；
- Promise/coroutine 调度、取消和超时；
- bytes、BigInt、非有限数字和通用跨运行时对象 identity；
- `poly build` 独立应用。

## 构建与测试

```bash
cargo test --locked -p poly_python
cargo check --locked -p bun_runtime --tests
bun bd test test/cli/bun.test.ts
```

仓库包含 Bun fork 和 Poly 集成。更多信息：

- [`poly/docs/language-interop-v1.md`](poly/docs/language-interop-v1.md)
- [`poly/docs/technical-design.md`](poly/docs/technical-design.md)
- [`poly/docs/roadmap.md`](poly/docs/roadmap.md)
- [`poly/docs/validation.md`](poly/docs/validation.md)

Poly 源自 [Bun](https://bun.sh)，采用 Bun 的 MIT 许可证。
