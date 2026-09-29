import { Logo } from "./Logo";

// A stand-in for the Kinetic PDF window, modelled on the real toolbar layout,
// showing made-up drawing sheets so no real project documents appear.

const INK = "#1e293b";
const SLATE = "#64748b";
const LINE = "#e2e8f0";
const BLUE = "#2563eb";

export const APP = {
  width: 1000,
  titleHeight: 50,
  toolbarHeight: 56,
  contentHeight: 700,
  pageWidth: 900,
};
export const PAGE_HEIGHT = Math.round(APP.pageWidth / 1.414);

const Button: React.FC<{
  children: React.ReactNode;
  primary?: boolean;
  active?: boolean;
}> = ({ children, primary, active }) => (
  <div
    style={{
      height: 36,
      padding: "0 13px",
      display: "flex",
      alignItems: "center",
      borderRadius: 8,
      fontSize: 18,
      fontWeight: 500,
      whiteSpace: "nowrap",
      color: primary ? "white" : INK,
      background: primary ? BLUE : active ? "#dbeafe" : "white",
      border: primary ? "none" : `1.5px solid ${LINE}`,
    }}
  >
    {children}
  </div>
);

const Dot: React.FC<{ color: string; ring?: boolean }> = ({ color, ring }) => (
  <div
    style={{
      width: 22,
      height: 22,
      borderRadius: "50%",
      background: color,
      boxShadow: ring ? `0 0 0 3px white, 0 0 0 5px ${color}` : "none",
    }}
  />
);

const Toolbar: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <div
    style={{
      height: APP.toolbarHeight,
      display: "flex",
      alignItems: "center",
      gap: 8,
      padding: "0 14px",
      borderBottom: `1.5px solid ${LINE}`,
      background: "white",
    }}
  >
    {children}
  </div>
);

export const AppWindow: React.FC<{
  fileName?: string;
  zoomLabel?: string;
  pageLabel?: string;
  children: React.ReactNode;
}> = ({ fileName, zoomLabel = "57%", pageLabel = "1 / 12", children }) => (
  <div
    style={{
      width: APP.width,
      borderRadius: 18,
      overflow: "hidden",
      background: "white",
      border: `2px solid ${LINE}`,
      boxShadow:
        "0 50px 100px -20px rgba(30, 41, 59, 0.28), 0 20px 40px -20px rgba(37, 99, 235, 0.25)",
    }}
  >
    <div
      style={{
        height: APP.titleHeight,
        display: "flex",
        alignItems: "center",
        gap: 12,
        padding: "0 18px",
        fontSize: 19,
        color: SLATE,
        borderBottom: `1.5px solid ${LINE}`,
      }}
    >
      <Logo size={26} shadow={false} />
      {fileName ? `${fileName} - Kinetic PDF` : "Kinetic PDF"}
      <div style={{ flex: 1 }} />
      <span style={{ letterSpacing: 26 }}>–▢✕</span>
    </div>
    <Toolbar>
      <Button primary>Open PDF…</Button>
      <Button>−</Button>
      <div style={{ width: 64, textAlign: "center", fontSize: 18 }}>
        {zoomLabel}
      </div>
      <Button>+</Button>
      <Button active>Fit width</Button>
      <Button>Fit page</Button>
      <div style={{ fontSize: 18, color: SLATE, marginLeft: 8 }}>
        {pageLabel}
      </div>
      <div style={{ flex: 1 }} />
      <Button>Notes</Button>
    </Toolbar>
    <Toolbar>
      <Button active>Select</Button>
      <Button>Pen</Button>
      <Button>Rectangle</Button>
      <Button>Ellipse</Button>
      <Button>Arrow</Button>
      <div style={{ display: "flex", gap: 12, margin: "0 12px" }}>
        <Dot color="#dc2626" ring />
        <Dot color={BLUE} />
        <Dot color="#16a34a" />
        <Dot color={INK} />
      </div>
      <Button>Thin</Button>
      <Button active>Medium</Button>
    </Toolbar>
    <div
      style={{
        position: "relative",
        height: APP.contentHeight,
        background: "#e9edf2",
        overflow: "hidden",
      }}
    >
      {children}
    </div>
  </div>
);

export type SheetKind = "plan" | "section" | "detail";

const TitleBlock: React.FC<{ title: string; number: string }> = ({
  title,
  number,
}) => (
  <g fontFamily="Inter, sans-serif" fill={INK}>
    <rect
      x={1004}
      y={820}
      width={370}
      height={140}
      fill="white"
      stroke={INK}
      strokeWidth={3}
    />
    <line x1={1004} y1={868} x2={1374} y2={868} stroke={INK} strokeWidth={2} />
    <line x1={1250} y1={868} x2={1250} y2={960} stroke={INK} strokeWidth={2} />
    <text x={1020} y={854} fontSize={26} fontWeight={700}>
      SAMPLE PROJECT
    </text>
    <text x={1020} y={910} fontSize={22}>
      {title}
    </text>
    <text x={1020} y={942} fontSize={16} fill={SLATE}>
      FOR ILLUSTRATION ONLY
    </text>
    <text x={1264} y={926} fontSize={30} fontWeight={700}>
      {number}
    </text>
  </g>
);

const Plan: React.FC = () => (
  <g>
    {[180, 300, 420, 540, 660].map((y, i) => (
      <path
        key={y}
        d={`M 40 ${y} C 380 ${y - 90}, 640 ${y + 110}, 1374 ${y - 40 + i * 6}`}
        stroke="#bfdbfe"
        strokeWidth={3}
        fill="none"
      />
    ))}
    <path
      d="M 40 520 C 420 470, 760 600, 1374 470"
      stroke="#cbd5e1"
      strokeWidth={70}
      fill="none"
    />
    <path
      d="M 40 520 C 420 470, 760 600, 1374 470"
      stroke="white"
      strokeWidth={4}
      strokeDasharray="30 20"
      fill="none"
    />
    {Array.from({ length: 6 }, (_, i) => (
      <g key={i}>
        <rect
          x={90 + i * 150}
          y={200}
          width={140}
          height={210}
          fill="none"
          stroke={INK}
          strokeWidth={2.5}
        />
        <text
          x={140 + i * 150}
          y={315}
          fontSize={30}
          fontFamily="Inter, sans-serif"
          fill={SLATE}
        >
          {101 + i}
        </text>
        <rect
          x={90 + i * 150}
          y={620}
          width={140}
          height={170}
          fill="none"
          stroke={INK}
          strokeWidth={2.5}
        />
        <text
          x={140 + i * 150}
          y={715}
          fontSize={30}
          fontFamily="Inter, sans-serif"
          fill={SLATE}
        >
          {201 + i}
        </text>
      </g>
    ))}
    {/* Stormwater line and pits */}
    <path
      d="M 120 560 L 520 548 L 740 575 L 1300 510"
      stroke={BLUE}
      strokeWidth={5}
      fill="none"
    />
    {[
      [120, 560],
      [520, 548],
      [740, 575],
      [1300, 510],
    ].map(([x, y], i) => (
      <g key={i}>
        <rect
          x={x - 12}
          y={y - 12}
          width={24}
          height={24}
          fill="white"
          stroke={BLUE}
          strokeWidth={4}
        />
        <text
          x={x + 18}
          y={y - 18}
          fontSize={14}
          fontFamily="Inter, sans-serif"
          fill={INK}
        >
          PIT {i + 1} IL 24.{350 - i * 45}
        </text>
      </g>
    ))}
    <g transform="translate(1290 120)">
      <circle r={40} fill="none" stroke={INK} strokeWidth={3} />
      <path d="M 0 -52 L 16 10 L 0 0 L -16 10 Z" fill={INK} />
      <text
        x={-9}
        y={-58}
        fontSize={24}
        fontWeight={700}
        fontFamily="Inter, sans-serif"
        fill={INK}
      >
        N
      </text>
    </g>
    <TitleBlock title="SITE PLAN" number="C-101" />
  </g>
);

const Section: React.FC = () => (
  <g>
    {Array.from({ length: 14 }, (_, i) => (
      <line
        key={i}
        x1={80 + i * 90}
        y1={120}
        x2={80 + i * 90}
        y2={680}
        stroke={LINE}
        strokeWidth={2}
      />
    ))}
    <path
      d="M 80 330 L 260 300 L 440 340 L 620 290 L 800 320 L 980 270 L 1250 310"
      stroke="#16a34a"
      strokeWidth={5}
      fill="none"
    />
    <path d="M 80 520 L 1250 460" stroke={BLUE} strokeWidth={8} fill="none" />
    {[80, 470, 860, 1250].map((x, i) => (
      <rect
        key={x}
        x={x - 20}
        y={300 - i * 8}
        width={40}
        height={230 - i * 4}
        fill="none"
        stroke={INK}
        strokeWidth={3}
      />
    ))}
    <g fontFamily="Inter, sans-serif" fontSize={18} fill={INK}>
      {["CHAINAGE", "SURFACE", "INVERT", "DEPTH"].map((label, row) => (
        <g key={label}>
          <line
            x1={80}
            y1={710 + row * 40}
            x2={1250}
            y2={710 + row * 40}
            stroke={INK}
            strokeWidth={1.5}
          />
          <text x={20} y={736 + row * 40} fontSize={14} fontWeight={700}>
            {label}
          </text>
          {Array.from({ length: 13 }, (_, i) => (
            <text key={i} x={96 + i * 90} y={736 + row * 40}>
              {(row === 0
                ? i * 20
                : 24 + ((i * 7 + row * 3) % 10) / 10
              ).toFixed(row === 0 ? 0 : 2)}
            </text>
          ))}
        </g>
      ))}
    </g>
    <TitleBlock title="LONG SECTION" number="C-201" />
  </g>
);

const Detail: React.FC = () => (
  <g>
    <defs>
      <pattern
        id="hatch"
        width={14}
        height={14}
        patternUnits="userSpaceOnUse"
        patternTransform="rotate(45)"
      >
        <line x1={0} y1={0} x2={0} y2={14} stroke="#94a3b8" strokeWidth={2} />
      </pattern>
    </defs>
    <rect
      x={120}
      y={160}
      width={380}
      height={420}
      fill="url(#hatch)"
      stroke={INK}
      strokeWidth={4}
    />
    <rect
      x={180}
      y={220}
      width={260}
      height={300}
      fill="white"
      stroke={INK}
      strokeWidth={3}
    />
    <circle
      cx={310}
      cy={500}
      r={46}
      fill="none"
      stroke={BLUE}
      strokeWidth={6}
    />
    <line x1={120} y1={630} x2={500} y2={630} stroke={INK} strokeWidth={2} />
    <text
      x={270}
      y={662}
      fontSize={24}
      fontFamily="Inter, sans-serif"
      fill={INK}
    >
      1200
    </text>
    <rect
      x={640}
      y={200}
      width={300}
      height={300}
      rx={150}
      fill="url(#hatch)"
      stroke={INK}
      strokeWidth={4}
    />
    <circle
      cx={790}
      cy={350}
      r={90}
      fill="white"
      stroke={INK}
      strokeWidth={3}
    />
    <g fontFamily="Inter, sans-serif" fill={INK}>
      <text x={1030} y={200} fontSize={26} fontWeight={700}>
        NOTES
      </text>
      {[0, 1, 2, 3, 4, 5, 6].map((i) => (
        <rect
          key={i}
          x={1030}
          y={230 + i * 44}
          width={300 - ((i * 37) % 90)}
          height={14}
          rx={7}
          fill="#cbd5e1"
        />
      ))}
      <text x={200} y={130} fontSize={24} fontWeight={700}>
        PIT DETAIL
      </text>
      <text x={680} y={150} fontSize={24} fontWeight={700}>
        SECTION A
      </text>
    </g>
    <TitleBlock title="STANDARD DETAILS" number="C-301" />
  </g>
);

export const Sheet: React.FC<{ kind: SheetKind; width?: number }> = ({
  kind,
  width = APP.pageWidth,
}) => (
  <svg
    width={width}
    height={width / 1.414}
    viewBox="0 0 1414 1000"
    style={{ display: "block", background: "white" }}
  >
    <rect
      x={20}
      y={20}
      width={1374}
      height={960}
      fill="none"
      stroke={INK}
      strokeWidth={4}
    />
    {kind === "plan" ? <Plan /> : kind === "section" ? <Section /> : <Detail />}
  </svg>
);

export const Page: React.FC<{
  kind: SheetKind;
  style?: React.CSSProperties;
}> = ({ kind, style }) => (
  <div
    style={{
      position: "absolute",
      left: (APP.width - APP.pageWidth) / 2,
      boxShadow: "0 6px 20px rgba(15, 23, 42, 0.15)",
      ...style,
    }}
  >
    <Sheet kind={kind} />
  </div>
);

export const Cursor: React.FC<{ x: number; y: number }> = ({ x, y }) => (
  <svg
    width={44}
    height={56}
    viewBox="0 0 22 28"
    style={{ position: "absolute", left: x, top: y, overflow: "visible" }}
  >
    <path
      d="M1 1 L1 22 L6.5 16.5 L10.5 26 L14 24.5 L10 15.5 L18 15.5 Z"
      fill="white"
      stroke={INK}
      strokeWidth={1.6}
      strokeLinejoin="round"
    />
  </svg>
);
