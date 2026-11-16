export function log(level, event, fields = {}) {
  process.stdout.write(`${JSON.stringify({ at: new Date().toISOString(), level, event, ...fields })}\n`);
}
