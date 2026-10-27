//! `dict-store` — command line access to a dictionary library.
//!
//! Run `dict-store --help` for the full command list. Every command accepts the
//! global `--root <目录>` option; the default is the directory containing
//! `dict-store.exe`, which is where `library.json` and the per-version
//! directories live.
//!
//! The commands map one to one onto [`dict_store::Library`]: `add`, `upgrade`,
//! `rollback`, `remove`, `activate`, `deactivate`, plus the read-only `list` and
//! `history`.
//!
//! Output is a human-readable table on stdout; failures print a Chinese message
//! on stderr and exit with a non-zero status.

#![deny(missing_docs)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use dict_store::{
    Format, Library, NewPackage, PackageMeta, SourceId, StoreError,
};

/// Text printed by `dict-store --help`.
const GLOBAL_HELP: &str = "\
dict-store — 词典库管理（离线，不联网）

用法:
  dict-store [--root <目录>] <命令> [参数]

全局选项:
  --root <目录>   词典库根目录，library.json 与各版本目录都在这里
                  （默认：dict-store.exe 所在目录）
  -h, --help      显示帮助

命令:
  list                         列出已安装的词典版本
  add <目录> [选项]            从目录安装一个版本
  upgrade <目录> [选项]        安装新版本并把启用状态切过去
  rollback <id>                回退到上一个已安装版本
  remove <id> [--version <版本>]
                               删除一个版本；省略 --version 时删除该词典全部版本
  activate <id> <版本>         启用指定版本
  deactivate <id>              停用该词典（没有启用版本）
  history [id]                 查看历史记录（只追加，不修改）

安装类命令的选项（add / upgrade）:
  --id <id>           词典标识，跨版本不变，例如 ecdict（必填）
  --version <版本>    版本号，例如 2.4.2 或 2024.03（必填）
  --name <名称>       显示名（必填）
  --license <许可>    许可证，SPDX 标识或自由文本（必填，不能为空）
  --format <格式>     文件格式标签，例如 stardict / jsonl / mdx
  --homepage <地址>   来源主页
  --entries <数量>    词条数
  --force             允许覆盖同版本内容、安装不更新的版本、或覆盖残留目录

  必填项也可以写在包目录的 source.json 里，命令行给出的值优先。

示例:
  dict-store --root D:\\dicts list
  dict-store --root D:\\dicts add .\\ecdict-2.4.2 --id ecdict --version 2.4.2 \\
             --name ECDICT --license CC-BY-SA-4.0
  dict-store --root D:\\dicts upgrade .\\ecdict-2.4.3 --id ecdict --version 2.4.3 \\
             --name ECDICT --license CC-BY-SA-4.0
";

/// Help for `list`.
const HELP_LIST: &str = "\
用法: dict-store [--root <目录>] list

列出词典库里所有已安装的版本，`ACTIVE` 一列标 * 的是当前启用的版本。
";

/// Help for `add`.
const HELP_ADD: &str = "\
用法: dict-store [--root <目录>] add <目录> [选项]

把 <目录> 里的词典包复制进词典库，目录名 <id>-<版本>。忽略包目录顶层已有的
source.json（库会写入自己的副本）。

必填（或写在 source.json 里）: --id --version --name --license
可选: --format --homepage --entries --force
";

/// Help for `upgrade`.
const HELP_UPGRADE: &str = "\
用法: dict-store [--root <目录>] upgrade <目录> [选项]

安装一个新版本并把启用状态切过去，旧版本目录保留以便回退。
新版本必须比当前最新版本更高，否则需要 --force。

必填（或写在 source.json 里）: --id --version --name --license
可选: --format --homepage --entries --force
";

/// Help for `rollback`.
const HELP_ROLLBACK: &str = "\
用法: dict-store [--root <目录>] rollback <id>

把启用状态指回该词典上一个已安装版本（按版本号排序）。
";

/// Help for `remove`.
const HELP_REMOVE: &str = "\
用法: dict-store [--root <目录>] remove <id> [--version <版本>]

删除一个版本目录并追加一条 removed 记录。省略 --version 时删除该词典的全部版本。
删除的是启用版本时，自动启用剩下的最新版本（没有剩余版本则不再启用）。
";

/// Help for `activate`.
const HELP_ACTIVATE: &str = "\
用法: dict-store [--root <目录>] activate <id> <版本>

把指定版本设为该词典唯一启用的版本。
";

/// Help for `deactivate`.
const HELP_DEACTIVATE: &str = "\
用法: dict-store [--root <目录>] deactivate <id>

停用该词典的全部版本（词典仍在库里，但不再参与查询）。
";

/// Help for `history`.
const HELP_HISTORY: &str = "\
用法: dict-store [--root <目录>] history [id]

按时间顺序打印历史记录（installed / upgraded / rolled_back / removed /
activated / deactivated）。给出 id 时只显示该词典。
";

/// One failed command: a Chinese message and, when useful, a usage block.
#[derive(Debug)]
struct CliError {
    message: String,
    usage: Option<&'static str>,
}

impl CliError {
    /// A failure with no usage text.
    fn new(message: impl Into<String>) -> Self {
        CliError {
            message: message.into(),
            usage: None,
        }
    }

    /// Attach the usage text of the command that failed.
    fn with_usage(mut self, usage: &'static str) -> Self {
        self.usage = Some(usage);
        self
    }
}

impl From<StoreError> for CliError {
    fn from(error: StoreError) -> Self {
        CliError::new(error.to_string())
    }
}

/// Entry point: exit code 1 for any failure, 0 otherwise.
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("错误：{}", error.message);
            if let Some(usage) = error.usage {
                eprintln!();
                eprintln!("{usage}");
            }
            ExitCode::FAILURE
        }
    }
}

/// Run one command line.
fn run(args: &[String]) -> Result<(), CliError> {
    let (root, rest) = extract_root(args)?;
    let Some(command) = rest.first() else {
        println!("{GLOBAL_HELP}");
        return Ok(());
    };

    if command == "-h" || command == "--help" || command == "help" {
        println!("{GLOBAL_HELP}");
        return Ok(());
    }
    let tail = &rest[1..];
    if tail.iter().any(|arg| arg == "-h" || arg == "--help") {
        let help = command_help(command)
            .ok_or_else(|| CliError::new(format!("未知命令 {command}")).with_usage(GLOBAL_HELP))?;
        println!("{help}");
        return Ok(());
    }

    let root = resolve_root(root)?;
    match command.as_str() {
        "list" => list_command(&root, tail),
        "add" => install_command(&root, tail, false),
        "upgrade" => install_command(&root, tail, true),
        "rollback" => rollback_command(&root, tail),
        "remove" => remove_command(&root, tail),
        "activate" => activate_command(&root, tail),
        "deactivate" => deactivate_command(&root, tail),
        "history" => history_command(&root, tail),
        other => Err(CliError::new(format!("未知命令 {other}")).with_usage(GLOBAL_HELP)),
    }
}

/// Help text for a subcommand, if the name is known.
fn command_help(command: &str) -> Option<&'static str> {
    match command {
        "list" => Some(HELP_LIST),
        "add" => Some(HELP_ADD),
        "upgrade" => Some(HELP_UPGRADE),
        "rollback" => Some(HELP_ROLLBACK),
        "remove" => Some(HELP_REMOVE),
        "activate" => Some(HELP_ACTIVATE),
        "deactivate" => Some(HELP_DEACTIVATE),
        "history" => Some(HELP_HISTORY),
        _ => None,
    }
}

/// Pull the global `--root` option out of the argument list.
fn extract_root(args: &[String]) -> Result<(Option<PathBuf>, Vec<String>), CliError> {
    let mut root = None;
    let mut rest = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--root" {
            let value = args
                .get(index + 1)
                .ok_or_else(|| CliError::new("--root 需要一个目录参数").with_usage(GLOBAL_HELP))?;
            root = Some(PathBuf::from(value));
            index += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--root=") {
            if value.is_empty() {
                return Err(
                    CliError::new("--root 需要一个目录参数").with_usage(GLOBAL_HELP)
                );
            }
            root = Some(PathBuf::from(value));
            index += 1;
            continue;
        }
        rest.push(arg.clone());
        index += 1;
    }
    Ok((root, rest))
}

/// The library root: `--root` when given, otherwise the executable's directory.
fn resolve_root(explicit: Option<PathBuf>) -> Result<PathBuf, CliError> {
    if let Some(root) = explicit {
        return Ok(root);
    }
    let executable = std::env::current_exe()
        .map_err(|error| CliError::new(format!("无法定位程序自身路径：{error}")))?;
    executable
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| CliError::new("无法定位程序所在目录，请用 --root 指定词典库"))
}

/// Minimal aligned table printer.
struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl Table {
    /// A table with the given column headers.
    fn new(headers: &[&str]) -> Self {
        Table {
            headers: headers.iter().map(|header| header.to_string()).collect(),
            rows: Vec::new(),
        }
    }

    /// Add one row; it must have one cell per column.
    fn push(&mut self, cells: Vec<String>) {
        self.rows.push(cells);
    }

    /// Print the table to stdout.
    fn print(&self) {
        let widths: Vec<usize> = (0..self.headers.len())
            .map(|column| {
                let mut width = self.headers[column].chars().count();
                for row in &self.rows {
                    if let Some(cell) = row.get(column) {
                        width = width.max(cell.chars().count());
                    }
                }
                width
            })
            .collect();

        println!("{}", render_row(&self.headers, &widths));
        let separators: Vec<String> = widths.iter().map(|width| "-".repeat(*width)).collect();
        println!("{}", render_row(&separators, &widths));
        for row in &self.rows {
            println!("{}", render_row(row, &widths));
        }
    }
}

/// Render one row, padding every cell except the last to its column width.
fn render_row(cells: &[String], widths: &[usize]) -> String {
    let mut line = String::new();
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            line.push_str("  ");
        }
        line.push_str(cell);
        if let Some(width) = widths.get(index) {
            let padding = width.saturating_sub(cell.chars().count());
            line.push_str(&" ".repeat(padding));
        }
    }
    line.trim_end().to_string()
}

/// Format a byte count as `190.2 MB`.
fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// `list`: every installed version, active ones marked.
fn list_command(root: &Path, tail: &[String]) -> Result<(), CliError> {
    if let Some(extra) = tail.first() {
        return Err(CliError::new(format!("list 不接受参数（收到 {extra}）")).with_usage(HELP_LIST));
    }
    let library = Library::open(root)?;
    if library.list().is_empty() {
        println!("词典库为空：{}", root.display());
        println!(
            "用 `dict-store add <目录> --id … --version … --name … --license …` 安装第一本词典。"
        );
        return Ok(());
    }

    println!("词典库：{}（{} 个版本）", root.display(), library.list().len());
    let mut table = Table::new(&[
        "ID", "VERSION", "ACTIVE", "FORMAT", "ENTRIES", "SIZE", "NAME", "LICENSE",
    ]);
    for package in library.list() {
        table.push(vec![
            package.id.to_string(),
            package.version.clone(),
            if package.active { "*" } else { "" }.to_string(),
            package.format.to_string(),
            package
                .entries
                .map(|entries| entries.to_string())
                .unwrap_or_else(|| "-".to_string()),
            format_bytes(package.bytes),
            package.name.clone(),
            package.license.clone(),
        ]);
    }
    table.print();
    Ok(())
}

/// Parsed arguments of `add` / `upgrade`.
struct InstallArgs {
    /// Directory holding the package files.
    directory: PathBuf,
    /// Metadata given on the command line; `source.json` fills the gaps.
    meta: PackageMeta,
    /// Whether `--force` was given.
    force: bool,
}

/// `add` and `upgrade` (they differ only in the library call).
fn install_command(root: &Path, tail: &[String], upgrade: bool) -> Result<(), CliError> {
    let usage = if upgrade { HELP_UPGRADE } else { HELP_ADD };
    let args = parse_install_args(tail, usage)?;
    let mut library = Library::open(root)?;
    let package = NewPackage::resolve(&args.directory, &args.meta)?;

    if upgrade {
        let previous = match library.active(&package.id) {
            Some(active) => active.version.clone(),
            None => "?".to_string(),
        };
        let installed = library.upgrade(&args.directory, &package, args.force)?;
        println!(
            "已升级 {} {} → {}（{}）",
            installed.id, previous, installed.version, installed.name
        );
        println!(
            "  目录：{}",
            root.join(installed.directory_name()).display()
        );
        println!(
            "  sha256：{} · {} · {}",
            installed.sha256,
            format_bytes(installed.bytes),
            if installed.active { "启用中" } else { "未启用" }
        );
        return Ok(());
    }

    let installed = library.add(&args.directory, &package, args.force)?;
    println!(
        "已安装 {} {}（{}）",
        installed.id, installed.version, installed.name
    );
    println!(
        "  目录：{}",
        root.join(installed.directory_name()).display()
    );
    println!(
        "  sha256：{} · {} · {}",
        installed.sha256,
        format_bytes(installed.bytes),
        if installed.active { "启用中" } else { "未启用" }
    );
    if !installed.active {
        println!(
            "  提示：该词典已有启用版本，用 `dict-store activate {} {}` 切换。",
            installed.id, installed.version
        );
    }
    Ok(())
}

/// Parse the arguments shared by `add` and `upgrade`.
fn parse_install_args(tail: &[String], usage: &'static str) -> Result<InstallArgs, CliError> {
    let mut directory: Option<PathBuf> = None;
    let mut meta = PackageMeta::default();
    let mut force = false;
    let mut index = 0;

    while index < tail.len() {
        let arg = &tail[index];
        if arg == "--force" {
            force = true;
            index += 1;
            continue;
        }
        if let Some(flag_and_value) = arg.strip_prefix("--") {
            let (flag, inline_value) = match flag_and_value.split_once('=') {
                Some((flag, value)) => (format!("--{flag}"), Some(value.to_string())),
                None => (arg.clone(), None),
            };
            let value = match inline_value {
                Some(value) => value,
                None => {
                    let next = tail
                        .get(index + 1)
                        .ok_or_else(|| CliError::new(format!("{flag} 需要一个值")).with_usage(usage))?;
                    index += 1;
                    next.clone()
                }
            };
            apply_install_flag(&flag, value, &mut meta).map_err(|error| error.with_usage(usage))?;
            index += 1;
            continue;
        }
        if arg.starts_with('-') {
            return Err(CliError::new(format!("未知参数 {arg}")).with_usage(usage));
        }
        if directory.is_some() {
            return Err(CliError::new("只接受一个包目录参数").with_usage(usage));
        }
        directory = Some(PathBuf::from(arg));
        index += 1;
    }

    let directory = directory
        .ok_or_else(|| CliError::new("缺少包目录参数").with_usage(usage))?;
    Ok(InstallArgs {
        directory,
        meta,
        force,
    })
}

/// Store one `--flag value` pair into `meta`.
fn apply_install_flag(flag: &str, value: String, meta: &mut PackageMeta) -> Result<(), CliError> {
    match flag {
        "--id" => {
            let id = SourceId::new(value).map_err(|error| CliError::new(error.to_string()))?;
            meta.id = Some(id);
        }
        "--version" => meta.version = Some(value),
        "--name" => meta.name = Some(value),
        "--license" => meta.license = Some(value),
        "--format" => meta.format = Some(Format::parse(&value)),
        "--homepage" => meta.homepage = Some(value),
        "--entries" => {
            let entries: u64 = value.parse().map_err(|_| {
                CliError::new(format!("--entries 需要非负整数，收到 \"{value}\""))
            })?;
            meta.entries = Some(entries);
        }
        other => return Err(CliError::new(format!("未知参数 {other}"))),
    }
    Ok(())
}

/// Collect exactly `count` positional arguments, rejecting any flag.
fn positionals(
    tail: &[String],
    count: usize,
    usage: &'static str,
    command: &str,
) -> Result<Vec<String>, CliError> {
    let mut values = Vec::new();
    for arg in tail {
        if arg.starts_with('-') {
            return Err(CliError::new(format!("未知参数 {arg}")).with_usage(usage));
        }
        values.push(arg.clone());
    }
    if values.len() != count {
        return Err(CliError::new(format!(
            "{command} 需要 {count} 个参数，收到 {} 个",
            values.len()
        ))
        .with_usage(usage));
    }
    Ok(values)
}

/// Parse a dictionary id, reporting a readable error.
fn parse_id(raw: &str, usage: &'static str) -> Result<SourceId, CliError> {
    SourceId::new(raw).map_err(|error| CliError::new(error.to_string()).with_usage(usage))
}

/// `rollback`: step the active version back.
fn rollback_command(root: &Path, tail: &[String]) -> Result<(), CliError> {
    let values = positionals(tail, 1, HELP_ROLLBACK, "rollback")?;
    let id = parse_id(&values[0], HELP_ROLLBACK)?;
    let mut library = Library::open(root)?;
    let rolled_back = library.rollback(&id)?;
    println!(
        "已回退 {} 到 {}（{}）",
        id, rolled_back.version, rolled_back.name
    );
    println!(
        "  目录：{}",
        root.join(rolled_back.directory_name()).display()
    );
    Ok(())
}

/// `remove`: delete one version or every version of an id.
fn remove_command(root: &Path, tail: &[String]) -> Result<(), CliError> {
    let mut id: Option<SourceId> = None;
    let mut version: Option<String> = None;
    let mut index = 0;
    while index < tail.len() {
        let arg = &tail[index];
        if arg == "--version" {
            let value = tail
                .get(index + 1)
                .ok_or_else(|| CliError::new("--version 需要一个版本号").with_usage(HELP_REMOVE))?;
            version = Some(value.clone());
            index += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--version=") {
            version = Some(value.to_string());
            index += 1;
            continue;
        }
        if arg.starts_with('-') {
            return Err(CliError::new(format!("未知参数 {arg}")).with_usage(HELP_REMOVE));
        }
        if id.is_some() {
            return Err(CliError::new("remove 只接受一个 id").with_usage(HELP_REMOVE));
        }
        id = Some(parse_id(arg, HELP_REMOVE)?);
        index += 1;
    }

    let id = id.ok_or_else(|| CliError::new("缺少 id 参数").with_usage(HELP_REMOVE))?;
    let mut library = Library::open(root)?;
    let removed = library.remove(&id, version.as_deref())?;
    for package in &removed {
        println!(
            "已删除 {} {}（{}）",
            package.id, package.version, package.name
        );
    }
    match library.active(&id) {
        Some(active) => println!("现在启用：{} {}", active.id, active.version),
        None => println!("词典 {} 已没有可用版本", id),
    }
    Ok(())
}

/// `activate`: make one version active.
fn activate_command(root: &Path, tail: &[String]) -> Result<(), CliError> {
    let values = positionals(tail, 2, HELP_ACTIVATE, "activate")?;
    let id = parse_id(&values[0], HELP_ACTIVATE)?;
    let version = &values[1];
    let mut library = Library::open(root)?;
    let activated = library.activate(&id, version)?;
    println!("已启用 {} {}（{}）", id, activated.version, activated.name);
    Ok(())
}

/// `deactivate`: leave an id installed but inactive.
fn deactivate_command(root: &Path, tail: &[String]) -> Result<(), CliError> {
    let values = positionals(tail, 1, HELP_DEACTIVATE, "deactivate")?;
    let id = parse_id(&values[0], HELP_DEACTIVATE)?;
    let mut library = Library::open(root)?;
    let deactivated = library.deactivate(&id)?;
    if deactivated.is_empty() {
        println!("{} 本来就没有启用中的版本", id);
    } else {
        println!("已停用 {}", id);
    }
    Ok(())
}

/// `history`: print the append-only event log.
fn history_command(root: &Path, tail: &[String]) -> Result<(), CliError> {
    if tail.len() > 1 {
        return Err(CliError::new("history 最多接受一个 id 参数").with_usage(HELP_HISTORY));
    }
    let id = match tail.first() {
        Some(raw) => Some(parse_id(raw, HELP_HISTORY)?),
        None => None,
    };
    let library = Library::open(root)?;
    let events = library.history(id.as_ref());
    if events.is_empty() {
        println!("没有历史记录（词典库：{}）", root.display());
        return Ok(());
    }

    match &id {
        Some(id) => println!("{} 的历史记录（{} 条）", id, events.len()),
        None => println!("词典库 {} 的历史记录（{} 条）", root.display(), events.len()),
    }
    let mut table = Table::new(&["TIME", "EVENT", "ID", "VERSION", "DETAIL"]);
    for event in events {
        table.push(vec![
            event.at.clone(),
            event.event.to_string(),
            event.id.to_string(),
            event.version.clone().unwrap_or_else(|| "-".to_string()),
            event.detail.clone().unwrap_or_default(),
        ]);
    }
    table.print();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dict_store::Provider as _;

    #[test]
    fn bytes_are_human_readable() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(190 * 1024 * 1024), "190.0 MB");
    }

    #[test]
    fn root_can_appear_before_or_after_the_command() {
        let args = vec!["--root".to_string(), "D:\\dicts".to_string(), "list".to_string()];
        let (root, rest) = extract_root(&args).expect("root");
        assert_eq!(root, Some(PathBuf::from("D:\\dicts")));
        assert_eq!(rest, vec!["list".to_string()]);

        let args = vec!["list".to_string(), "--root=D:\\dicts".to_string()];
        let (root, rest) = extract_root(&args).expect("root");
        assert_eq!(root, Some(PathBuf::from("D:\\dicts")));
        assert_eq!(rest, vec!["list".to_string()]);
    }

    #[test]
    fn install_flags_become_metadata() {
        let tail: Vec<String> = [
            "pkg",
            "--id",
            "ecdict",
            "--version=2.4.2",
            "--name",
            "ECDICT",
            "--license",
            "MIT",
            "--format",
            "stardict",
            "--entries",
            "770000",
            "--force",
        ]
        .iter()
        .map(|value| value.to_string())
        .collect();
        let args = parse_install_args(&tail, HELP_ADD).expect("parse");
        assert_eq!(args.directory, PathBuf::from("pkg"));
        assert!(args.force);
        assert_eq!(args.meta.id.as_ref().map(SourceId::as_str), Some("ecdict"));
        assert_eq!(args.meta.version.as_deref(), Some("2.4.2"));
        assert_eq!(args.meta.entries, Some(770_000));
    }

    #[test]
    fn unknown_flags_and_bad_numbers_fail() {
        let tail: Vec<String> = ["pkg", "--nope", "x"]
            .iter()
            .map(|value| value.to_string())
            .collect();
        assert!(parse_install_args(&tail, HELP_ADD).is_err());

        let tail: Vec<String> = ["pkg", "--entries", "many"]
            .iter()
            .map(|value| value.to_string())
            .collect();
        assert!(parse_install_args(&tail, HELP_ADD).is_err());

        let tail: Vec<String> = ["pkg", "--id"].iter().map(|v| v.to_string()).collect();
        assert!(parse_install_args(&tail, HELP_ADD).is_err());
    }

    #[test]
    fn tables_render_aligned_rows() {
        let mut table = Table::new(&["A", "BB"]);
        table.push(vec!["1".to_string(), "two".to_string()]);
        table.push(vec!["long value".to_string(), "x".to_string()]);
        let widths = [10usize, 3usize];
        // "A" padded to 10, two spaces between columns, "BB" padded to 3, then
        // the row is trimmed at the end.
        assert_eq!(
            render_row(&table.headers, &widths),
            format!("A{}BB", " ".repeat(11))
        );
        assert_eq!(
            render_row(&table.rows[0], &widths),
            format!("1{}two", " ".repeat(11))
        );
    }

    #[test]
    fn reserved_http_provider_reports_unavailable() {
        let provider = dict_store::HttpProvider::new("https://example.invalid");
        let info = provider.describe();
        assert!(!info.available);
        assert!(info.requires_network);
    }

    #[test]
    fn manifest_helper_is_reachable() {
        // `Manifest` is re-exported for embedders; keep the import honest.
        let manifest = dict_store::Manifest::new();
        assert_eq!(manifest.schema, 1);
    }
}
