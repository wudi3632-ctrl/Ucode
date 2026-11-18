import { AppError } from "../shared/errors.js";

const DEFAULT_MAX_BYTES = 1024 * 1024;

export async function readJsonBody(request, { maxBytes = DEFAULT_MAX_BYTES } = {}) {
  const chunks = [];
  let size = 0;
  for await (const chunk of request) {
    const bytes = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    size += bytes.length;
    if (size > maxBytes) throw new AppError("body_too_large", "Request body exceeds 1 MiB", 413);
    chunks.push(bytes);
  }
  const value = Buffer.concat(chunks, size).toString("utf8");
  try { return value ? JSON.parse(value) : {}; }
  catch { throw new AppError("invalid_json", "Request body must be valid JSON", 400); }
}
