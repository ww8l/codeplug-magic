import { List } from "lucide-react";
import { api } from "../lib/api";
import { ListManager, type ListAdapter } from "../components/lists/ListManager";

const adapter: ListAdapter = {
  noun: "channel list",
  nounPlural: "channel lists",
  emptyIcon: <List size={40} strokeWidth={1.5} />,
  titleIcon: <List size={18} />,
  title: "Zones/Groups",
  listAll: () => api.listChannelLists(),
  create: async (name, description) => api.createChannelList(name, description),
  rename: (list, name, description) =>
    api.updateChannelList(list.id, name, description),
  remove: (id) => api.deleteChannelList(id),
  getChannels: (id) => api.getChannelListChannels(id),
  addChannel: (id, channelId) => api.addChannelToList(id, channelId),
  removeChannel: (id, channelId) => api.removeChannelFromList(id, channelId),
  reorder: (id, orderedIds) => api.reorderChannelList(id, orderedIds),
  // Create-or-mirror a scan list of the same name from this zone's channels.
  syncMatchingScanList: async (list) => {
    const zoneChannels = await api.getChannelListChannels(list.id);
    const targetIds = zoneChannels.map((c) => c.id);
    const scans = await api.listScanLists();
    const existing = scans.find((s) => s.name === list.name);

    if (!existing) {
      const scan = await api.createScanList(list.name, list.description);
      const added = await api.addChannelsToScanList(scan.id, targetIds);
      if (targetIds.length) await api.reorderScanList(scan.id, targetIds);
      return { created: true, added, removed: 0 };
    }

    // Mirror exactly: drop channels no longer in the zone, add the rest,
    // then match the zone's ordering.
    const current = await api.getScanListChannels(existing.id);
    const targetSet = new Set(targetIds);
    const toRemove = current.filter((c) => !targetSet.has(c.id));
    for (const c of toRemove) {
      await api.removeChannelFromScanList(existing.id, c.id);
    }
    const added = await api.addChannelsToScanList(existing.id, targetIds);
    if (targetIds.length) await api.reorderScanList(existing.id, targetIds);
    return { created: false, added, removed: toRemove.length };
  },
};

export function ChannelLists() {
  return <ListManager adapter={adapter} />;
}
