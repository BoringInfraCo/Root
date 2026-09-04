import React from "react";
import { AbsoluteFill } from "remotion";
import { theme } from "../../styles/theme";
import { GhosttyTerminal, TermLine } from "../../components/GhosttyTerminal";
import { MachineTag, SceneCaption } from "./SceneCaption";

const LINES: TermLine[] = [
  { delay: 8, type: "command", text: "root agent-bundle export" },
  { delay: 34, type: "blank", text: "" },
  { delay: 38, type: "output", text: "→ scanning local agent configs..." },
  { delay: 50, type: "output", text: "→ found:" },
  { delay: 56, type: "output", text: "  • Codex" },
  { delay: 62, type: "output", text: "  • Claude" },
  { delay: 68, type: "output", text: "  • OpenCode" },
  { delay: 80, type: "blank", text: "" },
  { delay: 90, type: "output", text: "→ bundling agent environment..." },
  { delay: 114, type: "success", text: "✓ created ./root-agent-bundle" },
];

export const ExportScene: React.FC = () => {
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
        <MachineTag label="old machine" delay={0} />
        <GhosttyTerminal
          lines={LINES}
          user="sergio"
          host="mbp-old"
          title="root-demo"
          width={1100}
          height={520}
          fontSize={21}
          typingSpeed={1}
        />
        <SceneCaption text="export current setup" delay={118} />
      </div>
    </AbsoluteFill>
  );
};
