import { useEffect, useRef, useState } from "react";
import type { ProjectSummary, TicketSummary } from "../domain/types";

interface SidebarProps {
  projects: ProjectSummary[];
  tickets: TicketSummary[];
  activeTicketId: string | null;
  activeProjectId: string | null;
  portableRoot: string;
  onSelectProject: (project: ProjectSummary) => void;
  onSelectTicket: (id: string) => void;
  onNewTask: () => void;
  onOpenProject: () => void;
  onRenameTicket: (id: string, title: string) => Promise<void>;
  onDeleteTicket: (id: string) => Promise<void>;
  onPinTicket: (id: string, pinned: boolean) => Promise<void>;
  onArchiveTicket: (id: string) => Promise<void>;
  onRestoreTicket: (id: string) => Promise<void>;
  onOpenSettings: () => void;
  onOpenHelp: () => void;
}

export function Sidebar({ projects, tickets, activeTicketId, activeProjectId, portableRoot, onSelectProject, onSelectTicket, onNewTask, onOpenProject, onRenameTicket, onDeleteTicket, onPinTicket, onArchiveTicket, onRestoreTicket, onOpenSettings, onOpenHelp }: SidebarProps) {
  const [menuTicketId, setMenuTicketId] = useState<string | null>(null);
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [draftTitle, setDraftTitle] = useState("");
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const [search, setSearch] = useState("");
  const [archiveOpen, setArchiveOpen] = useState(false);
  useEffect(() => {
    const close = (event: PointerEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) setMenuTicketId(null);
    };
    window.addEventListener("pointerdown", close);
    return () => window.removeEventListener("pointerdown", close);
  }, []);
  const stateLabel = (state: TicketSummary["state"]) => ({
    open: "等待开始", queued: "排队中", running: "执行中", waiting: "等待处理",
    stopping: "正在停止", stopped: "已停止", completed: "已完成", failed: "失败", rolled_back: "已回滚",
  })[state];
  const matches = (ticket: TicketSummary) => !search.trim() || `${ticket.title} ${ticket.request}`.toLocaleLowerCase().includes(search.trim().toLocaleLowerCase());
  const projectTickets = tickets.filter((ticket) => ticket.projectId === activeProjectId);
  const visible = projectTickets.filter((ticket) => !ticket.archivedAt && matches(ticket));
  const pinned = visible.filter((ticket) => ticket.pinned);
  const recent = visible.filter((ticket) => !ticket.pinned);
  const archived = projectTickets.filter((ticket) => ticket.archivedAt && matches(ticket));
  const renderTicket = (ticket: TicketSummary) => (
    <div className={`conversation-item ${ticket.id === activeTicketId ? "selected" : ""}`} key={ticket.id}>
      {renamingId === ticket.id ? (
        <form className="conversation-rename" onSubmit={async (event) => { event.preventDefault(); if (!draftTitle.trim()) return; await onRenameTicket(ticket.id, draftTitle); setRenamingId(null); }}>
          <input value={draftTitle} onChange={(event) => setDraftTitle(event.target.value)} autoFocus aria-label="会话名称" />
          <button type="submit">保存</button><button type="button" onClick={() => setRenamingId(null)}>取消</button>
        </form>
      ) : deletingId === ticket.id ? (
        <div className="delete-confirm"><span>删除这个会话？</span><button onClick={async () => { await onDeleteTicket(ticket.id); setDeletingId(null); }}>删除</button><button onClick={() => setDeletingId(null)}>取消</button></div>
      ) : <>
        <button className="conversation-row" onClick={() => onSelectTicket(ticket.id)} title={ticket.title}><span className="conversation-title-text">{ticket.pinned && <i className="pin-mark">◆</i>}{ticket.title}</span><small><i className={`conversation-state-dot ${ticket.state}`} />{stateLabel(ticket.state)}</small></button>
        <button className="conversation-menu-trigger" aria-label={`${ticket.title} 会话操作`} aria-expanded={menuTicketId === ticket.id} onClick={(event) => { event.stopPropagation(); setMenuTicketId((value) => value === ticket.id ? null : ticket.id); }}>•••</button>
        {menuTicketId === ticket.id && <div className="conversation-menu" ref={menuRef} role="menu">
          {!ticket.archivedAt && <button role="menuitem" onClick={async () => { await onPinTicket(ticket.id, !ticket.pinned); setMenuTicketId(null); }}>{ticket.pinned ? "取消固定" : "固定会话"}</button>}
          <button role="menuitem" onClick={() => { setDraftTitle(ticket.title); setRenamingId(ticket.id); setMenuTicketId(null); }}>重命名</button>
          <button role="menuitem" onClick={async () => { ticket.archivedAt ? await onRestoreTicket(ticket.id) : await onArchiveTicket(ticket.id); setMenuTicketId(null); }}>{ticket.archivedAt ? "恢复会话" : "归档会话"}</button>
          <button role="menuitem" className="danger-action" onClick={() => { setDeletingId(ticket.id); setMenuTicketId(null); }}>删除会话</button>
        </div>}
      </>}
    </div>
  );
  return (
    <aside className="sidebar">
      <div className="brand" aria-label="UCode Portable">U<span>Code</span></div>
      <button className="primary new-task" onClick={projects.length ? onNewTask : onOpenProject}>
        {projects.length ? "＋ 新任务" : "◇ 打开项目"}
      </button>

      <section className="nav-section projects">
        <p className="section-label">项目</p>
        {projects.map((project) => (
          <button className={`nav-row ${project.id === activeProjectId ? "active" : ""}`} key={project.id} onClick={() => onSelectProject(project)} aria-pressed={project.id === activeProjectId}>
            <span className="project-mark">◇</span>
            <span><b>{project.name}</b><small>{project.trusted ? "已信任" : "未信任"}</small></span>
          </button>
        ))}
        {projects.length > 0 && <button className="open-project" onClick={onOpenProject}>＋ 打开其他项目</button>}
      </section>

      <section className="nav-section conversations">
        <div className="conversation-section-heading"><p className="section-label">会话</p><span>{visible.length}</span></div>
        <label className="conversation-search"><span>⌕</span><input aria-label="搜索会话" value={search} onChange={(event) => setSearch(event.target.value)} placeholder="搜索会话" />{search && <button onClick={() => setSearch("")} aria-label="清除搜索">×</button>}</label>
        {pinned.length > 0 && <><p className="conversation-group">已固定</p>{pinned.map(renderTicket)}</>}
        {recent.length > 0 && <>{pinned.length > 0 && <p className="conversation-group">最近</p>}{recent.map(renderTicket)}</>}
        {!visible.length && <p className="conversation-empty">{search ? "没有匹配的会话" : "还没有会话"}</p>}
        {archived.length > 0 && <div className="archive-group"><button onClick={() => setArchiveOpen((value) => !value)} aria-expanded={archiveOpen}><span className={archiveOpen ? "open" : ""}>›</span>已归档 <small>{archived.length}</small></button>{archiveOpen && <div>{archived.map(renderTicket)}</div>}</div>}
      </section>

      <div className="portable-state">
        <span className="status-dot" /> 数据已保存
        <small title={portableRoot}>{portableRoot}</small>
        <div><button onClick={onOpenHelp}>? 帮助</button><button onClick={onOpenSettings}>⚙ 设置</button></div>
      </div>
    </aside>
  );
}
