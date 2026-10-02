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
pub(crate) const MAX_TRANSPORT_BYTES: usize = 64 * 1024;
impl_as_any!(DisplayControl);
impl DvcClientProcessor for DisplayControl {}
impl DvcProcessor for DisplayControl {
    fn channel_name(&self) -> &str {
        CHANNEL_NAME
    }
    fn max_message_size(&self) -> Option<usize> {
        // The server sends only the 20-byte Display Control CAPS PDU.
        Some(20)
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
    use ironrdp::{
        core::{Encode, encode_vec},
        dvc::{
            DrdynvcClient,
            pdu::{
                CreateRequestPdu, DataFirstPdu, DataPdu, DrdynvcDataPdu as Data,
                DrdynvcServerPdu as Server,
            },
        },
        svc::StaticVirtualChannel,
    };

    fn transport(area: Arc<AtomicU64>, id: u32) -> StaticVirtualChannel {
        let mut transport = StaticVirtualChannel::new(
            DrdynvcClient::new().with_dynamic_channel(DisplayControl(area)),
        );
        transport.set_max_message_size(MAX_TRANSPORT_BYTES);
        send_fragmented(
            &mut transport,
            &Server::Create(CreateRequestPdu::new(id, CHANNEL_NAME.into())),
        )
        .unwrap();
        transport
    }

    // Exercise real SVC decoding followed by DVC decoding and reassembly. Even
    // DVC headers are split across SVC chunks, rather than bypassing either layer.
    fn send_fragmented(transport: &mut StaticVirtualChannel, pdu: &impl Encode) -> PduResult<()> {
        let bytes = encode_vec(pdu).unwrap();
        for (index, data) in bytes.chunks(3).enumerate() {
            let mut flags = 0u32;
            if index == 0 {
                flags |= 1;
            }
            if (index + 1) * 3 >= bytes.len() {
                flags |= 2;
            }
            let mut chunk = (bytes.len() as u32).to_le_bytes().to_vec();
            chunk.extend_from_slice(&flags.to_le_bytes());
            chunk.extend_from_slice(data);
            transport.process(&chunk)?;
        }
        Ok(())
    }

    fn caps() -> Vec<u8> {
        [5u32, 20, 16, 4096, 2048]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect()
    }

    #[test]
    fn display_caps_survive_nested_fragmentation_and_channel_id_widths() {
        for id in [1, 256, 65536] {
            let area = Arc::new(AtomicU64::new(0));
            let mut channel = transport(area.clone(), id);
            let bytes = caps();
            send_fragmented(
                &mut channel,
                &Data::DataFirst(DataFirstPdu::new(id, 20, bytes[..7].to_vec())),
            )
            .unwrap();
            assert_eq!(area.load(Ordering::Relaxed), 0);
            send_fragmented(
                &mut channel,
                &Data::Data(DataPdu::new(id, bytes[7..].to_vec())),
            )
            .unwrap();
            assert_eq!(area.load(Ordering::Relaxed), 16 * 4096 * 2048);
            send_fragmented(&mut channel, &Data::Data(DataPdu::new(id, bytes))).unwrap();
        }
    }

    #[test]
    fn display_data_is_bounded_before_the_application_callback() {
        for id in [1, 256, 65536] {
            let area = Arc::new(AtomicU64::new(0));
            let mut channel = transport(area.clone(), id);
            let error = send_fragmented(
                &mut channel,
                &Data::DataFirst(DataFirstPdu::new(id, u32::MAX, vec![0])),
            )
            .unwrap_err();
            assert!(format!("{error:?}").contains("configured or declared limit"));
            assert_eq!(area.load(Ordering::Relaxed), 0);
        }
        for payload in [vec![0; 21], vec![0; 64]] {
            let mut channel = transport(Arc::new(AtomicU64::new(0)), 1);
            let error =
                send_fragmented(&mut channel, &Data::Data(DataPdu::new(1, payload))).unwrap_err();
            assert!(format!("{error:?}").contains("configured or declared limit"));
        }
    }

    #[test]
    fn outer_dynamic_transport_is_bounded_before_dvc_decoding() {
        let mut channel = transport(Arc::new(AtomicU64::new(0)), 1);
        let mut chunk = (MAX_TRANSPORT_BYTES as u32).to_le_bytes().to_vec();
        chunk.extend_from_slice(&0u32.to_le_bytes()); // No LAST: no DVC dispatch.
        chunk.extend_from_slice(&[0; 1024]);
        for _ in 0..MAX_TRANSPORT_BYTES / 1024 {
            assert!(channel.process(&chunk).unwrap().is_empty());
        }
        let error = channel.process(&chunk).unwrap_err();
        assert!(format!("{error:?}").contains("message exceeds configured limit"));
    }

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
