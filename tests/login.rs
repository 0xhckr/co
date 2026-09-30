use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::time::Duration;

#[test]
fn device_login_persists_session_and_doctor_and_logout_reuse_it() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("café config");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let api = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        for (path, token, body) in [
            (
                "/api/auth/device/code",
                false,
                json!({"device_code":"fictional-device", "user_code":"TEST", "verification_uri":"http://127.0.0.1:1/authorize", "interval":1, "expires_in":30}),
            ),
            (
                "/api/auth/device/token",
                false,
                json!({"access_token":"fictional-session"}),
            ),
            ("/health", false, json!({"ok":true})),
            (
                "/api/auth/get-session",
                true,
                json!({"user":{"username":"fixture","name":"Fixture","email":"fixture@example.test"}}),
            ),
            ("/api/auth/sign-out", true, json!({"success":true})),
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                let mut chunk = [0; 4096];
                let n = stream.read(&mut chunk).unwrap();
                assert!(n > 0);
                request.extend_from_slice(&chunk[..n]);
            }
            let headers = String::from_utf8_lossy(&request).to_lowercase();
            assert!(headers.lines().next().unwrap().contains(path));
            assert_eq!(
                headers.contains("authorization: bearer fictional-session"),
                token
            );
            let body = body.to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    for command in ["login", "doctor", "logout"] {
        let output = Command::new(env!("CARGO_BIN_EXE_co"))
            .arg(command)
            .env("CO_CONFIG_DIR", &config)
            .env("CO_API_URL", &api)
            .env("NO_PROXY", "127.0.0.1")
            .env("BROWSER", "false")
            .env_remove("CO_AGENT_ID")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{command}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("fictional-session"));
        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(config.join("config.json")).unwrap()).unwrap();
        assert_eq!(
            stored["session_token"],
            if command == "logout" {
                json!(null)
            } else {
                json!("fictional-session")
            }
        );
    }
    server.join().unwrap();
}
