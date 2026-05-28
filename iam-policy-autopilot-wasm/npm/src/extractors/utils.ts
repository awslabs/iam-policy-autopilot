/**
 * Represents an extracted AWS SDK method call.
 */
export interface SdkCall {
  /** Method/operation name (e.g. "GetObject", "put_object") */
  Name: string;
  /** AWS services this call could belong to (e.g. ["s3"]) */
  PossibleServices: string[];
}

/** Strip surrounding quotes from a string literal. */
export function stripQuotes(s: string): string {
  return s.replace(/^['"`]|['"`]$/g, "");
}
