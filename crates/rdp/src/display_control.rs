//! IronRDP displaycontrol 0.8 decodes the CAPS body without consuming its
//! eight-byte wire header. Decode the complete PDU here until that is fixed
//! upstream; keep using its validated monitor layout encoder.
use ironrdp::{
    core::{decode, impl_as_any},
    displaycontrol::{
        CHANNEL_NAME,
        pdu::{DisplayControlMonitorLayout, DisplayControlPdu},
    },
    dvc::{DvcClientProcessor, DvcMessage, DvcProcessor, encode_dvc_messages},
    pdu::{PduResult, decode_err},
    session::ActiveStage,
    svc::ChannelFlags,
};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

pub(crate) struct DisplayControl(pub Arc<AtomicU64>);
impl_as_any!(DisplayControl);
impl DvcClientProcessor for DisplayControl {}
impl DvcProcessor for DisplayControl {
    fn channel_name(&self) -> &str {
        CHANNEL_NAME
    }
    fn start(&mut self, _: u32) -> PduResult<Vec<DvcMessage>> {
        Ok(Vec::new())
    }
    fn process(&mut self, _: u32, payload: &[u8]) -> PduResult<Vec<DvcMessage>> {
        // Verify the declared length too: Decode alone accepts trailing data.
        if payload.len() != 20 || u32::from_le_bytes(payload[4..8].try_into().unwrap()) != 20 {
            return Err(ironrdp::pdu::other_err!(
                "Invalid Display Control capability length"
            ));
        }
        match decode::<DisplayControlPdu>(payload).map_err(|e| decode_err!(e))? {
            DisplayControlPdu::Caps(caps) => {
                self.0.store(caps.max_monitor_area(), Ordering::Relaxed)
            }
            _ => {
                return Err(ironrdp::pdu::other_err!(
                    "Expected Display Control capabilities"
                ));
            }
        }
        Ok(Vec::new())
    }
}

pub(crate) fn encode_resize(
    active: &mut ActiveStage,
    width: u16,
    height: u16,
) -> Result<Option<Vec<u8>>, crate::SessionError> {
    let Some(channel_id) = active
        .get_dvc::<DisplayControl>()
        .and_then(|dvc| dvc.channel_id())
    else {
        return Ok(None);
    };
    let pdu: DisplayControlPdu = DisplayControlMonitorLayout::new_single_primary_monitor(
        width.into(),
        height.into(),
        None,
        None,
    )
    .map_err(protocol_error)?
    .into();
    let messages = encode_dvc_messages(channel_id, vec![Box::new(pdu)], ChannelFlags::empty())
        .map_err(protocol_error)?;
    active
        .encode_dvc_messages(messages)
        .map(Some)
        .map_err(protocol_error)
}
fn protocol_error(e: impl std::fmt::Display) -> crate::SessionError {
    crate::SessionError::new(crate::ErrorStage::Protocol, e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_wire_caps_include_header_and_bound_the_actual_area() {
        let area = Arc::new(AtomicU64::new(0));
        let mut client = DisplayControl(area.clone());
        // xrdp 0.9.24: type=5, length=20, monitors=16, area=4096*2048 per monitor.
        let packet: [u32; 5] = [5, 20, 16, 4096, 2048];
        let bytes: Vec<u8> = packet.into_iter().flat_map(u32::to_le_bytes).collect();
        client.process(1, &bytes).unwrap();
        assert_eq!(area.load(Ordering::Relaxed), 16 * 4096 * 2048);
        assert!(client.process(1, &bytes[8..]).is_err());
        let mut invalid = bytes.clone();
        invalid[4] = 12;
        assert!(client.process(1, &invalid).is_err());
    }
}
