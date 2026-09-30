use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Command, Output, Stdio};
#[cfg(unix)]
use std::time::{SystemTime, UNIX_EPOCH};

fn success(output: Output) -> Output {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn git(root: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", root.join("empty.gitconfig"))
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "false")
        .env("CO_CONFIG_DIR", root)
        .env_remove("CO_AGENT_ID");
    command
}

fn credentials(root: &Path) -> Output {
    let mut child = git(root)
        .args(["credential", "fill"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"protocol=https\nhost=git.co.codes\npath=owner/repo.git\n\n")
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
#[cfg(unix)]
fn linked_credentials_survive_installation_symlink_upgrades() {
    for invocation in ["absolute", "relative", "path"] {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "co-helper-{}-{nonce}-{invocation}",
            std::process::id()
        ));
        // Exercise shell quoting as well as preserving installation symlinks.
        let bin = root.join("user's bin");
        fs::create_dir_all(&bin).unwrap();
        let old = root.join("co-old");
        fs::copy(env!("CARGO_BIN_EXE_co"), &old).unwrap();
        let installed = bin.join("co");
        symlink(&old, &installed).unwrap();
        fs::write(
            root.join("config.json"),
            r#"{"session_token":"test-session"}"#,
        )
        .unwrap();
        success(git(&root).args(["init", "--quiet"]).output().unwrap());
        let path = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .unwrap();
        let executable = match invocation {
            "absolute" => installed.clone(),
            "relative" => Path::new("user's bin/co").to_path_buf(),
            _ => Path::new("co").to_path_buf(),
        };
        success(
            Command::new(executable)
                .args(["link", "owner/repo"])
                .current_dir(&root)
                .env("PATH", path)
                .env("CO_CONFIG_DIR", &root)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .unwrap(),
        );
        let before = success(credentials(&root));
        assert!(String::from_utf8_lossy(&before.stdout).contains("password=test-session"));

        fs::remove_file(&installed).unwrap();
        symlink(env!("CARGO_BIN_EXE_co"), &installed).unwrap();
        fs::remove_file(old).unwrap();
        // Credential lookup no longer has the installation directory on PATH.
        let after = success(credentials(&root));
        assert_eq!(after.stdout, before.stdout);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn linked_credentials_work_from_unicode_paths_and_survive_binary_replacement() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("user's café bin");
    fs::create_dir(&bin).unwrap();
    let installed = bin.join(format!("co{}", std::env::consts::EXE_SUFFIX));
    fs::copy(env!("CARGO_BIN_EXE_co"), &installed).unwrap();
    fs::write(
        root.path().join("config.json"),
        r#"{"session_token":"test-session"}"#,
    )
    .unwrap();
    fs::write(root.path().join("empty.gitconfig"), "").unwrap();
    success(git(root.path()).args(["init", "--quiet"]).output().unwrap());
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    success(
        Command::new("co")
            .args(["link", "owner/repo"])
            .current_dir(root.path())
            .env("PATH", path)
            .env("CO_CONFIG_DIR", root.path())
            .env_remove("CO_AGENT_ID")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", root.path().join("empty.gitconfig"))
            .output()
            .unwrap(),
    );
    let before = success(credentials(root.path()));
    assert!(String::from_utf8_lossy(&before.stdout).contains("password=test-session"));
    let config = success(
        git(root.path())
            .args(["config", "--local", "--list"])
            .output()
            .unwrap(),
    );
    assert!(!String::from_utf8_lossy(&config.stdout).contains("test-session"));
    fs::remove_file(&installed).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_co"), installed).unwrap();
    assert_eq!(success(credentials(root.path())).stdout, before.stdout);
    let mut child = git(root.path())
        .args(["credential", "fill"])
        .env("CO_AGENT_ID", "missing-agent")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"protocol=https\nhost=git.co.codes\npath=owner/repo.git\n\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("test-session"));
}
