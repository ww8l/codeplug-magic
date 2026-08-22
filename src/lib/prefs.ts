/**
 * Local, machine-scoped preferences. Same storage the theme uses — these are
 * choices about this copy of the app, not data worth putting in the database
 * or carrying into a backup.
 */
import { useEffect, useState } from "react";

const ONLINE_GEOCODING_KEY = "cpm-online-geocoding";

/// May the app send a town name to OpenStreetMap's Nominatim service to look
/// up coordinates?
///
/// Defaults to ON, which is what the app has always done — but it is now said
/// out loud beside the city field and switchable here, because typing a town
/// was the one place data left the machine as a silent side effect (#80). Off
/// keeps every lookup local: the centroid of the user's own repeaters in that
/// town still fills coordinates in, it just never reaches the network.
export function onlineGeocodingAllowed(): boolean {
  return localStorage.getItem(ONLINE_GEOCODING_KEY) !== "off";
}

export function setOnlineGeocodingAllowed(allowed: boolean) {
  localStorage.setItem(ONLINE_GEOCODING_KEY, allowed ? "on" : "off");
  // Same-tab listeners: the `storage` event only fires in OTHER documents, and
  // this app is one window.
  window.dispatchEvent(new Event(ONLINE_GEOCODING_EVENT));
}

const ONLINE_GEOCODING_EVENT = "cpm-online-geocoding-changed";

/// React binding, so a panel that is already open reflects a change made in
/// Settings without being remounted.
export function useOnlineGeocoding(): [boolean, (v: boolean) => void] {
  const [allowed, setAllowed] = useState(onlineGeocodingAllowed);
  useEffect(() => {
    const sync = () => setAllowed(onlineGeocodingAllowed());
    window.addEventListener(ONLINE_GEOCODING_EVENT, sync);
    return () => window.removeEventListener(ONLINE_GEOCODING_EVENT, sync);
  }, []);
  return [
    allowed,
    (v: boolean) => {
      setOnlineGeocodingAllowed(v);
      setAllowed(v);
    },
  ];
}
