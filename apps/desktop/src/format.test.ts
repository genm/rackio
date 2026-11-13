import { describe, expect, it } from "vitest";

import { ago, shortDuration, uptime } from "./format";

describe("shortDuration", () => {
  it("keeps sub-90-second spans in seconds so a short trend is not rounded to 1 min", () => {
    expect(shortDuration(10)).toBe("10 s");
    expect(shortDuration(89)).toBe("89 s");
  });

  it("rounds longer spans to whole minutes", () => {
    expect(shortDuration(238)).toBe("4 min");
  });
});

describe("uptime", () => {
  it("reports an unknown uptime as unavailable rather than a fresh boot", () => {
    expect(uptime()).toBe("—");
    expect(uptime(null)).toBe("—");
    // A negative or non-finite value is a broken reading, not a machine that
    // booted in the future.
    expect(uptime(-1)).toBe("—");
    expect(uptime(Number.NaN)).toBe("—");
  });

  it("keeps a machine up for less than a minute in seconds", () => {
    expect(uptime(0)).toBe("0s");
    expect(uptime(45)).toBe("45s");
    expect(uptime(59.7)).toBe("59s");
  });

  it("moves to minutes, then hours, then days as the machine stays up", () => {
    expect(uptime(60)).toBe("1m");
    expect(uptime(59 * 60)).toBe("59m");
    expect(uptime(3_600)).toBe("1h 0m");
    expect(uptime(3 * 3_600 + 20 * 60)).toBe("3h 20m");
    expect(uptime(86_400)).toBe("1d 0h");
    expect(uptime(12 * 86_400 + 4 * 3_600 + 59 * 60)).toBe("12d 4h");
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
