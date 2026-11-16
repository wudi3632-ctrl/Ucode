import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { RichText } from "./RichText";
import { openExternalUrl } from "../bridge/desktop";

vi.mock("../bridge/desktop", () => ({ openExternalUrl: vi.fn().mockResolvedValue(undefined) }));

describe("RichText", () => {
  const writeText = vi.fn().mockResolvedValue(undefined);

  beforeEach(() => {
    writeText.mockClear();
    vi.mocked(openExternalUrl).mockClear();
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  });

  it("renders GFM structure and copies a code block without executing raw HTML", async () => {
    render(<RichText content={'## 结论\n\n- **安全**\n- `快速`\n\n| 项目 | 状态 |\n| --- | --- |\n| 流式 | 正常 |\n\n```ts\nconst value = 1;\n```\n\n<img src=x onerror="alert(1)">'} />);
    expect(screen.getByRole("heading", { name: "结论" })).toBeInTheDocument();
    expect(screen.getByRole("table")).toHaveTextContent("流式正常");
    expect(document.querySelector("img")).toBeNull();
    const copy = screen.getByRole("button", { name: "复制 ts 代码块" });
    fireEvent.click(copy);
    expect(writeText).toHaveBeenCalledWith("const value = 1;");
    expect(await screen.findByText("已复制")).toBeInTheDocument();
  });

  it("delegates safe links to the desktop shell without navigating the webview", () => {
    render(<RichText content={'[文档](https://example.com/docs) [危险](javascript:alert(1))'} />);
    const safe = screen.getByRole("link", { name: "文档" });
    expect(safe).toHaveAttribute("target", "_blank");
    expect(safe).toHaveAttribute("rel", expect.stringContaining("noopener"));
    const click = new MouseEvent("click", { bubbles: true, cancelable: true });
    safe.dispatchEvent(click);
    expect(click.defaultPrevented).toBe(true);
    expect(openExternalUrl).toHaveBeenCalledWith("https://example.com/docs");
    expect(screen.queryByRole("link", { name: "危险" })).not.toBeInTheDocument();
    expect(screen.getByText("危险")).toHaveAttribute("title", "该链接未通过安全校验");
  });

  it("keeps incomplete streaming markdown readable until the fence closes", () => {
    const view = render(<RichText content={'正在生成：\n\n```python\nprint("hello")'} streaming />);
    expect(screen.getByText('print("hello")')).toBeInTheDocument();
    expect(view.container.querySelector(".stream-cursor:not(.hidden)")).toBeInTheDocument();

    view.rerender(<RichText content={'正在生成：\n\n```python\nprint("hello")\n```'} />);
    expect(screen.getByRole("button", { name: "复制 python 代码块" })).toBeInTheDocument();
    expect(view.container.querySelector(".stream-cursor:not(.hidden)")).toBeNull();
  });

  it("repairs legacy code answers that contain escaped newlines and no fence", () => {
    const view = render(<RichText content={'def add(a, b):\\n    """Return the sum of two numbers."""\\n    return a + b\\n'} />);

    expect(view.container.querySelector('button[aria-label="复制 python 代码块"]')).toBeInTheDocument();
    expect(view.container.querySelector("code.language-python")).toHaveTextContent(/def add\(a, b\):/);
    expect(view.container.textContent).not.toMatch(/\\n/);
  });

  it("renders a long answer without leaving historical streaming state behind", () => {
    const content = Array.from({ length: 120 }, (_, index) => `- 第 ${index + 1} 项：保持历史内容稳定`).join("\n");
    const view = render(<RichText content={content} />);
    expect(view.container.querySelectorAll("li")).toHaveLength(120);
    expect(view.container.querySelector(".rich-text.is-streaming")).toBeNull();
    expect(view.container.querySelector(".stream-cursor:not(.hidden)")).toBeNull();
  });
});
