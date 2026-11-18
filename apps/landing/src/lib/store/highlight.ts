// Code as HTML with the highlighter's classes (no inline style: the CSP
// allows none), every character escaped by the serializer.
import { toHtml } from "hast-util-to-html";
import { common, createLowlight } from "lowlight";

const lowlight = createLowlight(common);

/** One file's text, highlighted when its language is known. */
export function highlight(text: string, language: string | null): string {
  if (!language || !lowlight.registered(language) || text.length > 200_000) return toHtml({ type: "text", value: text });
  return toHtml(lowlight.highlight(language, text));
}
