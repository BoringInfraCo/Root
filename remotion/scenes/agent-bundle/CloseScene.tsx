import React from "react";
import { AbsoluteFill } from "remotion";
import { theme } from "../../styles/theme";
import { FadeIn } from "../../components/FadeIn";

export const CloseScene: React.FC = () => {
  return (
    <AbsoluteFill
      style={{
        backgroundColor: theme.colors.bg.primary,
        justifyContent: "center",
        alignItems: "center",
        padding: 80,
      }}
    >
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          gap: 18,
        }}
      >
        <FadeIn delay={2} duration={18} direction="up" distance={12}>
          <h1
            style={{
              fontFamily: theme.fonts.sans,
              fontSize: 64,
              fontWeight: 600,
              color: theme.colors.text.primary,
              margin: 0,
              letterSpacing: "-0.04em",
            }}
          >
            Root
          </h1>
        </FadeIn>

        <FadeIn delay={12} duration={18} direction="up" distance={10}>
          <p
            style={{
              fontFamily: theme.fonts.sans,
              fontSize: 22,
              fontWeight: 400,
              color: theme.colors.text.secondary,
              margin: 0,
              letterSpacing: "-0.01em",
            }}
          >
            move your agent setup between machines
          </p>
        </FadeIn>

        <FadeIn delay={28} duration={18} direction="up" distance={8}>
          <p
            style={{
              fontFamily: theme.fonts.mono,
              fontSize: 18,
              color: theme.colors.accent.blue,
              margin: "8px 0 0 0",
            }}
          >
            github.com/BoringInfraCo/Root
          </p>
        </FadeIn>

        <FadeIn delay={32} duration={16} direction="none">
          <p
            style={{
              fontFamily: theme.fonts.mono,
              fontSize: 14,
              color: theme.colors.text.tertiary,
              margin: "12px 0 0 0",
            }}
          >
            Codex / Claude / OpenCode
          </p>
        </FadeIn>
      </div>
    </AbsoluteFill>
  );
};
