# LGPL 重链接说明

Lambda 把 7-Zip 26.04 的 C/C++ 源码静态链进两个产物：

- Windows：`lambda.exe`（`sevenzip-src/vendor/7zsrc` → `sevenzip-src/build.rs` → `lambda7z-msvc.rsp`）
- Android：`libbern_app.so`（同一份源码 → `ld.lld -r` 合并对象 → `liblambda7z.a` → Bern glue）

7-Zip 主体是 LGPL-2.1-or-later。LGPL 允许静态链接，但要求下游能修改 7-Zip 部分并重新链接出新的可执行文件。本仓库提供完整源码与构建脚本；下面是最小重链接步骤。

## 共同前提

- Rust 1.99.0（`rust-toolchain.toml`）
- Bern `=0.1.10`（`Cargo.toml`）
- Windows：Visual Studio 2022 的 MSVC 工具链
- Android：NDK 28.2、Gradle 9.5.1、`CC_aarch64_linux_android` 指向 NDK clang

## Windows

1. 修改 `sevenzip-src/vendor/7zsrc` 下的源码，或替换为同版本的修改副本。
2. `cargo build --release --target x86_64-pc-windows-msvc -p lambda-core`（或 Bern 的 glue 构建）生成 Rust 引擎与 `lambda7z-msvc.rsp`。rsp 的实际位置是构建时 `CARGO_TARGET_DIR` 的 profile 目录：直接在工作区根跑 cargo 时是 `target/release/lambda7z-msvc.rsp`，Bern glue 构建时是 `target/bern-glue-windows/release/lambda7z-msvc.rsp`（`build-sequential.cmd` 用的是后者）。
3. `build-sequential.cmd release`：脚本会把 `@lambda7z-msvc.rsp` 与 `/guard:cf` 注入 Bern 生成的 `build.bat`，然后编译链接出 `target/bern/windows/app/.../Lambda.exe`。
4. 重链接只需要：修改后的 7-Zip 源码、本仓库的 `sevenzip-src/build.rs` 与 `bridge/lambda7z.cpp`、以及 `lambda7z-msvc.rsp` 中列出的目标文件集合。rsp 是构建产物，若需要单独交付，可在上述构建目录中找到。

## Android

1. 修改 `sevenzip-src/vendor/7zsrc`。
2. 构建 Bern Android glue（`bern build --target android`，或仓库中记录的 Gradle 流程）。`sevenzip-src/build.rs` 会用 NDK clang 编译 324 个单元，`ld.lld -r` 合并成 `lambda7z_all.o`，再打成 `liblambda7z.a`，并链接进唯一的 `libbern_app.so`。
3. 重链接需要：修改后的源码、`vendor/sources-full.txt`、`bridge/lambda7z.cpp`、`sevenzip-src/build.rs`、Bern 生成的 `target/bern/android/project` 工程与 NDK 28.2。`libc++_shared.so` 从 NDK 复制，必须随包。

## 说明

- `Cargo.toml` 的 `license = "MIT"` 只覆盖 Lambda 自己的 Rust 代码；整体产物同时受 7-Zip 的 LGPL-2.1-or-later、unRAR 限制、NDK 的 Apache-2.0 WITH LLVM-exception 约束。见 `docs/THIRD-PARTY-NOTICES.md`。
- 本文不是法律意见；商用发行前按产品计划的要求做法务复核。
