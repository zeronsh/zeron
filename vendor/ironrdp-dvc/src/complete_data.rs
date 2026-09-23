use alloc::vec::Vec;
use core::cmp;

use ironrdp_core::{DecodeResult, cast_length, invalid_field_err};
use tracing::error;

use crate::pdu::{DataFirstPdu, DataPdu, DrdynvcDataPdu};

#[cfg(test)]
#[path = "complete_data_tests.rs"]
mod tests;

#[derive(Debug, PartialEq)]
pub(crate) struct CompleteData {
    total_size: usize,
    data: Vec<u8>,
    max_message_size: Option<usize>,
}

impl CompleteData {
    pub(crate) fn new(max_message_size: Option<usize>) -> Self {
        Self {
            total_size: 0,
            data: Vec::new(),
            max_message_size,
        }
    }

    pub(crate) fn process_data(&mut self, pdu: DrdynvcDataPdu) -> DecodeResult<Option<Vec<u8>>> {
        if let Some(maximum) = self.max_message_size {
            let valid = match &pdu {
                DrdynvcDataPdu::DataFirst(first) => usize::try_from(first.length())
                    .is_ok_and(|total| total <= maximum && first.data().len() <= total),
                DrdynvcDataPdu::Data(next) => self.data.len().checked_add(next.data().len())
                    .is_some_and(|total| total <= maximum && (self.total_size == 0 || total <= self.total_size)),
            };
            if !valid {
                self.data = Vec::new();
                self.total_size = 0;
                return Err(invalid_field_err!("DVC", "length", "message exceeds configured or declared limit"));
            }
        }
        match pdu {
            DrdynvcDataPdu::DataFirst(data_first) => self.process_data_first_pdu(data_first),
            DrdynvcDataPdu::Data(data) => self.process_data_pdu(data),
        }
    }

    fn process_data_first_pdu(&mut self, data_first: DataFirstPdu) -> DecodeResult<Option<Vec<u8>>> {
        let total_data_size: DecodeResult<_> = cast_length!("DataFirstPdu::length", data_first.length());
        let total_data_size = total_data_size?;
        if self.total_size != 0 || !self.data.is_empty() {
            error!("Incomplete DVC message, it will be skipped");

            self.data.clear();
        }

        if total_data_size == data_first.data().len() {
            // A complete replacement must not leave an old fragment length active.
            self.total_size = 0;
            Ok(Some(data_first.into_data()))
        } else {
            self.total_size = total_data_size;
            self.data = data_first.into_data();

            Ok(None)
        }
    }

    fn process_data_pdu(&mut self, mut data: DataPdu) -> DecodeResult<Option<Vec<u8>>> {
        if self.total_size == 0 && self.data.is_empty() {
            // message is not fragmented
            return Ok(Some(data.into_data()));
        }

        // The message is fragmented and needs to be reassembled.
        match self.data.len().checked_add(data.data().len()) {
            Some(actual_data_length) => {
                match actual_data_length.cmp(&(self.total_size)) {
                    cmp::Ordering::Less => {
                        // this is one of the fragmented messages, just append it
                        self.data.append(data.data_mut());
                        Ok(None)
                    }
                    cmp::Ordering::Equal => {
                        // this is the last fragmented message, need to return the whole reassembled message
                        self.total_size = 0;
                        self.data.append(data.data_mut());
                        Ok(Some(self.data.drain(..).collect()))
                    }
                    cmp::Ordering::Greater => {
                        error!("Actual DVC message size is grater than expected total DVC message size");
                        self.total_size = 0;
                        self.data.clear();
                        Ok(None)
                    }
                }
            }
            _ => Err(invalid_field_err!("DVC message", "data", "overflow occurred")),
        }
    }
}
