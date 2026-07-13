import type { Channel } from "../../lib/types";
import { fmtFreq } from "../../lib/constants";

/** Best available human-readable label for a channel. */
export function channelLabel(c: Channel): string {
  return (
    c.name_long ||
    c.name_short ||
    c.rb_name ||
    c.callsign ||
    `${fmtFreq(c.rx_freq)} MHz`
  );
}
