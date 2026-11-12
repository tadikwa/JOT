import { AbsoluteFill, Easing, interpolate, useCurrentFrame } from "remotion";
import { C, fontFamily } from "./theme";

const clamp = { extrapolateLeft: "clamp", extrapolateRight: "clamp" } as const;

/** The section's line of text over a soft scrim; each part arrives at its own frame. `note` is a
 * small honest label under it (what is shown, and how it was made). */
export const Caption: React.FC<{
  eyebrow: string;
  parts: { text: string; at: number }[];
  end: number;
  note?: string;
  place?: "top" | "bottom";
}> = ({ eyebrow, parts, end, note, place = "bottom" }) => {
  const frame = useCurrentFrame();
  const start = parts[0].at;
  const fade = (from: number) => interpolate(frame, [from, from + 14], [0, 1], { ...clamp, easing: Easing.out(Easing.cubic) });
  const out = interpolate(frame, [end - 12, end], [1, 0], clamp);
  const top = place === "top";
  return (
    <AbsoluteFill style={{ pointerEvents: "none" }}>
      <AbsoluteFill
        style={{
          opacity: fade(start - 6) * out,
          background: `linear-gradient(${top ? "180deg" : "0deg"}, rgba(11,14,19,0.97) 0%, rgba(11,14,19,0.9) 26%, rgba(11,14,19,0) 52%)`,
        }}
      />
      <div style={{ position: "absolute", left: 120, right: 120, [top ? "top" : "bottom"]: 84, fontFamily, opacity: out }}>
        <div
          style={{
            color: C.accent,
            fontSize: 24,
            fontWeight: 600,
            letterSpacing: 4,
            textTransform: "uppercase",
            marginBottom: 14,
            opacity: fade(start - 4),
            display: "flex",
            alignItems: "center",
            gap: 16,
          }}
        >
          <div style={{ width: 36, height: 3, borderRadius: 2, background: C.accent }} />
          {eyebrow}
        </div>
        <div style={{ color: C.text, fontSize: 60, fontWeight: 600, letterSpacing: -1, lineHeight: 1.15 }}>
          {parts.map((p, i) => {
            const o = fade(p.at);
            return (
              <span key={i} style={{ display: "inline-block", marginRight: "0.3em", opacity: o, translate: `0 ${(1 - o) * 24}px` }}>
                {p.text}
              </span>
            );
          })}
        </div>
        {note ? <div style={{ marginTop: 16, color: C.text2, fontSize: 24, fontWeight: 400, opacity: fade(start + 10) * 0.9 }}>{note}</div> : null}
      </div>
    </AbsoluteFill>
  );
};
