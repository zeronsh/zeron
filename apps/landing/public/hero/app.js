// The hero: the Zeron app's new chat, whole, over the Blossom scene
// (scene.js; the chrome in chrome.js).
//
// mountHero(container, { still, pose, dpr, lights }) → controller, or null
// when WebGL2 isn't there (the page keeps its still picture then). `still`
// (or Reduce motion): the scene holds still and nothing follows the pointer.

import { mountScene } from "./scene.js";
import { WINDOW, CARD_RADIUS, PAGE, NODE, drawUnder, drawOver, iconsReady } from "./chrome.js";

export const HERO = {
  label: "The Zeron app: a new chat over a cherry blossom scene drawn in characters",
  window: WINDOW,
  crop: [0, 0, WINDOW.w, WINDOW.h],
  scene: { slug: "blossom", layers: ["sky", "far", "near", "detail"] },
  node: NODE,
  page: PAGE,
  radius: CARD_RADIUS - 1,
  pointer: "page",
  under: drawUnder,
  over: drawOver,
  ready: iconsReady,
};

export function mountHero(container, options = {}) {
  return mountScene(container, HERO, { label: container.getAttribute("data-label") || undefined, ...options });
}
