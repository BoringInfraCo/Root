import React from "react";
import { AbsoluteFill } from "remotion";
import { theme } from "../../styles/theme";
import { GhosttyTerminal, TermLine } from "../../components/GhosttyTerminal";
import { MachineTag, SceneCaption } from "./SceneCaption";

const LINES: TermLine[] = [
  {
    delay: 8,
    type: "command",
    text: "root agent-bundle apply ./root-agent-bundle",
  },
  { delay: 40, type: "blank", text: "" },
  { delay: 44, type: "output", text: "→ unpacking agent environment..." },
  { delay: 56, type: "output", text: "→ restoring:" },
  { delay: 64, type: "output", text: "  • Codex config" },
  { delay: 72, type: "output", text: "  • Claude config" },
  { delay: 80, type: "output", text: "  • OpenCode config" },
  { delay: 92, type: "blank", text: "" },
  { delay: 96, type: "output", text: "→ verifying restored files..." },
  { delay: 112, type: "success", text: "✓ apply complete" },
];

export const ApplyScene: React.FC = () => {
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
        <MachineTag label="new machine" delay={0} />
        <GhosttyTerminal
          lines={LINES}
          user="sergio"
          host="mbp-new"
          title="root-demo"
          width={1100}
          height={540}
          fontSize={21}
          typingSpeed={0.7}
        />
        <SceneCaption text="restore the setup in one step" delay={116} />
      </div>
    </AbsoluteFill>
  );
};
