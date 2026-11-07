//! `lambda extract -p password123 archive.7z`
//!
//! A free-form CLI front-end. Bern's generated CLI is an IPC client with a
//! fixed `<namespace> <method> --param value` grammar and JSON output, so this
//! small separate binary provides the product's own command syntax and copy.
//! Both this binary and the Bern UI call the very same vendored 7-Zip engine.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use zeroize::Zeroize;

const USAGE: &str = "\
用法：
  lambda extract -p <密码> <归档.7z> [-o <输出目录>]
  lambda extract -p <归档.7z> [-o <输出目录>]   （从 stdin 读密码）
  lambda info

选项：
  -p, --password <密码>   解压密码（7z 也接受 -p<密码> 连写）
                          省略密码值时从 stdin 读一行
  -o, --output   <目录>   输出目录，默认 <归档同名目录>/
  -h, --help              显示本帮助

环境变量：
  LAMBDA_PASSWORD         没有给 -p 时使用的密码（避免进入命令行）";

#[derive(Default)]
struct Options {
    command: String,
    password: Option<String>,
    password_stdin: bool,
    output: Option<PathBuf>,
    archive: Option<PathBuf>,
    help: bool,
}

impl Drop for Options {
    fn drop(&mut self) {
        if let Some(password) = self.password.as_mut() {
            password.zeroize();
        }
    }
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut o = Options::default();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-h" | "--help" => o.help = true,
            "-p" | "--password" => {
                i += 1;
                match args.get(i) {
                    Some(value) => o.password = Some(value.clone()),
                    None => o.password_stdin = true,
                }
            }
            "-o" | "--output" => {
                i += 1;
                o.output = Some(PathBuf::from(
                    args.get(i).ok_or_else(|| format!("{a} 需要一个目录"))?,
                ));
            }
            _ if a.starts_with("-p") && a.len() > 2 => {
                o.password = Some(a[2..].to_string());
            }
            _ if a.starts_with("--password=") => {
                o.password = Some(a["--password=".len()..].to_string());
            }
            _ if a.starts_with("--output=") => {
                o.output = Some(PathBuf::from(&a["--output=".len()..]));
            }
            _ if a.starts_with('-') => return Err(format!("未知选项：{a}")),
            _ if o.command.is_empty() => o.command = a.clone(),
            _ if o.archive.is_none() => o.archive = Some(PathBuf::from(a)),
            _ => return Err(format!("多余的参数：{a}")),
        }
        i += 1;
    }
    Ok(o)
}

fn read_stdin_password() -> Result<String, String> {
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|error| format!("读取密码失败：{error}"))?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

fn default_output(archive: &Path) -> PathBuf {
    let stem = archive
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "lambda-output".into());
    archive.with_file_name(stem)
}

fn run() -> Result<(), String> {
    sevenzip_src::set_log_dir(std::env::temp_dir().join("lambda-cli-logs"));
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut o = parse(&args)?;
    if o.help || o.command.is_empty() {
        println!("{USAGE}");
        return Ok(());
    }
    if o.password_stdin {
        o.password = Some(read_stdin_password()?);
    }
    if o.password.is_none()
        && let Ok(value) = std::env::var("LAMBDA_PASSWORD")
        && !value.is_empty()
    {
        o.password = Some(value);
    }

    match o.command.as_str() {
        "info" => {
            println!("[lambda] 引擎：{}", sevenzip_src::engine_name());
            Ok(())
        }
        "extract" => {
            let archive = o.archive.take().ok_or_else(|| "缺少归档路径".to_string())?;
            let dest = o.output.take().unwrap_or_else(|| default_output(&archive));
            let password = o.password.as_deref();

            println!("[lambda] 正在解除拉姆达的封印...");
            let report = sevenzip_src::extract(&archive, password, &dest).map_err(|e| e.0)?;
            println!("[lambda] 成功释放 {} 个文件。", report.files);
            println!("[lambda] 输出目录：{}", dest.display());
            println!(
                "[lambda] 写入 {} 字节 · 引擎 {}",
                report.bytes,
                sevenzip_src::engine_name()
            );
            Ok(())
        }
        other => Err(format!("未知命令：{other}")),
    }
}

fn main() -> ExitCode {
    sevenzip_src::enable_utf8_console();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[lambda] 解除失败：{e}");
            eprintln!("[lambda] 用 `lambda --help` 查看用法。");
            ExitCode::FAILURE
        }
    }
}
