// The page a claimed device shows a browser without a session: sign in with
// a token someone issued (`tessaro-ctl access token create`), or open
// Webconfig from ctl or the GUI, which signs in on its own.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";

import { answer, client, failure } from "../api/client";
import { Button } from "../ui/controls";

export function SignIn() {
  const queries = useQueryClient();
  // Who the device is answers without a credential.
  const node = useQuery({
    queryKey: ["node"],
    queryFn: () => answer(client.GET("/api/v1/device/id")),
    staleTime: Infinity,
  });
  const name = node.data?.name ?? "this device";
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const session = await answer(client.POST("/api/v1/access/session", { body: { token: token.trim() } }));
      queries.setQueryData(["session"], session);
    } catch (problem) {
      setError(failure(problem).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex min-h-full items-center justify-center p-3">
      <form onSubmit={submit} className="w-full max-w-[420px] border border-border bg-panel">
        <div className="bg-chrome px-2.5 py-1 text-sm font-bold">Sign in to {name}</div>
        <div className="flex flex-col gap-2 p-3">
          <p className="text-sm">
            This device is claimed. Sign in with one of its tokens, or open Webconfig from{" "}
            <span className="font-bold">tessaro-ctl access webconfig</span> or the desktop app's Access page, which
            signs you in.
          </p>
          <input
            type="password"
            autoFocus
            autoComplete="off"
            placeholder="tsr_..."
            value={token}
            onChange={(event) => setToken(event.target.value)}
            className="font-mono"
          />
          {error && <p className="text-sm text-danger">{error}</p>}
          <div className="flex justify-end">
            <Button
              type="submit"
              kind="primary"
              disabled={busy || token.trim().length === 0}
              className="px-3.5 py-[3px]"
            >
              {busy ? "Signing in ..." : "Sign in"}
            </Button>
          </div>
        </div>
      </form>
    </div>
  );
}
