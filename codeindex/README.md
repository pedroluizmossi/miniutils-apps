# Code index (MiniUtils external app)

Watches a [code-index](../../database-index) server and searches it from the MiniUtils
cell: repositories indexed / pending / in error, the latest sync job and the enrichment
progress. The mascot sleeps when the server is offline, is busy while a sync runs and
raises the alarm when a sync fails or a repository is in error.

**Full access:** it is a program of its own, running with your user's permissions. It only
talks to the server in its settings and starts `code` (VS Code) or the default program when
you click a search result.

## Configuration

Needs a MiniUtils that understands `api = 2` (the `text` and `secret` settings). In
**Settings › Apps › Code index**:

| Setting | What |
|---|---|
| Server address | default `http://127.0.0.1:8765` |
| Access token | `cix_...`, created on the server; the field hides it and the app never logs it |
| Folder with the server's clones | optional: `<server project>/.repos`, so a click on a search result opens the file at the line in VS Code (`code -g`); without it, the web UI opens |

`CODE_INDEX_URL` and `CODE_INDEX_TOKEN` in the environment work too, when the setting is
empty. Changing the address or the token reconnects right away.

The token's scopes decide what the cell shows: `search:read` is required (status and
search); `index:write` adds the sync status and the sync button; `enrich:write` adds the
enrichment line. A rejected token (401) or a missing scope (403) shows up in the cell.

## What you can do

- **Search** (bigger cells): type, press Enter. A result opens at its line with
  `code -g` when `repos_dir` is set and the clone exists there; otherwise the server's web UI
  opens.
- **Sync next batch**: asks the server for a dry run first and shows the repositories of the
  batch; a second click ("Confirm sync") starts it.
- **Open web UI**: opens `<url>/ui/`.

Options: alarm when the server is offline, notify when a sync finishes or fails, number of
search results. The interface follows the hub's language (English or Brazilian Portuguese)
from `i18n/pt-BR.po`.

## Development

```sh
cargo build -p codeindex
mkdir -p codeindex/bin && ln -sf ../../target/debug/codeindex codeindex/bin/codeindex
miniutils --plugin-dev codeindex
```
