import { fetch } from "@tauri-apps/plugin-http";

export const MIN_VERSION = "4.12.0";

export interface PingResponse {
  code: number;
  data: string;
  msg: string;
}

export type ValidationErrorType =
  | "httpError"
  | "apiError"
  | "versionTooLow"
  | "connectionFailed";

export interface ValidationError {
  type: ValidationErrorType;
  params: Record<string, string>;
}

/**
 * Compare two semver version strings
 * Returns: -1 if a < b, 0 if a == b, 1 if a > b
 */
export function compareSemver(a: string, b: string): number {
  const partsA = a.split(".").map(Number);
  const partsB = b.split(".").map(Number);

  for (let i = 0; i < Math.max(partsA.length, partsB.length); i++) {
    const numA = partsA[i] || 0;
    const numB = partsB[i] || 0;
    if (numA < numB) return -1;
    if (numA > numB) return 1;
  }
  return 0;
}

/**
 * Validate site version by pinging the API endpoint
 * @param siteUrl - The base URL of the Cloudreve site
 * @returns The version string if valid, throws a ValidationError otherwise
 */
export async function validateSiteVersion(siteUrl: string): Promise<string> {
  let response: Response;
  try {
    const url = new URL("/api/v4/site/ping", siteUrl);
    response = await fetch(url.toString());
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    throw { type: "connectionFailed", params: { message } } as ValidationError;
  }

  if (!response.ok) {
    throw {
      type: "httpError",
      params: { status: String(response.status) },
    } as ValidationError;
  }

  const data: PingResponse = await response.json();
  if (data.code !== 0) {
    throw {
      type: "apiError",
      params: { message: data.msg || "Unknown error" },
    } as ValidationError;
  }

  // Remove -pro suffix if present
  const version = data.data.replace(/-pro$/, "");

  // Check if version is >= MIN_VERSION
  if (compareSemver(version, MIN_VERSION) < 0) {
    throw {
      type: "versionTooLow",
      params: { version, minVersion: MIN_VERSION },
    } as ValidationError;
  }

  return version;
}

/**
 * Type guard to check if an error is a ValidationError
 */
export function isValidationError(error: unknown): error is ValidationError {
  return (
    typeof error === "object" &&
    error !== null &&
    "type" in error &&
    "params" in error
  );
}
