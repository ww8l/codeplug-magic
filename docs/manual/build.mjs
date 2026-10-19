// Render docs/manual/manual.html to Codeplug-Magic-Guide.pdf with the Chrome
// (or Chromium/Edge) already on this machine. No npm dependency: headless
// Chrome's own --print-to-pdf honours the @page rules in manual.css.
//
//   npm run manual
//   CHROME=/path/to/chrome npm run manual

import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const src = join(here, "manual.html");
const out = join(here, "Codeplug-Magic-Guide.pdf");

const candidates = [
  process.env.CHROME,
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "/Applications/Chromium.app/Contents/MacOS/Chromium",
  "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
  "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
  "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
  "/usr/bin/google-chrome",
  "/usr/bin/chromium",
  "/usr/bin/chromium-browser",
].filter(Boolean);
const chrome = candidates.find((p) => existsSync(p));
if (!chrome) {
  console.error("No Chrome, Chromium or Edge found. Set CHROME=/path/to/browser.");
  process.exit(1);
}

// A throwaway profile, so a running browser's own profile is never touched.
const profile = mkdtempSync(join(tmpdir(), "cpm-manual-"));
try {
  execFileSync(chrome, [
    "--headless=new",
    `--user-data-dir=${profile}`,
    "--no-first-run",
    "--allow-file-access-from-files",
    "--no-pdf-header-footer",
    "--virtual-time-budget=5000",
    `--print-to-pdf=${out}`,
    pathToFileURL(src).href,
  ], { stdio: ["ignore", "ignore", "pipe"] });
} finally {
  rmSync(profile, { recursive: true, force: true });
}

const kb = Math.round(statSync(out).size / 1024);
console.log(`wrote ${out} (${kb} KB)`);
