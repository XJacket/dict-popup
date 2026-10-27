//! End-to-end tests of the `dict-store` binary against a scratch library.
//!
//! Every test drives the real executable (`CARGO_BIN_EXE_dict-store`), so these
//! cover argument parsing, the library state transitions and the printed output
//! together. Scratch directories live under `target/test-tmp` and are removed by
//! `Scratch::drop`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A unique scratch directory, deleted on drop.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp")
            .join(format!("cli-{label}-{}-{unique}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create scratch directory");
        Scratch { root }
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn library(&self) -> PathBuf {
        self.root.join("library")
    }

    /// Create a package directory with one data file and a `source.json`.
    fn package(&self, name: &str, payload: &str) -> PathBuf {
        let directory = self.root.join(name);
        fs::create_dir_all(&directory).expect("create package directory");
        fs::write(directory.join("data.txt"), payload).expect("write package data");
        fs::write(
            directory.join("source.json"),
            format!(
                "{{\"id\":\"ecdict\",\"version\":\"{name}\",\"format\":\"stardict\",\"name\":\"ECDICT {name}\",\"license\":\"MIT\"}}"
            ),
        )
        .expect("write source.json");
        directory
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Run the binary against the scratch library.
fn run(scratch: &Scratch, args: &[&str]) -> Output {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_dict-store"));
    command.arg("--root").arg(scratch.library()).args(args);
    command.output().expect("run dict-store")
}

/// Stdout of a successful run.
fn ok_stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "expected success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The full end-to-end flow required by the spec: add two versions, list,
/// upgrade, rollback, history, remove.
#[test]
fn full_lifecycle_through_the_cli() {
    let scratch = Scratch::new("lifecycle");
    let version_one = scratch.package("pkg-1.0", "one");
    let version_two = scratch.package("pkg-2.0", "two");
    let version_three = scratch.package("pkg-3.0", "three");
    let one = version_one.to_string_lossy().into_owned();
    let two = version_two.to_string_lossy().into_owned();
    let three = version_three.to_string_lossy().into_owned();
    let (one, two, three) = (one.as_str(), two.as_str(), three.as_str());

    // First install: the only version is active.
    let stdout = ok_stdout(&run(
        &scratch,
        &[
            "add", one, "--id", "ecdict", "--version", "1.0", "--name", "ECDICT",
            "--license", "MIT", "--format", "stardict",
        ],
    ));
    assert!(stdout.contains("已安装 ecdict 1.0"), "{stdout}");
    assert!(stdout.contains("启用中"), "{stdout}");
    assert!(scratch.library().join("ecdict-1.0/source.json").is_file());
    assert!(scratch.library().join("library.json").is_file());

    // Second version installed with `add`: inactive, the first stays active.
    let stdout = ok_stdout(&run(
        &scratch,
        &[
            "add", two, "--id", "ecdict", "--version", "2.0", "--name", "ECDICT",
            "--license", "MIT", "--format", "stardict",
        ],
    ));
    assert!(stdout.contains("未启用"), "{stdout}");

    let listed = ok_stdout(&run(&scratch, &["list"]));
    assert!(listed.contains("ecdict"), "{listed}");
    assert!(listed.contains("1.0"), "{listed}");
    assert!(listed.contains("2.0"), "{listed}");
    let active_line = listed
        .lines()
        .find(|line| line.starts_with("ecdict") && line.contains(" 1.0"))
        .expect("active row");
    assert!(active_line.contains('*'), "{active_line}");
    let inactive_line = listed
        .lines()
        .find(|line| line.starts_with("ecdict") && line.contains(" 2.0"))
        .expect("inactive row");
    assert!(!inactive_line.contains('*'), "{inactive_line}");

    // Upgrade: 3.0 becomes active, the older directories stay installed.
    let upgraded = ok_stdout(&run(
        &scratch,
        &[
            "upgrade", three, "--id", "ecdict", "--version", "3.0", "--name", "ECDICT",
            "--license", "MIT", "--format", "stardict",
        ],
    ));
    assert!(upgraded.contains("已升级 ecdict 1.0 → 3.0"), "{upgraded}");
    assert!(scratch.library().join("ecdict-1.0").is_dir());
    assert!(scratch.library().join("ecdict-2.0").is_dir());
    assert!(scratch.library().join("ecdict-3.0").is_dir());

    // Rollback: the newest version below 3.0 (2.0) becomes active.
    let rolled = ok_stdout(&run(&scratch, &["rollback", "ecdict"]));
    assert!(rolled.contains("已回退 ecdict 到 2.0"), "{rolled}");

    // History: every transition is recorded, oldest first.
    let history = ok_stdout(&run(&scratch, &["history", "ecdict"]));
    let installed = history.find("installed").expect("installed event");
    let upgraded = history.find("upgraded").expect("upgraded event");
    let rolled_back = history.find("rolled_back").expect("rollback event");
    assert!(installed < upgraded && upgraded < rolled_back, "{history}");

    // Remove one version; the active one is untouched.
    let removed = ok_stdout(&run(&scratch, &["remove", "ecdict", "--version", "3.0"]));
    assert!(removed.contains("已删除 ecdict 3.0"), "{removed}");
    assert!(!scratch.library().join("ecdict-3.0").exists());
    assert!(scratch.library().join("ecdict-2.0").is_dir());
    let listed = ok_stdout(&run(&scratch, &["list"]));
    let active_line = listed
        .lines()
        .find(|line| line.starts_with("ecdict") && line.contains(" 2.0"))
        .expect("active row");
    assert!(active_line.contains('*'), "{active_line}");

    // Remove the rest: the library is empty again.
    let removed = ok_stdout(&run(&scratch, &["remove", "ecdict"]));
    assert!(removed.contains("已删除 ecdict 1.0"), "{removed}");
    assert!(removed.contains("已删除 ecdict 2.0"), "{removed}");
    let listed = ok_stdout(&run(&scratch, &["list"]));
    assert!(listed.contains("词典库为空"), "{listed}");
}

#[test]
fn add_uses_source_json_and_refuses_duplicates() {
    let scratch = Scratch::new("source-json");
    let package = scratch.package("pkg-1.0", "payload");
    let path = package.to_string_lossy().into_owned();

    // `--format` comes from source.json, the rest from flags.
    let stdout = ok_stdout(&run(
        &scratch,
        &[
            "add", &path, "--id", "ecdict", "--version", "1.0", "--name", "ECDICT",
            "--license", "MIT",
        ],
    ));
    assert!(stdout.contains("已安装 ecdict 1.0"), "{stdout}");

    // Installing the same content again is refused with a readable message.
    let output = run(
        &scratch,
        &[
            "add", &path, "--id", "ecdict", "--version", "1.0", "--name", "ECDICT",
            "--license", "MIT",
        ],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("已经安装"), "{stderr}");
}

#[test]
fn errors_are_reported_with_non_zero_exit_codes() {
    let scratch = Scratch::new("errors");
    // A bare package directory: no source.json, so the licence must come from
    // the command line.
    let bare = scratch.path().join("bare-package");
    fs::create_dir_all(&bare).expect("create bare package");
    fs::write(bare.join("data.txt"), "payload").expect("write data");
    let bare = bare.to_string_lossy().into_owned();

    let output = run(
        &scratch,
        &[
            "add", &bare, "--id", "ecdict", "--version", "1.0", "--name", "ECDICT",
            "--format", "stardict",
        ],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("缺少 license"), "{stderr}");
    assert!(!scratch.library().exists(), "nothing may be written on refusal");

    // Unknown command.
    let output = run(&scratch, &["frobnicate"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("未知命令"), "{stderr}");

    // Rollback of a dictionary that is not installed.
    let output = run(&scratch, &["rollback", "ecdict"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ecdict"), "{stderr}");
}

#[test]
fn help_and_no_arguments_print_usage() {
    let scratch = Scratch::new("help");
    for args in [vec![], vec!["--help"], vec!["add", "--help"]] {
        let output = run(&scratch, &args);
        assert!(output.status.success(), "{args:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("用法"), "{args:?}: {stdout}");
        assert!(stdout.contains("dict-store"), "{args:?}: {stdout}");
    }
}

#[test]
fn activate_deactivate_and_noop_deactivate() {
    let scratch = Scratch::new("activate");
    let version_one = scratch.package("pkg-1.0", "one");
    let version_two = scratch.package("pkg-2.0", "two");
    let one = version_one.to_string_lossy().into_owned();
    let two = version_two.to_string_lossy().into_owned();
    for (path, version) in [(one.as_str(), "1.0"), (two.as_str(), "2.0")] {
        ok_stdout(&run(
            &scratch,
            &[
                "add", path, "--id", "ecdict", "--version", version, "--name", "ECDICT",
                "--license", "MIT", "--format", "stardict",
            ],
        ));
    }

    let stdout = ok_stdout(&run(&scratch, &["activate", "ecdict", "2.0"]));
    assert!(stdout.contains("已启用 ecdict 2.0"), "{stdout}");
    let listed = ok_stdout(&run(&scratch, &["list"]));
    let line = listed
        .lines()
        .find(|line| line.starts_with("ecdict") && line.contains("2.0"))
        .expect("row");
    assert!(line.contains('*'), "{line}");

    let stdout = ok_stdout(&run(&scratch, &["deactivate", "ecdict"]));
    assert!(stdout.contains("已停用 ecdict"), "{stdout}");
    let stdout = ok_stdout(&run(&scratch, &["deactivate", "ecdict"]));
    assert!(stdout.contains("本来就没有启用中"), "{stdout}");
}
