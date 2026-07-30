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
