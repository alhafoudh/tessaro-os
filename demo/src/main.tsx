import "./theme.css";

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { HashRouter, Route, Routes } from "react-router";

import { installMock } from "./bridge/mock";
import { StatusProvider } from "./features/statuses";
import { Home } from "./pages/Home";
import { MaintenanceReturn } from "./pages/MaintenanceReturn";
import { SectionPage } from "./pages/SectionPage";
import { Layout } from "./shell/Layout";

// A pretend device off a device (`demo:run`, or `?mock`); a real bridge
// always wins.
installMock(location.search);

// Routes live in the hash: nginx serves the demo's files and nothing else,
// so a path it does not have would be a 404 (docs/demo.md).
createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <HashRouter>
      <StatusProvider>
        <Routes>
          <Route path="/maintenance-return" element={<MaintenanceReturn />} />
          <Route
            path="*"
            element={
              <Layout>
                <Routes>
                  <Route path="/" element={<Home />} />
                  <Route path="/:id" element={<SectionPage />} />
                </Routes>
              </Layout>
            }
          />
        </Routes>
      </StatusProvider>
    </HashRouter>
  </StrictMode>,
);
