// Extracted from Bezel 6141af9c16f7353cdf36003f7404e0a94566a163; MIT. See THIRD_PARTY_NOTICES.md.
//! Twelve states × four sizes — original ports plus library-native concepts.

use crate::{
    engine::{ModeOpts, base_profile, scale_counts, scale_radii},
    types::{ModeKey, OrbSize, OrbState},
};

/// Fully resolved draw options for a (state, size) pair.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub mode: ModeKey,
    pub speed: f32,
    pub opts: ModeOpts,
}

struct Preset {
    speed: f32,
    count: f32,
    size: f32,
    // extra knobs merged after scaling
    scan_mul: Option<f32>,
    dim_base: Option<f32>,
    spin: Option<f32>,
    band_mul: Option<f32>,
    wob_mul: Option<f32>,
    spread: Option<f32>,
}

fn preset_for(mode: ModeKey, size: OrbSize) -> Preset {
    use ModeKey::*;

    // The large designs preserve each avatar preset's character while adding
    // detail gradually. `scale_counts` understands each painter's topology
    // (2-D lattices, flat lists, lane/segment pairs), so this is deliberately
    // not a naive coordinate scale.
    if matches!(size, OrbSize::Large | OrbSize::Hero) {
        let mut preset = preset_for(mode, OrbSize::Avatar);
        let (count, radius, speed) = match size {
            OrbSize::Large => (1.15, 1.05, 0.95),
            OrbSize::Hero => (1.30, 1.10, 0.90),
            _ => unreachable!(),
        };
        preset.count *= count;
        preset.size *= radius;
        preset.speed *= speed;
        return preset;
    }

    match (mode, size) {
        (Orbits, OrbSize::Avatar) => Preset {
            speed: 1.885,
            count: 1.0,
            size: 1.0,
            scan_mul: None,
            dim_base: None,
            spin: None,
            band_mul: None,
            wob_mul: None,
            spread: None,
        },
        (Orbits, OrbSize::Inline) => Preset {
            speed: 3.9,
            count: 0.238,
            size: 2.4,
            scan_mul: None,
            dim_base: None,
            spin: None,
            band_mul: None,
            wob_mul: None,
            spread: None,
        },
        (Globe, OrbSize::Avatar) => Preset {
            speed: 2.015,
            count: 0.42,
            size: 1.15,
            scan_mul: Some(4.08),
            dim_base: Some(0.45),
            spin: None,
            band_mul: None,
            wob_mul: None,
            spread: None,
        },
        (Globe, OrbSize::Inline) => Preset {
            speed: 2.665,
            count: 0.105,
            size: 1.75,
            scan_mul: Some(4.335),
            dim_base: Some(0.45),
            spin: None,
            band_mul: None,
            wob_mul: None,
            spread: None,
        },
        (Rubik, OrbSize::Avatar) => Preset {
            speed: 1.82,
            count: 0.35,
            size: 1.05,
            scan_mul: None,
            dim_base: None,
            spin: None,
            band_mul: None,
            wob_mul: None,
            spread: None,
        },
        (Rubik, OrbSize::Inline) => Preset {
            speed: 1.95,
            count: 0.088,
            size: 1.9,
            scan_mul: None,
            dim_base: None,
            spin: None,
            band_mul: None,
            wob_mul: None,
            spread: None,
        },
        (Wave, OrbSize::Avatar) => Preset {
            speed: 3.15,
            count: 0.341,
            size: 1.0,
            scan_mul: None,
            dim_base: None,
            spin: None,
            band_mul: None,
            wob_mul: None,
            spread: None,
        },
        (Wave, OrbSize::Inline) => Preset {
            speed: 3.35,
            count: 0.105,
            size: 1.6,
            scan_mul: None,
            dim_base: None,
            spin: None,
            band_mul: None,
            wob_mul: None,
            spread: None,
        },
        (Web, OrbSize::Avatar) => Preset {
            speed: 3.315,
            count: 1.35,
            size: 0.95,
            scan_mul: None,
            dim_base: None,
            spin: None,
            band_mul: None,
            wob_mul: None,
            spread: None,
        },
        (Web, OrbSize::Inline) => Preset {
            speed: 6.63,
            count: 0.25,
            size: 1.52,
            scan_mul: None,
            dim_base: None,
            spin: None,
            band_mul: None,
            wob_mul: None,
            spread: None,
        },
        (Braid, OrbSize::Avatar) => Preset {
            speed: 1.625,
            count: 0.5,
            size: 1.0,
            scan_mul: None,
            dim_base: None,
            spin: None,
            band_mul: None,
            wob_mul: None,
            spread: None,
        },
        (Braid, OrbSize::Inline) => Preset {
            speed: 2.75,
            count: 0.1125,
            size: 1.36,
            scan_mul: None,
            dim_base: None,
            spin: None,
            band_mul: None,
            wob_mul: None,
            spread: None,
        },
        (Ribbon, OrbSize::Avatar) => Preset {
            speed: 1.95,
            count: 0.25,
            size: 0.85,
            scan_mul: None,
            dim_base: None,
            spin: Some(0.0),
            band_mul: Some(2.9),
            wob_mul: Some(1.0),
            spread: None,
        },
        (Ribbon, OrbSize::Inline) => Preset {
            speed: 2.5,
            count: 0.051,
            size: 1.073,
            scan_mul: None,
            dim_base: None,
            spin: Some(0.0),
            band_mul: Some(3.6),
            wob_mul: Some(1.0),
            spread: None,
        },
        (Ring, OrbSize::Avatar) => Preset {
            speed: 1.55,
            count: 0.25,
            size: 0.956,
            scan_mul: None,
            dim_base: None,
            spin: Some(0.0),
            band_mul: Some(2.8),
            wob_mul: Some(0.368),
            spread: None,
        },
        (Ring, OrbSize::Inline) => Preset {
            speed: 2.1,
            count: 0.028,
            size: 1.622,
            scan_mul: None,
            dim_base: None,
            spin: Some(0.0),
            band_mul: Some(3.2),
            wob_mul: Some(0.565),
            spread: None,
        },
        (Morph, OrbSize::Avatar) => Preset {
            speed: 2.405,
            count: 0.702,
            size: 0.395,
            scan_mul: None,
            dim_base: None,
            spin: None,
            band_mul: None,
            wob_mul: None,
            spread: Some(1.45),
        },
        (Morph, OrbSize::Inline) => Preset {
            speed: 2.08,
            count: 0.53,
            size: 1.011,
            scan_mul: None,
            dim_base: None,
            spin: None,
            band_mul: None,
            wob_mul: None,
            spread: Some(1.45),
        },
        (Focus, OrbSize::Avatar) => Preset {
            speed: 1.72,
            count: 1.0,
            size: 1.0,
            scan_mul: None,
            dim_base: None,
            spin: None,
            band_mul: None,
            wob_mul: None,
            spread: Some(1.0),
        },
        (Focus, OrbSize::Inline) => Preset {
            speed: 2.2,
            count: 0.22,
            size: 1.45,
            scan_mul: None,
            dim_base: None,
            spin: None,
            band_mul: None,
            wob_mul: None,
            spread: Some(1.0),
        },
        (Gyroscope, OrbSize::Avatar) => Preset {
            speed: 1.48,
            count: 1.0,
            size: 1.0,
            scan_mul: None,
            dim_base: None,
            spin: None,
            band_mul: None,
            wob_mul: None,
            spread: Some(1.0),
        },
        (Gyroscope, OrbSize::Inline) => Preset {
            speed: 2.05,
            count: 0.25,
            size: 1.55,
            scan_mul: None,
            dim_base: None,
            spin: None,
            band_mul: None,
            wob_mul: None,
            spread: Some(1.0),
        },
        (Echo, OrbSize::Avatar) => Preset {
            speed: 1.85,
            count: 1.0,
            size: 1.0,
            scan_mul: None,
            dim_base: None,
            spin: None,
            band_mul: None,
            wob_mul: None,
            spread: Some(1.0),
        },
        (Echo, OrbSize::Inline) => Preset {
            speed: 2.45,
            count: 0.25,
            size: 1.55,
            scan_mul: None,
            dim_base: None,
            spin: None,
            band_mul: None,
            wob_mul: None,
            spread: Some(1.0),
        },
        (_, OrbSize::Large | OrbSize::Hero) => unreachable!(),
    }
}

/// Resolve a (state, size) pair to its mode + fully-scaled draw options.
pub fn resolve_preset(state: OrbState, size: OrbSize) -> Resolved {
    let mode = ModeKey::from_state(state);
    let preset = preset_for(mode, size);
    let mut opts = base_profile(mode);
    if (preset.count - 1.0).abs() > f32::EPSILON {
        opts = scale_counts(&opts, preset.count);
    }
    if (preset.size - 1.0).abs() > f32::EPSILON {
        opts = scale_radii(&opts, preset.size);
    }
    if let Some(v) = preset.scan_mul {
        opts.scan_mul = Some(v);
    }
    if let Some(v) = preset.dim_base {
        opts.dim_base = Some(v);
    }
    if let Some(v) = preset.spin {
        opts.spin = Some(v);
    }
    if let Some(v) = preset.band_mul {
        opts.band_mul = Some(v);
    }
    if let Some(v) = preset.wob_mul {
        opts.wob_mul = Some(v);
    }
    if let Some(v) = preset.spread {
        opts.spread = Some(v);
    }

    Resolved {
        mode,
        speed: preset.speed,
        opts,
    }
}
