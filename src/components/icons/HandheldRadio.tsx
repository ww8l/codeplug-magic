import type { SVGProps } from "react";

/**
 * Handheld transceiver (HT / "walkie-talkie") icon, drawn in the Lucide style
 * (24x24 viewBox, currentColor stroke, 2px round strokes) so it sits cleanly
 * next to the other lucide-react nav icons. Lucide has no HT/walkie-talkie
 * glyph, so this fills that gap for the "Radios" section.
 *
 * Silhouette: stubby antenna + volume/channel knob on top, a display window,
 * speaker grille, and a PTT button on the rounded body.
 */
export function HandheldRadio({
  size = 24,
  ...props
}: SVGProps<SVGSVGElement> & { size?: number }) {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      {...props}
    >
      {/* antenna */}
      <path d="M8 6V2" />
      {/* volume/channel knob */}
      <path d="M15 6V4" />
      {/* body */}
      <rect x="5" y="6" width="13" height="16" rx="2" />
      {/* display */}
      <rect x="8" y="9" width="7" height="3.5" rx="0.5" />
      {/* speaker grille */}
      <path d="M8.5 16h6" />
      <path d="M8.5 18.5h6" />
    </svg>
  );
}
