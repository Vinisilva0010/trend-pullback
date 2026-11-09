import { LoginForm } from "@/components/LoginForm";

// Rendered on each request so the page scripts get the CSP nonce.
export const dynamic = "force-dynamic";

export default function LoginPage() {
  return <LoginForm />;
}
