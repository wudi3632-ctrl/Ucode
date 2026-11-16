import { useMemo, useState, type ReactNode } from "react";
import type { OnboardingState, OnboardingStepId } from "../domain/types";

export const onboardingSteps: Array<{ id: OnboardingStepId; title: string; description: string; action: string }> = [
  { id: "project", title: "打开一个项目", description: "体验示例，或授权自己的项目目录。", action: "选择项目" },
  { id: "conversation", title: "完成一次对话", description: "提问、解释代码或分析项目；回答完成后才计入。", action: "去提问" },
  { id: "review_change", title: "审查并批准修改", description: "先看 Diff，再明确批准；批准前文件不会改变。", action: "打开 Diff" },
  { id: "terminal", title: "使用持久终端", description: "命令需要审批，可以在运行中发送 Ctrl+C。", action: "打开终端" },
  { id: "git", title: "查看 Git 状态", description: "查看变更与历史；非 Git 项目可以跳过。", action: "打开 Git" },
  { id: "agent", title: "完成一次 Agent 任务", description: "只读分析直接运行，编辑任务进入隔离 Worktree。", action: "打开 Agents" },
  { id: "evidence", title: "查看结果证据", description: "区分实际观察、执行结果和仅提出的计划。", action: "查看证据" },
];

export const completedOnboardingState: OnboardingState = {
  schemaVersion: 1,
  welcomeState: "completed",
  completedSteps: onboardingSteps.map((step) => step.id),
  skippedSteps: [],
  dismissedTips: [],
  updatedAt: "",
  demoProjectId: null,
};

export function OnboardingWelcome({ busy, onDemo, onProject, onDismiss }: { busy: boolean; onDemo: () => Promise<void>; onProject: () => Promise<void>; onDismiss: () => Promise<void> }) {
  const [choice, setChoice] = useState<"demo" | "project" | "dismiss" | null>(null);
  const run = async (next: typeof choice, action: () => Promise<void>) => {
    if (busy || choice) return;
    setChoice(next);
    try { await action(); } finally { setChoice(null); }
  };
  return <section className="onboarding-welcome" aria-labelledby="onboarding-welcome-title">
    <div className="onboarding-orbit" aria-hidden="true"><span>U</span><i /><i /></div>
    <p className="onboarding-kicker">UCode · 本地优先</p>
    <h2 id="onboarding-welcome-title">完成第一项真实工作</h2>
    <p>从对话开始，在写入前审查 Diff，并用证据确认结果。引导会跟随真实操作，不会替你执行。</p>
    <div className="onboarding-start-actions">
      <button className="primary" disabled={busy || Boolean(choice)} onClick={() => void run("demo", onDemo)}>{choice === "demo" ? "正在准备示例…" : "体验示例项目"}</button>
      <button className="quiet" disabled={busy || Boolean(choice)} onClick={() => void run("project", onProject)}>{choice === "project" ? "正在打开…" : "打开自己的项目"}</button>
    </div>
    <div className="onboarding-promises" aria-label="UCode 工作原则"><span>离线模型</span><span>写入前审批</span><span>修改可回滚</span></div>
    <button className="onboarding-later" disabled={busy || Boolean(choice)} onClick={() => void run("dismiss", onDismiss)}>暂时跳过，可从帮助继续</button>
  </section>;
}

export function OnboardingChecklist({ state, onAction, onSkip, onReset, onCreateDemo = async () => undefined }: { state: OnboardingState; onAction: (step: OnboardingStepId) => void; onSkip: (step: OnboardingStepId) => Promise<void>; onReset: () => Promise<void>; onCreateDemo?: () => Promise<void> }) {
  const resolved = useMemo(() => new Set([...state.completedSteps, ...state.skippedSteps]), [state.completedSteps, state.skippedSteps]);
  const completed = state.completedSteps.length;
  const progress = Math.round((resolved.size / onboardingSteps.length) * 100);
  const next = onboardingSteps.find((step) => !resolved.has(step.id));
  if (state.welcomeState === "completed") return <section className="onboarding-complete" aria-label="UCode 入门已完成">
    <div className="onboarding-complete-mark" aria-hidden="true"><span>✓</span></div>
    <p className="onboarding-kicker">7 项能力已探索</p><h2>你已经掌握 UCode 的工作方式</h2>
    <p>对话负责理解，审批控制副作用，证据说明真正发生了什么。</p>
    <button className="quiet" onClick={() => void onReset()}>重新开始引导</button>
  </section>;
  return <section className="onboarding-checklist" aria-label="开始使用 UCode">
    <header><div><p className="onboarding-kicker">开始使用</p><h2>{next ? next.title : "入门任务"}</h2></div><div className="onboarding-progress-number"><b>{resolved.size}</b><span>/ {onboardingSteps.length}</span></div></header>
    <div className="onboarding-progress" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={progress}><span style={{ width: `${progress}%` }} /></div>
    <p className="onboarding-summary">完成 {completed} 项{state.skippedSteps.length ? `，跳过 ${state.skippedSteps.length} 项` : ""}。进度保存在 U 盘，可在另一台电脑继续。</p>
    <ol>
      {onboardingSteps.map((step, index) => {
        const done = state.completedSteps.includes(step.id);
        const skipped = state.skippedSteps.includes(step.id);
        const active = next?.id === step.id;
        return <li key={step.id} className={`${done ? "done" : skipped ? "skipped" : active ? "active" : "waiting"}`}>
          <span className="onboarding-step-mark">{done ? "✓" : skipped ? "—" : index + 1}</span>
          <div><b>{step.title}</b><p>{done ? "已通过真实操作完成" : skipped ? "已跳过，可随时体验" : step.description}</p>
            {!done && !skipped && <div className="onboarding-step-actions"><button className={active ? "primary" : "quiet"} onClick={() => onAction(step.id)}>{step.action}</button><button className="onboarding-skip" onClick={() => void onSkip(step.id)}>跳过</button></div>}
          </div>
        </li>;
      })}
    </ol>
    <footer><span>不会自动运行或批准任何操作</span><div><button onClick={() => void onCreateDemo()}>新建示例</button><button onClick={() => void onReset()}>重置引导</button></div></footer>
  </section>;
}

export function OnboardingTip({ id, title, children, dismissed, onDismiss }: { id: string; title: string; children: ReactNode; dismissed: boolean; onDismiss: (id: string) => Promise<void> }) {
  if (dismissed) return null;
  return <aside className="onboarding-tip" role="note"><span className="onboarding-tip-mark">i</span><div><b>{title}</b><p>{children}</p></div><button aria-label={`关闭提示：${title}`} onClick={() => void onDismiss(id)}>×</button></aside>;
}
