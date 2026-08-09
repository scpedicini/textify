# Local release and Raycast deployment

Textify's local deployment is a native optimized ARM64 executable inside the standard macOS app
bundle layout. It does not package a browser runtime or an Electron-style helper process.

## Install or update

From the repository, run:

```sh
./scripts/install-local.sh
```

This command performs a Cargo release build with optimization level 3, thin link-time optimization,
and one code-generation unit. It creates `target/release/Textify.app`, then installs two symlinks:

- `~/Applications/Textify.app` → the generated local app bundle
- `~/bin/textify` → the generated release executable

The executable inside the development app bundle is also a relative symlink to that release
binary. Consequently, any later `cargo build --release --bin textify` updates both the command-line
and app launch targets in place. Re-run `install-local.sh` when the package version or bundle
metadata or icon changes. The editable icon source is `packaging/Textify.svg`; the bundle consumes
the committed `packaging/Textify.icns`. Run `scripts/build-icon.sh` after editing the SVG to
regenerate every macOS icon size.

The bundle is deliberately local and unsigned. A build for another Mac should copy the executable
into the bundle, code sign with a Developer ID, notarize, and staple it instead of using the
development symlink.

## Raycast

The installer prints the repository's `raycast` directory. In Raycast:

1. Open **Settings → Script Commands**.
2. Choose **Add Script Directory** and select `/Users/shaun/dev/textify/raycast`.
3. Search for **Textify** in Raycast. Optionally use **Configure Command** to assign a hotkey.

The Script Command asks macOS to open `~/Applications/Textify.app`. Launch Services focuses the
existing app when it is already running, avoiding the duplicate raw processes that direct shell
launches can create.

For terminal use, run `textify` after ensuring `~/bin` is on `PATH`. Development still uses
`cargo run`; Cargo's `default-run = "textify"` keeps that command unambiguous even though the
repository also contains the performance binary.

## Verification

The packaging scripts refuse to overwrite a non-symlink at either installation path. They also
validate `Info.plist` and confirm that the bundle executable resolves to an executable Mach-O. To
inspect an installation manually:

```sh
plutil -lint target/release/Textify.app/Contents/Info.plist
file target/release/Textify.app/Contents/MacOS/Textify
file target/release/Textify.app/Contents/Resources/Textify.icns
readlink ~/bin/textify
readlink ~/Applications/Textify.app
```
