import React from "react";
import {
  AbsoluteFill,
  interpolate,
  spring,
  useCurrentFrame,
  useVideoConfig,
} from "remotion";
import { theme } from "../../styles/theme";
import { FadeIn } from "../../components/FadeIn";

export const TransferScene: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  const slide = spring({
    frame: frame - 6,
    fps,
    config: { damping: 16, stiffness: 70, mass: 0.8 },
  });

  const cardLeft = interpolate(slide, [0, 1], [40, 700], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  const cardOpacity = interpolate(slide, [0, 0.12, 1], [0, 1, 1], {
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
          gap: 40,
          width: "100%",
          maxWidth: 1080,
        }}
      >
        <FadeIn delay={0} duration={16} direction="up" distance={10}>
          <p
            style={{
              fontFamily: theme.fonts.sans,
              fontSize: 28,
              fontWeight: 500,
              color: theme.colors.text.primary,
              margin: 0,
              letterSpacing: "-0.02em",
            }}
          >
            move bundle to new machine
          </p>
        </FadeIn>

        <div style={{ width: "100%" }}>
          <div
            style={{
              display: "flex",
              justifyContent: "space-between",
              marginBottom: 18,
            }}
          >
            <FadeIn delay={4} duration={16} direction="none">
              <span
                style={{
                  fontFamily: theme.fonts.mono,
                  fontSize: 14,
                  color: theme.colors.text.tertiary,
                }}
              >
                old machine
              </span>
            </FadeIn>
            <FadeIn delay={16} duration={16} direction="none">
              <span
                style={{
                  fontFamily: theme.fonts.mono,
                  fontSize: 14,
                  color: theme.colors.text.secondary,
                }}
              >
                new machine
              </span>
            </FadeIn>
          </div>

          <div
            style={{
              position: "relative",
              height: 88,
              borderRadius: theme.radius.lg,
              backgroundColor: theme.colors.bg.secondary,
              border: `1px solid ${theme.colors.border.subtle}`,
            }}
          >
            <div
              style={{
                position: "absolute",
                top: 16,
                left: cardLeft,
                opacity: cardOpacity,
              }}
            >
              <div
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 12,
                  padding: "14px 20px",
                  backgroundColor: theme.colors.bg.elevated,
                  border: `1px solid ${theme.colors.border.medium}`,
                  borderRadius: theme.radius.md,
                  boxShadow: "0 12px 32px rgba(0,0,0,0.4)",
                }}
              >
                <span
                  style={{
                    width: 10,
                    height: 10,
                    borderRadius: 2,
                    backgroundColor: theme.colors.text.tertiary,
                  }}
                />
                <span
                  style={{
                    fontFamily: theme.fonts.mono,
                    fontSize: 18,
                    color: theme.colors.text.primary,
                  }}
                >
                  ./root-agent-bundle
                </span>
              </div>
            </div>
          </div>
        </div>
      </div>
    </AbsoluteFill>
  );
};
