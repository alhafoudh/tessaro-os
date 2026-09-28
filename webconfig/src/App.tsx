// The routes: every page of the menu at its own path, the setting sections
// under /settings/, and `/` to Quick Setup on a fresh device and Overview on
// any other (docs/webconfig.md, "Where it opens").

import { Navigate, Route, Routes, useParams } from "react-router";

import { DeviceProvider } from "./device/DeviceContext";
import { PAGES, sectionTitle } from "./pages/registry";
import { SessionProvider, useSession } from "./session/SessionContext";
import { SignIn } from "./session/SignIn";
import { PageFrame } from "./shell/PageFrame";
import { Shell } from "./shell/Shell";
import { SettingsTable } from "./settings/SettingsTable";

function Section() {
  const { section = "" } = useParams();
  return (
    <PageFrame title={sectionTitle(section)}>
      <SettingsTable key={section} scope={{ prefix: section }} />
    </PageFrame>
  );
}

function Admitted() {
  const { session, admitted } = useSession();
  if (!admitted) {
    return <SignIn />;
  }
  return (
    <DeviceProvider>
      <Shell>
        <Routes>
          <Route index element={<Navigate to={session.fresh ? "/quick-setup" : "/overview"} replace />} />
          {PAGES.map((page) => (
            <Route key={page.path} path={page.path} element={<page.component info={page} />} />
          ))}
          <Route path="settings/:section" element={<Section />} />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </Shell>
    </DeviceProvider>
  );
}

export function App() {
  return (
    <SessionProvider
      fallback={(message) => (
        <div className="flex h-full items-center justify-center p-4 text-sm text-muted">{message}</div>
      )}
    >
      <Admitted />
    </SessionProvider>
  );
}
