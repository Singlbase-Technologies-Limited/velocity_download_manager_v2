import { describe, expect, it } from "vitest";

describe("fixture E", () => {
  it("fails one", () => {
    expect(1).toBe(2);
  });
  it("fails two", () => {
    expect("a").toBe("b");
  });
  it("fails three", () => {
    expect([1]).toEqual([2]);
  });
});
