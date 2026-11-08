//! Compile the vendored 7-Zip 26.04 sources.
//!
//! Android links the merged objects into `libbern_app.so` and drives the
//! console front-end through `bridge/lambda7z.cpp`. The objects are merged
//! into ONE relocatable object before archiving because 7-Zip registers its
//! codecs and archive handlers from global constructors in the `*Register.cpp`
//! units; a normal `.a` would only pull objects that resolve an undefined
//! symbol, so those registration units would silently disappear and the
//! engine would report "unsupported archive" at runtime.
//!
//! Windows compiles the same sources with MSVC. There is no `ld -r` there,
//! and Bern's generated app links a fixed library list, so this build does
//! not archive anything: it writes a response file with every object path
//! (`<profile>/lambda7z-msvc.rsp`) and `build-sequential.cmd` appends it to
//! the generated link line, exactly like the existing `/MP` patch.
//!
//! `vendor/sources*.txt` (7zr, Bundles/Alone7z) and `vendor/sources-full.txt`
//! (7zz, Bundles/Alone2) are generated from 7-Zip's own `makefile.gcc` plus
//! the explicit object -> source rules in `7zip_gcc.mak` (see
//! `tools/gen_sources.py`). `LAMBDA_7Z_BUNDLE` picks which one is compiled:
//! `7zz` by default (zip/rar are part of the product), `7zr` for the smaller
//! engine and for A/B size measurements.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Bundle {
    list_name: &'static str,
    variant: &'static str,
    exe: &'static str,
}

fn selected_bundle() -> Bundle {
    let bundle = env::var("LAMBDA_7Z_BUNDLE").unwrap_or_else(|_| "7zz".into());
    match bundle.as_str() {
        // 7zr = the small standalone build (Alone7z): 7z/LZMA/XZ only.
        "7zr" | "alone7z" => Bundle {
            list_name: "sources.txt",
            variant: "R",
            exe: "7zr",
        },
        // 7zz = the full standalone build (Alone2): 7z, zip, rar read, tar,
        // gzip, bzip2, xz plus the volume/filesystem handlers the sources list
        // also compiles (APFS, NTFS, Ext, VHD/VHDX/VMDK/QCOW, SquashFS, DMG,
        // PE/ELF, RPM, CPIO, ARJ, LZH, Zstd, …). The format surface is the
        // attack surface: see `docs/THIRD-PARTY-NOTICES.md` and the fixtures.
        "7zz" | "alone2" => Bundle {
            list_name: "sources-full.txt",
            variant: "Z",
            exe: "7zz",
        },
        other => panic!("LAMBDA_7Z_BUNDLE={other} is neither 7zr nor 7zz"),
    }
}

fn source_paths(base: &Path, list_name: &str) -> Vec<PathBuf> {
    let source_root = base.join("vendor").join("7zsrc");
    let list = fs::read_to_string(base.join("vendor").join(list_name)).unwrap_or_else(|_| {
        panic!("vendor/{list_name} is missing: run python tools/gen_sources.py")
    });
    let sources: Vec<PathBuf> = list
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| source_root.join(line))
        .collect();
    for source in &sources {
        assert!(
            source.is_file(),
            "missing 7-Zip source: {}",
            source.display()
        );
    }
    sources
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=vendor/sources.txt");
    println!("cargo:rerun-if-changed=vendor/sources-full.txt");
    println!("cargo:rerun-if-changed=vendor/7zsrc");
    println!("cargo:rerun-if-changed=bridge/lambda7z.cpp");
    println!("cargo:rerun-if-changed=../docs/THIRD-PARTY-NOTICES.md");
    println!("cargo:rerun-if-env-changed=LAMBDA_7Z_BUNDLE");

    match env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("android") => build_android(),
        Ok("windows") => build_windows(),
        _ => {
            // Host builds on other platforms only report the engine label.
            println!("cargo:rustc-env=LAMBDA_7Z_ENGINE=7-Zip 26.04 (host build)");
            println!("cargo:rustc-env=LAMBDA_7Z_EXE=7z");
        }
    }
}

/// Copy the notice and the two 7-Zip license texts into the package. The
/// repository is not shipped with the APK/installer; these files must be.
fn stage_notices() {
    let base = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let Some(workspace) = base.parent() else {
        return;
    };
    let notice = workspace.join("docs/THIRD-PARTY-NOTICES.md");
    let licenses = [
        (
            workspace.join("sevenzip-src/vendor/7zsrc/DOC/License.txt"),
            "7-Zip-License.txt",
        ),
        (
            workspace.join("sevenzip-src/vendor/7zsrc/DOC/unRarLicense.txt"),
            "unRarLicense.txt",
        ),
    ];
    let targets = match env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("android") => {
            vec![workspace.join("target/bern/android/project/app/src/main/assets/notices")]
        }
        Ok("windows") => vec![workspace.join("target/bern/windows/app/runtime/notices")],
        _ => return,
    };
    for dir in targets {
        if let Err(error) = fs::create_dir_all(&dir) {
            println!(
                "cargo:warning=无法创建第三方声明目录 {}：{error}",
                dir.display()
            );
            continue;
        }
        if let Err(error) = fs::copy(&notice, dir.join("THIRD-PARTY-NOTICES.md")) {
            println!(
                "cargo:warning=无法复制第三方声明 {}：{error}",
                notice.display()
            );
        }
        for (source, name) in &licenses {
            if let Err(error) = fs::copy(source, dir.join(name)) {
                println!("cargo:warning=无法复制 {}：{error}", source.display());
            }
        }
    }
}

fn build_android() {
    stage_notices();
    let bundle = selected_bundle();
    println!(
        "cargo:rustc-env=LAMBDA_7Z_ENGINE=7-Zip 26.04 ({})",
        bundle.exe
    );
    println!("cargo:rustc-env=LAMBDA_7Z_EXE={}", bundle.exe);
    println!(
        "cargo:warning=7-Zip bundle: {} ({})",
        bundle.list_name, bundle.exe
    );

    let base = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let sources = source_paths(&base, bundle.list_name);

    let mut build = cc::Build::new();
    // Deliberately `cpp(false)`: cc-rs would otherwise compile the `.c`
    // translation units with clang++ as well, and 7-Zip's C headers rely on
    // C tag/typedef namespace rules (MtDec.h breaks under C++). clang picks
    // C++ for `.cpp` and C for `.c` from the extension, so a single CC-driven
    // build produces one clean set of objects; libc++ is linked explicitly.
    build
        .pic(true)
        .warnings(false)
        .define("NDEBUG", None)
        .define("_REENTRANT", None)
        .define("_FILE_OFFSET_BITS", Some("64"))
        .define("_LARGEFILE_SOURCE", None)
        // The bridge hands over argv in-process instead of letting the console
        // front-end read the process command line (same switch as Windows).
        .define("Z7_LAMBDA_INPROCESS", None)
        // 7zr (Alone7z) vs 7zz (Alone2) build variant: the console code reads
        // this macro to decide which verbs and formats it advertises.
        .define(&format!("Z7_PROG_VARIANT_{}", bundle.variant), None)
        .flag_if_supported("-fstack-protector-strong")
        .flag_if_supported("-fexceptions")
        .flag_if_supported("-pthread")
        .file(base.join("bridge").join("lambda7z.cpp"));
    for source in &sources {
        build.file(source);
    }
    let objects = build.compile_intermediates();

    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let combined = out.join("lambda7z_all.o");
    let archive = out.join("liblambda7z.a");
    // Cargo keeps OUT_DIR across build.rs edits, so an archive left by an
    // earlier `Build::compile()` would still contain the separate objects
    // and the linker would keep pulling those instead of the merged one.
    let _ = fs::remove_file(&archive);

    let lld = tool("ld.lld");
    // 300+ object paths do not fit in one Windows command line (32,767
    // chars; alone2 blew up with `os error 206`), so hand them over in a
    // response file. Forward slashes keep lld's tokenizer from treating `\`
    // as an escape.
    let rsp = out.join("lambda7z_objects.rsp");
    let mut list = String::new();
    for object in &objects {
        list.push('"');
        list.push_str(&object.display().to_string().replace('\\', "/"));
        list.push_str("\"\n");
    }
    fs::write(&rsp, list).expect("cannot write the ld.lld response file");
    let status = Command::new(&lld)
        .arg("-r")
        .arg("-o")
        .arg(&combined)
        .arg(format!("@{}", rsp.display()))
        .status()
        .unwrap_or_else(|error| panic!("cannot run {}: {error}", lld.display()));
    assert!(status.success(), "ld.lld -r failed: {status}");

    let target = env::var("TARGET").expect("TARGET").replace('-', "_");
    let ar = env::var(format!("AR_{target}"))
        .or_else(|_| env::var("AR"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| tool("llvm-ar"));
    let status = Command::new(&ar)
        .arg("crs")
        .arg(&archive)
        .arg(&combined)
        .status()
        .unwrap_or_else(|error| panic!("cannot run {}: {error}", ar.display()));
    assert!(status.success(), "llvm-ar failed: {status}");

    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=lambda7z");
    // Use the NDK's *shared* C++ runtime and ship libc++_shared.so in the APK.
    //
    // Bern produces exactly one .so, so a C++ runtime has to come from
    // somewhere. Two real-device failures showed what does NOT work:
    //  1. `cargo:rustc-link-lib=c++_static` (no `static=`) is treated as a
    //     request for libc++_static.so, which does not exist; the flag is
    //     dropped and the .so keeps 179 undefined C++ ABI symbols ->
    //     `dlopen failed: cannot locate symbol "_ZTISt12length_error"`.
    //     Also, that typeinfo lives in libc++abi.a, not libc++_static.a.
    //  2. `static=c++_static` + `static=c++abi` resolves the symbols, but
    //     puts libc++'s global constructors into OUR .init_array. Android's
    //     linker runs them before libc++'s own global containers are built,
    //     so the first init function dereferences a NULL container ->
    //     SIGSEGV at dlopen (tombstone #01 == first .init_array entry).
    // With DT_NEEDED on libc++_shared.so the linker initialises the
    // dependency first, which fixes both problems at once.
    if let Some(ndk_libs) = ndk_lib_dir() {
        let shared = ndk_libs.join("libc++_shared.so");
        if shared.is_file() {
            // Expose ONLY libc++_shared.so through -L. The NDK sysroot lib
            // directory also holds libc.a / libdl.a / libm.a; passing the whole
            // directory makes `-ldl` resolve against libdl.a and drags a
            // static copy of bionic's getauxval plus a zeroed __libc_globals
            // into the .so (with no DT_NEEDED libc.so). Rust's
            // init_have_lse_atomics then calls that local getauxval from
            // .init_array during dlopen -> null dereference (SIGSEGV) on a
            // real device. A private directory containing just the runtime
            // avoids that entirely.
            let private = out.join("ndk_shared");
            fs::create_dir_all(&private).expect("cannot create the private NDK lib dir");
            fs::copy(&shared, private.join("libc++_shared.so"))
                .expect("cannot stage libc++_shared.so for the linker");
            println!("cargo:rustc-link-search=native={}", private.display());

            // Bern only copies libbern_app.so into jniLibs; place the runtime
            // next to it so Gradle packages both. Re-done on every build
            // because Bern regenerates that directory.
            let jni = base
                .parent()
                .expect("workspace root")
                .join("target/bern/android/project/app/src/main/jniLibs/arm64-v8a");
            if fs::create_dir_all(&jni).is_ok() {
                let _ = fs::copy(&shared, jni.join("libc++_shared.so"));
            }
        } else {
            println!(
                "cargo:warning=libc++_shared.so not found in the NDK sysroot; the APK may fail to load"
            );
        }
    } else {
        println!(
            "cargo:warning=could not locate the NDK sysroot lib dir; C++ runtime symbols may stay unresolved"
        );
    }
    println!("cargo:rustc-link-lib=dylib=c++_shared");
    println!("cargo:rustc-link-lib=dl");
}

fn build_windows() {
    stage_notices();
    let bundle = selected_bundle();
    println!(
        "cargo:rustc-env=LAMBDA_7Z_ENGINE=7-Zip 26.04 ({})",
        bundle.exe
    );
    println!("cargo:rustc-env=LAMBDA_7Z_EXE={}", bundle.exe);

    let base = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let mut sources = source_paths(&base, bundle.list_name);
    // The GCC lists leave out the Windows-only system units (their SYS_OBJS
    // in makefile.gcc are guarded by IS_MINGW). MSVC needs all five.
    for extra in [
        "CPP/Windows/DLL.cpp",
        "CPP/Windows/FileSystem.cpp",
        "CPP/Windows/MemoryLock.cpp",
        "CPP/Windows/Registry.cpp",
        "C/DllSecur.c",
    ] {
        let path = base.join("vendor").join("7zsrc").join(extra);
        assert!(path.is_file(), "missing 7-Zip source: {}", path.display());
        sources.push(path);
    }

    let mut build = cc::Build::new();
    build
        .warnings(false)
        .define("NDEBUG", None)
        // The upstream MSVC build defines UNICODE for every bundle; the
        // engine's wide-path handling depends on it.
        .define("UNICODE", None)
        .define("_UNICODE", None)
        .define("_CONSOLE", None)
        // Main.cpp/MainAr.cpp take argv instead of GetCommandLineW(), and
        // MainAr.cpp exports lambda7z_engine_main for the bridge.
        .define("Z7_LAMBDA_INPROCESS", None)
        .define(&format!("Z7_PROG_VARIANT_{}", bundle.variant), None)
        .flag_if_supported("/std:c++17")
        .flag_if_supported("/EHsc")
        .flag_if_supported("/Gy")
        .flag_if_supported("/GF")
        .flag_if_supported("/GR-")
        // Upstream's Build.mak adds -GS- to every MSVC build. Lambda parses
        // untrusted archives in-process next to the password store, so keep
        // the stack cookie and add Control Flow Guard instead.
        .flag_if_supported("/guard:cf")
        .flag_if_supported("/Zc:wchar_t")
        .flag_if_supported("/Zc:forScope")
        .flag_if_supported("/Zc:throwingNew")
        .flag_if_supported("/bigobj")
        .file(base.join("bridge").join("lambda7z.cpp"));
    for source in &sources {
        build.file(source);
    }
    let objects = build.compile_intermediates();

    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let mut list = String::new();
    for object in &objects {
        list.push('"');
        list.push_str(&object.display().to_string().replace('\\', "/"));
        list.push_str("\"\r\n");
    }

    // Every cargo-linked consumer (CLI, tests) gets the whole library:
    // without `/WHOLEARCHIVE` LINK drops the *Register.cpp members and the
    // engine finds no formats. The Bern app uses the response file instead.
    let objects_rsp = out.join("lambda7z_objects.rsp");
    fs::write(&objects_rsp, &list).expect("cannot write the archiver response file");
    let archive = out.join("lambda7z.lib");
    let _ = fs::remove_file(&archive);
    let mut librarian = build.get_archiver();
    let status = librarian
        .arg("/nologo")
        .arg(format!("/OUT:{}", archive.display()))
        .arg(format!("@{}", objects_rsp.display()))
        .status()
        .unwrap_or_else(|error| panic!("cannot run the librarian: {error}"));
    assert!(status.success(), "lib.exe failed: {status}");
    println!("cargo:rustc-link-search=native={}", out.display());
    // `+whole-archive` is required: MSVC pulls only objects that satisfy a
    // reference, and the format handlers register themselves from static
    // constructors, so a plain static lib would link but find no formats.
    println!("cargo:rustc-link-lib=static:+whole-archive=lambda7z");
    // System libraries the console front-end needs (same five as the
    // upstream MSVC build). Bern's hard-coded app link already lists them.
    for library in ["oleaut32", "ole32", "user32", "advapi32", "shell32"] {
        println!("cargo:rustc-link-lib={library}");
    }

    // OUT_DIR = <target>/<profile>/build/<pkg>-<hash>/out; the wrapper looks
    // for the response file under <target> and takes the newest one.
    let profile = out
        .ancestors()
        .nth(3)
        .expect("profile directory above OUT_DIR")
        .to_path_buf();
    let rsp = profile.join("lambda7z-msvc.rsp");
    fs::write(&rsp, &list).expect("cannot write the MSVC link response file");
    println!(
        "cargo:warning=7-Zip: {} objects listed in {}",
        objects.len(),
        rsp.display()
    );
}

/// A tool next to the NDK compiler (Windows NDK tools carry a `.exe`).
fn tool(name: &str) -> PathBuf {
    let target = env::var("TARGET").expect("TARGET").replace('-', "_");
    let cc = env::var(format!("CC_{target}"))
        .or_else(|_| env::var("CC"))
        .expect("the NDK C compiler must be set (CC_<target>)");
    let plain = Path::new(&cc)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(name);
    if plain.exists() {
        plain
    } else {
        PathBuf::from(format!("{}.exe", plain.display()))
    }
}

/// Directory holding the NDK's C++ runtime archives, derived from the CC
/// path Bern sets (.../toolchains/llvm/prebuilt/<host>/bin/<triple><api>-clang).
fn ndk_lib_dir() -> Option<PathBuf> {
    let target = env::var("TARGET").ok()?;
    let cc = env::var(format!("CC_{}", target.replace('-', "_")))
        .or_else(|_| env::var("CC"))
        .ok()?;
    let host = Path::new(&cc).parent()?.parent()?;
    let triple_dir = if target.starts_with("aarch64") {
        "aarch64-linux-android"
    } else if target.starts_with("armv7") || target.starts_with("thumbv7") {
        "arm-linux-androideabi"
    } else if target.starts_with("x86_64") {
        "x86_64-linux-android"
    } else {
        "i686-linux-android"
    };
    let dir = host
        .join("sysroot")
        .join("usr")
        .join("lib")
        .join(triple_dir);
    dir.is_dir().then_some(dir)
}
