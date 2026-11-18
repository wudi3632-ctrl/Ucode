import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { validateExecutorEvent, validateExecutorRequest } from "../agent/executor/protocol.js";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const fixture = JSON.parse(fs.readFileSync(path.join(root, "protocol", "executor-v1.fixture.json"), "utf8"));

test("shared Executor v1 fixture is accepted without Node-owned ticket state", () => {
  assert.equal(validateExecutorRequest(structuredClone(fixture)).requestId, "request-fixture");
});

test("chat cannot smuggle action authority and action must carry it", () => {
  const chat = structuredClone(fixture);
  chat.mode = "chat";
  assert.throws(() => validateExecutorRequest(chat), /cannot carry action authority/);
  const action = structuredClone(fixture);
  action.mandate = null;
  assert.throws(() => validateExecutorRequest(action), /must be an object/);
});

test("candidate mode requires bounded strategy scope acceptance and write authority", () => {
  const request = structuredClone(fixture);
  request.mode = "candidate";
  request.candidate = {
    raceId: "race-1",
    candidateId: "candidate-a",
    strategy: "minimal_change",
    allowedPaths: ["README.md"],
    acceptance: ["README heading is UCode"],
    generationAttempt: 1,
    feedback: null,
  };
  assert.equal(validateExecutorRequest(request).candidate.strategy, "minimal_change");

  const invalidStrategy = structuredClone(request);
  invalidStrategy.candidate.strategy = "creative_guess";
  assert.throws(() => validateExecutorRequest(invalidStrategy), /strategy is invalid/);

  const escaping = structuredClone(request);
  escaping.candidate.allowedPaths = ["../outside.txt"];
  assert.throws(() => validateExecutorRequest(escaping), /stay relative/);

  const overBudget = structuredClone(request);
  overBudget.candidate.generationAttempt = 3;
  assert.throws(() => validateExecutorRequest(overBudget), /attempt must be 1 or 2/);

  const readOnly = structuredClone(request);
  readOnly.mandate.capabilities = ["read", "search"];
  assert.throws(() => validateExecutorRequest(readOnly), /requires propose_write/);
});

test("workspace and attachment paths cannot escape the project", () => {
  const request = structuredClone(fixture);
  request.workspace.files[0].path = "../outside.txt";
  assert.throws(() => validateExecutorRequest(request), /stay relative/);

  const attachment = structuredClone(fixture);
  attachment.attachmentToken = "token";
  attachment.attachments = [{
    id: "attachment",
    displayName: "private.txt",
    source: "external_readonly",
    relativePath: "C:/private.txt",
    contentDigest: "0".repeat(64),
    content: "private",
  }];
  assert.throws(() => validateExecutorRequest(attachment), /cannot expose a path/);
});

test("event protocol has factual activity but no private reasoning event", () => {
  const request = validateExecutorRequest(structuredClone(fixture));
  assert.equal(validateExecutorEvent({
    version: request.version,
    requestId: request.requestId,
    turnId: request.turnId,
    sequence: 1,
    timestamp: "2026-01-01T00:00:00Z",
    type: "activity",
    activityId: "read-1",
    kind: "read_file",
    title: "读取 README.md",
    status: "succeeded",
    evidence: { paths: ["README.md"] },
  }, request).type, "activity");
  assert.throws(() => validateExecutorEvent({
    version: request.version,
    requestId: request.requestId,
    turnId: request.turnId,
    sequence: 2,
    timestamp: "2026-01-01T00:00:01Z",
    type: "thought",
    reasoning: "hidden",
  }, request), /event type|Private reasoning/);
});
