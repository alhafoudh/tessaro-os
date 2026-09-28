// Who this browser is to the device (docs/webconfig.md, "Sessions"). An
// unclaimed device needs nobody signed in; a claimed one shows the sign-in
// page until a token, a ticket or a claim gives this browser a session.
// A ticket arrives in the address's fragment from `tessaro-ctl access
// webconfig` or the GUI, and leaves the address as soon as it is read.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { createContext, useContext, useEffect, useState, type ReactNode } from "react";

import { answer, client, failure, type Schemas } from "../api/client";

interface Session {
  session: Schemas["WebSession"];
  /** Signed in, or no need to be: the pages may ask everything. */
  admitted: boolean;
  refetch: () => void;
  signOut: () => Promise<void>;
}

const Context = createContext<Session | null>(null);

export function useSession(): Session {
  const session = useContext(Context);
  if (!session) {
    throw new Error("useSession outside SessionProvider");
  }
  return session;
}

/** The ticket in `#ticket=...`, taken out of the address. */
function takeTicket(): string | null {
  const match = /(?:^#|&)ticket=([^&]+)/.exec(window.location.hash);
  if (!match?.[1]) {
    return null;
  }
  history.replaceState(null, "", window.location.pathname + window.location.search);
  return decodeURIComponent(match[1]);
}

export function SessionProvider({
  children,
  fallback,
}: {
  children: ReactNode;
  fallback: (error: string) => ReactNode;
}) {
  const queries = useQueryClient();
  const [ticket] = useState(takeTicket);
  const [redeemed, setRedeemed] = useState(ticket === null);
  const [ticketError, setTicketError] = useState<string | null>(null);

  useEffect(() => {
    if (!ticket) {
      return;
    }
    answer(client.POST("/api/v1/access/ticket/redeem", { body: { ticket } }))
      .then((session) => queries.setQueryData(["session"], session))
      .catch((error) => setTicketError(`the sign-in link did not work: ${failure(error).message}`))
      .finally(() => setRedeemed(true));
  }, [ticket, queries]);

  const query = useQuery({
    queryKey: ["session"],
    queryFn: () => answer(client.GET("/api/v1/access/session")),
    enabled: redeemed,
    retry: 2,
    refetchOnWindowFocus: true,
  });

  if (!query.data) {
    return <>{fallback(query.error ? failure(query.error).message : "connecting to the device ...")}</>;
  }

  const session = query.data;
  const value: Session = {
    session,
    admitted: !session.claimed || session.via !== "anonymous",
    refetch: () => void query.refetch(),
    signOut: async () => {
      await client.DELETE("/api/v1/access/session").catch(() => undefined);
      queries.clear();
      await query.refetch();
    },
  };
  return (
    <Context.Provider value={value}>
      {ticketError && <div className="bg-danger/20 px-2 py-1 text-sm text-danger">{ticketError}</div>}
      {children}
    </Context.Provider>
  );
}
