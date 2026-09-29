// The Kinetic PDF app icon, redrawn from kinetic-pdf/assets/make-icon.ps1
// (same 256-unit coordinates) so it stays sharp at any size.
// `highlight` (0 to 1) animates the yellow highlighter band across the line.

const LINES = [
  { y: 96, end: 176, lit: false },
  { y: 130, end: 168, lit: true },
  { y: 164, end: 176, lit: false },
  { y: 194, end: 136, lit: false },
];
const THICK = 13;
const PAD = THICK * 0.9;

export const Logo: React.FC<{
  size: number;
  highlight?: number;
  shadow?: boolean;
}> = ({ size, highlight = 1, shadow = true }) => (
  <svg
    width={size}
    height={size}
    viewBox="0 0 256 256"
    style={{
      filter: shadow
        ? "drop-shadow(0 12px 24px rgba(37, 99, 235, 0.35))"
        : "none",
    }}
  >
    <defs>
      <linearGradient
        id="kinetic-tile"
        x1="0"
        y1="8"
        x2="0"
        y2="248"
        gradientUnits="userSpaceOnUse"
      >
        <stop offset="0" stopColor="#5b9bf8" />
        <stop offset="1" stopColor="#2563eb" />
      </linearGradient>
    </defs>
    <rect
      x={8}
      y={8}
      width={240}
      height={240}
      rx={52}
      fill="url(#kinetic-tile)"
    />
    <polygon points="62,34 158,34 196,72 196,222 62,222" fill="#ffffff" />
    <polygon points="158,34 158,72 196,72" fill="#c7dafc" />
    {LINES.map((line) => (
      <g key={line.y}>
        {line.lit ? (
          <rect
            x={74}
            y={line.y - THICK / 2 - PAD}
            width={(line.end - 64) * highlight}
            height={THICK + 2 * PAD}
            rx={6}
            fill="#fcd34d"
          />
        ) : null}
        <rect
          x={84}
          y={line.y - THICK / 2}
          width={line.end - 84}
          height={THICK}
          rx={THICK / 2}
          fill={line.lit ? "#1e293b" : "#94a3b8"}
        />
      </g>
    ))}
  </svg>
);
