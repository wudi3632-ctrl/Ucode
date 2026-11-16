import fs from "node:fs";
import path from "node:path";
import { createHash, randomUUID } from "node:crypto";

export class AuditLog {
  constructor(dataDir) {
    fs.mkdirSync(dataDir, { recursive: true });
    this.file = path.join(dataDir, "audit.jsonl");
  }

  append(type, ticketId, payload = {}) {
    const event = { id: randomUUID(), at: new Date().toISOString(), type, ticketId, payload };
    fs.appendFileSync(this.file, `${JSON.stringify(event)}\n`, "utf8");
    return event;
  }

  events(ticketId) {
    if (!fs.existsSync(this.file)) return [];
    return fs.readFileSync(this.file, "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line)).filter((event) => !ticketId || event.ticketId === ticketId);
  }

  export(ticketId, destination) {
    const body = this.events(ticketId).map((event) => JSON.stringify(event)).join("\n") + "\n";
    fs.writeFileSync(destination, body, "utf8");
    return { events: this.events(ticketId).length, sha256: createHash("sha256").update(body).digest("hex") };
  }
}
