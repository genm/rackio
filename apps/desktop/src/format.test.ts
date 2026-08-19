import { describe, expect, it } from "vitest";

import { ago, shortDuration } from "./format";

describe("shortDuration", () => {
  it("keeps sub-90-second spans in seconds so a short trend is not rounded to 1 min", () => {
    expect(shortDuration(10)).toBe("10 s");
    expect(shortDuration(89)).toBe("89 s");
  });

  it("rounds longer spans to whole minutes", () => {
    expect(shortDuration(238)).toBe("4 min");
  });
});

describe("ago", () => {
  it("never reports a future last-contact when clocks disagree slightly", () => {
    expect(ago(2_000, 1_000)).toBe("0 s ago");
  });

  it("scales through minutes, hours, and days", () => {
    expect(ago(0, 30_000)).toBe("30 s ago");
    expect(ago(0, 5 * 60_000)).toBe("5 min ago");
    expect(ago(0, 3 * 3_600_000)).toBe("3 h ago");
    expect(ago(0, 72 * 3_600_000)).toBe("3 d ago");
  });
});
