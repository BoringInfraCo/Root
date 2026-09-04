import React from "react";
import { Composition } from "remotion";
import { RootLaunch } from "./Composition";
import { RootLaunchShort, SHORT_TOTAL_DURATION } from "./CompositionShort";
import { TOTAL_DURATION, FPS, WIDTH, HEIGHT } from "./data/script";
import { RootLaunchV2 } from "./CompositionV2";
import { RootLaunchV2Short } from "./CompositionV2Short";
import { V2_TOTAL_DURATION, V2_SHORT_TOTAL_DURATION } from "./data/script-v2";
import { RootAgentBundle } from "./CompositionAgentBundle";
import {
  AB_TOTAL_DURATION,
  AB_FPS,
  AB_WIDTH,
  AB_HEIGHT,
} from "./data/script-agent-bundle";

export const RemotionRoot: React.FC = () => {
  return (
    <>
      <Composition
        id="RootLaunch"
        component={RootLaunch}
        durationInFrames={TOTAL_DURATION}
        fps={FPS}
        width={WIDTH}
        height={HEIGHT}
      />
      <Composition
        id="RootLaunchShort"
        component={RootLaunchShort}
        durationInFrames={SHORT_TOTAL_DURATION}
        fps={FPS}
        width={WIDTH}
        height={HEIGHT}
      />
      <Composition
        id="RootLaunchV2"
        component={RootLaunchV2}
        durationInFrames={V2_TOTAL_DURATION}
        fps={FPS}
        width={WIDTH}
        height={HEIGHT}
      />
      <Composition
        id="RootLaunchV2Short"
        component={RootLaunchV2Short}
        durationInFrames={V2_SHORT_TOTAL_DURATION}
        fps={FPS}
        width={WIDTH}
        height={HEIGHT}
      />
      <Composition
        id="RootAgentBundle"
        component={RootAgentBundle}
        durationInFrames={AB_TOTAL_DURATION}
        fps={AB_FPS}
        width={AB_WIDTH}
        height={AB_HEIGHT}
      />
    </>
  );
};
