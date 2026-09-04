import React from "react";
import { Sequence } from "remotion";
import { AB_SCENES } from "./data/script-agent-bundle";
import { IntroScene } from "./scenes/agent-bundle/IntroScene";
import { ExportScene } from "./scenes/agent-bundle/ExportScene";
import { TransferScene } from "./scenes/agent-bundle/TransferScene";
import { ApplyScene } from "./scenes/agent-bundle/ApplyScene";
import { VerifyScene } from "./scenes/agent-bundle/VerifyScene";
import { CloseScene } from "./scenes/agent-bundle/CloseScene";

const SCENES = [
  IntroScene,
  ExportScene,
  TransferScene,
  ApplyScene,
  VerifyScene,
  CloseScene,
];

export const RootAgentBundle: React.FC = () => {
  const getFrom = (sceneIndex: number) => {
    let from = 0;
    for (let i = 0; i < sceneIndex; i++) {
      from += AB_SCENES[i].durationInFrames;
    }
    return from;
  };

  return (
    <>
      {SCENES.map((SceneComponent, i) => (
        <Sequence
          key={AB_SCENES[i].id}
          from={getFrom(i)}
          durationInFrames={AB_SCENES[i].durationInFrames}
        >
          <SceneComponent />
        </Sequence>
      ))}
    </>
  );
};
