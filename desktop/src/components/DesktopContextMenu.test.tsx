import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { DesktopContextMenu, isReloadShortcut } from "./DesktopContextMenu";

afterEach(cleanup);

describe("desktop WebView guards", () => {
  it("recognizes browser reload shortcuts without blocking normal desktop shortcuts", () => {
    expect(isReloadShortcut({ key: "F5", ctrlKey: false, metaKey: false })).toBe(true);
    expect(isReloadShortcut({ key: "r", ctrlKey: true, metaKey: false })).toBe(true);
    expect(isReloadShortcut({ key: "R", ctrlKey: true, metaKey: false })).toBe(true);
    expect(isReloadShortcut({ key: "c", ctrlKey: true, metaKey: false })).toBe(false);
  });

  it("suppresses the browser menu on blank space", () => {
    render(<><main data-testid="surface">workspace</main><DesktopContextMenu enabled /></>);
    const event = new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 20, clientY: 20 });
    screen.getByTestId("surface").dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    expect(screen.queryByRole("menu", { name: "文本操作" })).not.toBeInTheDocument();
  });

  it("replaces the WebView menu with text editing actions", () => {
    render(<><textarea aria-label="message" defaultValue="hello world" /><DesktopContextMenu enabled /></>);
    const input = screen.getByLabelText("message") as HTMLTextAreaElement;
    input.setSelectionRange(0, 5);
    fireEvent.contextMenu(input, { clientX: 32, clientY: 48 });
    const menu = screen.getByRole("menu", { name: "文本操作" });
    expect(menu).toHaveTextContent("剪切");
    expect(menu).toHaveTextContent("复制");
    expect(menu).toHaveTextContent("粘贴");
    expect(menu).toHaveTextContent("全选");
    expect(menu).not.toHaveTextContent("刷新");
  });

  it("blocks F5 and Ctrl+R at the window boundary", () => {
    render(<DesktopContextMenu enabled />);
    const f5 = new KeyboardEvent("keydown", { key: "F5", bubbles: true, cancelable: true });
    const ctrlR = new KeyboardEvent("keydown", { key: "r", ctrlKey: true, bubbles: true, cancelable: true });
    window.dispatchEvent(f5);
    window.dispatchEvent(ctrlR);
    expect(f5.defaultPrevented).toBe(true);
    expect(ctrlR.defaultPrevented).toBe(true);
  });
});
