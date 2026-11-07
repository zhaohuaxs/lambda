# 第三方组件与许可

仓库内下列组件保留各自原有的许可，随仓库分发时按其条款执行。

## 7-Zip 26.04（`sevenzip-src/vendor/7zsrc/`）

| 部分 | 许可 |
|---|---|
| 绝大多数 C/C++ 源文件 | LGPL-2.1-or-later |
| `CPP/7zip/Compress/Rar*` | LGPL-2.1-or-later **+ unRAR 限制** |
| `CPP/7zip/Compress/LzfseDecoder.cpp` | BSD-3-Clause（Apple LZFSE） |
| 文件内声明 public domain 的少数文件 | Public Domain |

- 许可原文随源码保留：`sevenzip-src/vendor/7zsrc/DOC/License.txt`、`sevenzip-src/vendor/7zsrc/DOC/unRarLicense.txt`
- **unRAR 限制**：unRAR 源码不得用于重建 RAR 压缩算法；分发修改版时须声明该代码不可用于开发 RAR（WinRAR）兼容压缩器。Lambda 只解压 RAR，不产生 RAR 压缩包
- 两端均为**静态链接**（Windows 编入 `lambda.exe`，Android 编入 `libbern_app.so`），下游需满足 LGPL 的「可修改、可重新链接」义务：完整源码与构建脚本已随本仓库提供

## Android NDK C++ 运行时

随包分发的 `libc++_shared.so` 来自 Android NDK（LLVM），按 Apache-2.0 WITH LLVM-exception 分发，需保留其许可声明。

## Rust 依赖

`Cargo.lock` 中的 crate 按各自许可分发。直接依赖及其许可：

| crate | 许可 |
|---|---|
| `bern` 及其同族 crate（[AimesSoft/Bern](https://github.com/AimesSoft/Bern)） | MIT |
| `chardet-rs` | MIT |
| `chardetng` | Apache-2.0 OR MIT |
| `encoding_rs` | Apache-2.0 OR MIT |
| `serde_json` | Apache-2.0 OR MIT |
| `zeroize` | Apache-2.0 OR MIT |
| `libc`（Android） | Apache-2.0 OR MIT |
| `cc`（构建依赖） | MIT OR Apache-2.0 |

其余传递依赖同样按 `Cargo.lock` 中各自的 `license` 字段分发；发行时须随包保留本文件与各许可原文。`tools/make_sbom.py` 可生成完整的包-许可清单。Windows 安装包与 Android APK 的 `assets/notices/` 由 `sevenzip-src/build.rs` 在构建时复制。