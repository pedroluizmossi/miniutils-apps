# Repositories (MiniUtils external app)

Your Git repositories and what is left in each one: uncommitted changes, unpushed commits
(↑), commits on the remote you do not have yet (↓), stash and half-done merge/rebase. Each
row has buttons to open a terminal, the folder or the editor (VS Code, VSCodium or Zed, if
installed) in the repository.

**Full access:** it is a program of its own, running with your user's permissions. It
reads the watched folders and runs `git` (and `gh`, if installed) in each repository. On
its own, it only writes to its data folder (`roots.txt` and `prefs.json`; older versions
named them `pastas.txt` and `preferencias.json`, and the app renames them on start). In
the repository, it only acts when you click: push, pull or delete branches.

The interface follows the hub's language (English or Brazilian Portuguese): the view and
the manifest are translated by the hub from `i18n/pt-BR.po`, and the app uses the same file
for the texts it builds.

## In each repository's row

- **↑ Push** shows up with commits ahead of the upstream: a `git push`.
- **↓ Pull** shows up when the remote has new commits and you have nothing local (no
  changes, no commits ahead): a `git pull --ff-only`, which only moves the branch forward
  and never creates a merge. Without a key or password available, it fails without asking,
  and the reason comes in a notification.
- **AI agent:** "Claude working", "Codex is waiting for you". Comes from the hub's
  `agents` provider (the same as the AI Agents app: Claude Code, Codex and OpenCode), by
  each session's folder. While an agent is active in a repository, its pending work does
  not count as old. When the agent finishes leaving uncommitted changes, the row shows
  "finished · not committed" for up to 30 min, and the mascot mentions it in the chatter.
- **CI ✓ / ✗ / …:** the last CI run of the current branch, through `gh` (GitHub CLI), only
  in repositories with a remote on github.com and only with `gh` installed and logged in
  (`gh auth login`). It also shows "CI on main failed" when you are on another branch, and
  "2 PRs await your review". Read every 30 min (the GitHub API has a quota) and 1 min after
  a push from the button. To turn it off: setting "Show GitHub CI and PRs".

## Detail

Click the repository's name to open it: the changed files (`M` modified, `A` added, `D`
deleted, `R` renamed, `U` conflicted, `?` untracked), the last 3 commits, the stash and the
local branches already merged into the main one. It also has:

- **Pin to top:** the repository comes first, even when clean (and even with "Show only
  repositories with pending work").
- **Hide:** it leaves the list, the mascot and the reminder. In the footer, "N hidden ·
  show" brings it back.
- **Delete merged branches:** needs a second click and uses `git branch -d`, which refuses
  to delete a branch that was not merged. Local ones only; the main and the current branch
  never go.

## The mascot (octopus)

- **Alarm:** a conflict, a half-done merge/rebase/cherry-pick, or the main branch's CI
  failing.
- **Restless:** some pending work (uncommitted or unpushed) older than the limit of the
  setting "Hours until the mascot nags about pending work" (default: 24 h).
- **Calm:** recent pending work; you (or an agent) are working.
- **Happy:** everything committed and pushed.

In the chatter between mascots, the app tells when a repository gets old pending work, a
conflict or a broken CI, when an agent finishes leaving uncommitted changes, and when
everything is up to date again.

## End-of-day reminder

From the hour of the setting "Reminder time" (default: 18 h), one notification a day if
there are unpushed commits: "You have 4 unpushed commits in 2 repositories". Hidden
repositories do not count. Turn it off with "Remind me at the end of the day about
unpushed commits".

## Watched folders

The first time, the app creates `roots.txt` in its data folder, with the project folders it
finds in your home folder (`~/Projetos`, `~/projects`, `~/src`, `~/code`…).

- **+ Folder** opens the system folder picker and adds the chosen one (on Linux, it needs
  `zenity`, `kdialog` or `yad`; on Windows, it is the system's own).
- **Folders** opens the file to edit by hand: one folder per line, `~` is the home folder.
  Save and click **Refresh**.

The search goes down to the depth of the setting "Search depth in the folders" (default: 2)
and does not go into hidden folders, `node_modules`, `target` and the like.

## When it updates

- **Right away:** commit, add, checkout, pull, fetch, or anything that touches the inside
  of `.git`. The app watches only `.git`, not the working tree, so it does not spend
  thousands of watches on `node_modules`.
- **Every 2 minutes, and when the hub window opens:** all of them (that is how an edited
  file shows up).
- **Every 20 minutes, and on Refresh:** looks for new repositories in the folders.
- **Every 30 minutes:** CI and PRs through `gh`; and, with the setting "Fetch from the
  remote" on, `git fetch` in those with a remote, without asking for a password (without a
  loaded key, the fetch fails silently and goes to the hub's log).

The app uses `git` itself (it must be on the PATH), with `--no-optional-locks`: reading does
not touch the repository.
