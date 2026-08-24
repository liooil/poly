# Poly

**English** | [简体中文](README.zh-CN.md)

Run TypeScript and Python in one executable, and import between them.

[![CI](https://github.com/liooil/poly/actions/workflows/ci.yml/badge.svg)](https://github.com/liooil/poly/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

## Start with one file

No project setup is required.

```ts
// hello.ts
console.log("hello from TypeScript");
```

```bash
poly hello.ts
```

Python works the same way:

```python
# hello.py
print("hello from Python")
```

```bash
poly hello.py
```

`package.json`, `pyproject.toml`, `poly.toml`, and lockfiles are optional. Add
them only when the project needs dependencies, locking, or Poly-specific
settings.

## Import Python from TypeScript

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

The `.py` module is loaded in-process. Calls are synchronous and return JSON-
compatible values or synchronous callable proxies.

For an explicit bridge call:

```ts
import { python } from "poly";

console.log(python.call("./math_tools.py", "add", [20, 22]));
```

## Import TypeScript from Python

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

Python's standard `import` is preferred. Normal Python modules win; when Python
cannot resolve a module, Poly falls back to Bun's resolver. `js.import_module()`
is the fallback for relative, absolute, or runtime-computed specifiers:

```python
import js

numbers = js.import_module("./app_numbers.ts")
print(numbers.double(21))  # 42
```

## Grow into a project

Start with source files, then add native metadata as needed:

```text
my-app/
├── main.ts
├── math_tools.py
├── package.json       # when JavaScript dependencies are needed
├── pyproject.toml     # when Python dependencies are needed
├── bun.lock           # optional Bun lockfile
├── uv.lock            # optional Python lockfile
└── poly.toml          # optional Poly settings
```

Python dependencies use the in-process uv integration:

```bash
poly sync
poly sync --check
poly sync --frozen
```

Without `pyproject.toml`, `poly sync` reports that there is no Python project
and changes nothing. `poly init` is not required.

## REPL

```bash
poly repl
poly repl python
poly repl shell
poly repl sql
poly repl c
```

The REPL keeps shared history and provides language-specific syntax highlighting.

## Current boundary

Implemented:

- `.ts` / `.py` entry points in one Poly executable;
- synchronous TS↔Python imports and callable proxies;
- JSON-compatible bridge values;
- normalized-path caching in the Poly bridge registry;
- in-process uv resolution, wheel staging, compatibility scanning, and optional
  RustPython smoke imports;
- zero-config single-file execution.

Not yet implemented or fully verified:

- reload API;
- one shared module identity between every bridge path and Python `sys.modules`;
- Promise/coroutine scheduling, cancellation, and timeout;
- bytes, BigInt, non-finite numbers, and general cross-runtime object identity;
- `poly build` standalone applications.

## Build and test

```bash
cargo test --locked -p poly_python
cargo check --locked -p bun_runtime --tests
bun bd test test/cli/bun.test.ts
```

The repository contains the Bun fork and Poly integration. See:

- [`poly/docs/language-interop-v1.md`](poly/docs/language-interop-v1.md)
- [`poly/docs/technical-design.md`](poly/docs/technical-design.md)
- [`poly/docs/roadmap.md`](poly/docs/roadmap.md)
- [`poly/docs/validation.md`](poly/docs/validation.md)

Poly is derived from [Bun](https://bun.sh) and uses Bun's MIT license.
