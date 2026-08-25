import type { SVGProps } from "react";

/**
 * The application icon — the wizard with the wand and the handheld — as an
 * inline SVG, so the sidebar wears the same mark as the dock, the taskbar and
 * the installer rather than a stand-in.
 *
 * Traced from `src-tauri/icon-source.svg`, which is what `tauri icon`
 * generates every platform icon from. **Those two are the same drawing and have
 * to stay that way** — change one and the app's own header stops matching its
 * dock icon. There is no build step tying them together; it is a copy.
 *
 * Unlike [`HandheldRadio`] this is a logo, not a lucide-style glyph: it is
 * multi-colour, carries its own rounded-square background (`rx=48` of 300, so
 * the rounding scales with the size), and deliberately ignores `currentColor`.
 * A brand mark that restyles itself per theme is not the brand mark.
 */
export function AppMark({
  size = 28,
  ...props
}: SVGProps<SVGSVGElement> & { size?: number }) {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      width={size}
      height={size}
      viewBox="0 0 300 300"
      role="img"
      aria-label="WW8L Codeplug Magic"
      {...props}
    >
      <rect x="0" y="0" width="300" height="300" rx="48" fill="#2B2350"/>
      <path d="M35,270 L125,270 L102,170 L58,170 Z" fill="#4A3F7A"/>
      <path d="M63,176 C50,190 42,205 44,222 L54,225 C56,206 64,192 74,180 Z" fill="#453A72"/>
      <ellipse cx="49" cy="227" rx="7" ry="6" fill="#F4EFE7"/>
      <line x1="44" y1="224" x2="54" y2="226" stroke="#C9BEAE" strokeWidth="1.3" opacity="0.7"/>
      <line x1="44" y1="229" x2="54" y2="231" stroke="#C9BEAE" strokeWidth="1.3" opacity="0.7"/>
      <circle cx="80" cy="145" r="20" fill="#F4EFE7"/>
      <circle cx="73" cy="142" r="1.8" fill="#2B2350"/>
      <circle cx="87" cy="142" r="1.8" fill="#2B2350"/>
      <path d="M68,136 Q73,132 78,136" stroke="#2B2350" strokeWidth="1.8" fill="none" strokeLinecap="round"/>
      <path d="M82,136 Q87,132 92,136" stroke="#2B2350" strokeWidth="1.8" fill="none" strokeLinecap="round"/>
      <path d="M80,141 Q82,147 79,150" stroke="#2B2350" strokeWidth="1.4" fill="none" strokeLinecap="round"/>
      <path d="M66,152 L80,180 L94,152 Z" fill="#DAD4C6"/>
      <ellipse cx="80" cy="133" rx="32" ry="8" fill="#E0A83E"/>
      <path d="M50,133 L80,45 L110,133 Z" fill="#E0A83E"/>
      <path d="M80,86 C83,92 83,92 89,95 C83,98 83,98 80,104 C77,98 77,98 71,95 C77,92 77,92 80,86 Z" fill="#F4EFE7"/>
      <path d="M92,165 C108,148 128,146 148,146 L155,160 C136,170 118,178 100,190 C96,183 93,174 92,165 Z" fill="#4A3F7A"/>
      <path d="M104,157 Q120,152 134,150" stroke="#382F5E" strokeWidth="2" fill="none" opacity="0.6"/>
      <ellipse cx="151" cy="153" rx="8" ry="7" fill="#F4EFE7"/>
      <line x1="145" y1="150" x2="153" y2="158" stroke="#C9BEAE" strokeWidth="1.2" opacity="0.6"/>
      <line x1="149" y1="148" x2="157" y2="156" stroke="#C9BEAE" strokeWidth="1.2" opacity="0.6"/>
      <line x1="153" y1="153" x2="207" y2="99" stroke="#E0A83E" strokeWidth="6" strokeLinecap="round"/>
      <path d="M219,72 C224,83 224,83 235,88 C224,93 224,93 219,104 C214,93 214,93 203,88 C214,83 214,83 219,72 Z" fill="#F0B84B"/>
      <path d="M230,58 C232.2,62.4 232.2,62.4 237,65 C232.2,67.6 232.2,67.6 230,72 C227.8,67.6 227.8,67.6 223,65 C227.8,62.4 227.8,62.4 230,58 Z" fill="#F0B84B" opacity="0.8"/>
      <path d="M224,74 C225.5,77 225.5,77 229,79 C225.5,81 225.5,81 224,84 C222.5,81 222.5,81 219,79 C222.5,77 222.5,77 224,74 Z" fill="#F0B84B" opacity="0.6"/>
      <path d="M229,150 L235,150 L237,105 Z" fill="#F4EFE7"/>
      <rect x="205" y="150" width="55" height="100" rx="10" fill="#F4EFE7"/>
      <rect x="213" y="160" width="39" height="18" rx="3" fill="#2B2350"/>
      <line x1="218" y1="169" x2="245" y2="169" stroke="#F0B84B" strokeWidth="2.5" strokeLinecap="round"/>
      <circle cx="213" cy="148" r="6" fill="#E0A83E"/>
      <circle cx="216" cy="185" r="2" fill="#2B2350"/>
      <circle cx="225" cy="185" r="2" fill="#2B2350"/>
      <circle cx="234" cy="185" r="2" fill="#2B2350"/>
      <circle cx="243" cy="185" r="2" fill="#2B2350"/>
      <circle cx="216" cy="195" r="2" fill="#2B2350"/>
      <circle cx="225" cy="195" r="2" fill="#2B2350"/>
      <circle cx="234" cy="195" r="2" fill="#2B2350"/>
      <circle cx="243" cy="195" r="2" fill="#2B2350"/>
      <rect x="197" y="170" width="9" height="22" rx="3" fill="#E0A83E"/>
      <circle cx="220" cy="225" r="4.5" fill="#2B2350"/>
      <circle cx="245" cy="225" r="4.5" fill="#2B2350"/>
    </svg>
  );
}
