# Groundhog as a Flatpak

```sh
flatpak/build.sh            # build, and install for this user
flatpak/build.sh bundle     # also write flatpak/out/groundhog.flatpak
flatpak run org.holsatus.Groundhog
```

`build.sh` needs `flatpak-builder` (the distribution's package, or
`flatpak install --user flathub org.flatpak.Builder`). The first build
installs the Freedesktop SDK and its Rust extension from Flathub, about
1.2 GB, and every build compiles the workspace in release mode from
scratch. Everything the build writes goes to `flatpak/out/`, which git
ignores.

The bundle is a single file that installs on another machine with
`flatpak install --user groundhog.flatpak`; the runtime comes from Flathub
there.

## The files

| File | What it is |
|---|---|
| `org.holsatus.Groundhog.yml` | The manifest: runtime, permissions, how to build |
| `org.holsatus.Groundhog.desktop` | The launcher entry: name, icon, categories |
| `org.holsatus.Groundhog.metainfo.xml` | What software centres show; add a `<release>` per version |
| `org.holsatus.Groundhog.svg` | The application icon (a placeholder) |

The application ID, `org.holsatus.Groundhog`, is also the window's
Wayland app ID and what Groundhog registers with the desktop portal
(`platform::APP_ID`). The desktop matches them to show the name and icon in
the taskbar and beside the inhibition that keeps the screen on, so they
must all agree; rename the files together with the constant.

## The build

The sandboxed build has no network. `build.sh` therefore runs
`cargo vendor` first, which copies every crate in `Cargo.lock` (git
dependencies and the iced `[patch]` included) to `flatpak/out/vendor` and
prints the source replacement that points Cargo at them; the build
appends that to its copy of `.cargo/config.toml`. The manifest builds the
working tree as it is, uncommitted changes included.

The Rust extension follows the runtime's release (26.08), not rustup, so
it can be a version behind the toolchain the workspace is developed with.

## The permissions

| Permission | Why |
|---|---|
| `--share=network` | UDP and TCP links, and map tiles |
| `--device=all` | The GPU, and serial adapters (`/dev/ttyACM*`, `/dev/ttyUSB*`), which no narrower permission reaches |
| `--filesystem=/run/udev:ro` | Listing serial ports asks udev which are USB adapters |
| `--system-talk-name=org.freedesktop.Avahi` | Cameras named `….local` (multicast DNS) are looked up through Avahi; the runtime has no resolver for those names |
| `--socket=wayland`, `--socket=fallback-x11`, `--share=ipc` | The window |

Nothing else is needed: file dialogs go through the desktop portal, which
grants access to the file chosen, and keeping the screen on is a portal
request too. Configuration, logs and stored map tiles live in
`~/.var/app/org.holsatus.Groundhog/`, apart from a native build's.

Serial adapters still need the user in the group that owns them
(`dialout` or `uucp`), as outside the sandbox.

## Publishing on Flathub

Flathub builds from a manifest in its own repository, and needs a few
things this one does not have yet:

- Sources fetched from URLs: the repository as a `git` source at a tagged
  commit, and the crates as a `cargo-sources.json` generated from
  `Cargo.lock` by `flatpak-cargo-generator.py` (from
  `flatpak-builder-tools`), in place of the vendored directory.
- A `<project_license>` in the metainfo, which needs the project to choose
  a licence.
- A real icon, and screenshots in the metainfo.
