//! What a plugin asks for, rated as its consent sheet says it: which of the
//! programs it names can do anything you can, which give it full control of
//! your computer, and which of the hosts it names reach this device or your
//! network. One implementation for every place that words a plugin's asks
//! (the app's consent sheet, the store's pages through the engine's shared
//! vectors), from [`commands`] and [`net`]'s rules.

use crate::{commands, net};

/// Programs that can do anything you can and aren't said otherwise: not a
/// shell or an interpreter (those are [`full_control`]), not git or gh
/// (said as what they do).
pub fn anything(programs: &[String]) -> Vec<String> {
    programs
        .iter()
        .filter(|c| commands::risk(c) == commands::Risk::Anything && !commands::full_control(c) && !commands::held_to_its_own_rules(c))
        .cloned()
        .collect()
}

/// Programs that give full control of your computer: shells and
/// interpreters.
pub fn full_control(programs: &[String]) -> Vec<String> {
    programs.iter().filter(|c| commands::full_control(c)).cloned().collect()
}

/// Hosts on this device or your network (the any-host `*` aside).
pub fn local(hosts: &[String]) -> Vec<String> {
    hosts.iter().filter(|h| *h != "*" && net::risky(h)).cloned().collect()
}

/// Hosts named exactly that don't read as local: nothing can tell from a
/// name where it points, so the sheet says it might be near.
pub fn named(hosts: &[String]) -> Vec<String> {
    hosts.iter().filter(|h| net::names_exactly(h) && !net::risky(h)).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn programs_and_hosts_are_rated_as_the_sheet_says_them() {
        let programs = list(&["git", "gh", "jq", "curl", "python3.12", "Bash.exe", "git-lfs"]);
        assert_eq!(anything(&programs), list(&["curl", "git-lfs"]));
        assert_eq!(full_control(&programs), list(&["python3.12", "Bash.exe"]));
        let hosts = list(&["*", "api.example.com", "*.example.org", "localhost:8080", "nas.local", "10.0.0.2"]);
        assert_eq!(local(&hosts), list(&["localhost:8080", "nas.local", "10.0.0.2"]));
        assert_eq!(named(&hosts), list(&["api.example.com"]));
    }

    /// The consent window's own cases (apps/zgx `plugins/consent.rs`
    /// `the_sheet_is_told_how_far_each_program_reaches` and
    /// `the_sheet_is_told_how_each_host_reads`), as this module rates them.
    #[test]
    fn the_consent_window_s_cases() {
        let programs = list(&["python3", "sh", "git", "gh", "rg", "jq"]);
        assert_eq!(full_control(&programs), list(&["python3", "sh"]));
        assert_eq!(anything(&programs), list(&["rg"]));
        let added = list(&["node", "git", "rg"]);
        assert_eq!((full_control(&added), anything(&added)), (list(&["node"]), list(&["rg"])));
        let hosts = list(&["localhost:8080", "127.1", "box.tail1234.ts.net", "api.example.com", "*.example.org", "*"]);
        assert_eq!(local(&hosts), list(&["localhost:8080", "127.1"]));
        assert_eq!(named(&hosts), list(&["box.tail1234.ts.net", "api.example.com"]));
        assert_eq!(named(&list(&["rebind.example.com", "*.example.net"])), list(&["rebind.example.com"]));
    }
}
