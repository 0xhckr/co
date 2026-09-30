# Contributing

Contributions are welcome.

## Development

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
nix flake check
```

Keep changes focused and add tests for behavior. Do not add analytics, background network calls, credential output, or persistent identifiers without an explicit design decision and documentation.

## CI runners

Linux CI and release jobs use Blacksmith Ubuntu 24.04 runners, with native ARM64 runners for ARM64 releases and package checks. Apple Silicon macOS jobs use Blacksmith macOS 15, retaining a macOS 14 deployment target for released ARM64 binaries. Intel macOS builds and Homebrew checks remain on GitHub's native Intel runners because Blacksmith supports only ARM64 macOS. The Blacksmith GitHub App must have access to this repository for jobs to be provisioned.

Windows x64 jobs use GitHub's `windows-2025` runner and the MSVC target. `scripts/test-windows.ps1` runs compiled Rust tests under a fresh standard-user profile and probes cross-identity config access. It creates temporary local accounts and is intended for an ephemeral elevated CI runner. `scripts/test-windows-managers.ps1` runs real Scoop, WinGet and Chocolatey install/fixture-upgrade/uninstall checks from the exact built ZIP. These native checks gate release publication; retain actual run URLs and source/artifact identities with verification results.

For local Windows development, install Rust's MSVC toolchain, Visual Studio C++ Build Tools and Git for Windows, then run the Cargo checks above. Release builds use the static MSVC CRT. Review branches named `review/windows-*`, pull requests and workflow dispatch trigger CI without creating a release.

## Compatibility

Release targets are listed in the README and installation guide. Windows work targets native x64 (`x86_64-pc-windows-msvc`). Keep Unix permissions and Windows current-user ACLs equally restrictive, preserve configuration overrides and agent/human credential boundaries, and exercise executable resolution through actual native executables. Cross-compilation and generated package fixtures are useful checks, but do not establish native runtime or manager lifecycle success.

## Security changes

Use the private process in SECURITY.md instead of opening a pull request that demonstrates an exploitable vulnerability.
