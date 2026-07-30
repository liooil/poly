# Poly

**English** | [简体中文](README.zh-CN.md)

**One runtime for TypeScript and Python.**

Run either language. Import across both ecosystems. Ship one executable.

[![CI](https://github.com/liooil/poly/actions/workflows/ci.yml/badge.svg)](https://github.com/liooil/poly/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

Poly is an experimental runtime and toolchain for applications that use both
TypeScript and Python. Bun runs JavaScript and TypeScript, RustPython runs
Python, and a Rust host brings them together in one executable and one OS
process.

The goal is not to hide one language behind an RPC service. Both languages
should be first-class parts of the same project, module graph, development
workflow, and release artifact.

> [!IMPORTANT]
> Poly is still an engineering prototype. The shared runtime and low-level
> in-process bridge exist today; cross-language imports, unified dependency
> management, and application bundling are roadmap features.

## The target experience

Python files should behave like modules, not services:

```ts
import { add } from "./math_tools.py" with { type: "python" };

console.log(add(20, 22)); // 42
```

Poly will also provide a built-in runtime module for cases that need explicit
access to Python:

```ts
import { python } from "poly";
```

The common project lifecycle should use one CLI:

```text
poly run app.ts        # run a TypeScript entry point
poly run tool.py       # run a Python entry point
poly sync              # resolve both ecosystems
poly build app.ts      # produce a standalone application
```

`poly run` entry-point routing is part of the current prototype. Static
cross-language imports, `poly sync`, and `poly build` are not implemented yet.

## One project, two ecosystems

A Poly project is designed to keep the native tools and metadata of both
ecosystems:

```text
my-app/
├── package.json       # JavaScript and TypeScript dependencies
├── bun.lock
├── pyproject.toml     # Python dependencies
├── uv.lock
├── poly.toml          # entries, runtimes, and interop settings
└── src/
    ├── main.ts
    └── model.py
```

| Layer | Direction |
|---|---|
| Runtime | Bun and embedded RustPython in one executable |
| Modules | Direct imports across TypeScript and Python |
| Dependencies | Bun for npm packages, uv for Python resolution and locking |
| Build | Collect both module graphs, dependencies, and resources |
| Distribution | One platform-native executable with no Python sidecar |

Poly owns the compatibility boundary. In particular, uv is used for dependency
resolution and materialization, but RustPython is not an officially supported uv
interpreter and cannot load arbitrary CPython native wheels. Poly therefore
needs its own compatibility checks and reports.

## How it fits together

The current M0 prototype integrates RustPython directly into a pinned Bun source
tree:

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

The current TypeScript-to-Python call path stays on the Bun/JSC caller thread.
It does not start a subprocess, sidecar, worker thread, socket, or stdio RPC
channel. Python execution is synchronous and blocks the Bun event loop.

This JSON bridge is a bootstrap layer. It proves that both runtimes can coexist
inside the same process while keeping their garbage-collected objects separate.
The roadmap replaces this low-level interface with imports, callable proxies,
tagged values, and explicit lifetime semantics.

## Project status

| Capability | Status |
|---|---|
| Bun and RustPython source integration | Implemented |
| `.js` / `.ts` and `.py` entry-point routing | Implemented in the downstream patch; full-build validation in progress |
| TypeScript → Python JSON bridge | Rust bridge tests pass; downstream JSC smoke test in progress |
| Python module cache and reload semantics | Planned |
| Static cross-language imports and callable proxies | Planned |
| Cooperative async execution | Planned |
| `poly sync` with uv compatibility checks | Planned |
| `poly build` standalone applications | Planned |

See the [validation record](docs/validation.md) for the exact verified boundary
and the [roadmap](docs/roadmap.md) for planned work.

## Try the current prototype

### Validate the RustPython bridge

This is the fastest check and does not compile Bun:

```powershell
cargo test -p poly_python
```

It initializes RustPython on one caller thread, invokes
`examples/math_tools.py::add(20, 22)`, verifies the return value, and checks
stdout capture. It does not produce the final `poly` executable.

### Build the complete runtime

A full build fetches the pinned Bun source, applies the Poly integration patch,
and compiles the downstream executable. Expect roughly 10 GB of disk usage and
a 10–30 minute build.

Prerequisites include Git, PowerShell 7, Bun 1.3.2, Rust, and Bun's native build
toolchain. See the
[Bun integration workflow](.github/workflows/bun-integration.yml) for the
complete Windows and Linux environments.

Poly pins Bun to commit
[`e7ddfeb19e8bc714f6137aa2b1cd5a7bb56b93d7`](https://github.com/oven-sh/bun/commit/e7ddfeb19e8bc714f6137aa2b1cd5a7bb56b93d7)
because `bun_runtime` and `bun_bin` are internal implementations rather than
stable embedding APIs.

#### Windows

```powershell
.\scripts\bootstrap-bun.ps1 -Configuration Release
.\dist\poly.exe examples\python_main.py -- first second
.\dist\poly.exe examples\main.ts
```

#### Linux

```bash
pwsh ./scripts/bootstrap-bun.ps1 -Configuration Release
./dist/poly examples/python_main.py -- first second
./dist/poly examples/main.ts
```

Omitting `-Configuration Release` produces a Debug build.

The TypeScript example currently uses the transitional low-level SDK in
[`examples/main.ts`](examples/main.ts). It is an integration fixture, not the
intended final developer experience.

## Repository layout

```text
crates/poly-python/            RustPython embedding and call bridge
patches/bun-in-process.patch   Integration patch for the pinned Bun commit
scripts/bootstrap-bun.ps1      Fetch, patch, and build the runtime
sdk/typescript/poly.ts         Transitional low-level TypeScript SDK
examples/                      TypeScript and Python integration examples
docs/                          Design, roadmap, and validation records
poly.toml                      Future Poly project manifest
upstream.toml                  Pinned Bun upstream source
```

## Documentation

- [Technical design](docs/technical-design.md)
- [Implementation roadmap](docs/roadmap.md)
- [Validation record](docs/validation.md)
- [Reproducible build and binary release plan](docs/release-build.md)

The linked design documents are currently written in Chinese.

## Website

The project website is published at
[liooil.github.io/poly](https://liooil.github.io/poly/). Its source lives in
[`website/`](website/), and changes on `main` are deployed through the
[GitHub Pages workflow](.github/workflows/pages.yml).

## Continuous integration and releases

- `CI` runs the RustPython bridge tests on Linux, macOS, and Windows. It also
  checks formatting, Clippy, the TypeScript fixture, the no-subprocess boundary,
  and patch applicability.
- `Bun integration build` builds Windows x64 and Linux x64 executables for
  relevant pull requests or manual runs, then exercises both language entry
  points and the TypeScript-to-Python path.
- `Release` accepts only `v*` tags and publishes assets only after both platform
  builds and smoke tests succeed.

## License

Code original to this project is available under the [MIT License](LICENSE).
The final `poly` executable also contains Bun, JavaScriptCore/WebKit,
RustPython, and other third-party components under their respective licenses.
See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for the preliminary
inventory and release requirements.

Tagged releases are intended to be built by GitHub Actions and distributed as
GitHub Release assets with build metadata and checksums.

## Upstream references

- [Bun main Rust workspace](https://github.com/oven-sh/bun/blob/main/Cargo.toml)
- [Building Bun from source](https://github.com/oven-sh/bun/blob/main/CONTRIBUTING.md)
- [RustPython 0.5.0 embedding API](https://docs.rs/rustpython/0.5.0/rustpython/)
- [Python implementations supported by uv](https://docs.astral.sh/uv/reference/policies/python/)
