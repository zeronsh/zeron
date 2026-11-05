//! The colours a terminal draws with when its program names none.
use alacritty_terminal::vte::ansi::Rgb;

/// A terminal's default colours: the sixteen ANSI colours, and the
/// foreground, background and cursor. The 256-colour cube and grey ramp
/// come from xterm's formula.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    /// Black, red, green, yellow, blue, magenta, cyan, white, then their
    /// bright forms.
    pub ansi: [Rgb; 16],
    /// Text.
    pub foreground: Rgb,
    /// Behind text.
    pub background: Rgb,
    /// The cursor.
    pub cursor: Rgb,
}
const fn rgb(value: u32) -> Rgb {
    Rgb {
        r: (value >> 16) as u8,
        g: (value >> 8) as u8,
        b: value as u8,
    }
}
impl Default for Palette {
    /// Dark: light text on near black.
    fn default() -> Self {
        Self {
            ansi: [
                rgb(0x1d1f21),
                rgb(0xcc6666),
                rgb(0xb5bd68),
                rgb(0xf0c674),
                rgb(0x81a2be),
                rgb(0xb294bb),
                rgb(0x8abeb7),
                rgb(0xc5c8c6),
                rgb(0x666666),
                rgb(0xd54e53),
                rgb(0xb9ca4a),
                rgb(0xe7c547),
                rgb(0x7aa6da),
                rgb(0xc397d8),
                rgb(0x70c0b1),
                rgb(0xeaeaea),
            ],
            foreground: rgb(0xdcdfe4),
            background: rgb(0x16181c),
            cursor: rgb(0xdcdfe4),
        }
    }
}
impl Palette {
    /// The colour at `index` as programs number them: 0-15 ANSI, 16-231
    /// the cube, 232-255 greys, then alacritty's named colours
    /// (foreground 256, background 257, cursor 258, dim 259-266, bright
    /// foreground 267, dim foreground 268).
    pub fn color(&self, index: usize) -> Rgb {
        match index {
            0..16 => self.ansi[index],
            16..232 => {
                let index = index - 16;
                let level = |v: usize| if v == 0 { 0 } else { (v * 40 + 55) as u8 };
                Rgb {
                    r: level(index / 36),
                    g: level(index / 6 % 6),
                    b: level(index % 6),
                }
            }
            232..256 => {
                let grey = ((index - 232) * 10 + 8) as u8;
                Rgb {
                    r: grey,
                    g: grey,
                    b: grey,
                }
            }
            256 | 267 => self.foreground,
            257 => self.background,
            258 => self.cursor,
            259..267 => dim(self.ansi[index - 259]),
            268 => dim(self.foreground),
            _ => self.foreground,
        }
    }
}
/// A colour at two thirds of its brightness (SGR 2, faint).
pub fn dim(color: Rgb) -> Rgb {
    Rgb {
        r: (u16::from(color.r) * 2 / 3) as u8,
        g: (u16::from(color.g) * 2 / 3) as u8,
        b: (u16::from(color.b) * 2 / 3) as u8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cube_and_ramp_are_xterms() {
        let palette = Palette::default();
        assert_eq!(palette.color(16), rgb(0x000000));
        assert_eq!(palette.color(196), rgb(0xff0000));
        assert_eq!(palette.color(231), rgb(0xffffff));
        assert_eq!(palette.color(232), rgb(0x080808));
        assert_eq!(palette.color(255), rgb(0xeeeeee));
        assert_eq!(palette.color(257), palette.background);
    }
}
