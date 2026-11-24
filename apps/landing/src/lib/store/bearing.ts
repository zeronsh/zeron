// Large type lines up by its ink, not its box (Wing's rule): a glyph's ink starts its left side bearing
// right of where its box does, 4 px for a 52 px "P". Each heading is drawn that much left, by its first
// letter: Geist at 600, in 64ths of an em (measured in the browser at 1000 px, tests/align), so the shift is
// right at any size. A letter not here starts on its box.

const BEARING: Record<string, number> = {"0": 2, "1": 2, "2": 3, "3": 2, "4": 1, "5": 3, "6": 2, "7": 1, "8": 2, "9": 2, "A": 1, "B": 5, "C": 2, "D": 5, "E": 5, "F": 5, "G": 2, "H": 5, "I": 5, "J": 3, "K": 5, "L": 5, "M": 5, "N": 5, "O": 2, "P": 5, "Q": 2, "R": 5, "S": 3, "U": 4, "V": 1, "W": 2, "Y": -1, "Z": 1, "a": 2, "b": 4, "c": 2, "d": 2, "e": 2, "f": 3, "g": 2, "h": 4, "i": 4, "j": -1, "k": 4, "l": 4, "m": 4, "n": 4, "o": 2, "p": 4, "q": 2, "r": 4, "s": 2, "t": 2, "u": 4, "v": 1, "w": 1, "x": 2, "y": 1, "z": 3};

/** The class that draws a heading its first letter's side bearing left (styles/look/store.css .lsb-*). */
export function lsb(text: string | null | undefined): string | undefined {
  const first = (text ?? "").trimStart().charAt(0);
  const n = BEARING[first] ?? 0;
  return n === 0 ? undefined : `lsb-${n < 0 ? `n${-n}` : n}`;
}
