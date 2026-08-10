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
and app launch targets in place. A process that is already running keeps its old executable in
memory, so quit Textify and launch it again after rebuilding. Use `scripts/build-release.sh` when
updating the current checkout; re-run `install-local.sh` when the package version, bundle metadata,
icon, or installation links change. The editable icon source is `packaging/Textify.svg`; the bundle
consumes the committed `packaging/Textify.icns`. Run `scripts/build-icon.sh` after editing the SVG
to regenerate every macOS icon size.

The bundle is deliberately local and unsigned. A build for another Mac should copy the executable
into the bundle, code sign with a Developer ID, notarize, and staple it instead of using the
development symlink.

## Raycast

The installer prints the repository's `raycast` directory. In Raycast:

1. Open **Settings → Script Commands**.
2. Choose **Add Script Directory** and select `/Users/shaun/dev/textify/raycast`.
3. Search for **Textify** in Raycast. Optionally use **Configure Command** to assign a hotkey.

Keep `/Users/shaun/dev/textify/raycast/textify.sh`. It is not a second build and it does not invoke
Cargo. It is the stable Raycast entry point for this optimized release chain:

```text
raycast/textify.sh
  -> open ~/Applications/Textify.app
  -> target/release/Textify.app
  -> target/release/textify (Cargo release profile)
```

The Script Command asks Launch Services to open the app bundle. This focuses an existing Textify
window instead of creating duplicate raw processes. Pointing Raycast directly at the Mach-O binary
would lose that application lifecycle behavior; relying only on Raycast's application index would
also give up the explicit Script Command and its stable configurable hotkey. The script therefore
remains the preferred setup.

The command launches the already-built binary and intentionally does not compile during a Raycast
invocation. After code changes, run `scripts/build-release.sh`, quit any running Textify instance,
and invoke Textify from Raycast again.

For terminal use, run `textify` after ensuring `~/bin` is on `PATH`. Development still uses
`cargo run`; Cargo's `default-run = "textify"` keeps that command unambiguous even though the
repository also contains the performance binary.

## Finder and Open With

The app bundle registers as an editor for plain text, source code, JSON, XML, HTML, CSS, and shell
scripts. Finder can deliver files both when Textify is closed and when it is already running.

The local installer places the app in the current user's `~/Applications` folder, not the system
`/Applications` folder. To choose it manually from Finder's **Open With → Other…** dialog, press
**Command-Shift-G**, enter `~/Applications`, and select **Textify.app**. To make it the default for
an extension, select a file in Finder, choose **File → Get Info**, select Textify under **Open
with**, then click **Change All…**. macOS stores that choice per document type.

## Verification

The packaging scripts refuse to overwrite a non-symlink at either installation path. They also
validate `Info.plist` and confirm that the bundle executable resolves to an executable Mach-O. To
inspect an installation manually:

```sh
plutil -lint target/release/Textify.app/Contents/Info.plist
file target/release/Textify.app/Contents/MacOS/Textify
file target/release/Textify.app/Contents/Resources/Textify.icns
zsh -n raycast/textify.sh
readlink ~/bin/textify
readlink ~/Applications/Textify.app
readlink ~/Applications/Textify.app/Contents/MacOS/Textify
```
