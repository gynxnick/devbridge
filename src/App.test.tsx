import { describe, expect, it } from "vitest";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { emit } from "@tauri-apps/api/event";
import App from "./App";
import { mockBackend, overviewOf, peer, workspace } from "./test/fixtures";

const nav = () => screen.getByRole("navigation");

describe("Dashboard", () => {
  it("shows an empty state when no projects exist", async () => {
    mockBackend(overviewOf([]));
    render(<App />);
    expect(await screen.findByText("No projects yet")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /import project/i })).toBeEnabled();
  });

  it("lists projects with their sync state and conflicts", async () => {
    mockBackend(
      overviewOf([
        workspace(),
        workspace({ id: "01JWORKSPACE00000000000002", name: "API", openConflicts: 2, rootAvailable: false }),
      ]),
    );
    render(<App />);
    const main = await screen.findByRole("main");
    expect(await within(main).findByText("Website")).toBeInTheDocument();
    expect(within(main).getAllByText("Local only")).toHaveLength(2);
    expect(within(main).getByText("2 conflicts")).toBeInTheDocument();
    expect(within(main).getByText("Folder missing")).toBeInTheDocument();
  });

  it("shows offers and joins an already-registered project without a folder", async () => {
    const user = userEvent.setup();
    const offer = {
      workspaceId: workspace().id,
      deviceId: peer.id,
      deviceName: "Gaming PC",
      name: "Website",
      color: "blue",
      icon: "code",
      offeredAt: Date.now(),
    };
    const calls = mockBackend(overviewOf([workspace()], { offers: [offer as never] }), { join_offer: () => null });
    render(<App />);
    expect(await screen.findByText("Shared with you")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Join…" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/already registered here/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: /join and sync/i }));
    await waitFor(() =>
      expect(calls.find((c) => c.cmd === "join_offer")?.args).toMatchObject({
        workspaceId: offer.workspaceId,
        deviceId: peer.id,
      }),
    );
  });
});

describe("Project detail", () => {
  it("removes a project only from DevBridge after confirmation", async () => {
    const user = userEvent.setup();
    const calls = mockBackend(overviewOf([workspace()]), { remove_workspace: () => null });
    render(<App />);
    await user.click(await within(nav()).findByText("Website"));
    await user.click(await screen.findByRole("button", { name: /remove from devbridge/i }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/is deleted or changed/)).toBeInTheDocument();
    expect(calls.some((c) => c.cmd === "remove_workspace")).toBe(false);
    await user.click(within(dialog).getByRole("button", { name: /remove from devbridge/i }));
    await waitFor(() => expect(calls.find((c) => c.cmd === "remove_workspace")?.args).toEqual({ id: workspace().id }));
  });

  it("shares a project with a paired device", async () => {
    const user = userEvent.setup();
    const calls = mockBackend(overviewOf([workspace()], { pairedDeviceCount: 1 }), {
      list_devices: () => [peer],
      share_workspace: () => null,
    });
    render(<App />);
    await user.click(await within(nav()).findByText("Website"));
    expect(await screen.findByText("Gaming PC")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /pause sync/i })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Share" }));
    await waitFor(() =>
      expect(calls.find((c) => c.cmd === "share_workspace")?.args).toEqual({ id: workspace().id, deviceId: peer.id }),
    );
  });

  it("surfaces backend errors from Open in VS Code", async () => {
    const user = userEvent.setup();
    mockBackend(overviewOf([workspace()]), {
      open_in_vscode: () => {
        throw { code: "launch", message: "VS Code wasn't found." };
      },
    });
    render(<App />);
    await user.click(await within(nav()).findByText("Website"));
    await user.click(await screen.findByRole("button", { name: /open in vs code/i }));
    expect(await screen.findByRole("alert")).toHaveTextContent("VS Code wasn't found.");
  });

  it("asks before a Git operation that touches uncommitted work", async () => {
    const user = userEvent.setup();
    const status = {
      branch: "main",
      head: "abc",
      upstream: "origin/main",
      ahead: 0,
      behind: 1,
      entries: [{ path: "src/a.ts", origPath: null, index: ".", worktree: "M", kind: "ordinary" }],
    };
    let pullCalls: unknown[] = [];
    mockBackend(overviewOf([workspace()]), {
      git_overview: () => ({
        isRepo: true,
        toplevel: "/w",
        status,
        log: [],
        branches: [{ name: "main", upstream: "origin/main", current: true }],
        remotes: [],
        autocrlf: null,
      }),
      git_pull: (a) => {
        pullCalls = [...pullCalls, a.confirmed];
        if (!a.confirmed) throw { code: "needs_confirmation", message: "1 file(s) have uncommitted changes." };
        return null;
      },
    });
    render(<App />);
    await user.click(await within(nav()).findByText("Website"));
    await user.click(await screen.findByRole("tab", { name: "Git" }));
    expect(await screen.findByRole("button", { name: /catch up git/i })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Pull" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("1 file(s) have uncommitted changes.")).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Continue" }));
    await waitFor(() => expect(pullCalls).toEqual([false, true]));
  });
});

describe("Pairing", () => {
  it("prompts for approval with the verification code", async () => {
    const user = userEvent.setup();
    const calls = mockBackend(overviewOf([]), { pairing_decide: () => null });
    render(<App />);
    await screen.findByText("No projects yet");
    await act(() =>
      emit("engine", {
        kind: "pairingRequest",
        requestId: "r1",
        deviceName: "Gaming PC",
        os: "windows",
        verificationCode: "123 456",
      }),
    );
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("123 456")).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: /approve/i }));
    await waitFor(() => expect(calls.find((c) => c.cmd === "pairing_decide")?.args).toEqual({ requestId: "r1", approve: true }));
  });

  it("shows the pairing code when hosting", async () => {
    const user = userEvent.setup();
    mockBackend(overviewOf([]), {
      pairing_open: () => ({
        code: "7KQ2-M9XD",
        expiresAt: Date.now() + 300_000,
        addresses: ["192.168.1.20:47821"],
        port: 47821,
        qrSvg: null,
      }),
      pairing_close: () => null,
    });
    render(<App />);
    await user.click(await within(nav()).findByText("Devices"));
    await user.click(await screen.findByRole("button", { name: /pair new device/i }));
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByText("7KQ2-M9XD")).toBeInTheDocument();
    expect(within(dialog).getByText("192.168.1.20:47821")).toBeInTheDocument();
    expect(within(dialog).getByText(/Network connections are off/)).toBeInTheDocument();
  });
});

describe("Conflict center", () => {
  it("compares both versions and resolves with keep remote", async () => {
    const user = userEvent.setup();
    const conflict = {
      id: "c1",
      workspaceId: workspace().id,
      path: "notes.txt",
      kind: "content",
      localVersionId: "v1",
      localHash: "a",
      localSize: 9,
      remoteVersionId: "v2",
      remoteHash: "b",
      remoteSize: 8,
      remoteDeviceId: peer.id,
      state: "open",
      resolution: null,
      detectedAt: Date.now(),
      resolvedAt: null,
      workspaceName: "Website",
      remoteDeviceName: "Gaming PC",
    };
    const calls = mockBackend(overviewOf([workspace({ openConflicts: 1 })]), {
      list_conflicts: () => [conflict],
      conflict_detail: () => ({
        ...conflict,
        local: { kind: "text", text: "mac edit\n" },
        remote: { kind: "text", text: "pc edit\n" },
        recovery: [],
      }),
      resolve_conflict: () => null,
    });
    render(<App />);
    await user.click(await within(nav()).findByText("Conflicts"));
    expect(await screen.findByText("mac edit")).toBeInTheDocument();
    expect(screen.getByText("pc edit")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /keep gaming pc's/i }));
    await waitFor(() =>
      expect(calls.find((c) => c.cmd === "resolve_conflict")?.args).toMatchObject({ id: "c1", resolution: "keep_remote" }),
    );
  });
});

describe("Settings", () => {
  it("warns when a secret pattern is opted in and saves it", async () => {
    const user = userEvent.setup();
    const calls = mockBackend(overviewOf([]), { save_settings: (a) => a.settings });
    render(<App />);
    await user.click(await screen.findByRole("button", { name: "Settings" }));
    const envBox = await screen.findByRole("checkbox", { name: ".env" });
    await user.click(envBox);
    expect(screen.getByText(/copied to every paired device/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /save changes/i }));
    await waitFor(() => {
      const saved = calls.find((c) => c.cmd === "save_settings")?.args.settings as { secretOptins: string[] };
      expect(saved.secretOptins).toEqual([".env"]);
    });
  });
});
