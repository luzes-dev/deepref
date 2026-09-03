import { request } from "node:https";

type Headers = Record<string, string | string[] | undefined>;

export interface DenialResult {
  statusCode: number;
  headers: Headers;
  location?: string;
}

function requiredHttpsUrl(name: string): URL {
  const raw = process.env[name]?.trim();
  if (!raw) throw new Error(`${name} is required`);
  const url = new URL(raw);
  if (url.protocol !== "https:") throw new Error(`${name} must use HTTPS`);
  return url;
}

export function unauthenticatedGet(url: URL, timeoutMs: number): Promise<DenialResult> {
  return new Promise((resolve, reject) => {
    const pending = request(url, {
      method: "GET",
      headers: {"User-Agent": "deepref-cloudwatch-synthetics-denial/1.0"},
      timeout: timeoutMs,
    }, (response) => {
      response.resume();
      response.on("end", () => resolve({
        statusCode: response.statusCode ?? 0,
        headers: response.headers,
        location: response.headers.location,
      }));
    });
    pending.once("timeout", () => pending.destroy(Object.assign(new Error("request timed out"), {code: "ETIMEDOUT"})));
    pending.once("error", reject);
    pending.end();
  });
}

export function assertAccessDenied(result: DenialResult): void {
  const redirect = [301, 302, 303, 307, 308].includes(result.statusCode);
  if (redirect && result.location) {
    const location = new URL(result.location, "https://invalid.example");
    const isCloudflareAccessHost = location.hostname === "cloudflareaccess.com" || location.hostname.endsWith(".cloudflareaccess.com");
    if (isCloudflareAccessHost || location.pathname.startsWith("/cdn-cgi/access/login")) return;
  }
  const server = Array.isArray(result.headers.server) ? result.headers.server.join(" ") : result.headers.server ?? "";
  const hasCloudflareEvidence = /cloudflare/i.test(server) && Boolean(result.headers["cf-ray"]);
  if (result.statusCode === 403 && hasCloudflareEvidence) return;
  throw new Error(`unauthenticated request was not denied by Cloudflare Access (HTTP ${result.statusCode})`);
}

export function assertOriginUnreachable(error: unknown): void {
  const code = typeof error === "object" && error !== null && "code" in error ? String(error.code) : "";
  if (!["ENOTFOUND", "ECONNREFUSED", "EHOSTUNREACH", "ETIMEDOUT"].includes(code)) {
    throw error instanceof Error ? error : new Error("public-origin probe failed without an expected network error");
  }
}

export async function handler(): Promise<void> {
  const perimeter = requiredHttpsUrl("DEEPREF_ACCESS_DENIAL_URL");
  const origin = requiredHttpsUrl("DEEPREF_PUBLIC_ORIGIN_PROBE_URL");
  const timeoutMs = Number(process.env.CANARY_TIMEOUT_MS ?? "10000");
  if (!Number.isInteger(timeoutMs) || timeoutMs < 1000 || timeoutMs > 30000) {
    throw new Error("CANARY_TIMEOUT_MS must be an integer between 1000 and 30000");
  }
  assertAccessDenied(await unauthenticatedGet(perimeter, timeoutMs));
  try {
    const result = await unauthenticatedGet(origin, timeoutMs);
    throw new Error(`forbidden public origin responded with HTTP ${result.statusCode}`);
  } catch (error) {
    assertOriginUnreachable(error);
  }
}
