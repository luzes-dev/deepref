import { request } from "node:https";

type Headers = Record<string, string | string[] | undefined>;

export interface HttpResult {
  statusCode: number;
  headers: Headers;
  location?: string;
}

interface AccessSecret {
  clientId: string;
  clientSecret: string;
}

function required(name: string): string {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`${name} is required`);
  return value;
}

function perimeterUrl(name: string): URL {
  const url = new URL(required(name));
  if (url.protocol !== "https:") throw new Error(`${name} must use HTTPS`);
  if (["localhost", "127.0.0.1", "::1"].includes(url.hostname)) {
    throw new Error(`${name} must target the Cloudflare perimeter`);
  }
  return url;
}

async function accessSecret(): Promise<AccessSecret> {
  const secretArn = required("CLOUDFLARE_ACCESS_SERVICE_TOKEN_SECRET_ARN");
  const { GetSecretValueCommand, SecretsManagerClient } = await import("@aws-sdk/client-secrets-manager");
  const response = await new SecretsManagerClient({}).send(new GetSecretValueCommand({ SecretId: secretArn }));
  if (!response.SecretString) throw new Error("Cloudflare Access service-token secret has no SecretString");
  const parsed = JSON.parse(response.SecretString) as Partial<AccessSecret>;
  if (!parsed.clientId || !parsed.clientSecret) {
    throw new Error("Cloudflare Access secret must contain clientId and clientSecret");
  }
  return { clientId: parsed.clientId, clientSecret: parsed.clientSecret };
}

export function get(url: URL, headers: Record<string, string>, timeoutMs: number): Promise<HttpResult> {
  return new Promise((resolve, reject) => {
    const pending = request(url, { method: "GET", headers, timeout: timeoutMs }, (response) => {
      response.resume();
      response.on("end", () => resolve({
        statusCode: response.statusCode ?? 0,
        headers: response.headers,
        location: response.headers.location,
      }));
    });
    pending.once("timeout", () => pending.destroy(new Error(`request timed out after ${timeoutMs}ms`)));
    pending.once("error", reject);
    pending.end();
  });
}

export function assertAuthenticatedResponse(result: HttpResult): void {
  if (result.statusCode < 200 || result.statusCode >= 400) {
    throw new Error(`authenticated perimeter request returned HTTP ${result.statusCode}`);
  }
  if (result.location?.includes("/cdn-cgi/access/login")) {
    throw new Error("authenticated service token was redirected to Cloudflare Access login");
  }
}

export async function handler(): Promise<void> {
  const url = perimeterUrl("DEEPREF_CORE_CANARY_URL");
  const timeoutMs = Number(process.env.CANARY_TIMEOUT_MS ?? "10000");
  if (!Number.isInteger(timeoutMs) || timeoutMs < 1000 || timeoutMs > 30000) {
    throw new Error("CANARY_TIMEOUT_MS must be an integer between 1000 and 30000");
  }
  const secret = await accessSecret();
  const result = await get(url, {
    "CF-Access-Client-Id": secret.clientId,
    "CF-Access-Client-Secret": secret.clientSecret,
    "User-Agent": "deepref-cloudwatch-synthetics/1.0",
  }, timeoutMs);
  assertAuthenticatedResponse(result);
}
