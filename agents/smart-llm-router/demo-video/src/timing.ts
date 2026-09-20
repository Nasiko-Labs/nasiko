/** 90s @ 30fps */
export const FPS = 30;
export const TOTAL_FRAMES = 90 * FPS; // 2700
export const WIDTH = 1920;
export const HEIGHT = 1080;

export const scene = {
  coldOpen: { from: 0, dur: 4 * FPS },
  problem: { from: 4 * FPS, dur: 10 * FPS },
  partners: { from: 14 * FPS, dur: 12 * FPS },
  architecture: { from: 26 * FPS, dur: 14 * FPS },
  nasikoConsole: { from: 40 * FPS, dur: 14 * FPS },
  analytics: { from: 54 * FPS, dur: 16 * FPS },
  contract: { from: 70 * FPS, dur: 12 * FPS },
  outro: { from: 82 * FPS, dur: 8 * FPS },
} as const;
