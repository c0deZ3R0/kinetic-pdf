import { loadFont } from "@remotion/google-fonts/Inter";
import { z } from "zod";
import {
  AbsoluteFill,
  Audio,
  Easing,
  Freeze,
  Img,
  interpolate,
  OffthreadVideo,
  Sequence,
  spring,
  staticFile,
  useCurrentFrame,
  useVideoConfig,
} from "remotion";
import {
  APP,
  AppWindow,
  Cursor,
  Page,
  PAGE_HEIGHT,
  Sheet,
  SheetKind,
} from "./AppMock";
import { Logo } from "./Logo";

const { fontFamily } = loadFont("normal", {
  weights: ["500", "700", "800"],
  subsets: ["latin"],
});

// Filenames inside public/. Leave `music` empty for no soundtrack.
export const promoSchema = z.object({
  music: z.string(),
  recording: z.string(),
});

type PromoProps = z.infer<typeof promoSchema>;

// Taken from the app icon (kinetic-pdf/assets/make-icon.ps1)
export const BRAND = {
  blue: "#2563eb",
  highlight: "#fcd34d",
  ink: "#1e293b",
  slate: "#64748b",
  line: "#e2e8f0",
  paper: "#f8fafc",
};

// Scene lengths in frames (30fps), cut to the hits in "Clear Pulse":
// lift at 3.5s, hits at 8.25s, 11s, 14s, 16s, breakdown 20-24s, final hit 24s.
const SCENES = {
  hook: 105,
  buffering: 143,
  reveal: 82,
  opens: 90,
  zoom: 60,
  scroll: 120,
  highlight: 120,
  cta: 135,
};
export const PROMO_DURATION = Object.values(SCENES).reduce((a, b) => a + b, 0);

const clamp = {
  extrapolateLeft: "clamp",
  extrapolateRight: "clamp",
} as const;

const useSpring = (delay = 0, durationInFrames?: number) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  return spring({
    frame: frame - delay,
    fps,
    config: { damping: 200 },
    durationInFrames,
  });
};

// Fades a scene out over its last few frames
const Scene: React.FC<{
  duration: number;
  children: React.ReactNode;
  justify?: React.CSSProperties["justifyContent"];
}> = ({ duration, children, justify = "center" }) => {
  const frame = useCurrentFrame();
  const opacity = interpolate(frame, [duration - 6, duration], [1, 0], clamp);
  return (
    <AbsoluteFill
      style={{
        opacity,
        alignItems: "center",
        justifyContent: justify,
        padding: "0 40px",
        fontFamily,
        color: BRAND.ink,
      }}
    >
      {children}
    </AbsoluteFill>
  );
};

const RiseIn: React.FC<{
  delay?: number;
  children: React.ReactNode;
  style?: React.CSSProperties;
}> = ({ delay = 0, children, style }) => {
  const p = useSpring(delay);
  return (
    <div
      style={{
        opacity: p,
        transform: `translateY(${interpolate(p, [0, 1], [50, 0])}px)`,
        ...style,
      }}
    >
      {children}
    </div>
  );
};

// A highlighter stroke that swipes in behind the text
const Mark: React.FC<{ delay: number; children: React.ReactNode }> = ({
  delay,
  children,
}) => {
  const p = useSpring(delay, 14);
  return (
    <span
      style={{
        position: "relative",
        display: "inline-block",
        padding: "0 0.1em",
      }}
    >
      <span
        style={{
          position: "absolute",
          left: 0,
          right: 0,
          top: "14%",
          bottom: "4%",
          background: BRAND.highlight,
          borderRadius: 14,
          transform: `scaleX(${p}) rotate(-1.2deg)`,
          transformOrigin: "left center",
        }}
      />
      <span style={{ position: "relative" }}>{children}</span>
    </span>
  );
};

const Headline: React.FC<{
  lines: React.ReactNode[];
  size?: number;
  delay?: number;
}> = ({ lines, size = 118, delay = 0 }) => (
  <div
    style={{
      fontSize: size,
      fontWeight: 800,
      letterSpacing: "-0.035em",
      lineHeight: 1.08,
      textAlign: "center",
    }}
  >
    {lines.map((line, i) => (
      <RiseIn key={i} delay={delay + i * 6}>
        {line}
      </RiseIn>
    ))}
  </div>
);

const Chip: React.FC<{
  delay: number;
  children: React.ReactNode;
  tone?: "yellow" | "white";
}> = ({ delay, children, tone = "white" }) => (
  <RiseIn
    delay={delay}
    style={{
      display: "flex",
      alignItems: "center",
      gap: 14,
      fontSize: 34,
      fontWeight: 700,
      padding: "14px 28px",
      borderRadius: 999,
      background: tone === "yellow" ? BRAND.highlight : "white",
      border: tone === "white" ? `2px solid ${BRAND.line}` : "none",
      color: BRAND.ink,
    }}
  >
    {children}
  </RiseIn>
);

const Hook: React.FC = () => (
  <Scene duration={SCENES.hook}>
    <Headline
      size={130}
      lines={[
        "Presenting",
        "a PDF?",
        <>
          <Mark delay={40}>Eyes on you.</Mark>
        </>,
      ]}
    />
  </Scene>
);

const Spinner: React.FC<{ size: number }> = ({ size }) => {
  const frame = useCurrentFrame();
  return (
    <div
      style={{
        width: size,
        height: size,
        borderRadius: "50%",
        border: `${size / 8}px solid ${BRAND.line}`,
        borderTopColor: "#64748b",
        rotate: `${frame * 10}deg`,
      }}
    />
  );
};

const SLOW_PAGES: SheetKind[] = [
  "plan",
  "section",
  "detail",
  "section",
  "plan",
];
const SLOW_PAGE_WIDTH = 760;
const SLOW_PAGE_HEIGHT = Math.round(SLOW_PAGE_WIDTH / 1.414);
const SLOW_CONTENT_HEIGHT = 820;
// Frame each page starts loading, and how long it spins before a blurry render
const SLOW_LOAD_START = [0, 20, 50, 80, 110];
const SLOW_LOAD_TIME = 28;

// A generic, slow PDF viewer. The scroll lurches forward on the beat and
// stalls, pages sit on spinners, then render blurry.
const SlowViewer: React.FC = () => {
  const frame = useCurrentFrame();
  const step = SLOW_PAGE_HEIGHT + 30;
  const offset = [15, 45, 75, 105].reduce(
    (sum, at) => sum + interpolate(frame, [at, at + 5], [0, step * 0.8], clamp),
    0,
  );
  const notResponding = frame >= 83;
  const page = Math.min(SLOW_PAGES.length, Math.floor(offset / step + 0.4) + 1);

  return (
    <RiseIn
      style={{
        width: 960,
        borderRadius: 24,
        overflow: "hidden",
        background: "white",
        border: `2px solid ${BRAND.line}`,
        boxShadow: "0 50px 100px -20px rgba(30, 41, 59, 0.25)",
      }}
    >
      <div
        style={{
          height: 60,
          display: "flex",
          alignItems: "center",
          padding: "0 24px",
          fontSize: 26,
          color: BRAND.slate,
          borderBottom: `2px solid ${BRAND.line}`,
        }}
      >
        Drawing-Set.pdf{notResponding ? " (Not Responding)" : ""}
      </div>
      <div
        style={{
          position: "relative",
          height: SLOW_CONTENT_HEIGHT,
          background: "#d9dee5",
          overflow: "hidden",
        }}
      >
        {SLOW_PAGES.map((kind, i) => {
          const top = 40 + i * step - offset;
          if (top > SLOW_CONTENT_HEIGHT || top + SLOW_PAGE_HEIGHT < 0) {
            return null;
          }
          const loadedAt = SLOW_LOAD_START[i] + SLOW_LOAD_TIME;
          const blur = interpolate(
            frame,
            [loadedAt, loadedAt + 60],
            [12, 4],
            clamp,
          );
          return (
            <div
              key={i}
              style={{
                position: "absolute",
                left: (960 - SLOW_PAGE_WIDTH) / 2,
                top,
                width: SLOW_PAGE_WIDTH,
                height: SLOW_PAGE_HEIGHT,
                background: "white",
                overflow: "hidden",
              }}
            >
              {frame < loadedAt ? (
                <AbsoluteFill
                  style={{
                    alignItems: "center",
                    justifyContent: "center",
                    gap: 20,
                    fontSize: 26,
                    color: BRAND.slate,
                  }}
                >
                  <Spinner size={80} />
                  Loading page {i + 1}…
                </AbsoluteFill>
              ) : (
                <div style={{ filter: `blur(${blur}px)` }}>
                  <Sheet kind={kind} width={SLOW_PAGE_WIDTH} />
                </div>
              )}
            </div>
          );
        })}
        {notResponding ? (
          <AbsoluteFill style={{ background: "rgba(255, 255, 255, 0.45)" }} />
        ) : null}
      </div>
      <div
        style={{
          height: 64,
          display: "flex",
          alignItems: "center",
          gap: 16,
          padding: "0 24px",
          fontSize: 26,
          color: BRAND.slate,
          borderTop: `2px solid ${BRAND.line}`,
        }}
      >
        <Spinner size={30} />
        Rendering page {page} of 340…
        <div style={{ flex: 1 }} />
        {((frame / 30) * 1.5 + 1).toFixed(1)}s
      </div>
    </RiseIn>
  );
};

const Buffering: React.FC = () => (
  <Scene duration={SCENES.buffering}>
    <Headline
      size={104}
      lines={[
        "Then every page",
        <>
          <Mark delay={83}>buffers…</Mark>
        </>,
      ]}
    />
    <div style={{ height: 60 }} />
    <SlowViewer />
  </Scene>
);

const Reveal: React.FC = () => {
  const pop = useSpring(0);
  const highlight = useSpring(14, 16);
  return (
    <Scene duration={SCENES.reveal}>
      <div
        style={{
          transform: `scale(${interpolate(pop, [0, 1], [0.6, 1])})`,
          opacity: pop,
        }}
      >
        <Logo size={300} highlight={highlight} />
      </div>
      <RiseIn
        delay={10}
        style={{
          fontSize: 150,
          fontWeight: 800,
          letterSpacing: "-0.04em",
          marginTop: 40,
        }}
      >
        Kinetic PDF
      </RiseIn>
      <RiseIn
        delay={18}
        style={{
          fontSize: 52,
          fontWeight: 500,
          color: BRAND.slate,
          textAlign: "center",
          marginTop: 10,
          lineHeight: 1.3,
        }}
      >
        Just your PDF.
        <br />
        No frills.
      </RiseIn>
    </Scene>
  );
};

// Headline on top, the app window below, chips under that
const AppScene: React.FC<{
  duration: number;
  lines: React.ReactNode[];
  chips?: React.ReactNode;
  size?: number;
  top?: number;
  children: React.ReactNode;
}> = ({ duration, lines, chips, size = 112, top = 290, children }) => {
  const p = useSpring(4);
  return (
    <Scene duration={duration} justify="flex-start">
      <div style={{ height: top }} />
      <Headline lines={lines} size={size} />
      <div style={{ height: 60 }} />
      <div
        style={{
          opacity: p,
          transform: `translateY(${interpolate(p, [0, 1], [80, 0])}px)`,
        }}
      >
        {children}
      </div>
      <div style={{ display: "flex", gap: 18, marginTop: 44 }}>{chips}</div>
    </Scene>
  );
};

// Areas of the 1920x1080 recording to show
type Crop = { x: number; y: number; w: number; h: number };
// Explorer plus the Kinetic PDF window
const OPEN_CROP: Crop = { x: 360, y: 255, w: 1080, h: 540 };
// The page column and page counter, zoomed in
const SCROLL_CROP: Crop = { x: 540, y: 280, w: 450, h: 508 };
// Seconds in the recording
const OPEN_FROM = 0;
const SCROLL_FROM = 4;

// Shows part of the screen recording, scaled to `width`. Holds the first
// frame for `holdFrames` before playing.
const RecordingCard: React.FC<{
  src: string;
  crop: Crop;
  width: number;
  fromSec: number;
  holdFrames?: number;
}> = ({ src, crop, width, fromSec, holdFrames = 0 }) => {
  const { fps } = useVideoConfig();
  const scale = width / crop.w;
  const video = (
    <OffthreadVideo
      src={staticFile(src)}
      muted
      trimBefore={Math.round(fromSec * fps)}
      style={{
        position: "absolute",
        width: 1920 * scale,
        height: 1080 * scale,
        left: -crop.x * scale,
        top: -crop.y * scale,
        maxWidth: "none",
      }}
    />
  );
  return (
    <div
      style={{
        position: "relative",
        width,
        height: Math.round(crop.h * scale),
        borderRadius: 24,
        overflow: "hidden",
        background: "white",
        border: `2px solid ${BRAND.line}`,
        boxShadow:
          "0 50px 100px -20px rgba(30, 41, 59, 0.28), 0 20px 40px -20px rgba(37, 99, 235, 0.25)",
      }}
    >
      {holdFrames > 0 ? (
        <Sequence durationInFrames={holdFrames} layout="none">
          <Freeze frame={0}>{video}</Freeze>
        </Sequence>
      ) : null}
      <Sequence from={holdFrames} layout="none">
        {video}
      </Sequence>
    </div>
  );
};

const Opens: React.FC<{ src: string }> = ({ src }) => (
  <AppScene
    duration={SCENES.opens}
    top={440}
    lines={[
      "Double-click.",
      <>
        It&apos;s <Mark delay={24}>open.</Mark>
      </>,
    ]}
    chips={
      <Chip delay={20} tone="yellow">
        100-page drawing set
      </Chip>
    }
  >
    <RecordingCard
      src={src}
      crop={OPEN_CROP}
      width={1000}
      fromSec={OPEN_FROM}
      holdFrames={18}
    />
  </AppScene>
);

// Zooms smoothly toward a pit label on the site plan
const Zoom: React.FC = () => {
  const frame = useCurrentFrame();
  const zoom = interpolate(frame, [4, 46], [1, 3.4], {
    ...clamp,
    easing: Easing.inOut(Easing.cubic),
  });
  const pageTop = (APP.contentHeight - PAGE_HEIGHT) / 2;
  const target = {
    x: (520 / 1414) * APP.pageWidth,
    y: (548 / 1000) * PAGE_HEIGHT,
  };
  return (
    <AppScene
      duration={SCENES.zoom}
      lines={[
        "Zoom in",
        <>
          for the <Mark delay={18}>room.</Mark>
        </>,
      ]}
    >
      <AppWindow
        fileName="Site-Plan.pdf"
        zoomLabel={`${Math.round(57 * zoom)}%`}
      >
        <Page
          kind="plan"
          style={{
            top: pageTop,
            scale: String(zoom),
            transformOrigin: `${target.x}px ${target.y}px`,
            translate: `${interpolate(zoom, [1, 3.4], [0, APP.pageWidth / 2 - target.x])}px ${interpolate(zoom, [1, 3.4], [0, PAGE_HEIGHT / 2 - target.y])}px`,
          }}
        />
      </AppWindow>
    </AppScene>
  );
};

const Scroll: React.FC<{ src: string }> = ({ src }) => (
  <AppScene
    duration={SCENES.scroll}
    size={96}
    lines={[
      "100 pages,",
      <>
        <Mark delay={18}>fully rendered.</Mark>
      </>,
    ]}
    chips={
      <Chip delay={30} tone="yellow">
        No blank pages. No spinners.
      </Chip>
    }
  >
    <RecordingCard
      src={src}
      crop={SCROLL_CROP}
      width={800}
      fromSec={SCROLL_FROM}
    />
  </AppScene>
);

const NOTES = [
  "1. All dimensions are in millimetres.",
  "2. Confirm levels on site before starting.",
  "3. Locate existing services by hand.",
  "4. Refer to C-301 for the pit schedule.",
];

// Drag across a line to highlight it, then pin a note to it
const HighlightDemo: React.FC = () => {
  const frame = useCurrentFrame();
  const drag = interpolate(frame, [22, 40], [0, 1], {
    ...clamp,
    easing: Easing.inOut(Easing.quad),
  });
  const note = useSpring(52);
  const lineY = 250;
  const lineX = 90;
  const lineWidth = 700;
  return (
    <AppWindow fileName="General-Notes.pdf" pageLabel="2 / 12">
      <div
        style={{
          position: "absolute",
          left: 50,
          top: 40,
          width: 900,
          height: 620,
          background: "white",
          boxShadow: "0 6px 20px rgba(15, 23, 42, 0.15)",
          fontFamily,
          color: BRAND.ink,
        }}
      >
        <div
          style={{
            position: "absolute",
            left: lineX - 10,
            top: 60,
            fontSize: 38,
            fontWeight: 800,
          }}
        >
          GENERAL NOTES
        </div>
        <div
          style={{
            position: "absolute",
            left: lineX - 10,
            top: lineY - 8,
            width: lineWidth * drag,
            height: 58,
            background: BRAND.highlight,
            borderRadius: 6,
          }}
        />
        {NOTES.map((text, i) => (
          <div
            key={text}
            style={{
              position: "absolute",
              left: lineX,
              top: 150 + i * 100,
              fontSize: 32,
              fontWeight: 500,
            }}
          >
            {text}
          </div>
        ))}
        <div
          style={{
            position: "absolute",
            left: 420,
            top: lineY + 70,
            width: 420,
            padding: "22px 26px",
            borderRadius: 16,
            background: "white",
            border: `2px solid ${BRAND.line}`,
            borderLeft: `8px solid ${BRAND.highlight}`,
            boxShadow: "0 24px 50px -12px rgba(15, 23, 42, 0.35)",
            opacity: note,
            transform: `translateY(${interpolate(note, [0, 1], [30, 0])}px)`,
          }}
        >
          <div style={{ fontSize: 20, color: BRAND.slate, fontWeight: 700 }}>
            NOTE
          </div>
          <div style={{ fontSize: 30, fontWeight: 700, marginTop: 6 }}>
            Raise this in Monday&apos;s meeting
          </div>
        </div>
      </div>
      <Cursor
        x={interpolate(frame, [0, 22, 40], [620, 130, 130 + lineWidth - 40], {
          ...clamp,
          easing: Easing.inOut(Easing.quad),
        })}
        y={40 + lineY + 14}
      />
    </AppWindow>
  );
};

const Highlight: React.FC = () => (
  <AppScene
    duration={SCENES.highlight}
    lines={[
      <>
        <Mark delay={26}>Highlight.</Mark>
      </>,
      "Add a note.",
    ]}
    chips={
      <>
        <Chip delay={70}>No account</Chip>
        <Chip delay={76}>No sign-in</Chip>
        <Chip delay={82}>Nothing collected</Chip>
      </>
    }
  >
    <HighlightDemo />
  </AppScene>
);

const CallToAction: React.FC = () => {
  const pop = useSpring(0);
  const highlight = useSpring(10, 16);
  return (
    <Scene duration={SCENES.cta}>
      <div
        style={{
          transform: `scale(${interpolate(pop, [0, 1], [0.6, 1])})`,
          opacity: pop,
        }}
      >
        <Logo size={220} highlight={highlight} />
      </div>
      <RiseIn
        delay={8}
        style={{
          fontSize: 120,
          fontWeight: 800,
          letterSpacing: "-0.04em",
          marginTop: 36,
        }}
      >
        Kinetic PDF
      </RiseIn>
      <RiseIn
        delay={16}
        style={{
          fontSize: 56,
          fontWeight: 500,
          color: BRAND.slate,
          marginTop: 6,
        }}
      >
        <span style={{ color: BRAND.ink, fontWeight: 700 }}>
          <Mark delay={24}>Free</Mark>
        </span>{" "}
        to download
      </RiseIn>
      <RiseIn delay={22} style={{ marginTop: 40 }}>
        {/* Official badge from https://get.microsoft.com/images/en-us%20dark.svg, unmodified */}
        <Img
          src={staticFile("ms-store-badge.svg")}
          style={{ width: 320, height: 87, display: "block" }}
        />
      </RiseIn>
      <RiseIn
        delay={40}
        style={{
          fontSize: 44,
          fontWeight: 500,
          color: BRAND.slate,
          marginTop: 60,
        }}
      >
        Link in the comments ↓
      </RiseIn>
    </Scene>
  );
};

// Small logo lockup pinned to the top while the app is on screen
const BrandBar: React.FC<{ duration: number }> = ({ duration }) => {
  const frame = useCurrentFrame();
  const opacity = interpolate(
    frame,
    [0, 10, duration - 6, duration],
    [0, 1, 1, 0],
    clamp,
  );
  return (
    <AbsoluteFill
      style={{
        opacity,
        alignItems: "center",
        top: 120,
        fontFamily,
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 18,
          fontSize: 44,
          fontWeight: 700,
          letterSpacing: "-0.02em",
          color: BRAND.ink,
        }}
      >
        <Logo size={64} highlight={1} />
        Kinetic PDF
      </div>
    </AbsoluteFill>
  );
};

export const Promo: React.FC<PromoProps> = ({ music, recording }) => {
  const { durationInFrames } = useVideoConfig();
  let from = 0;
  const at = (duration: number) => {
    const start = from;
    from += duration;
    return { from: start, durationInFrames: duration };
  };

  const appStart = SCENES.hook + SCENES.buffering + SCENES.reveal;
  const appDuration =
    SCENES.opens + SCENES.zoom + SCENES.scroll + SCENES.highlight;

  return (
    <AbsoluteFill
      style={{
        backgroundColor: BRAND.paper,
        backgroundImage: [
          "radial-gradient(900px 700px at 85% 0%, rgba(91, 155, 248, 0.22), transparent 70%)",
          "radial-gradient(800px 600px at 0% 100%, rgba(252, 211, 77, 0.2), transparent 70%)",
        ].join(", "),
      }}
    >
      {music ? (
        <Audio
          src={staticFile(music)}
          volume={(f) =>
            interpolate(
              f,
              [0, 8, durationInFrames - 30, durationInFrames],
              [0, 1, 1, 0],
              clamp,
            )
          }
        />
      ) : null}
      <Sequence {...at(SCENES.hook)} name="Hook">
        <Hook />
      </Sequence>
      <Sequence {...at(SCENES.buffering)} name="Buffering">
        <Buffering />
      </Sequence>
      <Sequence {...at(SCENES.reveal)} name="Reveal">
        <Reveal />
      </Sequence>
      <Sequence {...at(SCENES.opens)} name="Opens">
        <Opens src={recording} />
      </Sequence>
      <Sequence {...at(SCENES.zoom)} name="Zoom">
        <Zoom />
      </Sequence>
      <Sequence {...at(SCENES.scroll)} name="Scroll">
        <Scroll src={recording} />
      </Sequence>
      <Sequence {...at(SCENES.highlight)} name="Highlight">
        <Highlight />
      </Sequence>
      <Sequence {...at(SCENES.cta)} name="Call to action">
        <CallToAction />
      </Sequence>
      <Sequence from={appStart} durationInFrames={appDuration} name="Brand bar">
        <BrandBar duration={appDuration} />
      </Sequence>
    </AbsoluteFill>
  );
};
