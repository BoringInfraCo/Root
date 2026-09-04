import React from "react";
import { interpolate, spring, useCurrentFrame, useVideoConfig } from "remotion";

export type TermPart = {
  text: string;
  color?: string;
};

export type TermLine = {
  delay: number; // frame when this line starts
  type: "command" | "output" | "success" | "blank";
  text: string;
  parts?: TermPart[]; // optional colored segments for output/success
};

export interface GhosttyTerminalProps {
  lines: TermLine[];
  width?: number; // default 1080
  height?: number; // default 560
  title?: string; // default "root-demo"
  user?: string;
  host?: string;
  typingSpeed?: number; // frames per character for commands, default 1
  showCursor?: boolean; // default true
  fontSize?: number; // default 22
}

const TRAFFIC_LIGHTS = ["#ff5f57", "#febc2e", "#28c840"] as const;

const COLOR = {
  user: "#d97706",
  host: "#60a5fa",
  muted: "#737373",
  command: "#f5f5f5",
  output: "#a3a3a3",
  success: "#e5e5e5",
  check: "#4ade80",
  cursor: "#fafafa",
} as const;

const FONT_MONO = "'JetBrains Mono', 'SF Mono', 'Fira Code', monospace";
const FONT_TITLE = "'JetBrains Mono', 'SF Mono', monospace";

const Prompt: React.FC<{ user: string; host: string }> = ({ user, host }) => (
  <>
    <span style={{ color: COLOR.user }}>{user}</span>
    <span style={{ color: COLOR.muted }}>@</span>
    <span style={{ color: COLOR.host }}>{host}</span>
    <span style={{ color: COLOR.muted }}> ~ % </span>
  </>
);

const CursorBlock: React.FC<{ fontSize: number }> = ({ fontSize }) => (
  <span
    style={{
      display: "inline-block",
      width: 10,
      height: fontSize * 0.85,
      backgroundColor: COLOR.cursor,
      marginLeft: 2,
      verticalAlign: "middle",
    }}
  />
);

const renderParts = (parts: TermPart[], fallback: string) =>
  parts.map((part, i) => (
    <span key={i} style={{ color: part.color ?? fallback }}>
      {part.text}
    </span>
  ));

const commandDoneFrame = (line: TermLine, typingSpeed: number): number =>
  line.delay + line.text.length * typingSpeed;

const lineDoneFrame = (line: TermLine, typingSpeed: number): number =>
  line.type === "command" ? commandDoneFrame(line, typingSpeed) : line.delay;

export const GhosttyTerminal: React.FC<GhosttyTerminalProps> = ({
  lines,
  width = 1080,
  height = 560,
  title = "root-demo",
  user = "sergio",
  host = "mbp-old",
  typingSpeed = 1,
  showCursor = true,
  fontSize = 22,
}) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  const progress = spring({
    frame,
    fps,
    config: { damping: 20, stiffness: 90, mass: 0.5 },
  });

  const opacity = interpolate(progress, [0, 1], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  const scale = interpolate(progress, [0, 1], [0.97, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

  const blinkOn = Math.floor(frame / 10) % 2 === 0;
  const lineHeightPx = fontSize * 1.65;

  const cursorLineIndex = ((): number | null => {
    for (let i = 0; i < lines.length; i++) {
      const line = lines[i];
      if (line.type !== "command" || frame < line.delay) {
        continue;
      }
      const typingDone = frame >= commandDoneFrame(line, typingSpeed);
      if (!typingDone) {
        return i;
      }
      const nextVisible = lines
        .slice(i + 1)
        .find((candidate) => candidate.type !== "blank");
      if (nextVisible && frame < nextVisible.delay) {
        return i;
      }
    }
    return null;
  })();

  const last = lines.at(-1);
  const allLinesDone =
    last === undefined || frame >= lineDoneFrame(last, typingSpeed);
  const showTrailingPrompt = allLinesDone && cursorLineIndex === null;

  const typedCommandText = (line: TermLine): string => {
    const elapsed = frame - line.delay;
    const charCount = Math.floor(elapsed / typingSpeed);
    return line.text.slice(0, Math.max(0, charCount));
  };

  const renderSuccess = (line: TermLine) => {
    if (line.parts) {
      return renderParts(line.parts, COLOR.success);
    }
    if (line.text.startsWith("✓")) {
      return (
        <>
          <span style={{ color: COLOR.check }}>✓</span>
          <span style={{ color: COLOR.success }}>{line.text.slice(1)}</span>
        </>
      );
    }
    return <span style={{ color: COLOR.success }}>{line.text}</span>;
  };

  const renderLineBody = (line: TermLine, index: number) => {
    const cursorHere =
      showCursor && blinkOn && cursorLineIndex === index ? (
        <CursorBlock fontSize={fontSize} />
      ) : null;

    switch (line.type) {
      case "command":
        return (
          <>
            <Prompt user={user} host={host} />
            <span style={{ color: COLOR.command }}>{typedCommandText(line)}</span>
            {cursorHere}
          </>
        );
      case "output":
        return line.parts ? (
          renderParts(line.parts, COLOR.output)
        ) : (
          <span style={{ color: COLOR.output }}>{line.text}</span>
        );
      case "success":
        return renderSuccess(line);
      case "blank":
        return null;
    }
  };

  return (
    <div
      style={{
        width,
        height,
        opacity,
        transform: `scale(${scale})`,
        transformOrigin: "center center",
        borderRadius: 14,
        boxShadow: "0 28px 80px rgba(0,0,0,0.55)",
      }}
    >
      <div
        style={{
          width: "100%",
          height: "100%",
          backgroundColor: "#0c0c0c",
          borderRadius: 14,
          border: "1px solid #2a2a2a",
          overflow: "hidden",
          display: "flex",
          flexDirection: "column",
          boxSizing: "border-box",
        }}
      >
        <div
          style={{
            height: 44,
            backgroundColor: "#161616",
            borderBottom: "1px solid #2a2a2a",
            position: "relative",
            flexShrink: 0,
          }}
        >
          <div
            style={{
              position: "absolute",
              left: 16,
              top: 0,
              bottom: 0,
              display: "flex",
              alignItems: "center",
              gap: 8,
            }}
          >
            {TRAFFIC_LIGHTS.map((color) => (
              <div
                key={color}
                style={{
                  width: 11,
                  height: 11,
                  borderRadius: "50%",
                  backgroundColor: color,
                }}
              />
            ))}
          </div>
          <div
            style={{
              position: "absolute",
              inset: 0,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontFamily: FONT_TITLE,
              fontSize: 13,
              color: COLOR.muted,
              pointerEvents: "none",
            }}
          >
            {title}
          </div>
        </div>

        <div
          style={{
            flex: 1,
            padding: 28,
            fontFamily: FONT_MONO,
            fontSize,
            lineHeight: 1.65,
            overflow: "hidden",
            whiteSpace: "pre-wrap",
          }}
        >
          {lines.map((line, i) => {
            if (frame < line.delay) {
              return null;
            }
            return (
              <div key={i} style={{ minHeight: lineHeightPx }}>
                {renderLineBody(line, i)}
              </div>
            );
          })}
          {showTrailingPrompt ? (
            <div style={{ minHeight: lineHeightPx }}>
              <Prompt user={user} host={host} />
              {showCursor && blinkOn ? <CursorBlock fontSize={fontSize} /> : null}
            </div>
          ) : null}
        </div>
      </div>
    </div>
  );
};
