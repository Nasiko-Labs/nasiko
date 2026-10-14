import { describe, expect, it } from "vitest";
import { classifyHref, isSafeHref, isSafeMediaSrc } from "./content-policy";
import { DataLinkSchema, DisplayTextSchema } from "./content-schemas";

const BAD = [
  "javascript:alert(1)",
  "JAVASCRIPT:alert(1)",
  " javascript:alert(1)",
  "java\u0000script:alert(1)",
  "data:text/html,<script>alert(1)</script>",
  "vbscript:msgbox",
  "//evil.test/x",
  "file:///etc/passwd",
  "blob:https://x/y",
  "ftp://x/y",
  "https://x/y\n",
  // Origin bypass found in the Phase 3 review: the parser turns `/\` into `//`.
  "/\\evil.test/x",
  "\\\\evil.test/x",
  "/\\/evil.test",
  "/\\\\evil.test",
  "\\/evil.test",
  "/ /evil.test",
  "/\t/evil.test",
  // Credentials in the authority (phishing / confusion) are never links.
  "https://user:pw@nasiko.example/",
  "https://nasiko.example@evil.test/a",
  // No host at all.
  "http:///x",
  "https://",
  "",
  "x".repeat(2_049),
  42,
  null,
  undefined,
  { href: "https://x" },
];
const GOOD = [
  "https://nasiko.example/agents/1",
  "http://localhost:8080/api",
  "/workspaces/ws-a",
  "./rel",
  "../up",
  "?q=1",
  "#top",
  "mailto:ops@nasiko.example",
];

describe("classifyHref normalises before judging", () => {
  it("resolves relative candidates against an internal origin and refuses anything that escapes it", () => {
    for (const h of ["/\\evil.test/x", "\\\\evil.test", "//evil.test", "/\\/evil.test"]) {
      const v = classifyHref(h);
      expect(v.ok).toBe(false);
    }
    expect(classifyHref("/workspaces/a?x=1#f")).toMatchObject({ ok: true, kind: "relative" });
  });
  it("keeps the parsed URL for absolute links so consumers can read origin and host", () => {
    const v = classifyHref("https://docs.nasiko.example/a");
    expect(v.ok && v.kind === "absolute" && v.url.origin).toBe("https://docs.nasiko.example");
  });
  it("`allowedHosts: []` means same-origin/relative only", () => {
    expect(isSafeHref("https://nasiko.example/a", { allowedHosts: [] })).toBe(false);
    expect(isSafeHref("/a", { allowedHosts: [] })).toBe(true);
    expect(isSafeHref("/\\evil.test/a", { allowedHosts: [] })).toBe(false);
  });
});

describe("isSafeHref", () => {
  it.each(BAD)("rejects %j", (h) => {
    expect(isSafeHref(h)).toBe(false);
  });
  it.each(GOOD)("accepts %s", (h) => {
    expect(isSafeHref(h)).toBe(true);
  });
  it("enforces a host allow-list for absolute http(s) links, including subdomains, not look-alikes", () => {
    const p = { allowedHosts: ["nasiko.example"] };
    expect(isSafeHref("https://nasiko.example/a", p)).toBe(true);
    expect(isSafeHref("https://app.nasiko.example/a", p)).toBe(true);
    expect(isSafeHref("https://nasiko.example.evil.test/a", p)).toBe(false);
    expect(isSafeHref("https://evilnasiko.example/a", p)).toBe(false);
    expect(isSafeHref("https://nasiko.example@evil.test/a", p)).toBe(false);
    expect(isSafeHref("/relative", p)).toBe(true);
  });
  it("respects a narrower protocol list", () => {
    expect(isSafeHref("mailto:x@y", { protocols: ["https:"] })).toBe(false);
    expect(isSafeHref("http://x/y", { protocols: ["https:"] })).toBe(false);
  });
});

describe("isSafeMediaSrc", () => {
  it("accepts same-origin paths and https only", () => {
    expect(isSafeMediaSrc("/img/a.png")).toBe(true);
    expect(isSafeMediaSrc("https://cdn.nasiko.example/a.png")).toBe(true);
    expect(isSafeMediaSrc("http://cdn.nasiko.example/a.png")).toBe(false);
    expect(isSafeMediaSrc("data:image/png;base64,AAAA")).toBe(false);
    expect(isSafeMediaSrc("blob:https://x/y")).toBe(false);
    expect(isSafeMediaSrc("//cdn/x.png")).toBe(false);
    expect(isSafeMediaSrc("/\\cdn.evil/x.png")).toBe(false);
    expect(isSafeMediaSrc("/\\cdn.evil/x.png", [])).toBe(false);
    expect(isSafeMediaSrc("https://user@cdn.nasiko.example/a.png")).toBe(false);
  });
  it("host allow-list applies to media too", () => {
    expect(isSafeMediaSrc("https://cdn.nasiko.example/a.png", ["nasiko.example"])).toBe(true);
    expect(isSafeMediaSrc("https://evil.test/a.png", ["nasiko.example"])).toBe(false);
  });
});

describe("DisplayTextSchema / DataLinkSchema", () => {
  it("allows tabs and newlines but no other control characters, and bounds length", () => {
    expect(DisplayTextSchema.safeParse("line 1\nline\t2").success).toBe(true);
    expect(DisplayTextSchema.safeParse("x\u0007y").success).toBe(false);
    expect(DisplayTextSchema.safeParse("x\u001by[31m").success).toBe(false);
    expect(DisplayTextSchema.safeParse("a".repeat(4_001)).success).toBe(false);
  });
  it("DataLink is strict: no extra keys", () => {
    expect(DataLinkSchema.safeParse({ href: "/a", label: "A" }).success).toBe(true);
    expect(DataLinkSchema.safeParse({ href: "/a", label: "A", onclick: "x" }).success).toBe(false);
  });
});
