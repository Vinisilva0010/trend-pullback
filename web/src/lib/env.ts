/**
 * Reads an environment variable without surrounding spaces or line breaks.
 * Values pasted from a Windows editor can end with "\r", which would make a
 * correct password or token fail the comparison.
 */
export function env(name: string): string {
  return (process.env[name] ?? "").trim();
}
