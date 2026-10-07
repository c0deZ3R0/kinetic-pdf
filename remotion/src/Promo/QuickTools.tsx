import React from "react";
import {
  AbsoluteFill,
  Img,
  interpolate,
  staticFile,
  useCurrentFrame,
} from "remotion";
import { Logo } from "./Logo";

// A motion illustration of the kept-tools workflow, using the generated sample PDF.
export const QuickTools: React.FC = () => {
  const f = useCurrentFrame();
  const phase = f < 75 ? 0 : f < 150 ? 1 : f < 240 ? 2 : 3;
  const query =
    f < 160 ? "" : "Slab".slice(0, Math.min(4, Math.floor((f - 160) / 6) + 1));
  const open = f >= 75 && f < 240;
  const opacity = interpolate(f, [0, 12, 348, 359], [0, 1, 1, 0], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  const tools = query ? ["Slab 200"] : ["Slab 200", "Fence", "Wall finish"];
  return (
    <AbsoluteFill
      style={{
        background: "#edf3fa",
        fontFamily: "Segoe UI, Arial, sans-serif",
        color: "#162942",
        opacity,
      }}
    >
      <div
        style={{
          position: "absolute",
          inset: 0,
          background:
            "radial-gradient(ellipse at 90% 15%, #c9dcff 0%, transparent 60%)",
        }}
      />
      <div
        style={{
          position: "absolute",
          top: 105,
          left: 76,
          display: "flex",
          alignItems: "center",
          gap: 20,
        }}
      >
        <Logo size={64} shadow={false} />
        <span style={{ fontSize: 33, fontWeight: 650 }}>Kinetic PDF</span>
      </div>
      <div style={{ position: "absolute", top: 265, left: 76, right: 76 }}>
        <div
          style={{
            fontSize: 21,
            letterSpacing: 4,
            fontWeight: 700,
            color: "#2563eb",
            marginBottom: 28,
          }}
        >
          A SHORTCUT TO YOUR SAVED TOOLS
        </div>
        <div
          style={{
            fontSize: 92,
            fontWeight: 750,
            lineHeight: 1.05,
            letterSpacing: -4,
          }}
        >
          Find your
          <br />
          tools fast.
        </div>
        <div style={{ marginTop: 38, fontSize: 31, color: "#52647b" }}>
          Your settings. Ready when you are.
        </div>
      </div>
      <div
        style={{
          position: "absolute",
          top: 660,
          left: 56,
          width: 968,
          height: 760,
          background: "#fff",
          borderRadius: 24,
          overflow: "hidden",
          boxShadow: "0 24px 65px #263f6826",
          border: "1px solid #d1dce9",
        }}
      >
        <div
          style={{
            height: 61,
            display: "flex",
            alignItems: "center",
            padding: "0 25px",
            background: "#fff",
            fontSize: 21,
            borderBottom: "1px solid #e2e8f0",
          }}
        >
          Kestrel Lane — Ground floor plan{" "}
          <span style={{ marginLeft: "auto", color: "#98a5b6" }}>— □ ×</span>
        </div>
        <div
          style={{
            height: 57,
            padding: "0 24px",
            display: "flex",
            alignItems: "center",
            gap: 27,
            fontSize: 20,
            borderBottom: "1px solid #e2e8f0",
          }}
        >
          File <span>Select</span>
          <span>Measure</span>
          <span>Markup</span>
          <span style={{ color: "#2563eb", marginLeft: "auto" }}>
            Kept tools
          </span>
        </div>
        <Img
          src={staticFile("ctrl-k-drawing.png")}
          style={{
            position: "absolute",
            top: 133,
            left: 18,
            width: 930,
            height: 600,
            objectFit: "cover",
            objectPosition: "30% 40%",
          }}
        />
        {open && (
          <div
            style={{
              position: "absolute",
              inset: "118px 0 0",
              background: "#14243c24",
            }}
          />
        )}
        {open && (
          <div
            style={{
              position: "absolute",
              top: 205,
              left: 102,
              width: 764,
              padding: 26,
              boxSizing: "border-box",
              background: "#fff",
              borderRadius: 14,
              boxShadow: "0 20px 70px #16294238",
            }}
          >
            <div style={{ fontSize: 21, color: "#65768d", marginBottom: 17 }}>
              KEPT TOOLS
            </div>
            <div
              style={{
                height: 66,
                border: "2px solid #3b82f6",
                borderRadius: 7,
                display: "flex",
                alignItems: "center",
                padding: "0 18px",
                fontSize: 29,
                marginBottom: 19,
              }}
            >
              {query || (
                <span style={{ color: "#8996a7", fontSize: 24 }}>
                  Type a kept tool’s name or group…
                </span>
              )}
              <span style={{ color: "#2563eb", opacity: f % 30 < 18 ? 1 : 0 }}>
                │
              </span>
            </div>
            {!query && (
              <div
                style={{ padding: "14px 16px", fontSize: 25, color: "#52647b" }}
              >
                ＋ Create new tool…
              </div>
            )}
            {tools.map((t, i) => (
              <div
                key={t}
                style={{
                  padding: "18px 16px",
                  borderRadius: 7,
                  background: i === 0 && query ? "#e8f0ff" : "transparent",
                  display: "flex",
                  alignItems: "center",
                  fontSize: 27,
                }}
              >
                <span
                  style={{
                    width: 18,
                    height: 18,
                    background: i === 0 ? "#2563eb" : "#c17e32",
                    marginRight: 17,
                    borderRadius: 3,
                  }}
                />
                {t}
                <span
                  style={{ marginLeft: "auto", fontSize: 19, color: "#65768d" }}
                >
                  {i === 0 ? "Concrete" : "Site"}
                </span>
              </div>
            ))}
            <div style={{ marginTop: 16, fontSize: 18, color: "#7b899b" }}>
              ↑ ↓ navigate  Enter select  Esc close
            </div>
          </div>
        )}
        {phase === 3 && (
          <>
            <svg
              width="968"
              height="760"
              style={{ position: "absolute", top: 118, left: 0 }}
            >
              <polygon
                points="330,265 560,265 560,440 330,440"
                fill="#2563eb30"
                stroke="#2563eb"
                strokeWidth="5"
              />
            </svg>
            <div
              style={{
                position: "absolute",
                bottom: 32,
                left: 30,
                background: "#2563eb",
                color: "#fff",
                padding: "15px 23px",
                borderRadius: 10,
                fontSize: 25,
              }}
            >
              Slab 200 · Area tool ready
            </div>
          </>
        )}
      </div>
      <div
        style={{
          position: "absolute",
          top: 1500,
          left: 76,
          right: 76,
          display: "flex",
          alignItems: "center",
          gap: 23,
        }}
      >
        {(phase < 2 ? ["Ctrl", "K"] : ["Enter"]).map((k) => (
          <div
            key={k}
            style={{
              background: "#fff",
              border: "2px solid #c6d4e5",
              borderBottomWidth: 7,
              borderRadius: 16,
              padding: "17px 28px",
              fontSize: 43,
              fontWeight: 650,
            }}
          >
            {k}
          </div>
        ))}
        <span style={{ fontSize: 36, fontWeight: 650 }}>
          {
            [
              "Open your tools",
              "Search by name",
              "Select your tool",
              "Get straight to work",
            ][phase]
          }
        </span>
      </div>
      <div
        style={{
          position: "absolute",
          bottom: 124,
          left: 76,
          right: 76,
          fontSize: 26,
          color: "#65768d",
        }}
      >
        Ctrl + K → Type a name → Enter
      </div>
      <div
        style={{
          position: "absolute",
          bottom: 66,
          left: 76,
          fontSize: 18,
          color: "#8291a5",
        }}
      >
        Illustrated workflow · Sample drawing
      </div>
    </AbsoluteFill>
  );
};
