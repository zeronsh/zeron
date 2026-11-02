//! Energy saver: one switch that trades decorative work for battery.
//!
//! While active it
//! - snaps interface animations to rest (through [`crate::motion`]); activity
//!   indicators keep a slow, subtle pulse so running agents stay visible;
//! - holds the composer caret solid instead of repainting twice a second;
//! - paints surfaces opaque, skipping backdrop blur and window vibrancy;
//! - keeps video wallpapers playing, but decoded smaller and at a lower rate.
//!
//! `OnBattery` follows the power source (and macOS Low Power Mode / Windows
//! battery saver). The source is re-read when the main window regains focus
//! and on a slow timer that only runs in that mode, so the feature itself
//! costs almost nothing.

use std::time::Duration;

use gpui::{App, AppContext as _, Global, Task};
use serde::{Deserialize, Serialize};

/// How often `OnBattery` re-reads the power source. Unplugging is noticed
/// within this window (or immediately on the next window focus).
const POLL: Duration = Duration::from_secs(30);

/// The user's energy saver preference. Persisted in `ui-settings.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EnergySaver {
    Off,
    /// On while running on battery or in the OS low-power mode.
    #[default]
    OnBattery,
    On,
}

impl EnergySaver {
    pub const ALL: [Self; 3] = [Self::Off, Self::OnBattery, Self::On];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::OnBattery => "On battery",
            Self::On => "Always",
        }
    }
}

/// What the OS last reported about power.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PowerStatus {
    pub on_battery: bool,
    /// macOS Low Power Mode or Windows battery saver.
    pub low_power: bool,
}

/// Whether energy saver applies for a preference and power status.
pub fn resolve(preference: EnergySaver, status: PowerStatus) -> bool {
    match preference {
        EnergySaver::Off => false,
        EnergySaver::On => true,
        EnergySaver::OnBattery => status.on_battery || status.low_power,
    }
}

struct EnergyState {
    preference: EnergySaver,
    status: PowerStatus,
    active: bool,
    _poll: Option<Task<()>>,
}

impl Global for EnergyState {}

/// Install the state before the theme and motion are initialized, so the
/// first frame already uses the right surfaces and motion.
pub fn init(preference: EnergySaver, cx: &mut App) {
    let status = read_status();
    let active = resolve(preference, status);
    cx.set_global(EnergyState {
        preference,
        status,
        active,
        _poll: None,
    });
    restart_poll(cx);
}

/// Whether energy saver is in effect right now.
pub fn active(cx: &App) -> bool {
    cx.try_global::<EnergyState>()
        .is_some_and(|state| state.active)
}

pub fn preference(cx: &App) -> EnergySaver {
    cx.try_global::<EnergyState>()
        .map(|state| state.preference)
        .unwrap_or_default()
}

/// The power status as last read; drives the Settings helper text.
pub fn status(cx: &App) -> PowerStatus {
    cx.try_global::<EnergyState>()
        .map(|state| state.status)
        .unwrap_or_default()
}

pub fn set_preference(preference: EnergySaver, cx: &mut App) {
    let Some(state) = state_mut(cx) else {
        return;
    };
    if state.preference == preference {
        return;
    }
    state.preference = preference;
    restart_poll(cx);
    apply(cx);
    crate::settings::update(crate::settings::SavePolicy::Immediate, cx, |settings| {
        settings.energy_saver = preference;
    });
}

/// Re-read the power source (window focus is a natural moment: plugging in
/// or out usually happens while the app is in the background).
pub fn refresh(cx: &mut App) {
    if preference(cx) != EnergySaver::OnBattery {
        return;
    }
    let read = cx.background_spawn(async { read_status() });
    cx.spawn(async move |cx| {
        let status = read.await;
        cx.update(|cx| set_status(status, cx));
    })
    .detach();
}

fn state_mut(cx: &mut App) -> Option<&mut EnergyState> {
    cx.has_global::<EnergyState>()
        .then(|| cx.global_mut::<EnergyState>())
}

fn set_status(status: PowerStatus, cx: &mut App) {
    let Some(state) = state_mut(cx) else {
        return;
    };
    if state.status == status {
        return;
    }
    state.status = status;
    apply(cx);
}

/// Only `OnBattery` depends on the power source, so only it polls.
fn restart_poll(cx: &mut App) {
    let poll = (preference(cx) == EnergySaver::OnBattery).then(|| {
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let status = cx
                    .background_executor()
                    .spawn(async { read_status() })
                    .await;
                cx.update(|cx| set_status(status, cx));
            }
        })
    });
    if let Some(state) = state_mut(cx) {
        state._poll = poll;
    }
}

/// Recompute the flag and push it into everything that depends on it.
fn apply(cx: &mut App) {
    let Some(state) = state_mut(cx) else {
        return;
    };
    let active = resolve(state.preference, state.status);
    if state.active == active {
        return;
    }
    state.active = active;
    tracing::debug!(active, "energy saver");
    crate::motion::set_energy_saver(active, cx);
    // Opaque surfaces while saving; the stored surface preference is untouched.
    crate::appearance::apply(cx);
    // Video wallpapers pick their decode size and cadence on the next paint.
    cx.refresh_windows();
}

// ---------------------------------------------------------------------------
// Power source
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn read_status() -> PowerStatus {
    use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
    use core_foundation::string::{CFString, CFStringRef};
    use objc::runtime::{NO, Object};
    use objc::{class, msg_send, sel, sel_impl};

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
        fn IOPSGetProvidingPowerSourceType(snapshot: CFTypeRef) -> CFStringRef;
    }

    let on_battery = unsafe {
        let snapshot = IOPSCopyPowerSourcesInfo();
        if snapshot.is_null() {
            false
        } else {
            // "Get" rule: the type string belongs to the snapshot.
            let kind = IOPSGetProvidingPowerSourceType(snapshot);
            let battery = !kind.is_null()
                && CFString::wrap_under_get_rule(kind).to_string() == "Battery Power";
            CFRelease(snapshot);
            battery
        }
    };
    // Low Power Mode exists on macOS 12+; older systems simply report off.
    let low_power = unsafe {
        let info: *mut Object = msg_send![class!(NSProcessInfo), processInfo];
        let responds: objc::runtime::BOOL =
            msg_send![info, respondsToSelector: sel!(isLowPowerModeEnabled)];
        responds != NO && {
            let enabled: objc::runtime::BOOL = msg_send![info, isLowPowerModeEnabled];
            enabled != NO
        }
    };
    PowerStatus {
        on_battery,
        low_power,
    }
}

/// `/sys/class/power_supply`: on battery when a system battery discharges
/// and no mains/USB supply is online. Desktops without a battery read as AC.
#[cfg(target_os = "linux")]
fn read_status() -> PowerStatus {
    let read = |path: std::path::PathBuf| {
        std::fs::read_to_string(path)
            .ok()
            .map(|value| value.trim().to_owned())
    };
    let Ok(entries) = std::fs::read_dir("/sys/class/power_supply") else {
        return PowerStatus::default();
    };
    let mut external = false;
    let mut discharging = false;
    for entry in entries.flatten() {
        let path = entry.path();
        match read(path.join("type")).as_deref() {
            Some("Mains" | "USB") => external |= read(path.join("online")).as_deref() == Some("1"),
            // Peripheral batteries (mice, headsets) report scope "Device".
            Some("Battery") if read(path.join("scope")).as_deref() != Some("Device") => {
                discharging |= read(path.join("status")).as_deref() == Some("Discharging");
            }
            _ => {}
        }
    }
    PowerStatus {
        on_battery: discharging && !external,
        low_power: false,
    }
}

#[cfg(windows)]
fn read_status() -> PowerStatus {
    /// `SYSTEM_POWER_STATUS` (winbase.h).
    #[repr(C)]
    #[derive(Default)]
    struct SystemPowerStatus {
        ac_line_status: u8,
        battery_flag: u8,
        battery_life_percent: u8,
        system_status_flag: u8,
        battery_life_time: u32,
        battery_full_life_time: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetSystemPowerStatus(status: *mut SystemPowerStatus) -> i32;
    }
    let mut status = SystemPowerStatus::default();
    if unsafe { GetSystemPowerStatus(&mut status) } == 0 {
        return PowerStatus::default();
    }
    PowerStatus {
        // 0 = offline, 1 = online, 255 = unknown.
        on_battery: status.ac_line_status == 0,
        // 1 = battery saver on.
        low_power: status.system_status_flag == 1,
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn read_status() -> PowerStatus {
    PowerStatus::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_follows_preference_and_power() {
        let ac = PowerStatus::default();
        let battery = PowerStatus {
            on_battery: true,
            low_power: false,
        };
        let low_power = PowerStatus {
            on_battery: false,
            low_power: true,
        };
        assert!(!resolve(EnergySaver::Off, battery));
        assert!(!resolve(EnergySaver::Off, low_power));
        assert!(resolve(EnergySaver::On, ac));
        assert!(!resolve(EnergySaver::OnBattery, ac));
        assert!(resolve(EnergySaver::OnBattery, battery));
        assert!(resolve(EnergySaver::OnBattery, low_power));
    }

    #[test]
    fn preference_round_trips_and_defaults_to_on_battery() {
        assert_eq!(EnergySaver::default(), EnergySaver::OnBattery);
        for preference in EnergySaver::ALL {
            let json = serde_json::to_string(&preference).unwrap();
            assert_eq!(
                serde_json::from_str::<EnergySaver>(&json).unwrap(),
                preference
            );
        }
        assert_eq!(
            serde_json::to_string(&EnergySaver::OnBattery).unwrap(),
            "\"onBattery\""
        );
    }

    #[test]
    fn reading_the_power_source_never_panics() {
        let _ = read_status();
    }

    /// Manual check against `pmset -g batt` / the OS battery indicator:
    /// `cargo test -p zeron-ui --lib energy::tests::prints -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn prints_the_power_source() {
        println!("power status: {:?}", read_status());
    }
}
