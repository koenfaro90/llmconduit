import { it, expect } from "vitest";
import {
  parseMatrix,
  measured,
  isBenchmarkRun,
  type BenchmarkCell,
} from "./benchmarkModel";
it("rejects duplicate, excessive, fractional and injected matrix values", () => {
  expect(parseMatrix("1,2,8", 1, 32, 6)).toEqual([1, 2, 8]);
  for (const v of ["1,1", "33", "1.5", "1; echo bad", ""])
    expect(() => parseMatrix(v, 1, 32, 6)).toThrow();
});
it("failed and skipped cells never become throughput or latency numbers", () => {
  const c = {
    status: "skipped",
    aggregate_tps: -1,
    client_ttft_p50: 0,
  } as BenchmarkCell;
  expect(measured(c, "aggregate_tps")).toBeNull();
  c.status = "failed";
  expect(measured(c, "client_ttft_p50")).toBeNull();
  c.status = "complete";
  c.client_ttft_p50 = 0.25;
  expect(measured(c, "client_ttft_p50")).toBe(250);
});
it("rejects malformed reports before rendering", () => {
  expect(isBenchmarkRun({ id: "x" })).toBe(false);
});
