use crate::{Frame, SessionError, validate_size};
use ironrdp::session::image::DecodedImage;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// Limits both full-frame copies and publications. Protocol deltas always update
/// the canonical DecodedImage even when no snapshot is produced.
pub(crate) struct Publisher {
    generation: u64,
    sequence: u64,
    last: Option<Instant>,
    dirty: bool,
}
impl Publisher {
    pub fn new(generation: u64) -> Self {
        Self {
            generation,
            sequence: 0,
            last: None,
            dirty: false,
        }
    }
    pub fn wait_for_graphics(&mut self) {
        self.dirty = false;
    }
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }
    pub fn publish(
        &mut self,
        image: &DecodedImage,
        visible: bool,
        now: Instant,
    ) -> Result<Option<Arc<Frame>>, SessionError> {
        if !visible
            || !self.dirty
            || self
                .last
                .is_some_and(|last| now.duration_since(last) < Duration::from_millis(33))
        {
            return Ok(None);
        }
        let len = validate_size(image.width(), image.height())?;
        if image.data().len() != len {
            return Err(SessionError::new(
                crate::ErrorStage::Protocol,
                "Invalid framebuffer stride",
            ));
        }
        let mut bytes = image.data().to_vec();
        for pixel in bytes.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        self.sequence += 1;
        self.dirty = false;
        self.last = Some(now);
        Ok(Some(Arc::new(Frame {
            generation: self.generation,
            sequence: self.sequence,
            width: image.width(),
            height: image.height(),
            bgra: bytes.into(),
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copies_only_changed_visible_frames_at_bounded_rate() {
        let image = DecodedImage::new(
            ironrdp::graphics::image_processing::PixelFormat::BgrA32,
            200,
            200,
        );
        let now = Instant::now();
        let mut p = Publisher::new(9);
        p.mark_dirty();
        assert!(p.publish(&image, false, now).unwrap().is_none());
        let first = p.publish(&image, true, now).unwrap().unwrap();
        assert_eq!(first.bgra.len(), 200 * 200 * 4);
        assert!(first.bgra.chunks_exact(4).all(|p| p[3] == 255));
        p.mark_dirty();
        assert!(p.publish(&image, true, now).unwrap().is_none());
        assert_eq!(
            p.publish(&image, true, now + Duration::from_millis(34))
                .unwrap()
                .unwrap()
                .sequence,
            2
        );
        assert!(
            p.publish(&image, true, now + Duration::from_millis(70))
                .unwrap()
                .is_none()
        );
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    use ironrdp::{
        core::{WriteBuf, encode_vec},
        pdu::{
            bitmap::{BitmapData, BitmapUpdateData, Compression},
            fast_path::{
                EncryptionFlags, FastPathHeader, FastPathUpdatePdu, Fragmentation, UpdateCode,
            },
            geometry::InclusiveRectangle,
        },
        session::fast_path::ProcessorBuilder,
    };
    #[test]
    fn hidden_incremental_bitmaps_keep_color_origin_and_all_prior_deltas() {
        let mut decoder = ProcessorBuilder {
            io_channel_id: 1003,
            user_channel_id: 1001,
            share_id: 1,
            enable_server_pointer: false,
            pointer_software_rendering: false,
            bulk_decompressor: None,
        }
        .build();
        let mut image = DecodedImage::new(
            ironrdp::graphics::image_processing::PixelFormat::BgrA32,
            200,
            200,
        );
        let mut publisher = Publisher::new(3);
        for (x, y, top, bottom) in [
            (10, 12, [0, 0, 255], [255, 0, 0]),
            (100, 110, [0, 255, 0], [255, 255, 255]),
        ] {
            // RDP bitmap rows are bottom-up, BGR24, with a four-pixel stride.
            let data = [bottom.repeat(4), top.repeat(4)].concat();
            let bitmap = encode_vec(&BitmapUpdateData {
                rectangles: vec![BitmapData {
                    rectangle: InclusiveRectangle {
                        left: x,
                        top: y,
                        right: x + 3,
                        bottom: y + 1,
                    },
                    width: 4,
                    height: 2,
                    bits_per_pixel: 24,
                    compression_flags: Compression::empty(),
                    compressed_data_header: None,
                    bitmap_data: &data,
                }],
            })
            .unwrap();
            let update = encode_vec(&FastPathUpdatePdu {
                fragmentation: Fragmentation::Single,
                update_code: UpdateCode::Bitmap,
                compression_flags: None,
                compression_type: None,
                data: &bitmap,
            })
            .unwrap();
            let mut packet =
                encode_vec(&FastPathHeader::new(EncryptionFlags::empty(), update.len())).unwrap();
            packet.extend(update);
            assert!(
                !decoder
                    .process(&mut image, &packet, &mut WriteBuf::new())
                    .unwrap()
                    .is_empty()
            );
            publisher.mark_dirty();
            assert!(
                publisher
                    .publish(&image, false, Instant::now())
                    .unwrap()
                    .is_none()
            );
        }
        let frame = publisher
            .publish(&image, true, Instant::now())
            .unwrap()
            .unwrap();
        for (x, y, bgra) in [
            (10, 12, [0, 0, 255, 255]),
            (10, 13, [255, 0, 0, 255]),
            (100, 110, [0, 255, 0, 255]),
            (100, 111, [255, 255, 255, 255]),
        ] {
            let offset = (y * 200 + x) * 4;
            assert_eq!(&frame.bgra[offset..offset + 4], &bgra);
        }
        assert_eq!(frame.sequence, 1);
    }
}
