use super::*;

const LIMIT: usize = 1024 * 1024 + 8;

fn chunk(length: u32, flags: ChannelControlFlags, data: &[u8]) -> Vec<u8> {
    let mut bytes = length.to_le_bytes().to_vec();
    bytes.extend_from_slice(&flags.bits().to_le_bytes());
    bytes.extend_from_slice(data);
    bytes
}

fn bounded() -> ChunkProcessor {
    let mut processor = ChunkProcessor::new();
    processor.max_message_size = Some(LIMIT);
    processor
}

#[test]
fn huge_declared_size_is_rejected_without_allocation() {
    let mut processor = bounded();
    assert!(
        processor
            .dechunkify(&chunk(u32::MAX, ChannelControlFlags::FLAG_FIRST, &[1]))
            .is_err()
    );
    assert_eq!(processor.chunked_pdu.capacity(), 0);
}

#[test]
fn lying_lengths_and_repeated_first_flags_do_not_reset_the_budget() {
    let mut processor = bounded();
    let first = vec![0; LIMIT];
    assert!(
        processor
            .dechunkify(&chunk(1, ChannelControlFlags::FLAG_FIRST, &first))
            .unwrap()
            .is_none()
    );
    assert!(
        processor
            .dechunkify(&chunk(1, ChannelControlFlags::FLAG_FIRST, &[1]))
            .is_err()
    );
    assert_eq!(processor.chunked_pdu.capacity(), 0);
}

#[test]
fn oversized_unfragmented_payload_is_rejected_even_with_small_declaration() {
    let mut processor = bounded();
    assert!(
        processor
            .dechunkify(&chunk(
                1,
                ChannelControlFlags::FLAG_FIRST | ChannelControlFlags::FLAG_LAST,
                &vec![0; LIMIT + 1]
            ))
            .is_err()
    );
    assert_eq!(processor.chunked_pdu.capacity(), 0);
}

#[test]
fn endless_clipboard_fragments_fail_before_growing_past_limit() {
    let mut processor = bounded();
    let data = [42; 1024];
    for _ in 0..1024 {
        assert!(
            processor
                .dechunkify(&chunk(LIMIT as u32, ChannelControlFlags::empty(), &data))
                .unwrap()
                .is_none()
        );
    }
    assert!(
        processor
            .dechunkify(&chunk(LIMIT as u32, ChannelControlFlags::empty(), &data))
            .is_err()
    );
    assert_eq!(
        processor.chunked_pdu.capacity(),
        0,
        "overflow releases accumulated memory"
    );
}

#[test]
fn exact_clipboard_limit_and_following_message_succeed() {
    let mut processor = bounded();
    let first = vec![42; LIMIT - 8];
    assert!(
        processor
            .dechunkify(&chunk(LIMIT as u32, ChannelControlFlags::FLAG_FIRST, &first))
            .unwrap()
            .is_none()
    );
    let complete = processor
        .dechunkify(&chunk(LIMIT as u32, ChannelControlFlags::FLAG_LAST, &[0; 8]))
        .unwrap()
        .unwrap();
    assert_eq!(complete.len(), LIMIT);
    let next = processor
        .dechunkify(&chunk(
            2,
            ChannelControlFlags::FLAG_FIRST | ChannelControlFlags::FLAG_LAST,
            &[1, 2],
        ))
        .unwrap()
        .unwrap();
    assert_eq!(next, [1, 2]);
}
