# Builds, packages, and GitHub Releases

Textify ships as native Rust/GPUI code. It does not bundle a browser engine or Electron runtime.
Production builds are created on the matching operating system so each release is both compiled and
tested with its native windowing and graphics stack.

## Automated production releases

[`prod-release.yml`](../.github/workflows/prod-release.yml) runs after every push to the `prod`
branch. It can also be started manually from the Actions page. Three jobs run in parallel:

| Runner | Release files | Verification |
| --- | --- | --- |
| Ubuntu 22.04 x64 | `textify-<version>-linux-x64.tar.gz`, `textify-<version>-linux-amd64.deb` | Complete test suite, package inspection, and an Xvfb/software-Vulkan GUI startup smoke test |
| Windows Server 2022 x64 | `textify-<version>-windows-x64.zip`, `textify-<version>-windows-x64-setup.exe` | Complete test suite, silent installer test, and a ten-second launch of the installed executable |
| macOS 15 ARM64 | `textify-<version>-macos-universal.zip` | Complete native test suite, validation that the app contains ARM64 and Intel x64 slices, and a ten-second packaged-app launch |

The publish job runs only after all three jobs succeed. It downloads the verified files, creates
`SHA256SUMS`, and publishes a GitHub Release using a unique tag such as
`prod-42-a1b2c3d`. The built-in, short-lived `GITHUB_TOKEN` is used; no personal access token is
needed. The workflow grants write access only to the publish job.

Before the first release, make sure GitHub Actions is enabled for the repository and that repository
or organization policy permits workflows to write repository contents. Then create or update the
branch normally:

```sh
git switch -c prod       # only when the branch does not exist yet
git push -u origin prod
```

Afterward, any push to `prod` starts another release. A failed build, test, package inspection,
installer run, or smoke test prevents publication. The `prod` concurrency group allows only one
release run at a time and retains the newest pending push. Branch protection requiring a review
before updates to `prod` is recommended when releases become public-facing.

## Signing status

The automated artifacts are currently **unsigned** because the repository has no Apple Developer ID
or Windows code-signing certificate. They are real native release binaries, but macOS Gatekeeper
and Windows SmartScreen can warn users who download them. The release notes state this explicitly,
and `SHA256SUMS` provides an integrity check.

For broad public distribution, add signing as a later hardening step:

- Import an Apple Developer ID Application certificate on the macOS runner, sign the copied app,
  notarize its ZIP, and staple the result. `package-macos-release.sh` already honors
  `TEXTIFY_MACOS_SIGN_IDENTITY` for local or CI signing, but notarization credentials are not stored
  in this repository.
- Import a trusted Authenticode certificate on the Windows runner and sign both `Textify.exe` and
  the setup executable before upload.

Do not commit certificate files or passwords. Store them as GitHub Actions secrets or use an
external signing service.

## Local packaging

The checked-in `rust-toolchain.toml` pins Rust 1.93.1 so local and hosted builds use the same
compiler. All package scripts write generated output under ignored `dist/` and use the Cargo release
profile with thin LTO and one code-generation unit.

### Linux x64 in Docker

On any Docker host, including Apple Silicon macOS:

```sh
./scripts/test-linux-docker.sh
```

The script forces `linux/amd64`, builds from Ubuntu 22.04, runs tests, packages both formats, starts
the GUI under Xvfb, and copies verified output to `dist/linux-docker/`. Docker BuildKit caches the
Rust registry and target directory, so later runs are incremental.

On an Ubuntu/Debian development machine, the equivalent native commands are:

```sh
./scripts/install-linux-build-deps.sh
cargo test --locked --workspace --all-targets -- --test-threads=1
./scripts/package-linux.sh
./scripts/smoke-linux.sh ./target/release/textify
```

Install the Debian package with `sudo apt install ./textify-<version>-linux-amd64.deb`, or extract
the tarball and run `bin/textify`. The Debian package installs the executable, scalable icon,
desktop entry, AppStream metadata, and Open With MIME declarations.

### Windows x64

Run from PowerShell on a Windows machine with Rust, the MSVC build tools/Windows SDK, and Inno Setup
6 installed:

```powershell
./scripts/package-windows.ps1
```

The portable ZIP contains `Textify.exe`. The per-user installer writes to
`%LOCALAPPDATA%\Programs\Textify`, requires no administrator privileges, adds Start-menu and
optional desktop shortcuts, and registers Textify in Open With for common text/source extensions.
The executable contains the multi-resolution `Textify.ico` resource and uses the static MSVC C
runtime. GitHub's fixed `windows-2022` image provides the required SDK shader compiler and Inno
Setup, so the hosted Windows job is the authoritative Windows build and runtime test.

### macOS

Create the distributable universal bundle and ZIP with:

```sh
./scripts/package-macos-release.sh
```

Unlike the development bundle, the distribution app contains a copied universal Mach-O and is
self-contained. Set `TEXTIFY_MACOS_ARCHES=aarch64-apple-darwin` for a faster ARM64-only local check.
The default builds both `aarch64-apple-darwin` and `x86_64-apple-darwin` and combines them with
`lipo`.

The existing local/Raycast workflow remains available:

```sh
./scripts/install-local.sh
```

It installs development symlinks at `~/Applications/Textify.app` and `~/bin/textify`. Those links
are convenient for this checkout but are not used in GitHub Releases. Finder can deliver registered
text documents to either the local or distribution app through Open With.

## Manual verification

Useful checks before pushing `prod` are:

```sh
cargo fmt --all -- --check
cargo test --locked --workspace --all-targets -- --test-threads=1
cargo clippy --locked --workspace --all-targets -- -D warnings
actionlint .github/workflows/prod-release.yml
shellcheck scripts/*.sh
```

Windows execution cannot be reproduced natively on macOS. The workflow handles that gap by running
tests on Windows, installing the generated setup executable into the ephemeral runner, launching the
installed editor, and withholding the Release if any of those steps fail.
