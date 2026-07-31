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

## M4：语言级 interop

- [ ] TS `import x from "./x.py"` loader
- [ ] Python `import js.x` finder/loader
- [ ] callable proxy
- [ ] Promise / coroutine 映射
- [ ] Python→JS 安全重入
- [ ] GC、弱引用与释放协议

## M5：降低 fork 成本

- [ ] Bun 上游变更监测
- [ ] Bun callback integration tests
- [ ] 评估上游 embedding API
- [ ] 后端 conformance suite
- [ ] capability 权限模型

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
