import { useEffect, useState } from "react";
import { RefreshCw } from "lucide-react";
import { api } from "../lib/api";
import type { PortInfo } from "../lib/types";
import { Button, Select } from "./ui";

/**
 * The port picker every radio dialog and bar shares. A radio that is its own
 * USB device (`usbPort` set, the MD-380) has nothing to pick: the token is
 * fixed and the list is never fetched. `onError` receives a failed listing;
 * without it the failure surfaces from the operation that needs the port.
 */
export function usePortChoice(usbPort: string | null, onError?: (e: string) => void) {
  const [ports, setPorts] = useState<PortInfo[]>([]);
  const [port, setPort] = useState(usbPort ?? "");

  /// `forget` drops the current choice first — for after a radio that
  /// reboots and re-enumerates USB on commit (the AnyTone), whose old port
  /// name is gone.
  const refresh = async (forget = false) => {
    if (usbPort) return;
    if (forget) setPort("");
    try {
      const list = await api.listSerialPorts();
      setPorts(list);
      const usb = list.find((p) => p.kind === "usb");
      setPort((cur) => cur || usb?.name || list[0]?.name || "");
    } catch (e) {
      onError?.(typeof e === "string" ? e : String(e));
    }
  };

  useEffect(() => {
    if (usbPort) setPort(usbPort);
    else refresh();
  }, [usbPort]);

  return { ports, port, setPort, refresh };
}

/** The select + rescan row, or the USB-direct line in its place. */
export function PortSelect({
  usbPort,
  modelLabel,
  choice,
}: {
  usbPort: string | null;
  modelLabel: string;
  choice: ReturnType<typeof usePortChoice>;
}) {
  if (usbPort) {
    return (
      <span className="block text-xs text-slate-600 dark:text-slate-300">
        Connected over USB directly — switch the {modelLabel} on with the cable in.
      </span>
    );
  }
  const { ports, port, setPort, refresh } = choice;
  return (
    <div className="flex items-center gap-2">
      <Select
        className="min-w-0 flex-1"
        value={port}
        onChange={(e) => setPort(e.target.value)}
      >
        {ports.length === 0 && <option value="">No ports found</option>}
        {ports.map((p) => (
          <option key={p.name} value={p.name}>
            {p.name}
            {p.kind === "usb" ? "  ·  USB" : ""}
            {p.product ? `  ·  ${p.product}` : ""}
          </option>
        ))}
      </Select>
      <Button variant="ghost" onClick={() => refresh()} title="Rescan ports">
        <RefreshCw size={14} />
      </Button>
    </div>
  );
}
