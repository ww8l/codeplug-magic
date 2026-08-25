// The application icon — the wizard with the wand and the handheld.
//
// Imported from the very file `tauri icon` generates every platform icon from,
// so the sidebar, the dock, the taskbar and the installer cannot drift apart.
// This used to be a hand-copy of the same paths; nothing in the repo would have
// caught the two diverging (there is no frontend test runner), and the failure
// would have shipped silently. An import makes it a build artifact instead: move
// or rename the source and the frontend build fails loudly, which is the failure
// mode worth having.
//
// At ~3 KB Vite inlines this as a `data:` URI, which the app's CSP allows
// (`img-src 'self' asset: http://asset.localhost data:`); if the artwork ever
// grows past `assetsInlineLimit` Vite emits a file served from 'self' instead,
// which the same CSP also allows. Either way there is no extra request.
//
// ⚠ `vite.config.ts` ignores `src-tauri/**` for watching, so editing the source
// SVG needs a dev-server restart to show up. It changes about once a year.
import markUrl from "../../../src-tauri/icon-source.svg?url";

/**
 * Decorative by default: in the sidebar the app's name sits right beside it as
 * real text, so an accessible name here would make a screen reader announce
 * "WW8L Codeplug Magic" twice. Pass `alt` where the mark stands alone and has
 * to carry the name itself.
 */
export function AppMark({
  size = 28,
  alt = "",
  className,
}: {
  size?: number;
  alt?: string;
  className?: string;
}) {
  return (
    <img
      src={markUrl}
      width={size}
      height={size}
      alt={alt}
      className={className}
      draggable={false}
    />
  );
}
