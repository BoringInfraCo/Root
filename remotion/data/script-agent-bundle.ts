export interface AgentBundleSceneConfig {
  id: string;
  durationInFrames: number;
}

export const AB_FPS = 30;
export const AB_WIDTH = 1920;
export const AB_HEIGHT = 1080;

export const AB_SCENES: AgentBundleSceneConfig[] = [
  { id: "Intro", durationInFrames: 75 },
  { id: "Export", durationInFrames: 150 },
  { id: "Transfer", durationInFrames: 60 },
  { id: "Apply", durationInFrames: 165 },
  { id: "Verify", durationInFrames: 75 },
  { id: "Close", durationInFrames: 75 },
];

export const AB_TOTAL_DURATION = AB_SCENES.reduce(
  (sum, scene) => sum + scene.durationInFrames,
  0
);
