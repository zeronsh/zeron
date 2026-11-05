//! LaTeX math for the scripts (`ds/math`): zatex, the shipping app's layout
//! (crates/ui/src/markdown/math.rs), over its bundled math faces.
//!
//! `ui.call_sync("math.layout", { tex, display?, face? })` answers at once:
//!
//! ```text
//! { w, ascent, descent, left, path = { op, …, op, … } }   in ems
//! { error, at }                                           it can't be set
//! ```
//!
//! zatex's layout is linear in the size, so a formula is laid out once at
//! 1 em and the script scales it to its text. `path` is every glyph outline
//! and rule as one path, the origin `left` in from the box's left edge on
//! the baseline, `ascent` down from its top: opcodes 0 move (x y), 1 line
//! (x y), 2 quad (x1 y1 x y), 3 cubic (x1 y1 x2 y2 x y), 4 close, each
//! followed by its numbers. Filled with the non-zero rule. Answers are kept
//! by source, face and style, bounded by count and size; laying a formula
//! out takes microseconds, so it runs here, on the UI thread.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};

use zatex::bundled::Face;
use zatex::{Options, PathSink};
use zgx_script::{ScriptHost, Value};

/// Formulas kept: every one on screen plus recent scrollback.
const KEEP: usize = 2048;
/// And the numbers they may hold in all (a long formula is thousands).
const KEEP_NUMBERS: usize = 2_000_000;

/// A face by the name the settings keep ("fira", "stix", …): Fira Math,
/// the shipping app's default, for any other.
pub fn face(name: &str) -> Face {
    match name {
        "stix" => Face::StixTwo,
        "computer-modern" => Face::ComputerModern,
        "libertinus" => Face::Libertinus,
        "pagella" => Face::Pagella,
        "euler" => Face::Euler,
        _ => Face::Fira,
    }
}

/// A formula laid out at 1 em, as the script gets it.
#[derive(Clone, Debug, PartialEq)]
pub enum Laid {
    Formula { w: f32, ascent: f32, descent: f32, left: f32, path: Vec<f32> },
    Error { message: String, at: usize },
}

impl Laid {
    fn numbers(&self) -> usize {
        match self {
            Laid::Formula { path, .. } => path.len() + 4,
            Laid::Error { message, .. } => message.len() / 4 + 1,
        }
    }
}

/// The outlines as opcodes and numbers.
struct Ops(Vec<f32>);

impl PathSink for Ops {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.extend([0., x, y]);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.extend([1., x, y]);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.extend([2., x1, y1, x, y]);
    }
    fn cubic_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.extend([3., x1, y1, x2, y2, x, y]);
    }
    fn close(&mut self) {
        self.0.push(4.);
    }
}

/// `tex` laid out at 1 em in `face`, display style or inline.
pub fn lay_out(tex: &str, display: bool, face: Face) -> Laid {
    let options = if display { Options::display(1.0) } else { Options::inline(1.0) };
    match zatex::bundled::layout(tex, face, &options) {
        Ok(formula) => {
            let mut ops = Ops(Vec::new());
            // The origin `-left` in from the box's left edge, on the
            // baseline `ascent` down: the box's top-left is (0, 0).
            formula.paint(zatex::bundled::font(face), -formula.left, formula.ascent, &mut ops);
            Laid::Formula { w: formula.span(), ascent: formula.ascent, descent: formula.descent, left: formula.left, path: ops.0 }
        }
        Err(error) => Laid::Error { message: format!("{:?}", error.kind), at: error.range.start },
    }
}

/// What's been laid out, newest last.
#[derive(Default)]
struct Kept {
    by: HashMap<(String, bool, Face), Laid>,
    order: VecDeque<(String, bool, Face)>,
    numbers: usize,
}

impl Kept {
    fn get(&mut self, tex: &str, display: bool, face: Face) -> Laid {
        let key = (tex.to_string(), display, face);
        if let Some(laid) = self.by.get(&key) {
            return laid.clone();
        }
        let laid = lay_out(tex, display, face);
        self.numbers += laid.numbers();
        self.by.insert(key.clone(), laid.clone());
        self.order.push_back(key);
        while self.order.len() > KEEP || self.numbers > KEEP_NUMBERS {
            let Some(old) = self.order.pop_front() else { break };
            if let Some(gone) = self.by.remove(&old) {
                self.numbers -= gone.numbers();
            }
        }
        laid
    }
}

thread_local! {
    static KEPT: RefCell<Kept> = RefCell::default();
}

fn to_value(laid: &Laid) -> Value {
    match laid {
        Laid::Formula { w, ascent, descent, left, path } => Value::record([
            ("w", Value::from(*w as f64)),
            ("ascent", Value::from(*ascent as f64)),
            ("descent", Value::from(*descent as f64)),
            ("left", Value::from(*left as f64)),
            ("path", Value::List(path.iter().map(|n| Value::from(*n as f64)).collect::<Vec<_>>().into())),
        ]),
        Laid::Error { message, at } => {
            Value::record([("error", Value::from(message.as_str())), ("at", Value::from(*at as f64))])
        }
    }
}

pub fn install(host: &ScriptHost) {
    host.function("math.layout", |arg| {
        let text = |v: Option<&Value>| v.map(crate::auth::text).unwrap_or_default();
        let tex = text(arg.field("tex"));
        let display = arg.field("display").is_some_and(Value::truthy);
        let face = face(&text(arg.field("face")));
        KEPT.with(|kept| to_value(&kept.borrow_mut().get(&tex, display, face)))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_formula_is_one_path_in_ems_inside_its_box() {
        let Laid::Formula { w, ascent, descent, left, path } = lay_out(r"\frac{a}{b} + \sqrt{x}", false, Face::Fira) else {
            panic!("laid out")
        };
        assert!(w > 1.0 && w < 10.0 && ascent > 0.3 && descent > 0.1 && left <= 0.0, "{w} {ascent} {descent} {left}");
        assert!(path.len() > 50 && path[0] == 0.0);
        // Every point sits in the box (a little ink may overhang it).
        let mut i = 0;
        let (h, slack) = (ascent + descent, 0.2);
        while i < path.len() {
            let take = match path[i] as u8 {
                0 | 1 => 2,
                2 => 4,
                3 => 6,
                4 => 0,
                op => panic!("opcode {op}"),
            };
            for pair in path[i + 1..i + 1 + take].chunks(2) {
                assert!(pair[0] > -slack && pair[0] < w + slack && pair[1] > -slack && pair[1] < h + slack, "{pair:?}");
            }
            i += 1 + take;
        }
    }

    #[test]
    fn display_is_larger_and_errors_say_where() {
        let size = |display| match lay_out(r"\sum_{i=1}^n i", display, Face::Fira) {
            Laid::Formula { ascent, descent, .. } => ascent + descent,
            Laid::Error { .. } => panic!("laid out"),
        };
        assert!(size(true) > size(false));
        match lay_out(r"x + \nosuchcommand", false, Face::Fira) {
            Laid::Error { at, .. } => assert_eq!(at, 4),
            Laid::Formula { .. } => panic!("an unknown command is an error"),
        }
    }

    #[test]
    fn layouts_are_kept_and_bounded() {
        let mut kept = Kept::default();
        let first = kept.get("x^2", false, Face::Fira);
        assert_eq!(kept.get("x^2", false, Face::Fira), first);
        assert_ne!(kept.get("x^2", false, Face::Euler), first, "another face, another layout");
        for n in 0..KEEP + 10 {
            kept.get(&format!("x_{{{n}}}"), false, Face::Fira);
        }
        assert!(kept.by.len() <= KEEP && kept.order.len() == kept.by.len());
        assert_eq!(face("pagella"), Face::Pagella);
        assert_eq!(face("anything"), Face::Fira);
    }
}
