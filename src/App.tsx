import { HashRouter, Routes, Route } from "react-router-dom";
import { Layout } from "./components/Layout";
import { Dashboard } from "./pages/Dashboard";
import { Channels } from "./pages/Channels";
import { ChannelLists } from "./pages/ChannelLists";
import { ScanLists } from "./pages/ScanLists";
import { Talkgroups } from "./pages/Talkgroups";
import { DmrContacts } from "./pages/DmrContacts";
import { Codeplugs, CodeplugDetail } from "./pages/Codeplugs";
import { RadioProfiles } from "./pages/RadioProfiles";
import { Settings } from "./pages/Settings";

export default function App() {
  return (
    <HashRouter>
      <Routes>
        <Route element={<Layout />}>
          <Route index element={<Dashboard />} />
          <Route path="channels" element={<Channels />} />
          <Route path="channel-lists" element={<ChannelLists />} />
          <Route path="scan-lists" element={<ScanLists />} />
          <Route path="talkgroups" element={<Talkgroups />} />
          <Route path="dmr-contacts" element={<DmrContacts />} />
          <Route path="codeplugs" element={<Codeplugs />} />
          <Route path="codeplugs/:id" element={<CodeplugDetail />} />
          <Route path="radio-profiles" element={<RadioProfiles />} />
          <Route path="settings" element={<Settings />} />
        </Route>
      </Routes>
    </HashRouter>
  );
}
