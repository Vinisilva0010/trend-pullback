"use client";

export function LogoutButton() {
  return (
    <button
      type="button"
      className="rounded border-2 border-ink px-4 py-2 font-extrabold"
      onClick={async () => {
        await fetch("/api/logout", { method: "POST" });
        window.location.href = "/login";
      }}
    >
      Sair
    </button>
  );
}
