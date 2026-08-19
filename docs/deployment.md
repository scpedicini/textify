# Builds, packages, and GitHub Releases

Textify ships as native Rust/GPUI code. It does not bundle a browser engine or Electron runtime.
Production builds are created on the matching operating system so each release is both compiled and
tested with its native windowing and graphics stack.

## Releasing

`prod` is the release branch. Work lands on `main` as usual; a release happens when `main` is merged
into `prod` through a pull request.

```sh
gh pr create --base prod --head main --title "Release" --label release:minor
```

The `release:*` label on that pull request decides the new version:

| Label | Effect on `0.4.2` |
| --- | --- |
| `release:major` | `1.0.0` |
| `release:minor` | `0.5.0` |
| `release:patch`, or no label | `0.4.3` |

Three workflows implement this:

- [`prod-pr-check.yml`](../.github/workflows/prod-pr-check.yml) runs on every pull request that
  targets `prod`. It builds the real release packages on all three platforms and reports which
  version part the label will raise, so merge-time failures are rare.
- [`prod-release.yml`](../.github/workflows/prod-release.yml) runs after the merge. Its first job
  reads the merged pull request's labels, runs [`bump-version.sh`](../scripts/bump-version.sh),
  and commits `chore(release): v<version>` to `prod`. Every platform is then rebuilt from that exact
  commit, so the version inside the binaries matches the release.
- [`build-platforms.yml`](../.github/workflows/build-platforms.yml) is the shared build called by
  both of the above:

| Runner | Release files | Verification |
| --- | --- | --- |
| Ubuntu 22.04 x64 | `textify-<version>-linux-x64.tar.gz`, `textify-<version>-linux-amd64.deb` | Complete test suite, package inspection, and an Xvfb/software-Vulkan GUI startup smoke test |
| Windows Server 2022 x64 | `textify-<version>-windows-x64.zip`, `textify-<version>-windows-x64-setup.exe` | Complete test suite, silent installer test, and a ten-second launch of the installed executable |
| macOS 15 ARM64 | `textify-<version>-macos-universal.zip` | Complete native test suite, validation that the app contains ARM64 and Intel x64 slices, and a ten-second packaged-app launch |

The publish job runs only after all three builds succeed. It downloads the verified files, writes
`SHA256SUMS`, and publishes a GitHub Release tagged `v<version>`. The tag is created by that release,
so a failed build never leaves a tag behind. The built-in, short-lived `GITHUB_TOKEN` is used; no
personal access token is needed, and write access is granted only to the two jobs that need it.

A release can also be started by hand from the Actions page with the **Production release** workflow,
which takes the bump level as an input.

The version is raised before the builds run, because the number has to be inside the binaries and
package names. If a build then fails, `prod` keeps the raised version but no tag or release is
created, so the next successful release simply skips that number. Nothing needs to be cleaned up.

### One-time setup

GitHub Actions must be enabled, workflows must be permitted to write repository contents
(Settings → Actions → General → Workflow permissions), and the labels must exist:

```sh
gh label create release:major --color B60205 --description "Merging raises the major version"
gh label create release:minor --color 0E8A16 --description "Merging raises the minor version"
gh label create release:patch --color 1D76DB --description "Merging raises the patch version"
git switch -c prod && git push -u origin prod
```

Branch protection on `prod` should require the pull request checks, but must allow the
`github-actions[bot]` push that carries the version bump. If protection blocks that push, either
exempt the bot or drop the "restrict who can push" rule for `prod`.

### Version bookkeeping

Only `Cargo.toml` and `Cargo.lock` carry the version, and only `prod` is bumped. `main` keeps the
older number until the next release merge brings the bump commit back in the merge base, which
resolves without conflict because `main` never edits that line. If you prefer the numbers to match,
merge `prod` back into `main` after a release.

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

Useful checks before opening a release pull request are:

```sh
cargo fmt --all -- --check
cargo test --locked --workspace --all-targets -- --test-threads=1
cargo clippy --locked --workspace --all-targets -- -D warnings
actionlint .github/workflows/*.yml
shellcheck scripts/bump-version.sh scripts/package-linux.sh scripts/smoke-linux.sh
```

Windows execution cannot be reproduced natively on macOS. The workflow handles that gap by running
tests on Windows, installing the generated setup executable into the ephemeral runner, launching the
installed editor, and withholding the Release if any of those steps fail.
