use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);

impl Fixture {
    fn new(config: Value) -> Self {
        let path = std::env::temp_dir().join(format!(
            "co-month-access-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("config.json"), config.to_string()).unwrap();
        Self(path)
    }

    fn config(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.0.join("config.json")).unwrap()).unwrap()
    }

    fn command(&self, api: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_co"));
        command
            .env("CO_CONFIG_DIR", &self.0)
            .env("CO_API_URL", api)
            .env("NO_PROXY", "127.0.0.1")
            .env("BROWSER", "/bin/true")
            .env_remove("CO_AGENT_ID");
        command
    }

    fn credential(&self, api: &str) -> Output {
        let mut process = self
            .command(api)
            .args(["git-credential", "get"])
            .env("CO_AGENT_ID", "selected")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        process
            .stdin
            .take()
            .unwrap()
            .write_all(b"protocol=https\nhost=git.co.codes\npath=owner/repo.git\n\n")
            .unwrap();
        process.wait_with_output().unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Step {
    method: &'static str,
    path: &'static str,
    authorization: &'static str,
    status: u16,
    body: Value,
    ttl: Option<u64>,
}

fn server(steps: Vec<Step>) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let api = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        for step in steps {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "missing request {}", step.path);
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let header_end = loop {
                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    break end + 4;
                }
                let mut chunk = [0; 4096];
                let read = stream.read(&mut chunk).unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&chunk[..read]);
            };
            let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
            assert!(headers.starts_with(&format!("{} {} HTTP/1.1\r\n", step.method, step.path)));
            assert!(headers.to_lowercase().contains(&format!(
                "authorization: {}",
                step.authorization.to_lowercase()
            )));
            let length = headers
                .lines()
                .find_map(|line| {
                    line.to_lowercase()
                        .strip_prefix("content-length: ")
                        .and_then(|value| value.parse::<usize>().ok())
                })
                .unwrap_or(0);
            while bytes.len() < header_end + length {
                let mut chunk = [0; 4096];
                let read = stream.read(&mut chunk).unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&chunk[..read]);
            }
            if let Some(ttl) = step.ttl {
                let submission: Value =
                    serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                assert_eq!(submission["ttlSeconds"], ttl);
                assert_eq!(submission["operations"], json!(["pull", "push"]));
                assert!(headers.to_lowercase().contains("idempotency-key: co-"));
            }
            let body = step.body.to_string();
            write!(stream, "HTTP/1.1 {} Response\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", step.status, body.len()).unwrap();
        }
    });
    (api, handle)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn approved() -> Step {
    Step {
        method: "GET",
        path: "/grant-requests/month-request",
        authorization: "Poll poll-secret",
        status: 200,
        body: json!({"status":"approved","grantId":"month-grant","lineageToken":"lineage-secret"}),
        ttl: None,
    }
}

fn token(value: &str) -> Step {
    Step {
        method: "POST",
        path: "/grants/month-grant/token",
        authorization: "Lineage lineage-secret",
        status: 200,
        body: json!({"accessToken":value}),
        ttl: None,
    }
}

fn pending(expires: u64, approval_deadline: Option<u64>) -> Value {
    let mut request = json!({"id":"month-request","agent_id":"selected","owner":"owner","repo":"repo","poll_token":"poll-secret","operations":["pull","push"],"requested_expires_unix":expires});
    if let Some(deadline) = approval_deadline {
        request["approval_expires_unix"] = json!(deadline);
    }
    request
}

#[test]
fn reports_month_ttl_and_rejects_invalid_values_before_submission() {
    let f = Fixture::new(json!({}));
    let help = f
        .command("http://127.0.0.1:1")
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(
        String::from_utf8_lossy(&help.stdout)
            .contains("grant lifetime, 300-2592000 (30 days; default: 3600)")
    );
    for (ttl, expected) in [
        (
            "299",
            "--ttl must be between 300 and 2592000 seconds (30 days)",
        ),
        (
            "2592001",
            "--ttl must be between 300 and 2592000 seconds (30 days)",
        ),
        ("3600.5", "--ttl must be an integer"),
        ("18446744073709551616", "--ttl must be an integer"),
    ] {
        let result = f
            .command("http://127.0.0.1:1")
            .args(["access", "request", "--ttl", ttl, "owner/repo"])
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains(expected));
        assert!(result.stdout.is_empty());
        assert_eq!(f.config(), json!({}));
    }
}

#[test]
fn requests_month_access_persists_authoritative_expiry_and_refreshes_each_git_use() {
    let expires = now() + 2_592_000;
    let (api, server) = server(vec![
        Step {
            method: "POST",
            path: "/agents/selected/request-capabilities",
            authorization: "Bearer human-bootstrap",
            status: 201,
            body: json!({"requestCapability":"request-secret"}),
            ttl: None,
        },
        Step {
            method: "POST",
            path: "/grant-requests",
            authorization: "Agent-Request request-secret",
            status: 202,
            body: json!({"id":"month-request","pollToken":"poll-secret","approvalUrl":"http://example.invalid/approve","expiresAtUnix":expires,"approvalExpiresAtUnix":now()+86_400}),
            ttl: Some(2_592_000),
        },
        approved(),
        token("first-short-lived-jwt"),
        token("refreshed-short-lived-jwt"),
    ]);
    let f = Fixture::new(
        json!({"session_token":"human-bootstrap","agents":[{"id":"selected","name":"Helper"}]}),
    );
    let result = f
        .command(&api)
        .args([
            "access",
            "request",
            "--agent",
            "selected",
            "--push",
            "--ttl",
            "2592000",
            "owner/repo",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let saved = f.config();
    assert_eq!(saved["agent_grants"][0]["expires_unix"], expires);
    assert_eq!(saved["agent_grants"][0]["lineage_token"], "lineage-secret");
    assert_eq!(saved["pending_agent_requests"], json!([]));
    assert_eq!(saved["pending_agent_submissions"], json!([]));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(f.0.join("config.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    for expected in ["first-short-lived-jwt", "refreshed-short-lived-jwt"] {
        let credential = f.credential(&api);
        assert!(credential.status.success());
        assert!(
            String::from_utf8_lossy(&credential.stdout).contains(&format!("password={expected}"))
        );
        assert!(!String::from_utf8_lossy(&credential.stdout).contains("human-bootstrap"));
        assert_eq!(
            f.config(),
            saved,
            "refresh never persists an access JWT or shortens the lineage"
        );
    }
    server.join().unwrap();
}

#[test]
fn resumes_approved_month_access_after_one_day_and_reads_legacy_one_day_requests() {
    for (expires, deadline) in [
        (now() + 29 * 86_400, Some(now() - 3600)),
        (now() + 86_400, None),
    ] {
        let (api, server) = server(vec![approved()]);
        let f = Fixture::new(json!({"pending_agent_requests":[pending(expires, deadline)]}));
        let result = f
            .command(&api)
            .args(["access", "wait", "month-request"])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(f.config()["agent_grants"][0]["expires_unix"], expires);
        server.join().unwrap();
    }
}

#[test]
fn revoked_month_grants_and_expired_roots_never_use_human_credentials() {
    let mut rejected = token("unused");
    rejected.status = 404;
    rejected.body = json!({"error":"not_found"});
    let (api, server) = server(vec![rejected]);
    let f = Fixture::new(
        json!({"session_token":"human-secret","agent_grants":[{"agent_id":"selected","owner":"owner","repo":"repo","grant_id":"month-grant","lineage_token":"lineage-secret","operations":["push"],"expires_unix":now()+29*86_400}]}),
    );
    let result = f.credential(&api);
    assert!(!result.status.success());
    assert_eq!(String::from_utf8_lossy(&result.stdout), "quit=true\n\n");
    assert!(!String::from_utf8_lossy(&result.stderr).contains("human-secret"));
    server.join().unwrap();
    let mut expired = f.config();
    expired["agent_grants"][0]["expires_unix"] = json!(now() - 1);
    std::fs::write(f.0.join("config.json"), expired.to_string()).unwrap();
    let result = f.credential("http://127.0.0.1:1");
    assert!(!result.status.success());
    assert_eq!(String::from_utf8_lossy(&result.stdout), "quit=true\n\n");
    assert!(String::from_utf8_lossy(&result.stderr).contains("no live push grant"));
}

#[test]
fn separates_expired_pending_approval_from_a_future_grant_expiry() {
    let mut step = approved();
    step.body = json!({"status":"pending"});
    let (api, server) = server(vec![step]);
    let f =
        Fixture::new(json!({"pending_agent_requests":[pending(now()+29*86_400, Some(now()-1))]}));
    let result = f.command(&api).args(["access", "wait"]).output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("access request expired"));
    assert_eq!(f.config()["pending_agent_requests"], json!([]));
    assert_eq!(f.config()["agent_grants"], json!([]));
    server.join().unwrap();
}
