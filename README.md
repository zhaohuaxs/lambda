# Lambda · Bern + 7-Zip 双端压缩包管理器

基于 [Bern](https://github.com/AimesSoft/Bern) 的 Windows / Android 压缩包管理器：一份 UI / 协议源码同时生成 WinUI 3 与 Compose。两端都把 **7-Zip 26.04 官方源码**编进各自的应用（Windows 链进 `lambda.exe`，Android 链进 `libbern_app.so`），进程内 argv 调用，**不依赖外部 `7z.exe` / `7z.dll`**。

> Bern 0.1.10 · Windows 11 · Android arm64-v8a（NDK 28.2 + Gradle 9.5.1）· 双端真机端到端验证通过（2026-11-07）

## 最终结论

**两端都能出包，且共用一份 UI / 协议源码；Android 端已在真机跑通完整闭环。** 界面是完整的压缩包管理器：目录条（SAF 授权树）+ 面包屑 + 条目表（多选 / 排序）+ 压缩设置（格式 / 级别 / 密码）+ 后台任务进度与取消；归档浏览（进归档看内容、按需提取）、文本预览与图片预览均已接上。引擎层（`sevenzip-src`）两端共用同一套源码引擎：解压、压缩、列举、预览都已就绪。

```console
$ lambda extract -p password123 archive.7z
[lambda] 正在解除拉姆达的封印...
[lambda] 成功释放 1024 个文件。
[lambda] 输出目录：archive
[lambda] 写入 19373 字节 · 引擎 7-Zip 26.04 (7zz)
```
（上面是 Windows 独立 CLI 的验收入口；文件数、字节数随归档而定。）

## 用户操作

| 动作 | 表现 |
|---|---|
| `＋ 目录` | 选一个文件夹授权（Win `FolderPicker` / Android `ACTION_OPEN_DOCUMENT_TREE`），存进 `locations.json`，下次启动自动进第一个目录 |
| 目录条 | 一条目录一个 chip；授权失效的变成 `↻ 名字`，点一下原地重新授权 |
| 面包屑 | 从目录根到当前目录逐级可点；`↑ 上级` 在根处置灰 |
| 条目表 | 三列：`选`（`○`/`✓` 图标按钮，整格可点）/ `名称`（点表头按名称排序，副标题是「文件夹 · 3 天前」）/ `大小`（文件夹与归档这一格是强调色 `›` 箭头，点进目录或打开归档） |
| `全选` / `全不选` | 按当前页的选中情况自动切换文案；换目录自动归零 |
| `刷新` / `按时间` / `检查目录` | 重列当前目录 / 按修改时间排序 / 逐个验证目录授权是否还有效 |
| `7z` / `级别 · 标准` | 点一下换格式（7z ↔ zip）/ 循环三档压缩级别（快速 1、标准 5、极限 9） |
| `压缩 N 项` + 密码框 | 把选中的文件与文件夹压成一个归档，落到**目录根**（`export_path` 只能写树根）；密码留空就是不加密。任务在后台跑，期间其它按钮照常响应 |
| 进度区 | 只有任务在跑时出现：「压缩 3 项 · 45%」+ 进度条 + 「压缩 a.txt」+ `取消`。Android 的引擎阶段取消要等 7-Zip 返回，文案会提前说清 |
| 归档文本预览 | 归档表里 TXT / XML / JSON / log / CSV / md 等已知文本格式，大小列变成文档图标；点开弹出预览模态，显示编码与「第 N / M 页」，页脚上一页 / 下一页；大文件按 12,000 字符分页流式读取，不落临时文件 |
| 归档图片预览 | PNG / JPEG / GIF / WebP / BMP 等已知图片格式，大小列变成图片图标；点开弹出图片预览，宿主在后台读取 `files::serve` 提供的流并按显示尺寸采样解码；状态行显示格式、像素与大小，超 64 MB 或 2400 万像素拒绝预览；模态按内容定高，同一层有多张图时页脚出现「上一张 / 第 N / M 张 / 下一张」，单张不显示翻页 |
| 边界补测 | 不再出现在界面：CLI/MCP 调 `boundary()` 运行，报告写到应用私有目录（`m5-report.txt`） |
| 上次任务未完成 | 进程被系统回收后再启动，多一行「上次任务未完成：压缩 1 项（7z，停在「压缩」）· 刚刚」+ `重试` / `忽略` |
| 底部 | 「已选 N 项 · 1.7 MB」+「共 N 项 · 名称 ↑」+ 状态行 + 引擎名 |

WinUI 与 Compose 由同一份 `ui!` 生成，`pick: draft.tree` 在两端分别落到系统面板与 SAF，业务代码零分叉。

## 快速开始

```powershell
# 在仓库根目录（Cargo.toml 所在目录）执行
$env:RUSTUP_TOOLCHAIN = "stable"

# Windows 独立 CLI（不改 Bern 运行时）
cargo run -p lambda-cli -- extract -p password123 archive.7z -o out

# Bern 声明检查 + 单测
bern check
cargo test --workspace
```

Windows 桌面应用（WinUI 3）：

```powershell
# 在仓库根目录执行
bern build --target windows     # 生成 target/bern/windows/Lambda/Lambda.exe；C++ 阶段可能因 /MP 报 C3859
cmd /c build-sequential.cmd     # 串行重编 7-Zip 并注入 lambda7z-msvc.rsp；每次 bern build 后都要跑
```

Android 构建（按本机安装路径填写以下环境变量）：

```bash
export RUSTUP_TOOLCHAIN=stable
export PATH="/c/Windows/System32:$PATH"
export BERN_GRADLE="<gradle-9.5.1>/bin/gradle.bat"
export ANDROID_NDK_HOME="<android-sdk>/ndk/28.2.13676358"
export BERN_JAVA_HOME="<jdk-21>"
export ANDROID_HOME="<android-sdk>"
export JAVA_HOME="$BERN_JAVA_HOME"
# 在仓库根目录执行
bern build --target android      # target/bern/android/Lambda.apk
bern package --target android    # target/bern/dist/*.apk / *.aab（无 --release 参数）
```

发布包签名读取 `BERN_ANDROID_KEYSTORE` / `BERN_ANDROID_KEYSTORE_PASSWORD` / `BERN_ANDROID_KEY_ALIAS`（`BERN_ANDROID_KEY_PASSWORD` 可选，默认取 store 密码）；未配置时 Gradle 回退到 debug 签名，`package` 会提示产物是 debug key 签的。

真机安装：`adb -s <serial> install -r target/bern/dist/Lambda-0.1.0-android.apk`。同一台设备可能同时存在多条 adb transport（USB / 无线调试 / mDNS），**所有 adb 命令都要带 `-s`**；如果机器上装的是 debug 签名的旧包，换发布签名包前要先 `adb uninstall dev.bern.lambda`，否则 `install -r` 会报签名不一致。

## 工程结构

| 层 | 内容 |
|---|---|
| `sevenzip-src/` | **两端共用**：vendored `vendor/7zsrc/`（7-Zip 26.04 官方源码，1292 文件）、`vendor/sources.txt`（7zr，164 单元）/ `sources-full.txt`（7zz，324 单元；Windows 另加 5 个系统单元）、`build.rs`（Android `ld.lld -r` 合并；Windows 生成 MSVC 链接 rsp）、`bridge/lambda7z.cpp`（fd 重定向 + 取消 + 引擎入口）、`src/lib.rs`（argv 构造 + FFI + 旧编码检测 + `failure_message`） |
| `lambda-protocol/` | 位置 / 浏览 / 选择 / 压缩 / 解压 / 归档 / 密码簿 / 外观 / 强调色 / 文本预览的状态与命令；命令顺序就是生成代码的调用下标，**新命令只能追加末尾** |
| `lambda-core/` | `browser.rs`（列举 + 自然排序 + 格式化 + 归档识别）、`locations.rs`（JSON 位置表，`id` = URI 的 FNV-1a）、`backend.rs`（`MediaSource` → 本地文件 / `export_path` 导出 / 按平台选引擎）、`preview.rs`（文本流解码 + 页环）、`probe.rs`（M5 边界补测，写 `m5-report.txt`） |
| `lambda-ui/` | 一份 `ui!`：目录条 + 面包屑 + 三列 `Table` + 底部统计。同时生成 WinUI 3 与 Compose |
| `lambda-cli/` | 独立 `[[bin]] name = "lambda"`，只依赖 `sevenzip-src`，不拉 Bern 运行时 |
| `Bern.toml` | `[targets.windows]` + `[targets.android]`（`abis = ["arm64-v8a"]`） |

### 引擎是怎么接进应用的

Android 侧的约束最硬：Bern 0.1.10 的后端把应用 crate 链成唯一的 `libbern_app.so`（`crates/bern-backend-compose/src/glue.rs`），并且**没有应用侧 Kotlin / 自定义 manifest / 额外 native 库的注入点**：`BernNative` 的 JNI 表固定，Kotlin 目录每次构建被 `Mirror` 裁剪。因此不能走「加一个 JNI 方法」的常规路线。Windows 复用同一份源码与桥接层，差别只在编译方式和链接注入。

本工程的做法：

1. 从官方 `Bundles/Alone2/makefile.gcc`（7zz 全量）和 `Bundles/Alone7z/makefile.gcc`（7zr 精简）配合 `7zip_gcc.mak` 的显式规则派生出两份清单：`vendor/sources-full.txt`（**324 个单元**，默认）与 `vendor/sources.txt`（**164 个单元：131 `.cpp` + 33 `.c`**，`LAMBDA_7Z_BUNDLE=7zr` 时使用）。`tools/gen_sources.py` 可重生成。
2. Android 上 `build.rs` 用 `cc` crate + Bern 已设置的 `CC_aarch64_linux_android` 交叉编译。**不能用 `.cpp(true)`**：cc-rs 会把 `.c` 也交给 clang++，7-Zip 的 `C/MtDec.h` 依赖 C 的 tag/typedef 命名空间，会报 `typedef redefinition`。改为 `cpp(false)`，靠扩展名让 clang 自动区分 C / C++。
3. 🔴 **Android 必须把所有 `.o` 用 `ld.lld -r` 合并成单一可重定位对象再打包**。7-Zip 的 codec / archive handler 靠 `*Reg.cpp` 里的全局构造函数注册（`REGISTER_ARC_*` 宏 → `.init_array`）；普通 `.a` 会被链接器按「解析未定义符号」裁剪，结果 `.so` 里没有 `NArchive::N7z::CHandler`，运行时报「找不到处理归档的代码」。另外 Cargo 的 `OUT_DIR` 在改 `build.rs` 后不变，归档前要 `remove_file`，否则旧成员残留。
   Windows 没有 `ld -r`：`build.rs` 用 MSVC 编译同样 324 个单元（另加 5 个 Windows 系统单元），把全部 `.o` 写进 `lambda7z-msvc.rsp`；`build-sequential.cmd` 把它注入 Bern 生成的链接命令，CLI 则链 `lambda7z.lib` 并加 `/WHOLEARCHIVE:lambda7z`。注册对象同样不能被裁掉，而且 `bern build --target windows` 每次都会重写 `build.bat`，所以包装脚本必须跟在每次构建之后。
4. `bridge/lambda7z.cpp` 导出 `lambda7z_run(...)` 与 `lambda7z_run_stream(...)`：用 `dup2`（Windows 上是 `_dup2`）把 stdout/stderr 临时重定向到日志文件或管道，**进程内调用 `MainAr.cpp` 的 `lambda7z_engine_main`**（`Z7_LAMBDA_INPROCESS` 下的稳定 C 入口；Windows 直接接收 argv，不再通过 `GetCommandLineW()` 重读命令行），返回退出码并把日志尾部交回 Rust；`std::mutex` 串行化调用。Windows 的 WinUI 进程没有控制台，桥接层会先把无效的 CRT 标准流 `_wfreopen(NUL)` 再重定向，否则 `fwrite(stdout)` 静默失败。取消由桥接层登记本次调用的回调、补丁后的 `TestBreakSignal()` 查询，7-Zip 在下一个进度回调点退出；不再使用全局 `g_BreakCounter`。
5. Rust 侧构造官方 7zz 参数：`x -y -o<dir> -p<pw> <archive>` / `a -t7z -y -p<pw> -mhe=on <out.7z> <inputs>`。
6. 🔴 **Android 的 C++ 运行时必须动态链 NDK 的 `libc++_shared.so` 并随包**。早期「静态链 `c++_static`」的写法在桌面编译能过、符号也能解析，但真机 `dlopen` 会崩。`build.rs` 从 `CC_<target>` 反推 NDK sysroot，把 `libc++_shared.so` 复制到**私有的 `OUT_DIR/ndk_shared`**（`-L` 只指这个目录），再复制到 `target/bern/android/project/app/src/main/jniLibs/arm64-v8a/`，最后 `println!("cargo:rustc-link-lib=dylib=c++_shared")`。Bern 每次构建都会重建 `jniLibs`，所以这一步每轮都要做。

`content://` 的单文件由 `bern::files::open()` 复制进应用私有 cache；文档树由 `backend::import_folder_with` 自己用 `bern::files::list` 递归导入（`platform::import_folder` 会回溯到整棵授权树，不能用）；结果通过 `bern::platform::export_path()`（文件与目录都能递归导出）写回用户选中的文件夹。全部路径在应用私有目录里，不需要 `READ/WRITE_EXTERNAL_STORAGE` 权限。

## 归档预览（文本 + 图片）

归档表里已知文本格式（TXT / XML / JSON / log / CSV / md / SVG / INI / YAML / 常见源码后缀，以及 README、LICENSE 等无后缀名）在大小列显示文档图标；点开是预览模态。标题栏只有条目名与一个关闭按钮；编码、读取进度与页数都在页脚状态行，页码「第 N 页」在读取中不带额外文字，读完后变成「第 N / M 页」。

- **不落临时文件**：`7zz x -so -spd -y -bso2 -bse2 -bsp2 -bd <archive> <entry>`。两端共用 `lambda7z_run_stream`：fd 1 换成 `pipe()`，读线程逐块回调 Rust，stderr 仍进日志；sink 返回 0 时关闭读端，7-Zip 被管道背压停下。
- **解码**：BOM → 严格 UTF-8 → chardet-rs 限定候选集（gb18030 / big5 / cp932 / euc-kr / euc-jp / cp1252）→ GB18030 / Windows-1252 兜底；`encoding_rs::Decoder` 增量解码，跨块的多字节字符不会截断。
- **页环**：12,000 字符/页，页尾优先在附近换行处切开；最多预读 2 页、保留最近 16 页。页环满时 worker 在 condvar 上停住，7-Zip 随之被管道背压停住；状态里只放当前页。
- **密码**：复用打开归档时留在 `OpenArchive` 内存里的那一份，不弹第二个密码框；密码仍不进 State / 日志 / 任务记录。加密条目没有可用密码时直接报「先在解压里输入密码」。
- **边界**：只认文本后缀，不解析 XML 实体、不执行内容、不提供编辑 / 搜索 / 高亮；NUL 与其他控制字符显示成空格。
- **图片**：`picture.rs` 用 `bern::files::serve("lambda-picture", ...)` 给 `Image(source:)` 提供带随机 token 的 URL。宿主在后台把流读完，再按控件尺寸采样解码；窗口关闭/切换预览时 token 被移除，流式 7z 调用随之中止。只接受 PNG / JPEG / GIF / WebP / BMP / APNG 等双端解码器都稳的格式；条目超过 64 MB 或像素超过 2400 万时只显示拒绝原因。状态行显示「PNG · 1024 × 1024 · 44.3 KB」这类信息。图片模态不设固定高度：`Adaptive` 放在模态外按窗口宽度选控件尺寸，Modal 只按内容量高，底部不再留空。同级图片在打开预览时按归档表当前顺序收集（不含文件夹与没有密码的加密图片），页脚提供「上一张 / 第 N / M 张 / 下一张」，首尾禁用；只有一张时不出现翻页行。不提供缩放、平移，也不做手势滑动。

## 体积实测（2026-11-08，7-Zip 26.04 release）

| 产物 | 大小 | 说明 |
|---|---:|---|
| Windows `target/release/lambda.exe` | 5,210,624 B（5.21 MB） | 独立 CLI，单文件；7zz 全量引擎静态链入，不再附带 `7z.exe` / `7z.dll`；zip 后 2.23 MB |
| Android `Lambda-0.1.0-android.apk`（release，发布密钥签名） | **5,675,375 B（5.68 MB）** | 7zz 全量引擎；仅 arm64-v8a |
| Android `Lambda-0.1.0-android.aab` | **12,818,220 B（12.82 MB）** | 含 `BUNDLE-METADATA` 里的 debug symbols 与 proguard map，不能和 APK 直接比 |
| `libbern_app.so`（release, strip, 7zz） | 7,674,432 B（7.67 MB） | 324 个 7-Zip 单元 + Rust 引擎与编码依赖 |
| `libc++_shared.so`（NDK 28.2） | 1,253,544 B（1.25 MB） | 必须随包，否则真机 `dlopen` 失败 |
| `classes.dex`（release） | 3,548,160 B（3.55 MB） | |

Debug 构建不能当交付体积：Android debug APK 38,583,985 B（38.58 MB；未 strip 的 `.so` 102,133,472 B，strip 后 17,412,216 B），Windows 应用 debug `Lambda.exe` 22,447,616 B（22.45 MB）。

Windows CLI 从上一版「交付合计」2,066,432 B，变成 5,210,624 B 的单文件：全量 7zz 引擎和编码依赖都进了 exe，换来零外部依赖。Android release APK 从 3,977,546 B 增至 5,675,375 B（约 +1.70 MB），增长来自 7-Zip 26.04 源码、全量格式集，以及旧编码检测与预览引入的 Rust 依赖（`chardet-rs` / `encoding_rs` / `regex` 等）。

## 已知边界

- **引擎现在是 7zz（全格式）**：7z（LZMA / LZMA2 / BCJ / BCJ2 / Delta / Copy + AES）、ZIP（含 `-mem=AES256` 创建）、RAR / RAR5（只读）、TAR、gz / bz2 / xz / zst、ISO、CAB。`LAMBDA_7Z_BUNDLE=7zr` 仍可编回精简版（164 单元），但只有 7z / LZMA / XZ / Split；体积会明显变小（26.04 下未单独实测）。
- **压缩、解压、预览都在后台线程**：任务由 `wake::Waker` 驱动，进度与取消都不阻塞界面；`backend::import_folder_with` / `export_path` 也在任务线程里跑。引擎调用仍由桥接层的 `std::mutex` 串行，第二个 7-Zip 操作要排队。取消通过本次调用的取消回调让 7-Zip 在下一个进度回调点退出（双端一致）；**单个固实块编码期间仍要等下一次回调**（384 MB 压缩从 17% 取消到落地曾实测 +61 s），进度区会提前说明。
- **`import_folder` / `export_path` 有上限**：Kotlin 实现限制 10,000 文件 / 10,000 目录 / 32 层导入、导出 128 层；超大文件夹要分块。
- **密码明文**：UI 五处密码输入都是 `kind: secure`（界面已遮蔽），引擎通过 argv `-p` 传递（同进程内无命令行泄露，但内存里是明文，桥接层返回前会清零自己的副本）。任务记录 `jobs/current.json` 只写 `locked: true`，**不存密码**；重试加密任务要重新输。
- **旧编码 ZIP 名**：ZIP 没打 UTF-8 标志时，7-Zip 的 Linux 版把文件名的原始字节原样写进 `-slt` 日志；列表按字节解析，并把整包所有旧编码名拼起来检测一次（chardetng 的 CJK 结果 + 限定候选集的 chardet-rs），支持 GBK/GB18030、Big5、Shift_JIS、EUC-KR、EUC-JP、Windows-1252 和 CP437。**Windows 引擎在输出前就会按 OEM 代码页转换名字**，所以引擎层会先读 ZIP 中央目录的原始名、跑同一套检测，再用 `-mcp=<代码页>` 让列表/解压/预览按同一编码解码。剩余风险：整包只有很短的名字时仍可能判错（例如单名 Big5 会判成 EUC-JP）；混合编码包按 UTF-8 合法性逐名处理，未打标志但恰好是合法 UTF-8 的旧编码名会被当成 UTF-8；CP437 与 CP1252 的单字节文本可能混淆。
- **文本预览只保证 UTF-8 / ASCII 条目名（Android）**：旧编码 ZIP 名在列表里能正确显示，但 Android 的 7-Zip 仍按原始字节匹配条目，把解码后的名字交回去会匹配失败，预览报错；Windows 走 `-mcp`，显示名与条目路径一致。后续三条路线（7-Zip 改码页 / Rust 兜底读 ZIP 中央目录 / 保留原始字节匹配）都还没做。
- **文本预览按后缀白名单**：不在名单里的无后缀文件与二进制不进入预览；页环只保留最近 16 页，更早的页不能回翻；XML 不解析实体；很短的样本仍可能判错编码。
- **图片预览的边界**：只认 png / jpg / jpeg / jfif / gif / webp / bmp / apng；单条 64 MB、2400 万像素上限。Bern 的宿主会把整条流读进内存再按显示尺寸采样，因此内存由上限兜底，而不是流式分页；关闭预览会丢弃引擎 token、等待在途读结束再返回；宿主按 URL 保留的解码缓存（64 MB LRU）不保证立即清空。多图只在打开预览时按当前层与表格顺序取同级图片，翻页用按钮、不做手势；切换排序或进出目录前翻页列表不会更新。不支持缩放、平移，也不对 EXIF 方向以外的元数据做解释。
- **压缩忽略 `import_folder` 的深度限制**：`backend::import_folder_with` 自己 `files::list` 递归，预算 200,000 条 / 32 层（`platform::import_folder` 会把整棵授权树搬进来，不能用）。
- **7zz 的 console front-end（`MainAr.cpp`）会临时安装 Ctrl-C / SIGINT/SIGTERM 处理器**（`CCtrlHandlerSetter` 析构时恢复），并在 stdout/stderr 上输出；桥接层用 `dup2` / `_dup2` 重定向到私有日志、mutex 串行化，正常路径不调用 `exit()`（只在收到多次中断信号时）。真机多次解压/压缩后进程仍存活，未破坏宿主。

## 许可

仓库内第三方组件——7-Zip 26.04 源码、NDK `libc++_shared.so`、Rust 依赖——保留各自许可，映射见 [THIRD-PARTY-NOTICES.md](docs/THIRD-PARTY-NOTICES.md)。

两端都把 7-Zip 源码**静态链**进应用（不随包 DLL / exe），下游需满足 LGPL 的「可修改、可重新链接」义务：完整源码与构建脚本已随本仓库提供。商用前请自行完成 LGPL 合规审查。