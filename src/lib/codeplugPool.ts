import { api } from "./api";
import type { Channel } from "./types";

/**
 * A codeplug's channel pool: every assigned channel list's channels, in list
 * order, first-seen wins — the same dedupe the export and the program builder
 * use, so a checklist built on it offers exactly the channels that get written.
 */
export async function loadCodeplugChannelPool(channelListIds: number[]): Promise<Channel[]> {
  const lists = await Promise.all(channelListIds.map((id) => api.getChannelListChannels(id)));
  const pool: Channel[] = [];
  const seen = new Set<number>();
  for (const list of lists) {
    for (const c of list) {
      if (!seen.has(c.id)) {
        seen.add(c.id);
        pool.push(c);
      }
    }
  }
  return pool;
}
