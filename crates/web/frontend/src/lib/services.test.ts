import { describe, expect, test } from "vitest";
import {
  formatRemoteEndpoint,
  formatServiceTarget,
  looksLikeIpv4Literal,
  looksLikeIpv6Literal,
  looksLikeRemoteHost,
} from "./services";

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

describe("looksLikeIpv4Literal", () => {
  test("accepts IPv4 dotted quads", () => {
    expect(looksLikeIpv4Literal("10.0.0.1")).toBe(true);
    expect(looksLikeIpv4Literal("192.168.1.42")).toBe(true);
  });

  test("rejects non-IPv4 strings", () => {
    expect(looksLikeIpv4Literal("2001:db8::1")).toBe(false);
    expect(looksLikeIpv4Literal("example.com")).toBe(false);
    expect(looksLikeIpv4Literal("")).toBe(false);
  });
});

describe("looksLikeRemoteHost", () => {
  test("accepts IPv6 literal", () => {
    expect(looksLikeRemoteHost("2001:db8::1")).toBe(true);
  });

  test("accepts IPv4 literal", () => {
    expect(looksLikeRemoteHost("10.0.0.1")).toBe(true);
  });

  test("accepts DNS name", () => {
    expect(looksLikeRemoteHost("db.example.com")).toBe(true);
    expect(looksLikeRemoteHost("internal-host")).toBe(true);
  });

  test("rejects localhost", () => {
    expect(looksLikeRemoteHost("localhost")).toBe(false);
    expect(looksLikeRemoteHost("LocalHost")).toBe(false);
  });

  test("rejects empty / oversized / underscore labels", () => {
    expect(looksLikeRemoteHost("")).toBe(false);
    expect(looksLikeRemoteHost("a".repeat(254))).toBe(false);
    expect(looksLikeRemoteHost("bad_underscore.example")).toBe(false);
  });

  test("rejects all-numeric strings", () => {
    expect(looksLikeRemoteHost("123.456")).toBe(false);
  });
});

describe("formatRemoteEndpoint", () => {
  test("brackets IPv6 literals", () => {
    expect(formatRemoteEndpoint("2001:db8::1", 5432)).toBe("[2001:db8::1]:5432");
  });

  test("does not bracket IPv4 or DNS hosts", () => {
    expect(formatRemoteEndpoint("10.0.0.1", 80)).toBe("10.0.0.1:80");
    expect(formatRemoteEndpoint("db.example.com", 5432))
      .toBe("db.example.com:5432");
  });
});
