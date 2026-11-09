import "server-only";
import { cookies } from "next/headers";
import { redirect } from "next/navigation";
import { SESSION_COOKIE, verifySession } from "./session";
import { env } from "./env";

export async function isLoggedIn(): Promise<boolean> {
  const jar = await cookies();
  return verifySession(jar.get(SESSION_COOKIE)?.value, env("SESSION_SECRET"));
}

/** For pages: sends to the login page when there is no valid session. */
export async function requireLogin(): Promise<void> {
  if (!(await isLoggedIn())) redirect("/login");
}
