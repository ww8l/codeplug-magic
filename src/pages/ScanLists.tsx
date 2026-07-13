import { ScanLine } from "lucide-react";
import { api } from "../lib/api";
import { ListManager, type ListAdapter } from "../components/lists/ListManager";

const adapter: ListAdapter = {
  noun: "scan list",
  nounPlural: "scan lists",
  emptyIcon: <ScanLine size={40} strokeWidth={1.5} />,
  titleIcon: <ScanLine size={18} />,
  listAll: () => api.listScanLists(),
  create: async (name, description) => api.createScanList(name, description),
  // Name/description-only save (kept for completeness; the Edit modal routes
  // through saveScanSettings, which persists these plus the seven settings).
  rename: (list, name, description) =>
    api.updateScanList(list.id, name, description, {
      priority_channel_id: list.priority_channel_id ?? null,
      priority_channel_2_id: list.priority_channel_2_id ?? null,
      priority_select: list.priority_select ?? 0,
      look_back_a: list.look_back_a ?? 20,
      look_back_b: list.look_back_b ?? 30,
      dropout_delay: list.dropout_delay ?? 31,
      dwell_time: list.dwell_time ?? 31,
      revert_channel: list.revert_channel ?? 4,
    }),
  remove: (id) => api.deleteScanList(id),
  getChannels: (id) => api.getScanListChannels(id),
  addChannel: (id, channelId) => api.addChannelToScanList(id, channelId),
  removeChannel: (id, channelId) => api.removeChannelFromScanList(id, channelId),
  reorder: (id, orderedIds) => api.reorderScanList(id, orderedIds),
  saveScanSettings: (list, name, description, settings) =>
    api.updateScanList(list.id, name, description, settings),
};

export function ScanLists() {
  return <ListManager adapter={adapter} />;
}
