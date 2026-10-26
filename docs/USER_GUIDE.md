# DevBridge — User Guide

DevBridge lets you start work on one computer (say your Mac) and continue on another
(say your Windows PC) with the same project, VS Code and Claude Code.

## Install
- **macOS:** open the `.dmg`, drag DevBridge to Applications. (Unsigned test builds: right-click → Open the first time.)
- **Windows:** run the installer (`.msi` or `-setup.exe`). It installs for your user only.
- You also need **VS Code**, **Git**, and the **Claude Code** extension (or CLI) on each
  computer. Settings → Tools shows what DevBridge found and how to fix anything missing.

## Two ways to work across computers

| | Synced project | Remote workspace |
|---|---|---|
| Where files live | A full copy on each computer | Only on the host computer |
| Works offline | Yes | No (needs a connection to the host) |
| Where Claude Code runs | On whichever computer you're using | Always on the host |
| Conflicts possible | Yes, if both copies change before syncing; DevBridge shows them | No, there's one copy |

Use **synced** for "I work on one machine at a time". Use **remote** when you want to keep
going on exactly the same environment, including running processes.

## First steps
1. **Add a project:** Dashboard → Import project (pick your project folder) or Create
   project. Nothing in the folder is moved or changed.
2. **Pair your computers:** install DevBridge on both. On computer A: Devices → *Pair new
   device* (allow network connections when asked). On computer B: Devices → *Join a device*,
   pick A from the list or type its address, and enter the code. A shows a request with a
   6-digit verification code; check that B shows the same code, then **Approve**.
3. **Share the project:** open the project on A → Sync tab → **Share** next to B.
4. **Join on B:** the dashboard shows "Shared with you" → **Join…** → choose an empty
   folder (or your existing clone of the same repository). Files start syncing.

## Day to day
- Edit normally. Saved changes reach the other computer within about a second when both are on.
- When the other computer is off, changes queue and are exchanged when it's back.
- **Pause sync** stops exchanging changes for a project (local changes are still recorded).
  **Reconcile** forces a full comparison.
- The project page shows the sync state, the last successful sync, pending changes,
  transfers, conflicts and files that can't sync.

## Git across computers
DevBridge syncs your working files, not Git history. The `.git` folder is never synced;
each computer has its own clone and you share history through your Git remote (GitHub,
GitLab, a server, …).

A typical hand-off: **commit and push on the Mac → on the PC, Git tab → Fetch.** Because
DevBridge already brought over the files, Git on the PC shows them as "changed" and the
branch as behind. Click **Catch up Git**: DevBridge checks that every changed file already
matches the pushed commit byte for byte and then fast-forwards the PC's branch. Nothing
in your files changes, and if anything differs it stops and tells you which file.

DevBridge never resets, force-pushes or discards changes. Pull and branch switching ask
before running when you have uncommitted work (Git itself also refuses to overwrite it).

## Claude Code
- Sign in to Claude Code on each computer as usual. DevBridge never reads or copies your
  credentials, settings or conversations.
- Syncing files does **not** move a running Claude Code conversation or terminal. On the
  other computer, start a new session in the same project.
- Project files such as `CLAUDE.md` and `.claude/settings.json` sync like other files.
  `.claude/settings.local.json` is machine-specific and excluded by default.
- Avoid running Claude Code on the same project on both computers at the same time in
  synced mode. If you want that, use Remote workspace.

## Conflicts
If a file changes on both computers before they sync, DevBridge keeps **both** versions and
lists the file in **Conflicts**. Compare them side by side, then choose:
- **Keep this device's** / **Keep the other device's**
- **Save both**: keeps yours and writes theirs next to it as `name (conflict from PC 2026-10-26).ext`
- **Merge…**: edit a combined text version (for text files)

Whatever you choose, both versions stay in **Recovery history** until you delete them. The
other computer receives the result and closes its copy of the conflict automatically.

## Files that can't sync
Names Windows doesn't allow (`aux.txt`, `what?.md`, names ending in a dot), names that differ
only in letter case, and symbolic links are listed under Conflicts → *Can't sync*. Rename
them to sync them.

## What is never synced
`.git/`, generated folders (`node_modules/`, `target/`, `dist/`, `build/`, …), editor
temp files, and likely secrets (`.env`, `*.pem`, `*.key`, SSH keys, credential files, …). You
can allow a secret pattern in Settings → Sync rules if you really want it on every paired
computer, and add your own exclusion rules. `.gitignore` is respected by default.

## Remote workspace
1. On the **host** (the computer that keeps the project): turn on its SSH server (macOS:
   System Settings → General → Sharing → Remote Login; Windows: Optional features → OpenSSH
   Server). Then open the project → **Remote access** → *Allow remote access to this project*.
2. On the **client**: Devices → Remote access → **Request access** next to the host.
3. Approve the request on the host.
4. On the client, click **Open remotely** next to the project. DevBridge adds one `Include`
   line to `~/.ssh/config` (after asking) and opens VS Code connected to the host. Install the
   "Remote - SSH" extension in VS Code if asked. Claude Code then runs on the host.
Revoke access anytime from either computer; removing a device also removes its key.

## Backups and recovery
- DevBridge keeps copies of files it replaced or deleted (for the retention period in
  Settings) and of both sides of every conflict (until you delete them). Use Conflicts →
  Recovery history → **Restore copy** to bring one back as a new file.
- DevBridge isn't a backup tool. Keep pushing to your Git remote and keep your usual
  backups (Time Machine, File History, …).
- App data lives in `~/Library/Application Support/gg.gynx.devbridge` (macOS) and
  `%APPDATA%\gg.gynx.devbridge` (Windows).

## Removing things
- *Remove from DevBridge* on a project only forgets it; your files and `.git` stay.
- *Remove* on a device unpairs it immediately. To reconnect, pair again.
