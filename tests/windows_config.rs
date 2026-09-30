#![cfg(windows)]

use std::fs;
use std::process::Command;

#[test]
fn config_rewrites_remain_owner_only_and_concurrent_writers_serialize() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("café credentials");
    let command = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_co"));
        command.arg("logout").env("CO_CONFIG_DIR", &config);
        command
    };
    assert!(command().status().unwrap().success());
    let mut processes: Vec<_> = (0..12).map(|_| command().spawn().unwrap()).collect();
    for process in &mut processes {
        assert!(process.wait().unwrap().success());
    }
    let stored: serde_json::Value =
        serde_json::from_slice(&fs::read(config.join("config.json")).unwrap()).unwrap();
    assert!(stored["session_token"].is_null());
    assert_eq!(fs::read_dir(&config).unwrap().count(), 2);
    let output = Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", r#"
        $ErrorActionPreference = 'Stop'
        $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
        foreach ($path in @($env:CO_CONFIG_DIR, "$env:CO_CONFIG_DIR\config.json", "$env:CO_CONFIG_DIR\config.json.lock")) {
            $acl = Get-Acl -LiteralPath $path
            if (!$acl.AreAccessRulesProtected) { throw 'Inherited credential ACL' }
            if ($acl.GetOwner([Security.Principal.SecurityIdentifier]).Value -ne $sid) { throw 'Wrong owner' }
            $rules = @($acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier]))
            if ($rules.Count -ne 1 -or $rules[0].IdentityReference.Value -ne $sid -or $rules[0].AccessControlType -ne 'Allow' -or $rules[0].FileSystemRights -ne 'FullControl') { throw 'Credential ACL grants another identity' }
        }
    "#])
    // Windows PowerShell must resolve its own modules, not PowerShell 7's
    // incompatible Security module inherited from the CI parent process.
    .env_remove("PSModulePath")
    .env("CO_CONFIG_DIR", &config).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
