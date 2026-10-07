import React from "react";
import { AbsoluteFill, OffthreadVideo, staticFile } from "remotion";

// Clean footage for reuse inside any ad scene. Captured at 30 fps.
export const ToolsRecording: React.FC = () => (
  <AbsoluteFill style={{ background: "#fff" }}>
    <OffthreadVideo
      src={staticFile("ctrl-k-tools.mp4")}
      muted
      style={{ width: "100%", height: "100%", objectFit: "contain" }}
    />
  </AbsoluteFill>
);
