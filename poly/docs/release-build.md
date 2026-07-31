# 可复现构建与二进制发布方案

## 目标

项目采用公开源码、公开构建流程和 GitHub 托管分发：

```text
Git tag
  -> GitHub Actions 多平台构建
  -> 测试、SBOM、校验和与构建来源证明
  -> GitHub Release Assets
  -> GitHub Pages 安装脚本下载并校验
```

官网不保存二进制。GitHub Pages 只提供安装说明和安装脚本，二进制由
GitHub Releases 托管。

GitHub Actions Artifacts 仅用于工作流步骤之间传递中间产物和诊断。
它们有保留期限，不作为用户安装地址。所有对用户发布的稳定文件必须上传
为 GitHub Release Assets。

## 版本与输入固定

每次发布必须由形如 `v0.1.0` 的 Git tag 触发，并固定以下输入：

- 本项目 Git commit；
- 当前 Git 历史中最近一次同步的 Bun upstream commit；
- Bun 所固定的 WebKit/JavaScriptCore 版本；
- `Cargo.lock`、`bun.lock` 和将来的 `uv.lock`；
- Rust 工具链版本和各平台构建工具版本；
- 第三方 GitHub Actions 的完整 commit SHA，不使用可移动的
  `@main` 或仅 `@v4` 引用。

构建不得从未记录版本的工具或 URL 获取可变内容。确实无法消除的差异
必须写入 `BUILDINFO.json`。

## 工作流

### 1. 验证

在所有平台构建前执行：

- Rust 格式与静态检查；
- `poly-python` 单元测试和 interop 测试；
- RustPython crate 与 Bun/JSC host bridge 的直接源码集成检查；
- lockfile 未被构建过程修改的检查；
- 第三方许可证和 SBOM 生成检查。

### 2. 多平台构建

第一版发布门禁：

| 平台 | 目标产物 |
|---|---|
| Windows x64 | `poly-vX.Y.Z-windows-x64.zip` |
| Linux x64 | `poly-vX.Y.Z-linux-x64.tar.gz` |

两个平台都从同一 tag 和固定输入构建，并在各自 runner 上完成 Python
入口及 TS→Python smoke test。任一平台未通过时不得创建 Release。Linux
arm64 与 macOS 留到后续版本，不能上传未经验证的占位产物。

### 3. 发布包内容

每个压缩包至少包含：

```text
poly / poly.exe
LICENSE
THIRD_PARTY_NOTICES.md
licenses/
RELINKING.md
BUILDINFO.json
SBOM.spdx.json
```

Release 还必须包含：

```text
SHA256SUMS
SHA256SUMS.sig                 # 启用发布签名后
poly-source-vX.Y.Z.tar.zst     # 完整对应源码或可重建源码包
poly-relink-vX.Y.Z.tar.zst     # LGPL 重新链接所需材料（如单独提供）
```

不能只依赖 GitHub 自动生成的 Source ZIP：它可能不包含子模块、外部
依赖源码。源码包或重建脚本必须包含当前 fork 中精确的 Bun 源码、
WebKit 来源信息、RustPython 版本和本项目的全部修改。

### 4. 构建元数据

`BUILDINFO.json` 至少记录：

```json
{
  "version": "0.1.0",
  "source_commit": "<full-sha>",
  "bun_upstream": "<full-sha>",
  "webkit_revision": "<exact-revision>",
  "rust_toolchain": "<exact-version>",
  "target": "<target-triple>",
  "workflow": "<workflow-file-and-run-url>",
  "source_date_epoch": "<tag-commit-time>"
}
```

工作流生成 SPDX SBOM、SHA-256 校验和以及 GitHub Artifact
Attestation。构建日志可以作为诊断材料保留，但不能代替
`BUILDINFO.json`、源码包或许可证材料。

## GitHub Release 发布权限

Release 工作流采用最小权限：

```yaml
permissions:
  contents: write
  id-token: write
  attestations: write
```

其中 `contents: write` 用于创建 Release 和上传资产，另外两项用于生成
构建来源证明。普通 pull request 构建保持只读权限，并且不能发布资产。

推荐分成两个阶段：

1. 各平台构建任务产生中间 Artifact；
2. 单独的 release 任务只在受保护 tag 上下载这些产物、校验、生成最终
   清单并上传 GitHub Release。

## 官网安装脚本

GitHub Pages 上的安装脚本只访问 GitHub Release Assets，例如：

```text
https://github.com/<owner>/<repo>/releases/latest/download/poly-windows-x64.zip
```

安装过程必须：

1. 根据 OS 和 CPU 选择明确的资产名；
2. 同时下载 `SHA256SUMS`；
3. 在执行任何文件前验证 SHA-256；
4. 启用签名后继续验证签名或 provenance；
5. 允许用户指定版本，避免只能安装 `latest`；
6. 出错时停止，不静默回退到未经验证的镜像或旧版本。

安装脚本本身也保存在本仓库中，由 GitHub Pages 从固定分支部署。官网
页面应链接到对应 Release、源代码、许可证和验证说明。

## LGPL 重新链接路径

由于 Bun 静态链接 JavaScriptCore/WebKit 等 LGPL 组件，构建透明并不
自动完成所有发行义务。每次 Release 还必须提供一条实际可执行的路径：

1. 获取本项目 tag；该 tag 已包含 Bun 源码和 Poly 修改；
2. 按该源码树记录的 revision 获取 WebKit 等 source-backed 依赖；
3. 修改 LGPL 组件；
4. 使用 `poly/scripts/build.ps1` 重新构建并产生替换后的 `poly` 二进制。

在首次公开发布前，需要用一台干净的受支持构建机实际走通
`RELINKING.md`。如果仅有完整源码仍不足以让用户替换 LGPL 组件，则发布
包还需要提供对应的对象文件或其他重新链接材料。

## 发布门禁

只有同时满足以下条件才允许把 Release 标记为稳定版：

- 所有声明支持的平台完成运行测试；
- Release 资产的校验和与实际文件一致；
- SBOM 和第三方许可证检查通过；
- 完整源码和重新链接步骤可用；
- provenance 可以验证到本仓库、tag 和工作流；
- 从 GitHub Pages 安装脚本完成一次干净环境安装和启动验证。

本方案是工程与开源许可合规基线，不构成法律意见。首次发行和依赖许可
发生重大变化时应进行专业法律审阅。
