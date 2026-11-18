import { describe, expect, it } from "vitest";
import { candidateRaceRequestFromPrompt } from "./candidateRace";

describe("candidate race request gate", () => {
  it("activates only for explicit multi-file mutations with mechanical acceptance", () => {
    expect(candidateRaceRequestFromPrompt("修改 `src/cart.ts` 和 `tests/cart.test.ts`，然后运行 npm test")).toEqual({
      targets: [
        { path: "src/cart.ts", operation: "modify" },
        { path: "tests/cart.test.ts", operation: "modify" },
      ],
      allowedPaths: ["src/cart.ts", "tests/cart.test.ts"],
      acceptance: ["npm test"],
    });
    expect(candidateRaceRequestFromPrompt("创建 api.py 和 test_api.py，并运行 pytest")).toEqual({
      targets: [
        { path: "api.py", operation: "create" },
        { path: "test_api.py", operation: "create" },
      ],
      allowedPaths: ["api.py", "test_api.py"],
      acceptance: ["pytest"],
    });
  });

  it("keeps chat, single-file and unverified work on the normal route", () => {
    expect(candidateRaceRequestFromPrompt("你好，你能做什么？")).toBeNull();
    expect(candidateRaceRequestFromPrompt("修改 `src/cart.ts` 并运行 npm test")).toBeNull();
    expect(candidateRaceRequestFromPrompt("修改 `src/cart.ts` 和 `tests/cart.test.ts`")).toBeNull();
    expect(candidateRaceRequestFromPrompt("解释 src/cart.ts 和 tests/cart.test.ts")).toBeNull();
  });

  it("rejects unsafe paths and accepts only verifier-whitelisted scripts", () => {
    expect(candidateRaceRequestFromPrompt("修改 `../src/a.ts` 和 `C:/src/b.ts`，运行 npm test")).toBeNull();
    expect(candidateRaceRequestFromPrompt("修改 `src/a.ts` 和 `src/b.ts`，运行 npm run deploy")).toBeNull();
    expect(candidateRaceRequestFromPrompt("修改 src/a.ts 和 src/b.ts，运行 npm run typecheck")).toEqual({
      targets: [
        { path: "src/a.ts", operation: "modify" },
        { path: "src/b.ts", operation: "modify" },
      ],
      allowedPaths: ["src/a.ts", "src/b.ts"],
      acceptance: ["npm run typecheck"],
    });
  });

  it("extracts an explicit operation for every mixed target", () => {
    expect(candidateRaceRequestFromPrompt("创建 `a.py`；修改 `b.py`；删除 `c.py`；最后运行 pytest")).toEqual({
      targets: [
        { path: "a.py", operation: "create" },
        { path: "b.py", operation: "modify" },
        { path: "c.py", operation: "delete" },
      ],
      allowedPaths: ["a.py", "b.py", "c.py"],
      acceptance: ["pytest"],
    });
    expect(candidateRaceRequestFromPrompt("处理 `a.py` 和 `b.py`，然后运行 pytest")).toBeNull();
  });

  it("recognizes the real three-file desktop acceptance prompt", () => {
    expect(candidateRaceRequestFromPrompt("创建 `ucodebench/constants.py`，内容为 `DEFAULT_CURRENCY = \"CNY\"`；修改 `ucodebench/report.py`，把 `items=` 改为 `count=`；删除 `plugins/alpha/config.py`；三个操作必须是一个待审批批次，批准后运行 pytest。")).toEqual({
      targets: [
        { path: "ucodebench/constants.py", operation: "create" },
        { path: "ucodebench/report.py", operation: "modify" },
        { path: "plugins/alpha/config.py", operation: "delete" },
      ],
      allowedPaths: ["ucodebench/constants.py", "ucodebench/report.py", "plugins/alpha/config.py"],
      acceptance: ["pytest"],
    });
  });
});
