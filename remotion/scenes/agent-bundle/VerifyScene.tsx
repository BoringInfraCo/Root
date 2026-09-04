import React from "react";
import { AbsoluteFill } from "remotion";
import { theme } from "../../styles/theme";
import { GhosttyTerminal, TermLine } from "../../components/GhosttyTerminal";
import { SceneCaption } from "./SceneCaption";

const LINES: TermLine[] = [
  { delay: 4, type: "command", text: "root agent-bundle verify" },
  { delay: 29, type: "blank", text: "" },
  {
    delay: 32,
    type: "output",
    text: "Agent environments",
    parts: [{ text: "Agent environments", color: "#e5e5e5" }],
  },
  {
    delay: 35,
    type: "output",
    text: "──────────────────",
    parts: [{ text: "──────────────────", color: "#737373" }],
  },
  {
    delay: 38,
    type: "output",
    text: "Codex      ✓ configured",
    parts: [
      { text: "Codex      ", color: "#a3a3a3" },
      { text: "✓", color: "#4ade80" },
      { text: " configured", color: "#e5e5e5" },
    ],
  },
  {
    delay: 42,
    type: "output",
    text: "Claude     ✓ configured",
    parts: [
      { text: "Claude     ", color: "#a3a3a3" },
      { text: "✓", color: "#4ade80" },
      { text: " configured", color: "#e5e5e5" },
    ],
  },
  {
    delay: 46,
    type: "output",
    text: "OpenCode   ✓ configured",
    parts: [
      { text: "OpenCode   ", color: "#a3a3a3" },
      { text: "✓", color: "#4ade80" },
      { text: " configured", color: "#e5e5e5" },
    ],
  },
];

export const VerifyScene: React.FC = () => {
  return (
    <AbsoluteFill
      style={{
        backgroundColor: theme.colors.bg.primary,
        justifyContent: "center",
        alignItems: "center",
        padding: 72,
      }}
    >
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          gap: 22,
          width: "100%",
        }}
      >
        <GhosttyTerminal
          lines={LINES}
          user="sergio"
          host="mbp-new"
          title="root-demo"
          width={960}
          height={440}
          fontSize={21}
          typingSpeed={1}
        />
        <SceneCaption
          text="no more setting it all up again from scratch"
          delay={50}
        />
      </div>
    </AbsoluteFill>
  );
};
