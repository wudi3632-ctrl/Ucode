import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { OnboardingState } from "../domain/types";
import { OnboardingChecklist, OnboardingWelcome } from "./Onboarding";

afterEach(cleanup);

const active: OnboardingState = {
  schemaVersion: 1,
  welcomeState: "active",
  completedSteps: ["project", "conversation"],
  skippedSteps: ["git"],
  dismissedTips: [],
  updatedAt: "2026-11-12T00:00:00Z",
  demoProjectId: "demo",
};

describe("progressive onboarding", () => {
  it("offers a real demo, a user project and a non-destructive later choice", async () => {
    const demo = vi.fn().mockResolvedValue(undefined);
    const project = vi.fn().mockResolvedValue(undefined);
    const dismiss = vi.fn().mockResolvedValue(undefined);
    render(<OnboardingWelcome busy={false} onDemo={demo} onProject={project} onDismiss={dismiss} />);
    expect(screen.getByRole("heading", { name: "完成第一项真实工作" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "体验示例项目" }));
    expect(demo).toHaveBeenCalledOnce();
    expect(project).not.toHaveBeenCalled();
    expect(dismiss).not.toHaveBeenCalled();
  });

  it("shows verified, skipped and next missions without claiming skipped work completed", () => {
    const action = vi.fn();
    const skip = vi.fn().mockResolvedValue(undefined);
    render(<OnboardingChecklist state={active} onAction={action} onSkip={skip} onReset={vi.fn().mockResolvedValue(undefined)} />);
    expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "43");
    expect(screen.getByText(/完成 2 项，跳过 1 项/)).toBeInTheDocument();
    const list = screen.getByRole("list");
    expect(within(list).getByText("打开一个项目").closest("li")).toHaveClass("done");
    expect(within(list).getByText("查看 Git 状态").closest("li")).toHaveClass("skipped");
    fireEvent.click(screen.getByRole("button", { name: "打开 Diff" }));
    expect(action).toHaveBeenCalledWith("review_change");
  });

  it("collapses to a completion surface when every mission is resolved", () => {
    render(<OnboardingChecklist state={{ ...active, welcomeState: "completed", completedSteps: ["project", "conversation", "review_change", "terminal", "git", "agent", "evidence"], skippedSteps: [] }} onAction={vi.fn()} onSkip={vi.fn()} onReset={vi.fn()} />);
    expect(screen.getByRole("region", { name: "UCode 入门已完成" })).toHaveTextContent("你已经掌握 UCode 的工作方式");
  });
});
