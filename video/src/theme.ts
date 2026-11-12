import { loadFont } from "@remotion/google-fonts/Inter";

// Jot's own dark palette (crates/jot/src/theme.rs).
export const C = {
  bg: "#141820",
  deep: "#0B0E13",
  surface: "#1B212B",
  border: "#354151",
  text: "#E3E3E3",
  text2: "#C4C7C5",
  accent: "#A8C7FA",
  accentDeep: "#0842A0",
};

export const { fontFamily } = loadFont("normal", {
  weights: ["400", "500", "600", "700"],
  subsets: ["latin", "latin-ext"],
});

export const FPS = 30;
export const W = 1920;
export const H = 1080;
