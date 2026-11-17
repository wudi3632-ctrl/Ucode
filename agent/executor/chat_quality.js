const EXACT_REPLY_PATTERNS = [
  /(?:^|[，,。；;：:！？!?\s])(?:请)?(?:只|仅)(?:需|要)?(?:回答|回复|输出|说)\s*[：:]?\s*[`'“”"]?([^\n，,。；;！？!?]{1,160}?)[`'“”"]?(?=$|[，,。；;！？!?])/iu,
  /(?:^|\s)(?:please\s+)?(?:reply|respond|answer|output)\s+(?:with\s+)?(?:only|exactly|only\s+with|exactly\s+with)\s*[`'"]?([^\n,.;!?]{1,160}?)[`'"]?(?=$|[,.;!?])/iu,
  /(?:^|\s)(?:please\s+)?(?:only|exactly)\s+(?:reply|respond|answer|output)(?:\s+with)?\s*[`'"]?([^\n,.;!?]{1,160}?)[`'"]?(?=$|[,.;!?])/iu,
];

export function exactReplyConstraint(request) {
  const value = String(request || "").trim();
  for (const pattern of EXACT_REPLY_PATTERNS) {
    const match = value.match(pattern);
    if (!match) continue;
    const target = stripBalancedQuotes(match[1].trim());
    if (!target || target.length > 160 || /[\r\n]/.test(target)) return null;
    return { target };
  }
  return null;
}

export function exactReplyMatches(content, constraint) {
  return String(content || "").trim() === constraint?.target;
}

export function exactReplyRetryMessages(target) {
  return [
    {
      role: "system",
      content: "Copy the requested text exactly. Treat it as an opaque string, not as code, a variable, an expression, or a boolean. Do not explain, translate, evaluate, quote, or format it.",
    },
    { role: "user", content: "只回答 alpha_beta" },
    { role: "assistant", content: "alpha_beta" },
    { role: "user", content: "只回答 hello_world" },
    { role: "assistant", content: "hello_world" },
    { role: "user", content: `只回答 ${target}` },
  ];
}

export function oneSentenceConstraint(request) {
  return /(?:请|用|以)?(?:只)?(?:用)?一句话(?:回答|解释|说明|概括|总结|介绍)?|(?:in|with)\s+(?:exactly\s+)?one\s+sentence/iu.test(String(request || ""));
}

export function enforceOneSentence(content) {
  const value = String(content || "").trim();
  if (!value) return value;
  const compact = value
    .split(/\r?\n/u)
    .map((line) => line.trim().replace(/^[-*•]\s+/u, ""))
    .filter(Boolean)
    .join("，")
    .replace(/[。！？!?]+(?=\s*\S)/gu, "，")
    .replace(/，{2,}/gu, "，")
    .replace(/\s+/gu, " ")
    .trim();
  return /[。！？!?]$/u.test(compact) ? compact : `${compact}。`;
}

function stripBalancedQuotes(value) {
  const pairs = new Map([["`", "`"], ["'", "'"], ['"', '"'], ["“", "”"], ["‘", "’"]]);
  const closing = pairs.get(value[0]);
  return closing && value.endsWith(closing) && value.length >= 2 ? value.slice(1, -1).trim() : value;
}
