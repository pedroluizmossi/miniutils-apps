# miniutils-apps

External apps for [MiniUtils](../hub-app-rust): each one is its own executable, with full
access, that the hub starts when the app is turned on and talks to in JSON, one message
per line. The view is a `view.slint`, which the hub interprets and fits into the panel
cell.

| Folder | What it is |
|---|---|
| `sdk/` | crate `miniutils-app`: the app side of the protocol (`listen`, `set`, `mood`, `fact`, `migrated`…), translation of the app's own texts (`i18n::Catalog`) and per-OS odds and ends (`os::open_terminal`, `os::open_path`, `os::pick_folder`, `os::command` without a window on Windows) |
| `repos/` | **Repositories**: your Git repositories and what is left to commit and push, with one-click push and pull, the AI agent in each one and the GitHub CI |

Data with a versioned format: `start` carries `data_version` (the version of the data in
the folder). If it is lower than the `version` of `[data]` in your manifest, migrate and
call `miniutils_app::migrated(version)`; the hub keeps a copy beforehand and restores it if
the app exits without that.

Needs a MiniUtils with support for external apps (`[app]` in the manifest). The protocol
is at the top of the hub's `src/apps/external.rs`, and the manifest format at the top of
`src/core/plugins.rs`.

## A new app

1. Copy `repos/` to a new folder and add it to `members` in `Cargo.toml`.
2. `manifest.toml`: an `id` of its own (`io.github.<you>.<app>`), `name`, `icon` (one of
   the hub's mascots), `uses` (hub providers the app wants to receive) and the `[app]`
   section with `view = "view.slint"`, `linux = "bin/<name>"` and
   `windows = "bin/<name>.exe"`.
3. `view.slint`: exports `component Main`, gets `theme`, `mascot` and `mood` from the hub,
   calls `callback action(string, string)`.
4. `src/main.rs`: `miniutils_app::listen` and a loop over the messages.

Rules: standard output belongs to the protocol (debug with `eprintln!`, which goes to the
hub's log); on `stop` or when the input closes, exit soon.

## Translation

Texts are written in English and translated with a gettext catalog in the app folder,
`i18n/pt-BR.po`:

- **View and manifest:** `@tr("Push")`, `@tr("{} hidden", n)` in `view.slint`, and the
  `name`, `description` and `label`s of `manifest.toml`. The hub translates them when it
  loads the app.
- **Texts the executable builds** (summary, notifications, rows, errors): the hub sends the
  language in `start` (`Start::lang`); load the catalog with
  `miniutils_app::i18n::Catalog::for_lang(&start.lang, &[("pt-BR", include_str!("../i18n/pt-BR.po"))])`
  and pass each text through `tr("…")` / `fmt("… {} …", &[&x])`.
- A test in each app checks that every text has a translation and that the `.po` has no
  unused entry (`repos/src/i18n.rs`).

## Develop

```sh
cargo build -p repos
mkdir -p repos/bin && ln -sf ../../target/debug/repos repos/bin/repos
# in the hub's repository:
cargo run -- --plugin-dev ../miniutils-apps/repos
```

In dev mode the link in `bin/` works, and each `cargo build -p repos` restarts the app in
the hub. What the app writes with `eprintln!` shows up in the hub's terminal. On Windows,
copy `target\debug\repos.exe` to `repos\bin\repos.exe`.

Tests: `cargo test --workspace`.

## Publish to the store

1. Bump the version in `<app>/Cargo.toml`, commit and create the tag `<app>-v<version>`:
   ```sh
   git tag repos-v0.1.0 && git push origin master repos-v0.1.0
   ```
2. The **Release** workflow (`.github/workflows/release.yml`) runs the tests, builds the
   app for Linux (static, musl: runs on any distro) and Windows and publishes a Release
   with the executables (`repos-linux-x86_64`, `repos-windows-x86_64.exe`), the SHA-256
   sums and the entry ready for the store index (in the notes and in
   `registry-entry.json`).
3. With the Release ready, in the store's repository (`miniutils-store`):
   ```sh
   tools/publish.sh pedroluizmossi/miniutils-apps repos-v0.1.0
   ```
   The script builds the entry from the manifest and the Release, checks the executables,
   installs into a throwaway folder, shows the diff and, once you confirm, signs with your
   key and pushes. All on your machine (see the store's README).

The entry pins the tag's commit and, per system, the executable's address and SHA-256; the
hub only installs if the index signature and the hash match. The executable is built by CI
from the tag, not on a personal machine. The rest of the app folder (manifest, view,
`i18n/`) comes from the pinned commit. To build the entry by hand:
`python3 tools/index_entry.py --help`.

The repository must be public: the hub clones the code through the index's `https://` URL.
