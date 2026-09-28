import "./styles.css";

import { QueryCache, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router";

import { watchActivity } from "./api/activity";
import { failure } from "./api/client";
import { App } from "./App";

// A read refused for want of a credential - the session timed out, its token
// was revoked - sends the browser back to the sign-in page.
const queries: QueryClient = new QueryClient({
  queryCache: new QueryCache({
    onError(error, query) {
      if (failure(error).signedOut && query.queryKey[0] !== "session") {
        void queries.invalidateQueries({ queryKey: ["session"] });
      }
    },
  }),
  defaultOptions: {
    queries: { refetchOnWindowFocus: false, retry: 1 },
  },
});

watchActivity(window);

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={queries}>
      <BrowserRouter>
        <App />
      </BrowserRouter>
    </QueryClientProvider>
  </StrictMode>,
);
