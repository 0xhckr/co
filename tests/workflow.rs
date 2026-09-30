use std::ffi::OsString;
use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    if std::env::var_os("CO_WORKFLOW_CHECKER_PROBE").is_some() {
        let expected = [
            OsString::from("run"),
            std::env::var_os("EXPECTED_CHECKER").unwrap(),
            OsString::from(".co/workflows/example.ts"),
        ];
        if std::env::args_os().skip(1).collect::<Vec<_>>() != expected {
            eprintln!("incorrect workflow checker arguments");
            std::process::exit(3);
        }
        println!("diagnostic: bad selector");
        std::process::exit(1);
    }
    workflow_check_passes_one_file_as_an_argument_and_returns_failure_status();
    println!("workflow checker argument, diagnostic and failure-status probe passed");
}

fn workflow_check_passes_one_file_as_an_argument_and_returns_failure_status() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("co-check-{}-{nonce}", std::process::id()));
    let package = root.join("node_modules/@cocodes/workflows/src");
    fs::create_dir_all(&package).unwrap();
    fs::write(
        package.join("check.ts"),
        "// The CLI must pass this file to Bun.",
    )
    .unwrap();
    let bin = root.join("bin");
    fs::create_dir(&bin).unwrap();
    // Copy this native test executable so Command::new("bun") resolves bun.exe
    // on Windows through the same boundary as the real Bun executable.
    fs::copy(
        std::env::current_exe().unwrap(),
        bin.join(format!("bun{}", std::env::consts::EXE_SUFFIX)),
    )
    .unwrap();
    let checker = fs::canonicalize(package.join("check.ts")).unwrap();
    #[cfg(windows)]
    let checker = checker
        .to_str()
        .unwrap()
        .strip_prefix(r"\\?\")
        .unwrap_or(checker.to_str().unwrap())
        .to_owned();
    let output = Command::new(env!("CARGO_BIN_EXE_co"))
        .args(["workflow", "check", ".co/workflows/example.ts"])
        .current_dir(&root)
        .env("PATH", &bin)
        .env("CO_WORKFLOW_CHECKER_PROBE", "1")
        // macOS resolves /var to /private/var when the CLI calls current_dir().
        .env("EXPECTED_CHECKER", checker)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("diagnostic: bad selector"),
        "status: {}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("workflow check failed"));
    fs::remove_dir_all(&root).unwrap();
}
