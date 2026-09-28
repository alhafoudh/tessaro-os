// A settings table, as the GUI's settings window (device.rs section_view):
// the keys of a scope with their value, default, source and what a change
// restarts; Edit, Reset to default and Copy on the selected row; Add and
// Delete for the custom `data.*` values. The device checks every value at
// `config set` and its refusal shows in the edit dialog.

import { useState } from "react";

import { failure, type Schemas } from "../api/client";
import { useDevice } from "../device/DeviceContext";
import { Button, Toolbar } from "../ui/controls";
import { Dialog, Field, Intro } from "../ui/Dialog";
import { copy } from "../ui/dialogs";
import { Table } from "../ui/Table";
import { DATA_PREFIX, DATA_SECTION, isParam, rows, type Scope, type SettingRow } from "./scope";

export function SettingsTable({ scope }: { scope: Scope }) {
  const { settings, keys, link, unset } = useDevice();
  const [selected, setSelected] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [editing, setEditing] = useState<{ key: string; adding: boolean } | null>(null);

  if (!settings) {
    return <p className="text-sm text-muted">reading the settings ...</p>;
  }
  const data = scope.prefix === DATA_SECTION;
  const all = rows(settings, keys, scope);
  const needle = filter.trim().toLowerCase();
  const shown = all.filter(
    (row) => !needle || [row.short, row.value, row.default].some((text) => text.toLowerCase().includes(needle)),
  );
  const chosen = shown.find((row) => row.key === selected);
  const online = link === "online";
  const editable = chosen && online && chosen.source !== "live" ? chosen : undefined;

  return (
    <div className="flex min-w-0 flex-col gap-1.5">
      <Toolbar
        end={
          <input
            placeholder="Find"
            value={filter}
            onChange={(event) => setFilter(event.target.value)}
            className="w-44"
            aria-label="Find a setting"
          />
        }
      >
        {data && (
          <Button disabled={!online} onClick={() => setEditing({ key: "", adding: true })}>
            Add
          </Button>
        )}
        <Button disabled={!editable} onClick={() => editable && setEditing({ key: editable.key, adding: false })}>
          Edit
        </Button>
        <Button
          disabled={!editable || editable.source !== "set"}
          onClick={() => editable && void unset([editable.key]).catch(() => undefined)}
        >
          {data ? "Delete" : "Reset to default"}
        </Button>
        <Button disabled={!chosen} onClick={() => chosen && void copy(chosen.value)}>
          Copy value
        </Button>
      </Toolbar>
      <Table
        columns={[
          { title: "", width: "18px" },
          { title: "Key", width: "190px" },
          { title: "Value" },
          { title: "Default" },
          { title: "Source", width: "70px" },
          { title: "Applies", width: "110px" },
        ]}
        rows={shown.map((row) => ({
          key: row.key,
          cells: [
            row.guarded ? (
              <span className="text-warning" title="guarded: reverts on its own unless confirmed">
                !
              </span>
            ) : (
              ""
            ),
            <span title={row.info?.doc}>{row.short}</span>,
            <span
              className={`block max-w-[28rem] truncate ${row.source === "set" ? "" : "text-muted"}`}
              title={row.value}
            >
              {row.value}
            </span>,
            <span className="block max-w-[20rem] truncate text-muted" title={row.default}>
              {row.default}
            </span>,
            row.source === "set" ? (
              "set"
            ) : (
              <span className="text-muted">{row.source === "live" ? "read-only" : "default"}</span>
            ),
            <span className="text-muted">{row.applies}</span>,
          ],
        }))}
        selected={selected}
        onSelect={setSelected}
        onActivate={(key) => {
          const row = shown.find((item) => item.key === key);
          if (row && online && row.source !== "live") setEditing({ key, adding: false });
        }}
        empty={data && all.length === 0 ? "no custom values yet" : "nothing matches"}
      />
      {editing && (
        <EditSetting
          row={all.find((row) => row.key === editing.key)}
          adding={editing.adding}
          template={keys.get(`${DATA_PREFIX}<name>`)}
          onClose={() => setEditing(null)}
        />
      )}
    </div>
  );
}

function EditSetting({
  row,
  adding,
  template,
  onClose,
}: {
  row: SettingRow | undefined;
  adding: boolean;
  template: Schemas["KeyInfo"] | undefined;
  onClose: () => void;
}) {
  const { set, unset } = useDevice();
  const info = row?.info ?? template;
  // Taken once: a refresh underneath does not overwrite what is typed.
  const [value, setValue] = useState(row?.value ?? "");
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const key = adding ? `${DATA_PREFIX}${name.trim()}` : (row?.key ?? "");
  const problem = adding && !isParam(name.trim()) ? "a name of lower-case letters, digits and _, up to 32" : null;
  const input = info?.input ?? { kind: "text" as const };

  const attempt = async (change: () => Promise<unknown>, close: boolean) => {
    setBusy(true);
    setError(null);
    try {
      await change();
      if (close) onClose();
    } catch (problem) {
      setError(failure(problem).message);
    } finally {
      setBusy(false);
    }
  };
  const apply = (close: boolean) => attempt(() => set({ [key]: value }), close);

  return (
    <Dialog
      title={adding ? "Add a custom value" : `Edit ${row?.key ?? ""}`}
      onClose={onClose}
      onSubmit={() => void apply(true)}
      submit="OK"
      busy={busy}
      disabled={!!problem}
      extra={
        <>
          {!adding && row?.source === "set" && (
            <Button disabled={busy} onClick={() => void attempt(() => unset([key]), true)}>
              Default
            </Button>
          )}
          <Button disabled={busy || !!problem} onClick={() => void apply(false)}>
            Apply
          </Button>
        </>
      }
    >
      {info?.doc && <Intro>{info.doc}</Intro>}
      {adding && (
        <Field label="Name" hint={problem ?? `used in browser.url as {${key}}`}>
          <input value={name} onChange={(event) => setName(event.target.value)} spellCheck={false} />
        </Field>
      )}
      <Field label="Value" hint={info?.values}>
        {input.kind === "flag" ? (
          <input
            type="checkbox"
            checked={value === "1"}
            onChange={(event) => setValue(event.target.checked ? "1" : "0")}
            className="h-4 w-4 self-start"
          />
        ) : input.kind === "choice" ? (
          <select value={value} onChange={(event) => setValue(event.target.value)}>
            {!input.choices.includes(value) && <option value={value}>{value || "(default)"}</option>}
            {input.choices.map((choice) => (
              <option key={choice} value={choice}>
                {choice}
              </option>
            ))}
          </select>
        ) : input.kind === "int" ? (
          <input
            type="number"
            min={input.min}
            max={input.max}
            value={value}
            onChange={(event) => setValue(event.target.value)}
          />
        ) : (
          <input value={value} onChange={(event) => setValue(event.target.value)} spellCheck={false} />
        )}
      </Field>
      {row && row.default && (
        <Field label="Default">
          <span className="text-sm text-muted break-all">{row.default}</span>
        </Field>
      )}
      {info?.guarded && (
        <Intro warn>Applied on probation: it goes back on its own unless confirmed on the Screen page.</Intro>
      )}
      {error && <p className="text-sm text-danger">{error}</p>}
    </Dialog>
  );
}
