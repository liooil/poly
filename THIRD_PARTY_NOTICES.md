# Third-party notices

Polyglot Runtime 自身的代码依据仓库根目录的 [MIT License](LICENSE)
授权。最终发布的 `poly` 二进制还会包含第三方组件；这些组件继续适用各自
的许可证，本项目的 MIT License 不会替代或缩减这些条款。

## 直接集成的上游

| 组件 | 当前版本或来源 | 许可证 | 说明 |
|---|---|---|---|
| Bun | `upstream.toml` 固定的提交 | MIT | 本项目在 Bun 源码树内加入同进程 RustPython 集成补丁 |
| JavaScriptCore / WebKit | Bun 固定并打补丁的版本 | LGPL-2.0 | Bun 静态链接该组件；发布包必须提供许可文本以及修改、重新构建和重新链接所需的信息或材料 |
| TinyCC | Bun 构建固定的版本 | LGPL-2.1 | Bun 静态链接的组件之一 |
| RustPython | 0.5.0 | MIT | 作为 Rust crate 链接，并冻结 Python 标准库 |
| uv | 0.12.3 (`5072309`) | MIT OR Apache-2.0 | 固定 revision 的 Rust crates，同进程用于 Python 项目发现、解析、锁定与依赖物化；不调用 uv 子进程 |

Bun 还静态链接或嵌入 BoringSSL、libarchive、Brotli、zstd、ICU、
libuv、uSockets 和其他组件。完整、权威的清单必须以
`upstream.toml` 指向的 Bun 源码、其 `LICENSE.md`、构建锁文件及最终
SBOM 为准。

## 发布要求

正式二进制发布前，发布流程必须：

1. 从固定的 Bun 和 Rust 依赖版本生成完整 SBOM；
2. 把所有要求随二进制提供的版权声明和许可证文本放入发布包；
3. 记录 Bun、WebKit、RustPython、本项目及构建工具链的精确版本；
4. 提供 `RELINKING.md`，说明如何取得对应源码、应用本项目补丁并重新
   构建包含修改版 LGPL 组件的程序；
5. 在 GitHub Release 中长期提供该版本对应的源码和重新链接材料。

本文件是项目初期的许可清单，不应被当作最终发行包的完整第三方许可证
报告。新增或升级依赖时必须同步更新此文件和自动生成的 SBOM。
