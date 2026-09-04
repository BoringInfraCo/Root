import React from "react";
import { theme } from "../../styles/theme";
import { FadeIn } from "../../components/FadeIn";

interface SceneCaptionProps {
  text: string;
  delay?: number;
  size?: "sm" | "md";
}

export const SceneCaption: React.FC<SceneCaptionProps> = ({
  text,
  delay = 0,
  size = "md",
}) => (
  <FadeIn delay={delay} duration={16} direction="up" distance={10}>
    <p
      style={{
        fontFamily: theme.fonts.sans,
        fontSize: size === "sm" ? 16 : 20,
        fontWeight: 500,
        color:
          size === "sm" ? theme.colors.text.secondary : "#d4d4d4",
        margin: 0,
        letterSpacing: "-0.01em",
        textAlign: "center",
      }}
    >
      {text}
    </p>
  </FadeIn>
);

interface MachineTagProps {
  label: string;
  delay?: number;
}

export const MachineTag: React.FC<MachineTagProps> = ({ label, delay = 0 }) => (
  <FadeIn delay={delay} duration={16} direction="none">
    <div
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 8,
        padding: "6px 12px",
        borderRadius: 99,
        backgroundColor: theme.colors.bg.tertiary,
        border: `1px solid ${theme.colors.border.subtle}`,
        fontFamily: theme.fonts.mono,
        fontSize: 13,
        color: theme.colors.text.secondary,
        letterSpacing: "0.02em",
      }}
    >
      <span
        style={{
          width: 6,
          height: 6,
          borderRadius: "50%",
          backgroundColor: theme.colors.accent.green,
        }}
      />
      {label}
    </div>
  </FadeIn>
);
