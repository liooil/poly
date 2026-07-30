# 验证记录

验证日期：2026-07-30

## 已通过

- `cargo fmt --all -- --check`
- `cargo test -p poly_python`
  - 在一个调用者线程中初始化 RustPython；
  - 调用 `examples/math_tools.py::add(20, 22)`；
  - 返回 JSON 值 `42`；
  - 捕获 Python stdout。
- 把 `poly_python` 放入固定 Bun workspace 后执行 `cargo check -p poly_python`
- Bun 补丁正向/反向 apply 检查
- 使用发行版 Bun 对 `examples/main.ts` 与 TypeScript SDK 做 bundle 检查
- 搜索确认运行时源码不含 `Command::new`、`Bun.spawn`、`child_process` 或 `mpsc`

## 尚未通过

最终修改版 Bun 可执行文件尚未在本机完成编译，因此下面两项仍待端到端验证：

- `poly examples/python_main.py`
- `poly examples/main.ts`，包含 JSC host function 到 RustPython 的实际链接调用

当前阻点不是项目源码测试，而是本机缺少 Bun Windows 源码构建所需的 Visual Studio Desktop C++ 环境及对应 LLVM 工具链。`cargo check -p bun_runtime -p bun_bin` 已进入 Bun crate graph，但在 Bun 生成文件缺失处停止；这些文件需要先运行官方 `bun bd --configure-only`，该步骤又依赖完整的 Windows 原生构建环境。

完成环境安装后，执行：

```powershell
.\scripts\bootstrap-bun.ps1
.\dist\poly.exe examples\python_main.py
.\dist\poly.exe examples\main.ts
```
