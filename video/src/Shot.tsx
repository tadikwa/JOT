import { AbsoluteFill, Easing, Img, interpolate, staticFile, useCurrentFrame } from "remotion";
import { C, H, W } from "./theme";

/** Every still is a real capture of a Jot window (capture/README.md) at its own size [sw, sh] px.
 * The camera looks at a point (x, y) of it with `w` source pixels across the video's width: more
 * than sw shows the window floating. Keep w >= about 900, so a capture is never enlarged more than
 * about 2x. */
export type Key = { f: number; x: number; y: number; w: number };
export type Ring = { f: number; rect: [number, number, number, number] };
export type Frame = { at: number; src: string };

/** The whole window, centred, with room around it. */
export const float = ([sw, sh]: [number, number], f: number, room = 1.5): Key => ({ f, x: sw / 2, y: sh / 2, w: Math.max(sw * room, ((sh * room) * W) / H) });

const ease = Easing.bezier(0.65, 0, 0.35, 1);
const clamp = { extrapolateLeft: "clamp", extrapolateRight: "clamp" } as const;

function camera(keys: Key[], frame: number): Key {
  if (frame <= keys[0].f) return { ...keys[0] };
  for (let i = 0; i < keys.length - 1; i++) {
    const a = keys[i];
    const b = keys[i + 1];
    if (frame <= b.f) {
      const t = ease((frame - a.f) / (b.f - a.f));
      // Zoom moves in log space, so a push-in feels even all the way.
      const w = Math.exp(Math.log(a.w) + (Math.log(b.w) - Math.log(a.w)) * t);
      return { f: frame, x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t, w };
    }
  }
  return { ...keys[keys.length - 1] };
}

/** A window capture, or a series of them (one per click), under a moving camera. */
export const Shot: React.FC<{ size: [number, number]; frames: Frame[]; keys: Key[]; rings?: Ring[] }> = ({ size: [SRC_W, SRC_H], frames, keys, rings = [] }) => {
  const frame = useCurrentFrame();
  const cam = camera(keys, frame);
  const s = W / cam.w;
  return (
    <AbsoluteFill style={{ overflow: "hidden" }}>
      <div
        style={{
          position: "absolute",
          width: SRC_W,
          height: SRC_H,
          transformOrigin: "0 0",
          transform: `translate(${W / 2 - cam.x * s}px, ${H / 2 - cam.y * s}px) scale(${s})`,
          borderRadius: 14,
          overflow: "hidden",
          background: C.bg,
          boxShadow: `0 0 0 2px rgba(168,199,250,0.22), 0 40px 140px rgba(0,0,0,0.7), 0 0 200px rgba(168,199,250,0.12)`,
        }}
      >
        {frames.map((f, i) => {
          const next = frames[i + 1]?.at ?? 1e9; // interpolate() rejects Infinity
          // A short dissolve between two captures, like the window redrawing after a click.
          const o = i === 0 ? interpolate(frame, [next, next + 4], [1, 0], clamp) : interpolate(frame, [f.at, f.at + 4, next, next + 4], [0, 1, 1, 0], clamp);
          if (o === 0) return null;
          return <Img key={f.src} src={staticFile(`stills/${f.src}.png`)} style={{ position: "absolute", width: SRC_W, height: SRC_H, opacity: o }} />;
        })}
        {rings.map((r, i) => {
          const t = frame - r.f;
          if (t < -2 || t > 30) return null;
          const o = interpolate(t, [-2, 3, 18, 30], [0, 1, 0.8, 0], clamp);
          const grow = interpolate(t, [-2, 8], [1.25, 1], { extrapolateRight: "clamp", easing: Easing.out(Easing.cubic) });
          const [x, y, w, h] = r.rect;
          const pad = 6;
          return (
            <div
              key={i}
              style={{
                position: "absolute",
                left: x - pad,
                top: y - pad,
                width: w + 2 * pad,
                height: h + 2 * pad,
                borderRadius: 14,
                border: `3px solid ${C.accent}`,
                boxShadow: `0 0 28px rgba(168,199,250,0.6)`,
                opacity: o,
                scale: String(grow),
              }}
            />
          );
        })}
      </div>
    </AbsoluteFill>
  );
};
