// Label lists for the TD-H3 radio "options"/settings, mirroring the value lists
// in CHIRP's `tdh8.py` (H3 branch). Each multi-value `Tdh3Settings` field is a
// zero-based index into the matching list here. Keep these in lockstep with the
// decode/encode ranges in `src-tauri/src/commands/tdh3.rs`.

export const SQUELCH = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
export const BRIGHTNESS = ["1", "2", "3", "4", "5"];
export const BACKLIGHT = ["CONT", "5s", "10s", "15s", "30s"];
export const VOX_GAIN = ["Off", "1", "2", "3", "4", "5"];
export const VOX_DELAY = ["1.05s", "2.0s", "3.0s"];
export const TOT = ["Off", "30s", "60s", "90s", "120s", "150s", "180s", "210s"];
export const SCAN_MODE = ["TO (time)", "CO (carrier)", "SE (search)"];
export const PONMSG = ["Off", "Message", "Icon"];
export const POWER_SAVE = ["Off", "1:1", "1:2", "1:3", "1:4", "1:8"];
export const BREATH_LED = ["Off", "5s", "10s", "15s", "30s"];
export const DISPLAY_MODE = ["Frequency", "Name"];
export const ALARM_MODE = ["On site", "Alarm"];
