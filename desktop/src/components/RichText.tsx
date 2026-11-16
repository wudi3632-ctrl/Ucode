import { Children, isValidElement, type MouseEvent, type ReactNode, useState } from "react";
import Markdown, { defaultUrlTransform } from "react-markdown";
import remarkGfm from "remark-gfm";
import { openExternalUrl } from "../bridge/desktop";

export function RichText({ content, streaming = false }: { content: string; streaming?: boolean }) {
  const markdown = normalizeMarkdownContent(content);
  return (
    <div className={`rich-text ${streaming ? "is-streaming" : ""}`}>
      <Markdown
        remarkPlugins={[remarkGfm]}
        urlTransform={(url) => safeUrl(url) ? defaultUrlTransform(url) : ""}
        components={{
          pre: ({ children }) => <CodeBlockFromPre>{children}</CodeBlockFromPre>,
          code: ({ className, children, ...props }) => <code className={className ? `${className} inline-code` : "inline-code"} {...props}>{children}</code>,
          a: ({ href, children, ...props }) => href ? <a href={href} target="_blank" rel="noreferrer noopener" onClick={(event) => void openLink(event, href)} {...props}>{children}</a> : <span className="blocked-link" title="该链接未通过安全校验">{children}</span>,
        }}
      >{markdown}</Markdown>
      <span className={streaming ? "stream-cursor" : "stream-cursor hidden"} />
    </div>
  );
}

function normalizeMarkdownContent(value: string) {
  if (/```/.test(value)) return value;
  const literalBreaks = value.match(/\\n/g)?.length ?? 0;
  const expanded = literalBreaks >= 2 ? value.replace(/\\r\\n|\\n/g, "\n").replace(/\\t/g, "\t") : value;
  const trimmed = expanded.trim();
  const codeLike = /^(?:async\s+def|def|class|from\s+\S+\s+import|import\s+\S+|function|export\s+(?:default\s+)?(?:function|class|const|let|var)|const|let|var|#include\b|fn\s+\w+)/.test(trimmed);
  if (!codeLike || !trimmed.includes("\n")) return expanded;
  const language = /^(?:async\s+def|def|class|from\s+\S+\s+import|import\s+\S+)/.test(trimmed)
    ? "python"
    : /^#include\b/.test(trimmed)
      ? "cpp"
      : /^fn\s+\w+/.test(trimmed)
        ? "rust"
        : "javascript";
  return `\`\`\`${language}\n${trimmed}\n\`\`\``;
}

async function openLink(event: MouseEvent<HTMLAnchorElement>, href: string) {
  event.preventDefault();
  event.stopPropagation();
  await openExternalUrl(href);
}

function CodeBlockFromPre({ children }: { children: ReactNode }) {
  const child = Children.count(children) === 1 ? Children.only(children) : null;
  const props = isValidElement<{ className?: string; children?: ReactNode }>(child) ? child.props : null;
  const language = props?.className?.match(/language-([\w.+-]+)/)?.[1] || "text";
  const content = textFromChildren(props?.children ?? children).replace(/\n$/, "");
  return <CodeBlock language={language} content={content} />;
}

function CodeBlock({ language, content }: { language: string; content: string }) {
  const [copied, setCopied] = useState(false);
  async function copy() {
    await navigator.clipboard.writeText(content);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1_200);
  }
  return (
    <div className="markdown-code-block">
      <div className="markdown-code-heading"><span>{language}</span><button type="button" onClick={() => void copy()} aria-label={`复制 ${language} 代码块`}>{copied ? "已复制" : "复制"}</button></div>
      <pre><code className={`language-${language}`}>{content}</code></pre>
    </div>
  );
}

function textFromChildren(value: ReactNode): string {
  if (typeof value === "string" || typeof value === "number") return String(value);
  if (Array.isArray(value)) return value.map(textFromChildren).join("");
  return "";
}

function safeUrl(value: string) {
  if (!value) return false;
  try { return ["http:", "https:", "mailto:"].includes(new URL(value).protocol); }
  catch { return false; }
}
