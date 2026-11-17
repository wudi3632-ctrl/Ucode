import { useEffect, useState } from "react";

type EditableTarget = HTMLInputElement | HTMLTextAreaElement;

interface MenuState {
  x: number;
  y: number;
  editable: EditableTarget | null;
  selectedText: string;
}

interface DesktopContextMenuProps {
  enabled?: boolean;
}

export function DesktopContextMenu({ enabled = import.meta.env.PROD }: DesktopContextMenuProps) {
  const [menu, setMenu] = useState<MenuState | null>(null);

  useEffect(() => {
    if (!enabled) return;
    const close = (event?: Event) => {
      if (event?.target instanceof Element && event.target.closest(".desktop-context-menu")) return;
      setMenu(null);
    };
    const contextMenu = (event: MouseEvent) => {
      event.preventDefault();
      const editable = editableTarget(event.target);
      const selectedText = editable ? editableSelection(editable) : window.getSelection()?.toString() ?? "";
      if (!editable && !selectedText) {
        setMenu(null);
        return;
      }
      setMenu({
        x: Math.max(8, Math.min(event.clientX, window.innerWidth - 188)),
        y: Math.max(8, Math.min(event.clientY, window.innerHeight - (editable ? 174 : 48))),
        editable,
        selectedText,
      });
    };
    const keyDown = (event: KeyboardEvent) => {
      if (isReloadShortcut(event)) {
        event.preventDefault();
        event.stopPropagation();
        return;
      }
      if (event.key === "Escape") setMenu(null);
    };
    window.addEventListener("contextmenu", contextMenu, true);
    window.addEventListener("keydown", keyDown, true);
    window.addEventListener("pointerdown", close, true);
    window.addEventListener("blur", close);
    window.addEventListener("resize", close);
    window.addEventListener("scroll", close, true);
    return () => {
      window.removeEventListener("contextmenu", contextMenu, true);
      window.removeEventListener("keydown", keyDown, true);
      window.removeEventListener("pointerdown", close, true);
      window.removeEventListener("blur", close);
      window.removeEventListener("resize", close);
      window.removeEventListener("scroll", close, true);
    };
  }, [enabled]);

  if (!menu) return null;
  const writable = Boolean(menu.editable && !menu.editable.readOnly && !menu.editable.disabled);
  const copy = async () => {
    if (menu.selectedText) await writeClipboard(menu.selectedText);
    setMenu(null);
  };
  const cut = async () => {
    if (!menu.editable || !writable) return;
    const start = menu.editable.selectionStart ?? 0;
    const end = menu.editable.selectionEnd ?? start;
    if (end > start) {
      await writeClipboard(menu.editable.value.slice(start, end));
      replaceEditableSelection(menu.editable, "", "deleteByCut");
    }
    setMenu(null);
  };
  const paste = async () => {
    if (!menu.editable || !writable) return;
    try {
      replaceEditableSelection(menu.editable, await navigator.clipboard.readText(), "insertFromPaste");
    } catch {
      // Clipboard access can be denied by Windows policy. Keyboard paste remains available.
    }
    setMenu(null);
  };
  const selectAll = () => {
    menu.editable?.focus();
    menu.editable?.select();
    setMenu(null);
  };

  return <div className="desktop-context-menu" role="menu" aria-label="文本操作" style={{ left: menu.x, top: menu.y }}>
    {menu.editable && <>
      <button role="menuitem" disabled={!writable || !menu.selectedText} onClick={() => void cut()}><span>剪切</span><kbd>Ctrl+X</kbd></button>
      <button role="menuitem" disabled={!menu.selectedText} onClick={() => void copy()}><span>复制</span><kbd>Ctrl+C</kbd></button>
      <button role="menuitem" disabled={!writable} onClick={() => void paste()}><span>粘贴</span><kbd>Ctrl+V</kbd></button>
      <i />
      <button role="menuitem" onClick={selectAll}><span>全选</span><kbd>Ctrl+A</kbd></button>
    </>}
    {!menu.editable && <button role="menuitem" onClick={() => void copy()}><span>复制</span><kbd>Ctrl+C</kbd></button>}
  </div>;
}

export function isReloadShortcut(event: Pick<KeyboardEvent, "key" | "ctrlKey" | "metaKey">) {
  return event.key === "F5" || ((event.ctrlKey || event.metaKey) && event.key.toLocaleLowerCase() === "r");
}

function editableTarget(target: EventTarget | null): EditableTarget | null {
  if (!(target instanceof Element)) return null;
  const editable = target.closest("input, textarea");
  if (editable instanceof HTMLTextAreaElement) return editable;
  if (!(editable instanceof HTMLInputElement)) return null;
  return ["text", "search", "url", "email", "tel", "password"].includes(editable.type) ? editable : null;
}

function editableSelection(target: EditableTarget) {
  const start = target.selectionStart ?? 0;
  const end = target.selectionEnd ?? start;
  return target.value.slice(start, end);
}

async function writeClipboard(text: string) {
  if (!text) return;
  try { await navigator.clipboard.writeText(text); }
  catch { /* Ctrl+C remains available if clipboard policy blocks the WebView API. */ }
}

function replaceEditableSelection(target: EditableTarget, value: string, inputType: string) {
  const start = target.selectionStart ?? target.value.length;
  const end = target.selectionEnd ?? start;
  const next = `${target.value.slice(0, start)}${value}${target.value.slice(end)}`;
  const setter = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(target), "value")?.set;
  setter?.call(target, next);
  const cursor = start + value.length;
  target.setSelectionRange(cursor, cursor);
  target.dispatchEvent(new InputEvent("input", { bubbles: true, inputType, data: value || null }));
  target.focus();
}
