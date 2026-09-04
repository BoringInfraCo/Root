import React from "react";
import { AbsoluteFill, interpolate, useCurrentFrame } from "remotion";
import { theme } from "../../styles/theme";
import { FadeIn } from "../../components/FadeIn";

export const IntroScene: React.FC = () => {
  const frame = useCurrentFrame();
  const blinkOn = Math.floor(frame / 12) % 2 === 0;
  const drift = interpolate(frame, [0, 75], [8, -4], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

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
          gap: 20,
          transform: `translateY(${drift}px)`,
        }}
      >
        <FadeIn delay={4} duration={18} direction="up" distance={12}>
          <h1
            style={{
              fontFamily: theme.fonts.sans,
              fontSize: 56,
              fontWeight: 600,
              color: theme.colors.text.primary,
              margin: 0,
              letterSpacing: "-0.04em",
              lineHeight: 1.15,
              textAlign: "center",
            }}
          >
            new machine. same agent setup.
            <span
              style={{
                display: "inline-block",
                width: 3,
                height: 42,
                marginLeft: 12,
                backgroundColor: theme.colors.text.primary,
                verticalAlign: "middle",
                opacity: blinkOn ? 1 : 0,
              }}
            />
          </h1>
        </FadeIn>

        <FadeIn delay={18} duration={18} direction="up" distance={10}>
          <p
            style={{
              fontFamily: theme.fonts.sans,
              fontSize: 22,
              fontWeight: 400,
              color: theme.colors.text.secondary,
              margin: 0,
              textAlign: "center",
              letterSpacing: "-0.01em",
            }}
          >
            move your codex / claude / opencode setup with Root
          </p>
        </FadeIn>
      </div>
    </AbsoluteFill>
  );
};
