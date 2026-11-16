import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { bootstrap } from "../bridge/desktop";
import type { NotificationDiagnostics, NotificationIdentityStatus, RuntimeDiagnosticSnapshot } from "../domain/types";
import { NotificationIdentityCard, RuntimeHealthCard, Settings } from "./Settings";

const readyIdentity: NotificationIdentityStatus = {
  appId: "dev.ucode.portable",
  displayName: "UCode",
  health: "ready",
  registered: true,
  iconUri: "E:\\UCode\\UCode.png",
  checkedAt: "2026-11-12T05:00:00.000Z",
  lastError: null,
};

const diagnostics: NotificationDiagnostics = {
  identity: readyIdentity,
  telemetry: {
    attempts: 5,
    shown: 4,
    activated: 2,
    failed: 1,
    lastAttemptAt: "2026-11-12T05:00:00.000Z",
    lastShownAt: "2026-11-12T05:00:00.000Z",
    lastActivatedAt: "2026-11-12T05:00:00.000Z",
    lastError: null,
  },
};

afterEach(cleanup);

describe("verified local model profiles", () => {
  it("shows only physically inventoried verified packs instead of fake larger choices", async () => {
    const snapshot = await bootstrap();
    render(<Settings value={snapshot.settings} onClose={vi.fn()} onSave={vi.fn().mockResolvedValue(undefined)} onClearNotificationIdentity={vi.fn().mockResolvedValue(undefined)} />);

    const panel = await screen.findByRole("region", { name: "已安装本地模型" });
    expect(within(panel).getByText("Qwen2.5-Coder 1.5B Q4_K_M")).toBeInTheDocument();
    expect(within(panel).getByText("已验证")).toBeInTheDocument();
    expect(within(panel).queryByText(/3B|7B/)).not.toBeInTheDocument();
  });
});

describe("readable typography settings", () => {
  it("previews and saves UI and code scales independently", async () => {
    const snapshot = await bootstrap();
    const onSave = vi.fn().mockResolvedValue(undefined);
    render(<Settings value={snapshot.settings} onClose={vi.fn()} onSave={onSave} onClearNotificationIdentity={vi.fn().mockResolvedValue(undefined)} />);

    fireEvent.click(screen.getByRole("button", { name: /外观/ }));
    const uiRange = screen.getByRole("slider", { name: "界面与聊天字号" });
    const codeRange = screen.getByRole("slider", { name: "代码、Diff 与终端字号" });
    expect(uiRange).toHaveValue("18");
    expect(codeRange).toHaveValue("14");
    fireEvent.change(uiRange, { target: { value: "32" } });
    fireEvent.change(codeRange, { target: { value: "28" } });
    expect(document.documentElement.style.getPropertyValue("--ui-font-scale")).toBe(String(32 / 14));
    expect(document.documentElement.style.getPropertyValue("--code-font-scale")).toBe(String(28 / 12));

    fireEvent.click(screen.getByRole("button", { name: "保存设置" }));
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({ uiFontSize: 32, codeFontSize: 28 }));
  });
});

describe("appearance preview", () => {
  it("previews theme and reduced motion, then restores both when cancelled", async () => {
    const snapshot = await bootstrap();
    const value = { ...snapshot.settings, theme: "dark" as const, reduceMotion: false };
    document.documentElement.dataset.theme = "dark";
    document.documentElement.classList.remove("reduce-motion");
    const view = render(<Settings value={value} onClose={vi.fn()} onSave={vi.fn().mockResolvedValue(undefined)} onClearNotificationIdentity={vi.fn().mockResolvedValue(undefined)} />);

    fireEvent.click(screen.getByRole("button", { name: /外观/ }));
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "light" } });
    fireEvent.click(screen.getByRole("checkbox", { name: /减少动态效果/ }));
    expect(document.documentElement.dataset.theme).toBe("light");
    expect(document.documentElement).toHaveClass("reduce-motion");

    view.unmount();
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement).not.toHaveClass("reduce-motion");
  });
});

describe("NotificationIdentityCard", () => {
  it("shows verified Windows identity and runtime telemetry", () => {
    render(<NotificationIdentityCard identity={readyIdentity} diagnostics={diagnostics} busy={false} message={null} onRepair={vi.fn()} onClear={vi.fn()} />);

    expect(screen.getByText("UCode 已就绪")).toBeInTheDocument();
    expect(screen.getByTitle("E:\\UCode\\UCode.png")).toBeInTheDocument();
    expect(screen.getByText("已显示").nextSibling).toHaveTextContent("4");
    expect(screen.getByText("已点击").nextSibling).toHaveTextContent("2");
    expect(screen.getByText("失败").nextSibling).toHaveTextContent("1");
    expect(screen.queryByRole("button", { name: "修复通知身份" })).not.toBeInTheDocument();
  });

  it("offers a real repair action when registration has drifted", () => {
    const onRepair = vi.fn().mockResolvedValue(undefined);
    render(<NotificationIdentityCard identity={{ ...readyIdentity, health: "drifted" }} diagnostics={diagnostics} busy={false} message={null} onRepair={onRepair} onClear={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "修复通知身份" }));
    expect(onRepair).toHaveBeenCalledOnce();
    expect(screen.getByText("需要修复")).toBeInTheDocument();
  });

  it("surfaces inspection failures without pretending the identity is ready", () => {
    render(<NotificationIdentityCard identity={{ ...readyIdentity, health: "error", registered: false, lastError: "无法读取注册表" }} diagnostics={null} busy={false} message={null} onRepair={vi.fn()} onClear={vi.fn()} />);

    expect(screen.getByText("检查失败")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("无法读取注册表");
    expect(screen.getByRole("button", { name: "清除本机通知身份" })).toBeDisabled();
  });
});

describe("RuntimeHealthCard", () => {
  const runtime: RuntimeDiagnosticSnapshot = {
    overall: "degraded",
    services: [
      { id: "agent", label: "Agent", status: "stopped" },
      { id: "controller", label: "Controller", status: "ready" },
      { id: "coder", label: "Coder", status: "ready" },
      { id: "llama", label: "llama runtime", status: "ready" },
    ],
    runtimeRoot: "E:\\UCode\\runtime-pack",
    owned: true,
    starting: false,
    restartAttempts: 1,
    modelProfile: "qwen2-5-coder-1-5b-q4-k-m",
    checkedAt: "2026-11-12T05:00:00.000Z",
    lastError: "自动恢复后，本地 Agent 仍未通过健康检查。",
  };

  it("shows factual service health and invokes bounded recovery", () => {
    const onRecover = vi.fn().mockResolvedValue(undefined);
    render(<RuntimeHealthCard diagnostics={runtime} busy={false} message={null} onRefresh={vi.fn()} onRecover={onRecover} />);

    expect(screen.getByText("部分服务异常")).toBeInTheDocument();
    expect(screen.getByLabelText("本地运行时状态")).toHaveTextContent("Agent未响应Controller正常");
    expect(screen.getByRole("alert")).toHaveTextContent("自动恢复后");
    fireEvent.click(screen.getByRole("button", { name: "恢复本地服务" }));
    expect(onRecover).toHaveBeenCalledOnce();
  });

  it("keeps a healthy runtime quiet and refreshable", () => {
    const onRefresh = vi.fn().mockResolvedValue(undefined);
    render(<RuntimeHealthCard diagnostics={{ ...runtime, overall: "ready", services: runtime.services.map((service) => ({ ...service, status: "ready" as const })), lastError: null }} busy={false} message={null} onRefresh={onRefresh} onRecover={vi.fn()} />);

    expect(screen.getByText("运行正常")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "恢复本地服务" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "重新检查" }));
    expect(onRefresh).toHaveBeenCalledOnce();
  });

  it("keeps the core usable while Controller warms in the background", () => {
    render(<RuntimeHealthCard diagnostics={{
      ...runtime,
      overall: "starting",
      services: [
        { id: "agent", label: "Agent", status: "ready" },
        { id: "controller", label: "Controller", status: "starting" },
        { id: "coder", label: "Coder", status: "ready" },
        { id: "llama", label: "llama runtime", status: "ready" },
      ],
      lastError: null,
    }} busy={false} message={null} onRefresh={vi.fn()} onRecover={vi.fn()} />);

    expect(screen.getByText("核心可用")).toBeInTheDocument();
    expect(screen.getByText(/复杂规划能力仍在加载/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "恢复本地服务" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "重试复杂规划" })).not.toBeInTheDocument();
  });

  it("offers Controller-only recovery when the core remains healthy", () => {
    const onRecover = vi.fn().mockResolvedValue(undefined);
    render(<RuntimeHealthCard diagnostics={{
      ...runtime,
      services: [
        { id: "agent", label: "Agent", status: "ready" },
        { id: "controller", label: "Controller", status: "stopped" },
        { id: "coder", label: "Coder", status: "ready" },
        { id: "llama", label: "llama runtime", status: "ready" },
      ],
      lastError: "复杂规划模型启动超时，已仅停止 Controller。",
    }} busy={false} message={null} onRefresh={vi.fn()} onRecover={onRecover} />);

    expect(screen.getByText("核心可用")).toBeInTheDocument();
    expect(screen.getByText(/普通对话和明确文件任务/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "重试复杂规划" }));
    expect(onRecover).toHaveBeenCalledOnce();
    expect(screen.queryByRole("button", { name: "恢复本地服务" })).not.toBeInTheDocument();
  });
});
