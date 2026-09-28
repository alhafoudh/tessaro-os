// Every page's frame, as the GUI's `page()` (pages.rs): a wrapping toolbar -
// Configure first when the page's scope has settings, then the page's own
// actions, then those on the selected row - and the page below it.
// Configure opens the settings in a dialog over the page.

import { useState, type ReactNode } from "react";

import { useDevice } from "../device/DeviceContext";
import { rows, type Scope } from "../settings/scope";
import { SettingsTable } from "../settings/SettingsTable";
import { Button, Separator, Toolbar } from "../ui/controls";
import { Dialog } from "../ui/Dialog";

export function PageFrame({
  title,
  scope,
  tools,
  rowTools,
  children,
}: {
  title: string;
  scope?: Scope;
  tools?: ReactNode;
  rowTools?: ReactNode;
  children: ReactNode;
}) {
  const { settings, keys } = useDevice();
  const [configuring, setConfiguring] = useState(false);
  const configurable = !!scope && !!settings && rows(settings, keys, scope).length > 0;

  return (
    <section className="flex min-w-0 flex-col gap-2" aria-label={title}>
      {(configurable || tools || rowTools) && (
        <Toolbar>
          {configurable && <Button onClick={() => setConfiguring(true)}>Configure</Button>}
          {tools}
          {rowTools && (
            <>
              <Separator />
              {rowTools}
            </>
          )}
        </Toolbar>
      )}
      {children}
      {configuring && scope && (
        <Dialog title={`${title} settings`} onClose={() => setConfiguring(false)} wide>
          <SettingsTable scope={scope} />
        </Dialog>
      )}
    </section>
  );
}
