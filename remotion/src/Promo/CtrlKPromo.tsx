import React from "react";
import {
  AbsoluteFill,
  Audio,
  Easing,
  Freeze,
  interpolate,
  OffthreadVideo,
  Sequence,
  staticFile,
  useCurrentFrame,
} from "remotion";
import { Logo } from "./Logo";

export const CTRL_K_PROMO_FRAMES = 405;
// Hold the first-letter result, then continue from precisely that source frame.
const SOURCE_START = 113;
const OPENING_HOLD = 24;
const FOOTAGE_FRAMES = 312;
const clamp = { extrapolateLeft: "clamp", extrapolateRight: "clamp" } as const;
const ease = { ...clamp, easing: Easing.inOut(Easing.cubic) };
const blue = "#2563eb";

const Key: React.FC<{ children: React.ReactNode; active?: boolean }> = ({
  children,
  active,
}) => (
  <div
    style={{
      padding: "15px 27px",
      minWidth: 82,
      textAlign: "center",
      borderRadius: 16,
      fontSize: 40,
      fontWeight: 700,
      background: active ? blue : "#fff",
      color: active ? "#fff" : "#172b48",
      border: `2px solid ${active ? blue : "#d0dcec"}`,
      boxShadow: active ? "0 7px 0 #1747b0" : "0 7px 0 #d0dcec",
      transform: active ? "translateY(4px)" : "none",
    }}
  >
    {children}
  </div>
);

const Footage: React.FC = () => {
  const f = useCurrentFrame();
  const source = SOURCE_START + f;
  // Frame the real search list, then follow the action down to the measurement.
  const camera = interpolate(source, [184, 240], [0, 1], ease);
  const scale = interpolate(camera, [0, 1], [1.01, 1.48]);
  const cx = interpolate(camera, [0, 1], [960, 720]);
  const cy = interpolate(camera, [0, 1], [407, 650]);
  return (
    <OffthreadVideo
      src={staticFile("ctrl-k-tools.mp4")}
      trimBefore={SOURCE_START}
      muted
      style={{
        position: "absolute",
        width: 1920 * scale,
        height: 1080 * scale,
        maxWidth: "none",
        left: 492 - cx * scale,
        top: 400 - cy * scale,
      }}
    />
  );
};

export const CtrlKPromo: React.FC = () => {
  const f = useCurrentFrame();
  const source = SOURCE_START + Math.max(0, f - OPENING_HOLD);
  const step = f < OPENING_HOLD ? 0 : source < 178 ? 1 : source < 225 ? 2 : 3;
  const finished = f >= OPENING_HOLD + FOOTAGE_FRAMES;
  const headings = [
    <>
      Find your tools.
      <br />
      <span style={{ color: blue }}>Press Ctrl + K.</span>
    </>,
    <>
      Type a few letters.
      <br />
      <span style={{ color: blue }}>There’s your tool.</span>
    </>,
    <>
      Press Enter.
      <br />
      <span style={{ color: blue }}>Settings included.</span>
    </>,
    <>
      Same saved settings.
      <br />
      <span style={{ color: blue }}>Straight to drawing.</span>
    </>,
  ];
  return (
    <AbsoluteFill
      style={{
        fontFamily: "Segoe UI, Arial, sans-serif",
        background: "#f1f5fb",
        color: "#172b48",
      }}
    >
      <Audio
        src={staticFile("music.mp3")}
        volume={(frame) =>
          interpolate(frame, [0, 15, 365, 404], [0, 0.22, 0.22, 0], clamp)
        }
      />
      <AbsoluteFill
        style={{
          backgroundImage:
            "radial-gradient(ellipse at 100% 0%, #c9dcff 0%, transparent 65%)",
        }}
      />
      <div
        style={{
          position: "absolute",
          top: 100,
          left: 68,
          display: "flex",
          alignItems: "center",
          gap: 17,
          fontSize: 32,
          fontWeight: 700,
        }}
      >
        <Logo size={62} shadow={false} />
        Kinetic PDF
      </div>
      <div style={{ position: "absolute", top: 248, left: 68, right: 60 }}>
        <div
          style={{
            fontSize: 20,
            fontWeight: 700,
            letterSpacing: 3,
            color: "#617690",
            marginBottom: 27,
          }}
        >
          A SMALL UPDATE I’VE BEEN WORKING ON
        </div>
        <div
          key={step}
          style={{
            fontSize: step === 0 ? 90 : 76,
            lineHeight: 1.1,
            fontWeight: 750,
            letterSpacing: -3,
          }}
        >
          {finished ? (
            <>
              One less trip
              <br />
              <span style={{ color: blue }}>through the menus.</span>
            </>
          ) : (
            headings[step]
          )}
        </div>
      </div>
      <div
        style={{
          position: "absolute",
          left: 48,
          top: 600,
          width: 984,
          height: 870,
          borderRadius: 25,
          overflow: "hidden",
          background: "#fff",
          boxShadow: "0 30px 80px #233f6a2b",
          border: "1px solid #c8d6e8",
        }}
      >
        <div
          style={{
            height: 70,
            display: "flex",
            alignItems: "center",
            gap: 12,
            padding: "0 27px",
            borderBottom: "1px solid #e2e8f0",
            background: "#fff",
            fontSize: 20,
            color: "#64748b",
          }}
        >
          <span
            style={{
              width: 10,
              height: 10,
              background: blue,
              borderRadius: "50%",
            }}
          />
          Kinetic PDF <span style={{ marginLeft: "auto" }}>Ctrl + K</span>
        </div>
        <div
          style={{
            position: "absolute",
            top: 70,
            left: 0,
            width: 984,
            height: 800,
            overflow: "hidden",
          }}
        >
          {/* Frame zero doubles as a feed thumbnail: show matches after the first letter. */}
          {f < OPENING_HOLD && (
            <Freeze frame={0}>
              <Footage />
            </Freeze>
          )}
          <Sequence
            from={OPENING_HOLD}
            durationInFrames={FOOTAGE_FRAMES}
            layout="none"
          >
            <Footage />
          </Sequence>
          {finished && (
            <Freeze frame={FOOTAGE_FRAMES - 1}>
              <Footage />
            </Freeze>
          )}
        </div>
      </div>
      <div
        style={{
          position: "absolute",
          top: 1530,
          left: 68,
          right: 68,
          display: "flex",
          alignItems: "center",
          gap: 18,
        }}
      >
        {step < 2 ? (
          <>
            <Key active={f < OPENING_HOLD}>Ctrl</Key>
            <span style={{ fontSize: 36, color: "#7c8ca3" }}>+</span>
            <Key active={f < OPENING_HOLD}>K</Key>
          </>
        ) : step === 2 ? (
          <Key active>Enter ↵</Key>
        ) : (
          <div
            style={{
              width: 64,
              height: 64,
              borderRadius: "50%",
              background: blue,
              color: "#fff",
              display: "grid",
              placeItems: "center",
              fontSize: 36,
            }}
          >
            ✓
          </div>
        )}
        <div style={{ fontSize: 31, fontWeight: 650, marginLeft: 12 }}>
          {finished
            ? "Ctrl + K in Kinetic PDF"
            : [
                "Saved tools, by name.",
                "Search “slab”",
                "Select the saved tool",
                "Draw with the saved preset",
              ][step]}
        </div>
      </div>
      <div
        style={{
          position: "absolute",
          left: 68,
          right: 68,
          top: 1703,
          display: "flex",
          gap: 10,
        }}
      >
        {["FIND", "SELECT", "DRAW"].map((label, i) => (
          <div key={label} style={{ flex: 1 }}>
            <div
              style={{
                height: 5,
                borderRadius: 4,
                background: step >= i + 1 || i === 0 ? blue : "#d2deed",
                marginBottom: 19,
              }}
            />
            <div
              style={{
                fontSize: 19,
                fontWeight: 700,
                letterSpacing: 2,
                color: step >= i + 1 || i === 0 ? blue : "#8a9bb1",
              }}
            >
              {label}
            </div>
          </div>
        ))}
      </div>
    </AbsoluteFill>
  );
};
