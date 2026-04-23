import { describe, expect, test } from "vitest";
import { formatServiceTarget, looksLikeIpv6Literal } from "./services";

describe("formatServiceTarget", () => {
  test("site target uses _site/ shorthand", () => {
    expect(formatServiceTarget({ kind: "site", name: "postgres-prod" }))
      .toBe("_site/postgres-prod");
  });

  test("app target uses app/service shorthand", () => {
    expect(
      formatServiceTarget({ kind: "app", app: "api-app", service: "api" }),
    ).toBe("api-app/api");
  });
});

describe("looksLikeIpv6Literal", () => {
  test("accepts compressed and full IPv6 literals", () => {
    expect(looksLikeIpv6Literal("2001:db8::1")).toBe(true);
    expect(looksLikeIpv6Literal("fe80::1")).toBe(true);
    expect(looksLikeIpv6Literal("2407:8b00:1169:8100:62fb:98be:75d3:7054"))
      .toBe(true);
  });

  test("rejects IPv4 dotted quads", () => {
    expect(looksLikeIpv6Literal("10.0.0.1")).toBe(false);
    expect(looksLikeIpv6Literal("192.168.1.42")).toBe(false);
  });

  test("rejects bare DNS names", () => {
    expect(looksLikeIpv6Literal("example.com")).toBe(false);
    expect(looksLikeIpv6Literal("db.internal")).toBe(false);
  });

  test("rejects empty string", () => {
    expect(looksLikeIpv6Literal("")).toBe(false);
  });
});
