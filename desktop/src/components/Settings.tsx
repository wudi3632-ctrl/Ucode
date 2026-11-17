import { useEffect, useRef, useState } from "react";
import { chooseSkillDirectory, clearProviderSecret, getNotificationDiagnostics, getNotificationIdentityStatus, getProviderSecretStatus, getRuntimeDiagnostics, installSkill, listModelPacks, listSkills, previewSkill, recoverLocalRuntime, repairNotificationIdentity, saveProviderSecret, setSkillEnabled, syncNativeTheme, testModelProvider, uninstallSkill } from "../bridge/desktop";
import type { AppSettings, ModelPackInfo, NotificationDiagnostics, NotificationIdentityStatus, RuntimeDiagnosticSnapshot, SkillPreview, SkillSummary } from "../domain/types";
import { clampSoundVolume, previewAudioCue } from "../domain/audioFeedback";
import { CODE_FONT_DEFAULT, CODE_FONT_MAX, CODE_FONT_MIN, UI_FONT_DEFAULT, UI_FONT_MAX, UI_FONT_MIN, codeFontScale, codeFontSize, uiFontScale, uiFontSize } from "../domain/typography";

export function Settings({ value, onClose, onSave, onClearNotificationIdentity }: { value: AppSettings; onClose: () => void; onSave: (value: AppSettings) => Promise<void>; onClearNotificationIdentity: () => Promise<void> }) {
  const [draft, setDraft] = useState(value);
  const [section, setSection] = useState<"models" | "skills" | "terminal" | "git" | "appearance" | "privacy">("models");
  const [saving, setSaving] = useState(false);
  const [skills, setSkills] = useState<SkillSummary[]>([]);
  const [skillPreview, setSkillPreview] = useState<SkillPreview | null>(null);
  const [skillError, setSkillError] = useState("");
  const [skillBusy, setSkillBusy] = useState(false);
  const [providerSecret, setProviderSecret] = useState("");
  const [providerSecretSaved, setProviderSecretSaved] = useState(false);
  const [providerBusy, setProviderBusy] = useState(false);
  const [providerMessage, setProviderMessage] = useState<{ kind: "ok" | "error"; text: string } | null>(null);
  const [notificationIdentity, setNotificationIdentity] = useState<NotificationIdentityStatus | null>(null);
  const [notificationDiagnostics, setNotificationDiagnostics] = useState<NotificationDiagnostics | null>(null);
  const [notificationBusy, setNotificationBusy] = useState(false);
  const [notificationMessage, setNotificationMessage] = useState<{ kind: "ok" | "error"; text: string } | null>(null);
  const [runtimeDiagnostics, setRuntimeDiagnostics] = useState<RuntimeDiagnosticSnapshot | null>(null);
  const [modelPacks, setModelPacks] = useState<ModelPackInfo[]>([]);
  const [modelPackLoading, setModelPackLoading] = useState(false);
  const [runtimeBusy, setRuntimeBusy] = useState(false);
  const [runtimeMessage, setRuntimeMessage] = useState<{ kind: "ok" | "error"; text: string } | null>(null);
  const committedRef = useRef(false);
  useEffect(() => { if (section === "skills") void listSkills().then(setSkills).catch((error) => setSkillError(String(error))); }, [section]);
  useEffect(() => { if (section === "models") void getProviderSecretStatus().then((status) => setProviderSecretSaved(status.saved)).catch(() => setProviderSecretSaved(false)); }, [section]);
  useEffect(() => { if (section === "appearance") void Promise.all([getNotificationIdentityStatus(), getNotificationDiagnostics()]).then(([identity, diagnostics]) => { setNotificationIdentity(identity); setNotificationDiagnostics(diagnostics); }).catch((error) => { setNotificationIdentity(null); setNotificationDiagnostics(null); setNotificationMessage({ kind: "error", text: String(error) }); }); }, [section]);
  useEffect(() => { if (section === "models") void getRuntimeDiagnostics().then(setRuntimeDiagnostics).catch((error) => setRuntimeMessage({ kind: "error", text: String(error) })); }, [section]);
  useEffect(() => { if (section !== "models") return; setModelPackLoading(true); void listModelPacks().then(setModelPacks).catch((error) => setRuntimeMessage({ kind: "error", text: String(error) })).finally(() => setModelPackLoading(false)); }, [section]);
  useEffect(() => {
    const root = document.documentElement;
    root.style.setProperty("--ui-font-scale", String(uiFontScale(draft.uiFontSize)));
    root.style.setProperty("--code-font-scale", String(codeFontScale(draft.codeFontSize)));
    return () => {
      if (committedRef.current) return;
      root.style.setProperty("--ui-font-scale", String(uiFontScale(value.uiFontSize)));
      root.style.setProperty("--code-font-scale", String(codeFontScale(value.codeFontSize)));
    };
  }, [draft.uiFontSize, draft.codeFontSize, value.uiFontSize, value.codeFontSize]);
  useEffect(() => {
    const root = document.documentElement;
    const resolve = (theme: AppSettings["theme"]) => theme === "system"
      ? (window.matchMedia?.("(prefers-color-scheme: light)").matches ? "light" : "dark")
      : theme;
    root.dataset.themePreference = draft.theme;
    root.dataset.theme = resolve(draft.theme);
    void syncNativeTheme(draft.theme);
    root.classList.toggle("reduce-motion", draft.reduceMotion);
    return () => {
      if (committedRef.current) return;
      root.dataset.themePreference = value.theme;
      root.dataset.theme = resolve(value.theme);
      void syncNativeTheme(value.theme);
      root.classList.toggle("reduce-motion", value.reduceMotion);
    };
  }, [draft.theme, draft.reduceMotion, value.theme, value.reduceMotion]);
  const update = <K extends keyof AppSettings>(key: K, next: AppSettings[K]) => setDraft((current) => ({ ...current, [key]: next }));
  const selectedPack = modelPacks.find((pack) => pack.id === draft.bundledModelProfile && pack.state === "verified");
  const profileChanged = draft.bundledModelProfile !== value.bundledModelProfile;
  return <div className="settings-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}>
    <section className="settings-surface" role="dialog" aria-modal="true" aria-label="UCode 设置">
      <header><div><small>UCode Portable</small><h2>设置</h2></div><button onClick={onClose} aria-label="关闭设置">×</button></header>
      <div className="settings-layout">
        <nav aria-label="设置分类">
          {([['models','模型与任务'],['skills','技能'],['terminal','终端'],['git','Git'],['appearance','外观'],['privacy','隐私']] as const).map(([id,label]) => <button className={section === id ? "selected" : ""} onClick={() => setSection(id)} key={id}>{label}<span>›</span></button>)}
        </nav>
        <div className="settings-content" key={section}>
          {section === "models" && <><h3>模型与任务</h3><p>Controller、路径校验、审批和执行始终留在 UCode；这里只选择回答与代码生成模型。</p>
            <div className="provider-choice" role="group" aria-label="模型提供商">
              <button type="button" className={`provider-card ${draft.modelProvider === "bundled" ? "selected" : ""}`} aria-pressed={draft.modelProvider === "bundled"} onClick={() => { update("modelProvider", "bundled"); setProviderMessage(null); }}><span className="provider-glyph">◉</span><span><b>内置离线模型</b><small>{selectedPack ? `${selectedPack.name} · 已验证` : "只使用胶囊内已验证的 GGUF 模型包"}</small></span><i>便携默认</i></button>
              <button type="button" className={`provider-card ${draft.modelProvider === "openai-compatible" ? "selected" : ""}`} aria-pressed={draft.modelProvider === "openai-compatible"} onClick={() => { update("modelProvider", "openai-compatible"); setProviderMessage(null); }}><span className="provider-glyph">↗</span><span><b>OpenAI 兼容端点</b><small>LM Studio、Ollama 或你明确允许的网络服务</small></span><i>可选增强</i></button>
            </div>
            {draft.modelProvider === "bundled" && <section className="model-pack-panel" aria-label="已安装本地模型">
              <div className="model-pack-heading"><div><b>本地模型档位</b><small>只列出这枚胶囊中实际存在的模型包；选择变化会在保存时重启本地服务。</small></div><span>{modelPackLoading ? "验证中…" : `${modelPacks.filter((pack) => pack.state === "verified").length} 个可用`}</span></div>
              <div className="model-pack-list">
                {modelPacks.map((pack) => <button type="button" key={pack.id} className={`model-pack-row ${draft.bundledModelProfile === pack.id ? "selected" : ""} ${pack.state}`} disabled={pack.state !== "verified" || saving} aria-pressed={draft.bundledModelProfile === pack.id} onClick={() => { update("bundledModelProfile", pack.id); setRuntimeMessage(null); }}>
                  <span className="model-pack-radio" aria-hidden="true" />
                  <span className="model-pack-copy"><b>{pack.name}</b><small>{pack.state === "verified" ? `${pack.parameterClass} · ${(pack.bytes / 1024 ** 3).toFixed(2)} GiB · 建议内存 ${Math.ceil(pack.recommendedRamBytes / 1024 ** 3)} GiB · ${pack.contextSize.toLocaleString()} 上下文` : pack.error || "模型包未通过验证"}</small><em>{pack.taskSuitability.join(" · ")}</em></span>
                  <i>{pack.state === "verified" ? "已验证" : "不可用"}</i>
                </button>)}
                {!modelPackLoading && modelPacks.length === 0 && <div className="model-pack-empty"><b>没有可用的本地模型包</b><span>UCode 不会伪造 3B/7B 选项。请恢复胶囊中的 `model-packs` 与对应 GGUF 文件。</span></div>}
              </div>
              {profileChanged && <div className="model-pack-restart" role="status"><span>↻</span><div><b>保存后重启本地模型</b><small>当前任务和对话不会删除；运行中的生成应先结束。</small></div></div>}
            </section>}
            {draft.modelProvider === "openai-compatible" && <div className="provider-config">
              <div className="provider-notice"><b>内容边界</b><span>普通回答、项目摘要和待生成的代码会发送到该端点；工具权限与审批不会交给它。</span></div>
              <label className="setting-field"><span>Endpoint</span><input value={draft.providerBaseUrl} onChange={(event) => { update("providerBaseUrl", event.target.value); setProviderMessage(null); }} placeholder="http://127.0.0.1:1234/v1" spellCheck={false} /></label>
              <label className="setting-field"><span>Model</span><input value={draft.providerModel} onChange={(event) => { update("providerModel", event.target.value); setProviderMessage(null); }} placeholder="例如 qwen2.5-coder-7b-instruct" spellCheck={false} /></label>
              <label className="setting-field provider-key-field"><span>API key <small>{providerSecretSaved ? "已安全保存到本机" : "未保存（本机端点通常可留空）"}</small></span><input type="password" value={providerSecret} onChange={(event) => setProviderSecret(event.target.value)} placeholder={providerSecretSaved ? "已保存；输入新值可替换" : "不会写入 U 盘数据库"} autoComplete="new-password" /></label>
              <div className="provider-actions"><button type="button" className="quiet" disabled={providerBusy} onClick={async () => { setProviderBusy(true); setProviderMessage(null); try { const result = await testModelProvider(draft, providerSecret); setProviderMessage({ kind: "ok", text: `连接成功 · ${result.model}` }); } catch (error) { setProviderMessage({ kind: "error", text: String(error) }); } finally { setProviderBusy(false); } }}>{providerBusy ? "连接中…" : "测试连接"}</button><button type="button" className="quiet" disabled={providerBusy || !providerSecret} onClick={async () => { setProviderBusy(true); try { const status = await saveProviderSecret(providerSecret); setProviderSecretSaved(status.saved); setProviderSecret(""); setProviderMessage({ kind: "ok", text: "密钥已保存到 Windows 凭据管理器" }); } catch (error) { setProviderMessage({ kind: "error", text: String(error) }); } finally { setProviderBusy(false); } }}>保存密钥</button>{providerSecretSaved && <button type="button" className="danger" disabled={providerBusy} onClick={async () => { setProviderBusy(true); try { const status = await clearProviderSecret(); setProviderSecretSaved(status.saved); setProviderMessage({ kind: "ok", text: "本机密钥已清除" }); } catch (error) { setProviderMessage({ kind: "error", text: String(error) }); } finally { setProviderBusy(false); } }}>清除密钥</button>}</div>
              {providerMessage && <div className={`provider-result ${providerMessage.kind}`} role="status">{providerMessage.text}</div>}
            </div>}
            <RuntimeHealthCard diagnostics={runtimeDiagnostics} busy={runtimeBusy} message={runtimeMessage} onRefresh={async () => { setRuntimeBusy(true); setRuntimeMessage(null); try { setRuntimeDiagnostics(await getRuntimeDiagnostics()); } catch (error) { setRuntimeMessage({ kind: "error", text: String(error) }); } finally { setRuntimeBusy(false); } }} onRecover={async () => { const coreWasReady = runtimeDiagnostics ? runtimeCoreReady(runtimeDiagnostics) : false; setRuntimeBusy(true); setRuntimeMessage(null); try { const next = await recoverLocalRuntime(); setRuntimeDiagnostics(next); setRuntimeMessage({ kind: "ok", text: coreWasReady ? "已仅重试复杂规划服务；Agent、Coder 和 llama 没有重启。" : runtimeCoreReady(next) ? "Agent 与 Coder 已可用；Controller 如仍在启动会继续后台加载。" : "本地运行时恢复任务已完成。" }); } catch (error) { setRuntimeMessage({ kind: "error", text: String(error) }); try { setRuntimeDiagnostics(await getRuntimeDiagnostics()); } catch { /* Preserve the recovery error. */ } } finally { setRuntimeBusy(false); } }} />
            <SettingToggle label="逐 Token 流式输出" description="模型支持时立即显示真实解码 Token。" checked={draft.streamEnabled} onChange={(next) => update("streamEnabled", next)} />
          </>}
          {section === "skills" && <><div className="skills-heading"><div><h3>本地技能</h3><p>技能只补充模型的 system 说明，不会获得额外工具权限。</p></div><button className="approve" disabled={skillBusy} onClick={async () => { const path = await chooseSkillDirectory(); if (!path) return; setSkillBusy(true); setSkillError(""); try { setSkillPreview(await previewSkill(path)); } catch (error) { setSkillError(String(error)); } finally { setSkillBusy(false); } }}>导入目录</button></div>
            {skillError && <div className="skill-error" role="alert">{skillError}</div>}
            {skillPreview && <article className="skill-preview"><div><b>{skillPreview.name}</b><small>v{skillPreview.version} · {skillPreview.fileCount} 个文件 · {(skillPreview.totalBytes / 1024).toFixed(1)} KiB</small><p>{skillPreview.description || "未提供描述"}</p></div><span className={skillPreview.compatible ? "skill-ok" : "skill-warn"}>{skillPreview.compatible ? "可启用" : "仅可查看"}</span>{skillPreview.compatibilityReason && <em>{skillPreview.compatibilityReason}</em>}<div className="skill-actions"><button className="quiet" onClick={() => setSkillPreview(null)}>取消</button><button className="approve" disabled={skillBusy} onClick={async () => { setSkillBusy(true); try { setSkills(await installSkill(skillPreview)); setSkillPreview(null); } catch (error) { setSkillError(String(error)); } finally { setSkillBusy(false); } }}>批准安装</button></div></article>}
            <div className="skill-list">{skills.length === 0 && !skillPreview ? <div className="skill-empty">尚未安装技能。可导入包含 SKILL.md 的本地目录。</div> : skills.map((skill) => <article className="skill-card" key={skill.id}><div><b>{skill.name}</b><small>v{skill.version}</small><p>{skill.description || "无描述"}</p></div><span className={skill.compatible ? (skill.enabled ? "skill-ok" : "") : "skill-warn"}>{skill.enabled ? "已启用" : skill.compatible ? "未启用" : "不兼容"}</span>{skill.compatibilityReason && <em>{skill.compatibilityReason}</em>}<div className="skill-actions"><button className="quiet" disabled={skillBusy || !skill.compatible} onClick={async () => { if (!window.confirm(`${skill.enabled ? "停用" : "启用"}技能“${skill.name}”？`)) return; setSkillBusy(true); try { setSkills(await setSkillEnabled(skill.id, !skill.enabled)); } catch (error) { setSkillError(String(error)); } finally { setSkillBusy(false); } }}>{skill.enabled ? "停用" : "启用"}</button><button className="danger" disabled={skillBusy} onClick={async () => { if (!window.confirm(`卸载技能“${skill.name}”？文件会移入 .trash，可恢复。`)) return; setSkillBusy(true); try { setSkills(await uninstallSkill(skill.id)); } catch (error) { setSkillError(String(error)); } finally { setSkillBusy(false); } }}>卸载</button></div></article>)}</div>
          </>}
          {section === "terminal" && <><h3>终端</h3><p>命令始终在当前项目目录启动，并在执行前请求批准。</p><label className="setting-field"><span>Shell</span><input value={draft.terminalShell} onChange={(event) => update("terminalShell", event.target.value)} placeholder="powershell.exe" /></label></>}
          {section === "git" && <><h3>Git 身份</h3><p>提交时使用；留空则沿用仓库或系统 Git 配置。</p><label className="setting-field"><span>用户名</span><input value={draft.gitUserName} onChange={(event) => update("gitUserName", event.target.value)} /></label><label className="setting-field"><span>邮箱</span><input value={draft.gitUserEmail} onChange={(event) => update("gitUserEmail", event.target.value)} /></label></>}
          {section === "appearance" && <div className="typography-settings"><p>直接调整实际字号；修改会立即预览，不会缩放窗口或模糊内容。</p><FontSizeControl label="界面与聊天字号" value={uiFontSize(draft.uiFontSize)} min={UI_FONT_MIN} max={UI_FONT_MAX} onChange={(next) => update("uiFontSize", next)} /><FontSizeControl label="代码、Diff 与终端字号" value={codeFontSize(draft.codeFontSize)} min={CODE_FONT_MIN} max={CODE_FONT_MAX} onChange={(next) => update("codeFontSize", next)} /><button type="button" className="typography-reset quiet" disabled={draft.uiFontSize === UI_FONT_DEFAULT && draft.codeFontSize === CODE_FONT_DEFAULT} onClick={() => setDraft((current) => ({ ...current, uiFontSize: UI_FONT_DEFAULT, codeFontSize: CODE_FONT_DEFAULT }))}>恢复字号默认值</button></div>}
          {section === "appearance" && <><h3>外观与反馈</h3><label className="setting-field"><span>主题</span><select value={draft.theme} onChange={(event) => update("theme", event.target.value as AppSettings["theme"])}><option value="dark">深色</option><option value="light">浅色</option><option value="system">跟随系统</option></select></label><SettingToggle label="减少动态效果" description="保留必要状态变化，关闭位移和缩放动画。" checked={draft.reduceMotion} onChange={(next) => update("reduceMotion", next)} /><div className="sound-settings"><SettingToggle label="后台任务完成通知" description="仅在窗口失焦、最小化或隐藏到托盘时显示；不包含任务或文件内容。" checked={draft.completionNotificationsEnabled} onChange={(next) => update("completionNotificationsEnabled", next)} /><NotificationIdentityCard identity={notificationIdentity} diagnostics={notificationDiagnostics} busy={notificationBusy} message={notificationMessage} onRepair={async () => { setNotificationBusy(true); setNotificationMessage(null); try { const identity = await repairNotificationIdentity(); setNotificationIdentity(identity); setNotificationDiagnostics(await getNotificationDiagnostics()); setNotificationMessage({ kind: "ok", text: "UCode 通知身份已修复。" }); } catch (error) { setNotificationMessage({ kind: "error", text: String(error) }); } finally { setNotificationBusy(false); } }} onClear={async () => { if (!window.confirm("清除这台 Windows 上的 UCode 通知身份？这不会删除对话或项目，并会关闭后台完成通知。")) return; setNotificationBusy(true); setNotificationMessage(null); try { await onClearNotificationIdentity(); update("completionNotificationsEnabled", false); setNotificationIdentity({ appId: "dev.ucode.portable", displayName: "UCode", health: "missing", registered: false, iconUri: null, checkedAt: new Date().toISOString(), lastError: null }); setNotificationDiagnostics(await getNotificationDiagnostics()); setNotificationMessage({ kind: "ok", text: "已清除本机通知身份，并关闭后台完成通知。" }); } catch (error) { setNotificationMessage({ kind: "error", text: String(error) }); } finally { setNotificationBusy(false); } }} /><SettingToggle label="提示音" description="任务完成、等待审批或失败时播放本地短提示音。" checked={draft.soundEnabled} onChange={(next) => update("soundEnabled", next)} /><label><span>提示音音量 <b>{clampSoundVolume(draft.soundVolume)}%</b></span><input aria-label="提示音音量" type="range" min="0" max="100" step="1" value={clampSoundVolume(draft.soundVolume)} disabled={!draft.soundEnabled} onChange={(event) => update("soundVolume", clampSoundVolume(Number(event.target.value)))} /></label><button type="button" className="quiet" disabled={!draft.soundEnabled} onClick={() => previewAudioCue("completed", draft.soundVolume)}>试听提示音</button></div></>}
          {section === "privacy" && <><h3>隐私</h3><p>默认只允许 127.0.0.1 和 localhost 模型服务。</p><SettingToggle label="允许网络模型提供商" description="开启后才能配置非本机模型；项目内容可能离开设备。" checked={draft.allowNetworkProviders} onChange={(next) => update("allowNetworkProviders", next)} /></>}
        </div>
      </div>
      <footer><span>{runtimeMessage?.kind === "error" ? <em className="settings-save-error" role="alert">{runtimeMessage.text}</em> : "普通设置保存在 U 盘；模型密钥只保存在当前 Windows 用户凭据中。"}</span><div><button className="quiet" onClick={onClose}>取消</button><button className="approve" disabled={saving || (profileChanged && !selectedPack)} onClick={async () => { setSaving(true); setRuntimeMessage(null); committedRef.current = true; try { await onSave(draft); } catch (error) { committedRef.current = false; setRuntimeMessage({ kind: "error", text: String(error) }); } finally { setSaving(false); } }}>{saving ? (profileChanged ? "正在切换模型…" : "保存中…") : profileChanged ? "保存并重启模型" : "保存设置"}</button></div></footer>
    </section>
  </div>;
}

function SettingToggle({ label, description, checked, onChange }: { label: string; description: string; checked: boolean; onChange: (value: boolean) => void }) {
  return <label className="setting-toggle"><span><b>{label}</b><small>{description}</small></span><input type="checkbox" checked={checked} onChange={(event) => onChange(event.target.checked)} /><i /></label>;
}

function FontSizeControl({ label, value, min, max, onChange }: { label: string; value: number; min: number; max: number; onChange: (value: number) => void }) {
  return <fieldset className="font-size-control"><legend>{label}</legend><div><button type="button" aria-label={`${label}减小`} disabled={value <= min} onClick={() => onChange(value - 1)}>−</button><input aria-label={label} type="range" min={min} max={max} step="1" value={value} onChange={(event) => onChange(Number(event.target.value))} /><button type="button" aria-label={`${label}增大`} disabled={value >= max} onClick={() => onChange(value + 1)}>＋</button><output aria-live="polite">{value} px</output></div><small>{min}–{max} px</small></fieldset>;
}

export function RuntimeHealthCard({ diagnostics, busy, message, onRefresh, onRecover }: { diagnostics: RuntimeDiagnosticSnapshot | null; busy: boolean; message: { kind: "ok" | "error"; text: string } | null; onRefresh: () => Promise<void>; onRecover: () => Promise<void> }) {
  const overall = diagnostics?.overall ?? "starting";
  const coreReady = diagnostics ? runtimeCoreReady(diagnostics) : false;
  const controller = diagnostics?.services.find((service) => service.id === "controller");
  const controllerNeedsRecovery = coreReady && (controller?.status === "stopped" || controller?.status === "failed");
  const labels = { ready: "运行正常", starting: "正在启动", degraded: "部分服务异常", stopped: "服务已停止", missing: "运行时缺失" } as const;
  const descriptions = { ready: "本地协调、模型路由和生成服务均已通过健康检查。", starting: "UCode 正在启动本地服务，首次打开可能需要一些时间。", degraded: "核心仍可用；复杂规划暂不可用，普通对话和明确文件任务可继续。", stopped: "本地服务没有响应；对话和项目文件仍然安全。", missing: "便携胶囊旁未找到完整的本地运行时包。" } as const;
  const serviceLabels = { ready: "正常", starting: "启动中", stopped: "未响应", failed: "失败" } as const;
  const coreSummary = controllerNeedsRecovery ? descriptions.degraded : "Agent 与 Coder 已可用；复杂规划能力仍在加载。";
  return <section className="runtime-health-card" aria-label="本地运行时状态">
    <div className="runtime-health-heading"><div><b>本地运行时</b><small>{coreReady && overall !== "ready" ? coreSummary : descriptions[overall]}</small></div><span className={overall}>{diagnostics ? (coreReady && overall !== "ready" ? "核心可用" : labels[overall]) : "检查中"}</span></div>
    {diagnostics && <div className="runtime-service-grid">{diagnostics.services.map((service) => <div key={service.id}><i className={service.status} /><span>{service.label}</span><small>{serviceLabels[service.status]}</small></div>)}</div>}
    {diagnostics?.runtimeRoot && <code title={diagnostics.runtimeRoot}>{diagnostics.runtimeRoot}</code>}
    {diagnostics?.lastError && <p role="alert">{diagnostics.lastError}</p>}
    <div className="runtime-health-actions"><button type="button" className="quiet" disabled={busy} onClick={() => void onRefresh()}>{busy ? "检查中…" : "重新检查"}</button>{diagnostics && !coreReady && diagnostics.overall !== "missing" && <button type="button" className="approve" disabled={busy} onClick={() => void onRecover()}>{busy ? "恢复中…" : "恢复本地服务"}</button>}{controllerNeedsRecovery && <button type="button" className="approve" disabled={busy} onClick={() => void onRecover()}>{busy ? "重试中…" : "重试复杂规划"}</button>}</div>
    {message && <em className={message.kind} role="status">{message.text}</em>}
  </section>;
}

function runtimeCoreReady(diagnostics: Pick<RuntimeDiagnosticSnapshot, "services">): boolean {
  return ["agent", "coder"].every((id) => diagnostics.services.some((service) => service.id === id && service.status === "ready"));
}

export function NotificationIdentityCard({ identity, diagnostics, busy, message, onRepair, onClear }: { identity: NotificationIdentityStatus | null; diagnostics: NotificationDiagnostics | null; busy: boolean; message: { kind: "ok" | "error"; text: string } | null; onRepair: () => Promise<void>; onClear: () => Promise<void> }) {
  const health = identity?.health ?? "error";
  const labels = { ready: "UCode 已就绪", missing: "未注册", drifted: "需要修复", error: "检查失败" } as const;
  const descriptions = { ready: "Windows 已核验 UCode 名称和当前便携图标。", missing: "当前 Windows 用户尚未创建 UCode 通知身份。", drifted: "注册值、图标路径或图标文件与当前胶囊不一致。", error: "暂时无法核验 Windows 通知身份。" } as const;
  const telemetry = diagnostics?.telemetry;
  return <section className="notification-identity" aria-label="Windows 通知身份">
    <div className="notification-identity-heading"><div><b>Windows 通知身份</b><small>{descriptions[health]}</small></div><span className={health}>{labels[health]}</span></div>
    {identity?.iconUri && <code title={identity.iconUri}>{identity.iconUri}</code>}
    <dl aria-label="本次运行通知诊断"><div><dt>已显示</dt><dd>{telemetry?.shown ?? 0}</dd></div><div><dt>已点击</dt><dd>{telemetry?.activated ?? 0}</dd></div><div><dt>失败</dt><dd>{telemetry?.failed ?? 0}</dd></div></dl>
    {(identity?.lastError || telemetry?.lastError) && <p role="alert">{identity?.lastError || telemetry?.lastError}</p>}
    <div className="notification-identity-actions">{health !== "ready" && <button type="button" className="quiet" disabled={busy} onClick={() => void onRepair()}>{busy ? "修复中…" : "修复通知身份"}</button>}<button type="button" className="danger" disabled={busy || !identity?.registered} onClick={() => void onClear()}>{busy ? "处理中…" : "清除本机通知身份"}</button></div>
    {message && <em className={message.kind} role="status">{message.text}</em>}
  </section>;
}
