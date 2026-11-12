import { Composition, Still } from "remotion";
import { Poster, Presentation, T } from "./Presentation";
import { FPS, H, W } from "./theme";

export const RemotionRoot: React.FC = () => (
  <>
    <Composition id="Presentation" component={Presentation} durationInFrames={T.end} fps={FPS} width={W} height={H} />
    <Still id="Poster" component={Poster} width={W} height={H} />
  </>
);
