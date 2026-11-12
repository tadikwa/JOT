import { Audio } from "@remotion/media";
import { AbsoluteFill, Img, interpolate, Sequence, spring, staticFile, useCurrentFrame, useVideoConfig } from "remotion";
import { Caption } from "./Caption";
import { float, Key, Shot } from "./Shot";
import { C, fontFamily } from "./theme";

/** The edit, in frames at 30 fps. One bar of the score is 60 frames and every section starts on a
 * bar line: 0 intro, 180 first launch, 540 history, 900 dictionary, 1260 local mode, 1680 ending. */
export const T = { onboarding: 180, history: 540, dictionary: 900, local: 1260, outro: 1680, end: 1860 };
const XF = 12; // half a crossfade

const clamp = { extrapolateLeft: "clamp", extrapolateRight: "clamp" } as const;
const ONB: [number, number] = [994, 757]; // first-launch window captures
const MAIN: [number, number] = [1276, 822]; // main window captures

/** A section on screen from `from` to `to`, crossfading XF frames either side. Inside it, frame n
 * of the section is local frame n + XF (see `s`). */
const Section: React.FC<{ from: number; to: number; children: React.ReactNode; first?: boolean }> = ({ from, to, children, first }) => (
  <Sequence from={from - XF} durationInFrames={to - from + 2 * XF}>
    <FadeIn skip={first}>{children}</FadeIn>
  </Sequence>
);

const FadeIn: React.FC<{ children: React.ReactNode; skip?: boolean }> = ({ children, skip }) => {
  const frame = useCurrentFrame();
  const o = skip ? 1 : interpolate(frame, [0, 2 * XF], [0, 1], clamp);
  return <AbsoluteFill style={{ opacity: o }}>{children}</AbsoluteFill>;
};

/** Section frame -> local frame. */
const s = (n: number) => n + XF;
/** A camera key that keeps the point (x, y) of the capture at `screenY` px from the top of the
 * video, above the caption. */
const look = (n: number, x: number, y: number, w: number, screenY = 430): Key => ({ f: s(n), x, y: y + ((540 - screenY) * w) / 1920, w });

const Backdrop: React.FC = () => {
  const frame = useCurrentFrame();
  const drift = Math.sin(frame / 90) * 6;
  return <AbsoluteFill style={{ background: `radial-gradient(ellipse 70% 60% at ${50 + drift}% 42%, #17223a 0%, ${C.bg} 52%, ${C.deep} 100%)` }} />;
};

const Logo: React.FC<{ size: number; delay?: number }> = ({ size, delay = 0 }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const p = spring({ frame: frame - delay, fps, config: { damping: 14, stiffness: 120 } });
  return (
    <Img
      src={staticFile("logo.png")}
      style={{ width: size, height: size, opacity: Math.min(1, p * 1.5), scale: String(interpolate(p, [0, 1], [0.8, 1])), filter: "drop-shadow(0 24px 60px rgba(8,66,160,0.55))" }}
    />
  );
};

const Intro: React.FC = () => {
  const frame = useCurrentFrame();
  const words = ["Appuyez.", "Parlez.", "C’est écrit."].map((text, i) => ({ text, at: 60 + 26 * i }));
  const name = interpolate(frame, [26, 44], [0, 1], clamp);
  const sub = interpolate(frame, [136, 152], [0, 1], clamp);
  const leave = interpolate(frame, [168, 192], [1, 0], clamp);
  return (
    <AbsoluteFill style={{ alignItems: "center", justifyContent: "center", fontFamily, opacity: leave, scale: String(interpolate(frame, [0, 192], [1, 1.04])) }}>
      <div style={{ display: "flex", flexDirection: "column", alignItems: "center", translate: `0 ${interpolate(frame, [36, 66], [50, 0], clamp)}px` }}>
        <Logo size={190} delay={4} />
        <div style={{ marginTop: 34, fontSize: 92, fontWeight: 700, letterSpacing: -2, color: C.text, opacity: name, translate: `0 ${(1 - name) * 20}px` }}>
          Jot <span style={{ color: C.accent }}>1.5</span>
        </div>
        <div style={{ marginTop: 22, display: "flex", gap: 22, fontSize: 46, fontWeight: 500, color: C.text2 }}>
          {words.map((w, i) => {
            const o = interpolate(frame, [w.at, w.at + 14], [0, 1], clamp);
            return (
              <span key={i} style={{ opacity: o, translate: `0 ${(1 - o) * 18}px`, color: i === 2 ? C.accent : undefined }}>
                {w.text}
              </span>
            );
          })}
        </div>
        <div style={{ marginTop: 26, fontSize: 30, color: C.text2, opacity: sub * 0.9 }}>Dictée vocale pour Windows · Gemini ou 100 % local</div>
      </div>
    </AbsoluteFill>
  );
};

const Outro: React.FC = () => {
  const frame = useCurrentFrame(); // 0 = XF frames before the ending's bar line
  const show = (f: number) => interpolate(frame, [f, f + 16], [0, 1], clamp);
  const leave = interpolate(frame, [176, 192], [1, 0], clamp);
  return (
    <AbsoluteFill style={{ alignItems: "center", justifyContent: "center", fontFamily, opacity: leave }}>
      <Logo size={160} delay={10} />
      <div style={{ marginTop: 30, fontSize: 96, fontWeight: 700, letterSpacing: -2.5, color: C.text, opacity: show(26), translate: `0 ${(1 - show(26)) * 20}px` }}>
        Jot <span style={{ color: C.accent }}>1.5</span>
      </div>
      <div style={{ marginTop: 16, fontSize: 40, fontWeight: 500, color: C.text2, opacity: show(44) }}>
        {["Gratuit", "Open source", "Windows"].join("  ·  ")}
      </div>
      <div
        style={{
          marginTop: 50,
          padding: "16px 34px",
          borderRadius: 999,
          border: "2px solid rgba(168,199,250,0.45)",
          background: "rgba(168,199,250,0.08)",
          color: C.accent,
          fontSize: 34,
          fontWeight: 500,
          opacity: show(62),
        }}
      >
        github.com/tadikwa/jot-gemini-transcribe-Windows
      </div>
      <div style={{ marginTop: 26, fontSize: 24, color: C.text2, opacity: show(80) * 0.8 }}>Apache 2.0 · binaire non signé (avertissement SmartScreen possible)</div>
    </AbsoluteFill>
  );
};

export const Presentation: React.FC = () => (
  <AbsoluteFill style={{ background: C.deep }}>
    <Backdrop />
    <Audio src={staticFile("audio/score.wav")} volume={0.72} />

    <Section from={0} to={T.onboarding} first>
      <Intro />
    </Section>

    {/* Real first-launch window: the engine step, then the new activation step (Toggle preselected). */}
    <Section from={T.onboarding} to={T.history}>
      <Shot
        size={ONB}
        frames={[
          { at: 0, src: "o1" },
          { at: s(170), src: "o2" },
        ]}
        keys={[float(ONB, s(-XF), 1.3), float(ONB, s(10), 1.3), look(70, 480, 300, 1250), look(160, 480, 300, 1250), look(200, 497, 235, 1150), look(360, 497, 235, 1120)]}
        rings={[
          { f: s(110), rect: [107, 257, 254, 35] },
          { f: s(220), rect: [107, 194, 386, 69] },
        ]}
      />
      <Caption
        eyebrow="Premier lancement"
        parts={[
          { text: "Gemini ou 100 % local.", at: s(40) },
          { text: "Puis appuyer, ou maintenir pour parler.", at: s(190) },
        ]}
        note="Captures réelles de Jot 1.5.0, sur un profil de test vide."
        end={s(360)}
      />
    </Section>

    {/* A fictional dictation, said by a synthetic voice and processed by the real local engine. */}
    <Section from={T.history} to={T.dictionary}>
      <Shot
        size={MAIN}
        frames={[
          { at: 0, src: "h1" },
          { at: s(110), src: "h2" },
          { at: s(250), src: "h3" },
        ]}
        keys={[float(MAIN, s(-XF), 1.15), look(40, 460, 330, 1300), look(100, 460, 330, 1300), look(150, 995, 470, 1250), look(360, 995, 470, 1200)]}
        rings={[
          { f: s(96), rect: [214, 337, 68, 34] },
          { f: s(240), rect: [870, 468, 54, 34] },
        ]}
      />
      <Caption
        eyebrow="Historique"
        parts={[
          { text: "Vous énumérez,", at: s(30) },
          { text: "Jot fait la liste.", at: s(60) },
          { text: "Le brut reste là.", at: s(250) },
        ]}
        note="Dictées fictives lues par une voix de synthèse, traitées par le moteur local."
        end={s(360)}
      />
    </Section>

    {/* A real correction in the History editor, then the suggestion it produced. */}
    <Section from={T.dictionary} to={T.local}>
      <Shot
        size={MAIN}
        frames={[
          { at: 0, src: "d0" },
          { at: s(130), src: "d1" },
          { at: s(240), src: "d2" },
        ]}
        keys={[look(-XF, 995, 560, 1250), look(110, 995, 560, 1250), look(150, 730, 470, 1250), look(360, 730, 480, 1200)]}
        rings={[
          { f: s(30), rect: [983, 515, 30, 24] },
          { f: s(200), rect: [353, 497, 87, 34] },
        ]}
      />
      <Caption
        eyebrow="Dictionnaire intelligent"
        parts={[
          { text: "Corrigez une fois,", at: s(30) },
          { text: "Jot propose la règle.", at: s(140) },
        ]}
        note="Calculé sur votre PC. Rien n’est activé sans votre accord."
        end={s(360)}
      />
    </Section>

    <Section from={T.local} to={T.outro}>
      <Shot
        size={ONB}
        frames={[{ at: 0, src: "o3" }]}
        keys={[look(-XF, 497, 270, 1150), look(200, 497, 285, 1080)]}
        rings={[{ f: s(70), rect: [107, 420, 158, 33] }]}
      />
      <Sequence from={s(200)}>
        <FadeIn>
          <AbsoluteFill style={{ background: C.deep }}>
            <Backdrop />
            <Shot size={MAIN} frames={[{ at: 0, src: "s1" }]} keys={[look(0, 730, 400, 1350), look(220, 730, 420, 1250)]} rings={[{ f: 40, rect: [509, 301, 99, 35] }]} />
          </AbsoluteFill>
        </FadeIn>
      </Sequence>
      <Caption
        eyebrow="100 % local"
        parts={[
          { text: "Transcription et reformulation", at: s(30) },
          { text: "sur votre PC.", at: s(60) },
        ]}
        note="Téléchargement unique de 5,30 Go, vérifié par SHA-256. Jamais de bascule silencieuse vers Gemini."
        end={s(420)}
      />
    </Section>

    <Section from={T.outro} to={T.end}>
      <Outro />
    </Section>
  </AbsoluteFill>
);

/** The release page's poster: the name on the left, a real capture on the right. */
export const Poster: React.FC = () => (
  <AbsoluteFill style={{ background: C.deep, fontFamily }}>
    <Backdrop />
    <div style={{ position: "absolute", left: 130, top: 300, width: 640 }}>
      <Img src={staticFile("logo.png")} style={{ width: 132, height: 132 }} />
      <div style={{ marginTop: 26, fontSize: 104, fontWeight: 700, letterSpacing: -3, color: C.text }}>
        Jot <span style={{ color: C.accent }}>1.5</span>
      </div>
      <div style={{ marginTop: 12, fontSize: 40, fontWeight: 500, color: C.text2, lineHeight: 1.3 }}>Dictée vocale pour Windows</div>
      <div style={{ marginTop: 10, fontSize: 32, color: C.accent }}>Gemini ou 100 % local</div>
    </div>
    <Img
      src={staticFile("stills/h2.png")}
      style={{
        position: "absolute",
        left: 830,
        top: 170,
        width: 1020,
        borderRadius: 12,
        boxShadow: "0 0 0 2px rgba(168,199,250,0.22), 0 40px 140px rgba(0,0,0,0.7)",
        rotate: "0deg",
      }}
    />
    <div style={{ position: "absolute", right: 70, bottom: 40, fontSize: 20, color: C.text2, opacity: 0.7, fontFamily }}>Capture réelle · données fictives</div>
  </AbsoluteFill>
);

