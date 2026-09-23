use crate::validate_size;
use std::time::{Duration, Instant};
/// Geometry must remain unchanged for 200 ms. Hidden/transitioning surfaces
/// cancel pending requests; only server-confirmed dimensions drive input.
#[derive(Default)]
pub(crate) struct ResizeDebounce {
    candidate: Option<(u16, u16)>,
    since: Option<Instant>,
    sent: Option<(u16, u16)>,
}
impl ResizeDebounce {
    pub fn update(&mut self, size: Option<(u16, u16)>, visible: bool, now: Instant) {
        let candidate = size
            .filter(|(w, h)| visible && validate_size(*w, *h).is_ok())
            .map(|(w, h)| (w & !1, h));
        if candidate.is_none() {
            self.sent = None;
        }
        if self.candidate != candidate {
            self.candidate = candidate;
            self.since = candidate.map(|_| now);
        }
    }
    pub fn take_ready(
        &mut self,
        now: Instant,
        server_area: u64,
        current: (u16, u16),
    ) -> Option<(u16, u16)> {
        let size = self.candidate?;
        if size == current
            || Some(size) == self.sent
            || server_area == 0
            || u64::from(size.0) * u64::from(size.1) > server_area
            || now.duration_since(self.since?) < Duration::from_millis(200)
        {
            return None;
        }
        self.sent = Some(size);
        Some(size)
    }
}
#[cfg(test)]
mod tests {
    use super::ResizeDebounce;
    use std::time::{Duration, Instant};
    #[test]
    fn resize_waits_for_stability_visibility_and_server_limits() {
        let now = Instant::now();
        let mut r = ResizeDebounce::default();
        r.update(Some((1001, 600)), true, now);
        assert_eq!(
            r.take_ready(now + Duration::from_millis(199), 1_000_000, (800, 600)),
            None
        );
        r.update(Some((1200, 800)), true, now + Duration::from_millis(100));
        assert_eq!(
            r.take_ready(now + Duration::from_millis(299), 1_000_000, (800, 600)),
            None
        );
        assert_eq!(
            r.take_ready(now + Duration::from_millis(300), 0, (800, 600)),
            None
        );
        assert_eq!(
            r.take_ready(now + Duration::from_millis(300), 900_000, (800, 600)),
            None
        );
        assert_eq!(
            r.take_ready(now + Duration::from_millis(300), 1_000_000, (800, 600)),
            Some((1200, 800))
        );
        r.update(Some((1600, 900)), false, now);
        assert_eq!(
            r.take_ready(now + Duration::from_secs(1), 2_000_000, (800, 600)),
            None
        );
    }
}

/// MS-RDPBCGR permits Deactivate All with only the six-byte Share Control
/// header. xrdp emits this form; ironrdp-pdu 0.9 expects a shareId as well.
/// Match only this exact IO-channel message; all other PDUs use the decoder.
pub(crate) fn short_deactivation(
    action: ironrdp::pdu::Action,
    packet: &[u8],
    io_channel: u16,
) -> bool {
    if !matches!(action, ironrdp::pdu::Action::X224) {
        return false;
    }
    ironrdp::pdu::mcs::decode_send_data_indication(packet).is_ok_and(|ctx| {
        ctx.channel_id == io_channel
            && ctx.user_data.len() == 6
            && ctx.user_data[..4] == [6, 0, 0x16, 0]
    })
}

#[cfg(test)]
mod wire_tests {
    #[test]
    fn xrdp_short_deactivation_is_accepted_only_on_the_io_channel() {
        use ironrdp::{
            core::encode_vec,
            pdu::{Action, mcs::SendDataIndication, x224::X224},
        };
        let pdu = SendDataIndication {
            initiator_id: 1001,
            channel_id: 1003,
            user_data: (&[6, 0, 0x16, 0, 0xea, 3][..]).into(),
        };
        let bytes = encode_vec(&X224(pdu)).unwrap();
        assert!(super::short_deactivation(Action::X224, &bytes, 1003));
        assert!(!super::short_deactivation(Action::X224, &bytes, 1004));
        assert!(!super::short_deactivation(Action::FastPath, &bytes, 1003));
        assert!(!super::short_deactivation(
            Action::X224,
            &bytes[..bytes.len() - 1],
            1003
        ));
    }
}
