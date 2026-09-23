use super::*;
use alloc::vec;

fn bounded() -> CompleteData {
    CompleteData::new(Some(20))
}

fn first(total: u32, size: usize) -> DrdynvcDataPdu {
    DrdynvcDataPdu::DataFirst(DataFirstPdu::new(1, total, vec![7; size]))
}

fn next(size: usize) -> DrdynvcDataPdu {
    DrdynvcDataPdu::Data(DataPdu::new(1, vec![7; size]))
}

#[test]
fn huge_advertised_display_message_is_rejected_on_first_fragment() {
    let mut processor = bounded();
    assert!(processor.process_data(first(u32::MAX, 1)).is_err());
    assert_eq!(processor.data.capacity(), 0);
    assert_eq!(processor.total_size, 0);
}

#[test]
fn valid_fragmented_and_unfragmented_display_messages_succeed() {
    let mut processor = bounded();
    assert!(processor.process_data(first(20, 1)).unwrap().is_none());
    assert!(processor.process_data(next(9)).unwrap().is_none());
    assert_eq!(processor.process_data(next(10)).unwrap().unwrap(), vec![7; 20]);
    assert_eq!(processor.process_data(next(20)).unwrap().unwrap(), vec![7; 20]);
}

#[test]
fn oversized_single_data_and_false_first_lengths_are_rejected() {
    for pdu in [next(21), first(20, 21), first(1, 20)] {
        let mut processor = bounded();
        assert!(processor.process_data(pdu).is_err());
        assert_eq!(processor.data.capacity(), 0);
        assert_eq!(processor.total_size, 0);
    }
}

#[test]
fn continuation_cannot_exceed_budget_or_declared_size() {
    for (total, initial, additional) in [(20, 19, 2), (10, 9, 2)] {
        let mut processor = bounded();
        assert!(processor.process_data(first(total, initial)).unwrap().is_none());
        assert!(processor.process_data(next(additional)).is_err());
        assert_eq!(processor.data.capacity(), 0);
        assert_eq!(processor.total_size, 0);
    }
}

#[test]
fn replacement_and_following_message_do_not_retain_fragment_state() {
    let mut processor = bounded();
    assert!(processor.process_data(first(20, 10)).unwrap().is_none());
    assert_eq!(processor.process_data(first(20, 20)).unwrap().unwrap().len(), 20);
    assert_eq!(processor.total_size, 0);
    assert_eq!(processor.process_data(next(20)).unwrap().unwrap().len(), 20);
    assert!(processor.process_data(first(20, 10)).unwrap().is_none());
    assert!(processor.process_data(first(u32::MAX, 1)).is_err());
    assert_eq!(processor.data.capacity(), 0);
}
