use serde_json::json;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const STAGING_API: &str = "https://api.codevved.com";

fn cli(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_co"));
    command
        .current_dir(root)
        .env("CO_CONFIG_DIR", root)
        .env_remove("CO_API_URL")
        .env_remove("CO_AGENT_ID")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", root.join("empty.gitconfig"));
    command
}

fn git(root: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", root.join("empty.gitconfig"))
        .args(args)
        .output()
        .unwrap()
}

fn success(output: Output) -> Output {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn staging_link_selects_its_git_host_and_path_scoped_helper() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("empty.gitconfig"), "").unwrap();
    fs::write(
        root.path().join("config.json"),
        json!({"api_url": STAGING_API}).to_string(),
    )
    .unwrap();
    success(git(root.path(), &["init", "--quiet"]));
    success(
        cli(root.path())
            .args(["link", "--no-jj", "owner/repo"])
            .output()
            .unwrap(),
    );
    let remote = success(git(root.path(), &["remote", "get-url", "origin"]));
    assert_eq!(
        String::from_utf8_lossy(&remote.stdout).trim(),
        "https://git.codevved.com/owner/repo.git"
    );
    let scoped = success(git(
        root.path(),
        &[
            "config",
            "--get",
            "credential.https://git.codevved.com.useHttpPath",
        ],
    ));
    assert_eq!(String::from_utf8_lossy(&scoped.stdout).trim(), "true");
    let helpers = success(git(
        root.path(),
        &[
            "config",
            "--get-all",
            "credential.https://git.codevved.com.helper",
        ],
    ));
    let helpers = String::from_utf8(helpers.stdout).unwrap();
    assert!(helpers.starts_with('\n'));
    assert!(helpers.trim().ends_with("git-credential"));
    assert!(
        !git(
            root.path(),
            &[
                "config",
                "--get-all",
                "credential.https://git.co.codes.helper"
            ],
        )
        .status
        .success()
    );
    success(
        cli(root.path())
            .env("CO_API_URL", "https://api.co.codes")
            .args(["link", "--no-jj", "-u", "production", "owner/repo"])
            .output()
            .unwrap(),
    );
    let remote = success(git(root.path(), &["remote", "get-url", "production"]));
    assert_eq!(
        String::from_utf8_lossy(&remote.stdout).trim(),
        "https://git.co.codes/owner/repo.git"
    );
}

#[test]
fn staging_clone_configures_the_same_host_as_its_remote() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("config.json"),
        json!({"api_url": STAGING_API}).to_string(),
    )
    .unwrap();
    success(git(
        root.path(),
        &["init", "--bare", "--quiet", "source.git"],
    ));
    // Exercise the actual clone/setup path against a local transport fixture.
    // The persisted origin still identifies the selected staging Git host.
    let source = reqwest::Url::from_directory_path(root.path().join("source.git")).unwrap();
    success(git(
        root.path(),
        &[
            "config",
            "--global",
            &format!("url.{source}.insteadOf"),
            "https://git.codevved.com/owner/repo.git",
        ],
    ));
    success(
        cli(root.path())
            .args(["clone", "--no-jj", "owner/repo", "checkout"])
            .output()
            .unwrap(),
    );
    let checkout = root.path().join("checkout");
    let origin = success(git(&checkout, &["config", "--get", "remote.origin.url"]));
    assert_eq!(
        String::from_utf8_lossy(&origin.stdout).trim(),
        "https://git.codevved.com/owner/repo.git"
    );
    let scoped = success(git(
        &checkout,
        &[
            "config",
            "--get",
            "credential.https://git.codevved.com.useHttpPath",
        ],
    ));
    assert_eq!(String::from_utf8_lossy(&scoped.stdout).trim(), "true");
    let helpers = success(git(
        &checkout,
        &[
            "config",
            "--get-all",
            "credential.https://git.codevved.com.helper",
        ],
    ));
    assert!(String::from_utf8_lossy(&helpers.stdout).starts_with('\n'));
}

fn credential(root: &Path, api: Option<&str>, request: &str, agent: &str) -> Output {
    let mut command = cli(root);
    command
        .args(["git-credential", "get"])
        .env("CO_AGENT_ID", agent)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(api) = api {
        command.env("CO_API_URL", api);
    }
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn staging_agent_credentials_preserve_host_repository_identity_and_expiry_denials() {
    let root = tempfile::tempdir().unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    fs::write(
        root.path().join("config.json"),
        json!({
            "api_url": STAGING_API,
            "session_token": "human-secret",
            "agent_grants": [{
                "agent_id": "selected", "owner": "owner", "repo": "repo",
                "grant_id": "expired-grant", "lineage_token": "expired-lineage",
                "operations": ["pull", "push"], "expires_unix": now - 1
            }, {
                "agent_id": "other", "owner": "owner", "repo": "repo",
                "grant_id": "other-grant", "lineage_token": "other-lineage",
                "operations": ["pull", "push"], "expires_unix": now + 600
            }]
        })
        .to_string(),
    )
    .unwrap();
    for (host, path, api, expected) in [
        (
            "git.codevved.com",
            "owner/repo.git",
            None,
            "no live push grant",
        ),
        (
            "git.codevved.com:443",
            "owner/repo.git",
            Some(STAGING_API),
            "no live push grant",
        ),
        (
            "git.codevved.com",
            "owner/other.git",
            None,
            "no live push grant",
        ),
        (
            "git.co.codes",
            "owner/repo.git",
            None,
            "outside git.codevved.com",
        ),
        (
            "git.codevved.com",
            "owner/repo.git",
            Some("https://api.co.codes"),
            "outside git.co.codes",
        ),
        (
            "git.codevved.com",
            "owner/repo.git",
            Some("https://api.codevved.com.evil.example"),
            "outside git.co.codes",
        ),
        ("git.codevved.com:444", "owner/repo.git", None, "outside"),
        (
            "git.codevved.com.evil.example",
            "owner/repo.git",
            None,
            "outside",
        ),
        ("evil.example", "owner/repo.git", None, "outside"),
        (
            "git.codevved.com",
            "owner/repo.git/extra",
            None,
            "repository must be written",
        ),
        (
            "git.codevved.com",
            "owner/../repo.git",
            None,
            "repository must be written",
        ),
    ] {
        let request = format!("protocol=https\nhost={host}\npath={path}\n\n");
        let output = credential(root.path(), api, &request, "selected");
        assert!(!output.status.success(), "{host} {path}");
        assert_eq!(output.stdout, b"quit=true\n\n");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{host} {path}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains("human-secret"));
    }
}

#[test]
fn foreign_git_hosts_are_denied_before_loading_credentials() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("config.json"), "invalid config JSON").unwrap();
    for host in ["evil.example", "git.co.codes", "git.codevved.com:444"] {
        let output = credential(
            root.path(),
            Some(STAGING_API),
            &format!("protocol=https\nhost={host}\npath=owner/repo.git\n\n"),
            "selected",
        );
        assert!(!output.status.success());
        assert_eq!(output.stdout, b"quit=true\n\n");
        assert!(String::from_utf8_lossy(&output.stderr).contains("outside"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("invalid configuration"));
    }
}

#[test]
fn human_credentials_follow_only_the_selected_host() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("config.json"),
        json!({"api_url": STAGING_API, "session_token": "staging-human-secret"}).to_string(),
    )
    .unwrap();
    for (host, expected) in [
        (
            "git.codevved.com",
            "username=co\npassword=staging-human-secret\n\n",
        ),
        ("git.co.codes", ""),
        ("evil.example", ""),
    ] {
        let mut child = cli(root.path())
            .args(["git-credential", "get"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(format!("protocol=https\nhost={host}\npath=owner/repo.git\n\n").as_bytes())
            .unwrap();
        let output = success(child.wait_with_output().unwrap());
        assert_eq!(output.stdout, expected.as_bytes());
    }
}
