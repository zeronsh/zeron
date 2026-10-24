//! Loopback ports agent CLIs serve their own protocol on (`opencode serve`).
//! Such a server carries its run's origin marker like everything the agent
//! starts, but it is the agent itself, never a dev server to preview.
use std::{
    collections::{BTreeMap, HashSet},
    sync::Mutex,
};

static PORTS: Mutex<BTreeMap<u16, usize>> = Mutex::new(BTreeMap::new());

/// Holds `port` reserved until dropped.
pub struct Reservation(u16);

pub fn reserve(port: u16) -> Reservation {
    *lock().entry(port).or_default() += 1;
    Reservation(port)
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut ports = lock();
        if let Some(count) = ports.get_mut(&self.0) {
            *count -= 1;
            if *count == 0 {
                ports.remove(&self.0);
            }
        }
    }
}

/// Every port reserved right now.
pub fn reserved() -> HashSet<u16> {
    lock().keys().copied().collect()
}

fn lock() -> std::sync::MutexGuard<'static, BTreeMap<u16, usize>> {
    PORTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_port_stays_reserved_until_its_last_holder_drops() {
        let first = reserve(64_123);
        let second = reserve(64_123);
        assert!(reserved().contains(&64_123));
        drop(first);
        assert!(reserved().contains(&64_123));
        drop(second);
        assert!(!reserved().contains(&64_123));
    }
}
