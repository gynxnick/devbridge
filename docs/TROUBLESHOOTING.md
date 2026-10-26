# Troubleshooting

**The other computer doesn't appear / can't connect**
- Both computers must be on the same network (or a VPN). DevBridge doesn't use relays.
- On the computer you're connecting *to*: Settings → Network → *Allow connections from
  paired devices* must be on. Your firewall may have asked to allow DevBridge; allow it for
  private networks (Windows Defender Firewall → Allow an app).
- Wi-Fi "client isolation" (common on guest networks) blocks device-to-device traffic.
- Enter the address shown in the pairing dialog manually if discovery doesn't list it.
- Port in use? Change Settings → Network → Port on that computer (and re-pair or let
  discovery find the new port).

**Pairing says the code didn't match**
- Codes are single-use and expire after 5 minutes. Three wrong attempts close the window;
  open a new one. `O`/`0` and `I`/`L`/`1` are treated as the same.

**A file isn't syncing**
- Check Conflicts → *Can't sync* (Windows-invalid name, case-only difference, symlink).
- Check Settings → Sync rules: the file may match an exclusion, a secret pattern or `.gitignore`.
- The other device may have different rules: its *Can't sync* tab shows "Not accepted".
- Use **Reconcile** on the project page.

**"The project folder isn't available"**
- An external drive is disconnected or the folder was moved/renamed. Reconnect it or move
  it back. DevBridge doesn't change anything until the folder is back.

**Git shows every file as changed after syncing**
- Usually line endings: set the same `core.autocrlf` on both computers, or add a
  `.gitattributes` with `* text=auto eol=lf`. The Git tab warns when autocrlf is set.
- If the other computer committed and pushed, use **Fetch** then **Catch up Git**.

**macOS asks for the "login" keychain password**
- DevBridge keeps its device key in your login keychain; the password is your Mac account
  password. Choose *Always Allow*. If you click *Deny*, DevBridge uses a temporary identity for
  that session (pairings made then won't survive a restart) — nothing is lost or overwritten.
- Unsigned builds change their code signature with every build, so macOS asks again after each
  update. Signed releases ask at most once. Development builds don't use the keychain unless
  `DEVBRIDGE_KEYCHAIN=1` is set.

**Diagnostics show "temporary" device key**
- The system credential store wasn't available. Pairings made in this session won't survive a
  restart. On Linux, install a Secret Service provider; on macOS/Windows, check Keychain /
  Credential Manager isn't locked.

**Remote workspace won't open**
- Host: SSH server running? (Project → Remote access shows a warning if not.)
- Client: Devices → Remote access → *Check connection* shows whether SSH is reachable.
- VS Code needs the "Remote - SSH" extension.
- Windows host with an administrator account: copy the key line from
  `%USERPROFILE%\.ssh\authorized_keys` into `C:\ProgramData\ssh\administrators_authorized_keys`
  (elevated PowerShell), because sshd ignores the user file for administrators.

**Getting help**
- Settings → Logs & diagnostics → *Export diagnostic report* creates a JSON file without file
  contents or secrets (project paths are included). Logs are in the app data `logs` folder;
  set `DEVBRIDGE_LOG=debug` before launching for more detail.
