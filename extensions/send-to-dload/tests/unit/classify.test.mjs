// tests/unit/classify.test.mjs
import { describe, it, expect } from "vitest";
import { classify, hostMatches } from "../../src/shared/classify.js";

describe("classify()", () => {
  it("returns magnet for magnet: URIs", () => {
    const r = classify({ url: "magnet:?xt=urn:btih:abcdef" });
    expect(r.kind).toBe("magnet");
    expect(r.reason).toBe("magnet-scheme");
  });

  it("returns torrent for .torrent file URLs", () => {
    const r = classify({ url: "https://example.com/path/to/ubuntu.torrent" });
    expect(r.kind).toBe("torrent");
    expect(r.reason).toBe("torrent-extension");
  });

  it("returns torrent for application/x-bittorrent mime", () => {
    const r = classify({
      url: "https://example.com/handle?file=42",
      mime: "application/x-bittorrent",
    });
    expect(r.kind).toBe("torrent");
    expect(r.reason).toBe("torrent-mime");
  });

  it("returns torrent for .torrent with query string", () => {
    const r = classify({ url: "https://t.example/foo.torrent?download=1" });
    expect(r.kind).toBe("torrent");
  });

  it("returns http-download for archives", () => {
    expect(classify({ url: "https://example.com/x.zip" }).kind).toBe("http-download");
    expect(classify({ url: "https://example.com/x.iso" }).kind).toBe("http-download");
    expect(classify({ url: "https://example.com/x.7z" }).kind).toBe("http-download");
    expect(classify({ url: "https://example.com/x.tar.gz" }).kind).toBe("http-download");
  });

  it("returns http-download for videos", () => {
    expect(classify({ url: "https://example.com/x.mp4" }).kind).toBe("http-download");
    expect(classify({ url: "https://example.com/x.mkv" }).kind).toBe("http-download");
  });

  it("returns http-download for audio", () => {
    expect(classify({ url: "https://example.com/x.mp3" }).kind).toBe("http-download");
    expect(classify({ url: "https://example.com/x.flac" }).kind).toBe("http-download");
  });

  it("returns http-download for docs", () => {
    expect(classify({ url: "https://example.com/x.pdf" }).kind).toBe("http-download");
    expect(classify({ url: "https://example.com/x.epub" }).kind).toBe("http-download");
  });

  it("returns unknown for HTML / CSS / JS", () => {
    expect(classify({ url: "https://example.com/index.html" }).kind).toBe("unknown");
    expect(classify({ url: "https://example.com/style.css" }).kind).toBe("unknown");
    expect(classify({ url: "https://example.com/app.js" }).kind).toBe("unknown");
  });

  it("returns unknown for ftp URLs without known extension", () => {
    const r = classify({ url: "ftp://ftp.example.com/pub/notes.txt" });
    expect(r.kind).toBe("unknown");
  });

  it("returns unknown for bad URLs", () => {
    expect(classify({ url: "" }).kind).toBe("unknown");
    expect(classify({ url: "not a url" }).kind).toBe("unknown");
  });

  it("rejects unsupported schemes", () => {
    const r = classify({ url: "file:///etc/passwd" });
    expect(r.kind).toBe("unknown");
    expect(r.reason).toMatch(/^scheme:/);
  });

  it("extracts hostname + pathname", () => {
    const r = classify({ url: "https://cdn.example.com/path/foo.iso" });
    expect(r.hostname).toBe("cdn.example.com");
    expect(r.pathname).toBe("/path/foo.iso");
  });
});

describe("hostMatches()", () => {
  it("matches exact host", () => {
    expect(hostMatches("example.com", "example.com")).toBe(true);
    expect(hostMatches("example.com", "Example.COM")).toBe(true);
    expect(hostMatches("example.com", "other.com")).toBe(false);
  });

  it("matches *.example.com subdomains", () => {
    expect(hostMatches("cdn.example.com", "*.example.com")).toBe(true);
    expect(hostMatches("a.b.example.com", "*.example.com")).toBe(true);
    expect(hostMatches("example.com", "*.example.com")).toBe(false);
    expect(hostMatches("notexample.com", "*.example.com")).toBe(false);
  });

  it("rejects empty / invalid inputs", () => {
    expect(hostMatches("", "example.com")).toBe(false);
    expect(hostMatches("example.com", "")).toBe(false);
    expect(hostMatches("example.com", "   ")).toBe(false);
  });
});

describe("classify() — MIME + filename signals", () => {
  it("uses the suggested filename for extensionless download URLs", () => {
    const r = classify({ url: "https://host.example/download?id=123", filename: "ubuntu.iso" });
    expect(r.kind).toBe("http-download");
    expect(r.reason).toBe("archive-extension");
  });

  it("detects .torrent via the suggested filename", () => {
    const r = classify({ url: "https://host.example/get?x=1", filename: "linux.torrent" });
    expect(r.kind).toBe("torrent");
  });

  it("uses the Content-Type when URL and filename have no extension", () => {
    expect(classify({ url: "https://host.example/dl", mime: "video/mp4" }).kind).toBe("http-download");
    expect(classify({ url: "https://host.example/dl", mime: "audio/mpeg" }).kind).toBe("http-download");
    expect(classify({ url: "https://host.example/dl", mime: "application/zip" }).kind).toBe("http-download");
  });

  it("treats application/octet-stream as a generic download", () => {
    const r = classify({ url: "https://host.example/dl", mime: "application/octet-stream" });
    expect(r.kind).toBe("http-download");
    expect(r.reason).toBe("mime-generic");
  });

  it("stays unknown for non-download MIME types (html, json, images)", () => {
    expect(classify({ url: "https://host.example/x", mime: "text/html" }).kind).toBe("unknown");
    expect(classify({ url: "https://host.example/x", mime: "application/json" }).kind).toBe("unknown");
    expect(classify({ url: "https://host.example/x", mime: "image/png" }).kind).toBe("unknown");
  });

  it("prefers a URL/filename extension over the MIME family", () => {
    const r = classify({ url: "https://host.example/a.mp4", mime: "application/octet-stream" });
    expect(r.kind).toBe("http-download");
    expect(r.reason).toBe("video-extension");
  });
});