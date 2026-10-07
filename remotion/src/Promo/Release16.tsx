import React from "react";
import { AbsoluteFill, Img, staticFile } from "remotion";
import { Logo } from "./Logo";

const improvements = [
  {
    number: "01",
    title: "Smoother zooming & panning",
    body: "Move around dense drawings while keeping the detail intact.",
  },
  {
    number: "02",
    title: "Faster reopening of heavy sheets",
    body: "An improved drawing cache gets previously opened sheets ready sooner.",
  },
  {
    number: "03",
    title: "Lower memory use while caching",
    body: "Fewer large copies when preparing drawings for your next visit.",
  },
];

// Release copy grounded in packaging/store-changes-0.16.0.txt.
export const Release16: React.FC = () => (
  <AbsoluteFill
    style={{
      background: "#e9f0fa",
      fontFamily: "Segoe UI, Arial, sans-serif",
      color: "#172b48",
    }}
  >
    {/* Original Store announcement drafting grid (announce-tall.html). */}
    <AbsoluteFill
      style={{
        backgroundImage: [
          "linear-gradient(to right, rgba(37,99,235,0.10) 1px, transparent 1px)",
          "linear-gradient(to bottom, rgba(37,99,235,0.10) 1px, transparent 1px)",
          "linear-gradient(to right, rgba(37,99,235,0.045) 1px, transparent 1px)",
          "linear-gradient(to bottom, rgba(37,99,235,0.045) 1px, transparent 1px)",
        ].join(", "),
        backgroundSize: "90px 90px, 90px 90px, 18px 18px, 18px 18px",
        backgroundPosition: "-1px -1px",
        WebkitMaskImage:
          "radial-gradient(ellipse 90% 60% at 50% 25%, #000 30%, transparent 100%)",
      }}
    />
    <AbsoluteFill
      style={{
        background:
          "radial-gradient(ellipse 65% 40% at 50% 25%, rgba(255,255,255,0.8), transparent 70%)",
      }}
    />
    <div
      style={{
        position: "absolute",
        left: 64,
        right: 64,
        top: 58,
        display: "flex",
        alignItems: "center",
        gap: 17,
      }}
    >
      <Logo size={65} shadow={false} />
      <span style={{ fontSize: 31, fontWeight: 700 }}>Kinetic PDF</span>
      <span
        style={{
          marginLeft: "auto",
          fontSize: 24,
          fontWeight: 700,
          color: "#2563eb",
          background: "#e3edff",
          border: "1px solid #bed3f5",
          borderRadius: 30,
          padding: "12px 23px",
        }}
      >
        v0.16.0
      </span>
    </div>
    <div style={{ position: "absolute", left: 68, right: 68, top: 181 }}>
      <div
        style={{
          fontSize: 19,
          fontWeight: 700,
          letterSpacing: 3,
          color: "#617690",
        }}
      >
        THE LATEST UPDATE
      </div>
      <div
        style={{
          fontSize: 76,
          fontWeight: 750,
          lineHeight: 1.1,
          letterSpacing: -3,
          marginTop: 20,
        }}
      >
        I thought I couldn’t
        <br />
        <span style={{ color: "#2563eb" }}>make it faster.</span>
      </div>
      <div style={{ fontSize: 27, color: "#52677f", marginTop: 23 }}>
        Turns out, there was more room to improve.
      </div>
    </div>
    <div
      style={{
        position: "absolute",
        left: 64,
        right: 64,
        top: 480,
        height: 305,
        borderRadius: 18,
        overflow: "hidden",
        border: "1px solid #ccd9e9",
        background: "#fff",
        boxShadow: "0 16px 38px #213e6614",
      }}
    >
      <div
        style={{
          height: 47,
          padding: "0 22px",
          display: "flex",
          alignItems: "center",
          gap: 9,
          borderBottom: "1px solid #e1e8f0",
          fontSize: 17,
          color: "#66798f",
        }}
      >
        <span
          style={{
            width: 8,
            height: 8,
            borderRadius: "50%",
            background: "#2563eb",
          }}
        />
        Kestrel Lane · Ground floor plan
        <span style={{ marginLeft: "auto" }}>Sample drawing</span>
      </div>
      <Img
        src={staticFile("ctrl-k-drawing.png")}
        style={{
          position: "absolute",
          top: 47,
          left: 0,
          width: 952,
          height: 258,
          objectFit: "cover",
          objectPosition: "45% 59%",
        }}
      />
      <div
        style={{
          position: "absolute",
          right: 22,
          bottom: 20,
          background: "#2563eb",
          padding: "11px 19px",
          borderRadius: 9,
          color: "#fff",
          fontWeight: 650,
          fontSize: 21,
        }}
      >
        Drawing detail preserved
      </div>
    </div>
    <div style={{ position: "absolute", left: 68, right: 68, top: 819 }}>
      {improvements.map((item, i) => (
        <div
          key={item.number}
          style={{
            display: "flex",
            gap: 23,
            padding: "0 0 25px",
            marginBottom: 24,
            borderBottom: i < 2 ? "1px solid #d4deeb" : "none",
          }}
        >
          <div
            style={{
              flexShrink: 0,
              width: 51,
              height: 51,
              borderRadius: 13,
              background: "#e0ebff",
              color: "#2563eb",
              display: "grid",
              placeItems: "center",
              fontWeight: 750,
              fontSize: 22,
            }}
          >
            {item.number}
          </div>
          <div>
            <div
              style={{
                fontSize: 31,
                fontWeight: 700,
                letterSpacing: -0.5,
                lineHeight: 1.2,
              }}
            >
              {item.title}
            </div>
            <div
              style={{
                fontSize: 23,
                lineHeight: 1.42,
                color: "#5d718a",
                marginTop: 8,
                maxWidth: 795,
              }}
            >
              {item.body}
            </div>
          </div>
        </div>
      ))}
    </div>
    <div
      style={{
        position: "absolute",
        bottom: 43,
        left: 68,
        right: 68,
        display: "flex",
        justifyContent: "space-between",
        fontSize: 18,
        color: "#6a7e98",
      }}
    >
      <span>Small improvements to everyday PDF work.</span>
      <span>Kinetic PDF · 0.16.0</span>
    </div>
  </AbsoluteFill>
);
