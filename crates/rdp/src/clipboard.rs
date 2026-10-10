//! Manual Unicode clipboard exchange. This backend never accesses an OS clipboard.
use crate::{ErrorStage, MAX_CLIPBOARD_BYTES, SessionError};
use ironrdp::{
    cliprdr::{backend::CliprdrBackend, pdu::*},
    core::impl_as_any,
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub(crate) type SharedClipboard = Arc<Mutex<TextClipboard>>;
#[derive(Default)]
pub(crate) struct TextClipboard {
    pub ready: bool,
    pub initialize: bool,
    remote_unicode: bool,
    local: Option<String>,
    requests: Vec<bool>,
    inflight: Option<(u64, Instant, bool)>,
    pub completed: Option<(u64, Result<Arc<str>, SessionError>)>,
    pub failure: Option<SessionError>,
}
impl TextClipboard {
    pub fn offer(&mut self, text: String) -> Result<(), SessionError> {
        let text = normalize_send(&text)?;
        if !self.ready {
            return Err(failure("Remote clipboard channel is not ready"));
        }
        self.local = Some(text);
        Ok(())
    }
    pub fn request(&mut self, id: u64) -> Result<(), SessionError> {
        if !self.ready || !self.remote_unicode {
            return Err(failure("Remote clipboard does not offer Unicode text"));
        }
        if self.inflight.is_some() {
            return Err(failure(
                "A previous clipboard request is still pending; wait or reconnect",
            ));
        }
        self.inflight = Some((id, Instant::now(), false));
        Ok(())
    }
    pub fn tick(&mut self) {
        if let Some((id, since, reported)) = &mut self.inflight
            && !*reported
            && since.elapsed() > Duration::from_secs(10)
        {
            *reported = true;
            self.completed = Some((*id, Err(failure("Remote clipboard request timed out"))));
        }
    }
    pub fn replies(&mut self) -> Vec<OwnedFormatDataResponse> {
        std::mem::take(&mut self.requests)
            .into_iter()
            .map(|unicode| {
                if unicode && let Some(text) = &self.local {
                    FormatDataResponse::new_unicode_string(text)
                } else {
                    FormatDataResponse::new_error()
                }
            })
            .collect()
    }
}
fn failure(message: impl Into<String>) -> SessionError {
    SessionError::new(ErrorStage::Clipboard, message)
}
pub fn normalize_send(text: &str) -> Result<String, SessionError> {
    if text.len() > MAX_CLIPBOARD_BYTES || text.contains('\0') {
        return Err(failure(
            "Clipboard text must be at most 1 MiB and contain no NUL characters",
        ));
    }
    let text = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r\n");
    if (text.encode_utf16().count() + 1) * 2 > MAX_CLIPBOARD_BYTES {
        return Err(failure("Clipboard text exceeds 1 MiB as UTF-16"));
    }
    Ok(text)
}
fn decode(response: FormatDataResponse<'_>) -> Result<Arc<str>, SessionError> {
    if response.is_error() {
        return Err(failure("Remote clipboard text is no longer available"));
    }
    let bytes = response.data();
    if bytes.len() > MAX_CLIPBOARD_BYTES || bytes.len() % 2 != 0 {
        return Err(failure("Remote clipboard text is too large or malformed"));
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .take_while(|c| *c != 0)
        .collect();
    let text = String::from_utf16(&units)
        .map_err(|_| failure("Remote clipboard text is not valid UTF-16"))?;
    if text.len() > MAX_CLIPBOARD_BYTES {
        return Err(failure("Remote clipboard text exceeds 1 MiB"));
    }
    Ok(Arc::from(text.replace("\r\n", "\n")))
}
pub(crate) struct Backend(pub SharedClipboard);
impl std::fmt::Debug for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TextClipboardBackend")
    }
}
impl_as_any!(Backend);
impl CliprdrBackend for Backend {
    fn temporary_directory(&self) -> &str {
        ""
    }
    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        ClipboardGeneralCapabilityFlags::empty()
    }
    fn on_ready(&mut self) {
        self.0.lock().unwrap().ready = true;
    }
    fn on_request_format_list(&mut self) {
        self.0.lock().unwrap().initialize = true;
    }
    fn on_process_negotiated_capabilities(&mut self, _: ClipboardGeneralCapabilityFlags) {}
    fn on_remote_copy(&mut self, formats: &[ClipboardFormat]) {
        self.0.lock().unwrap().remote_unicode = formats
            .iter()
            .any(|f| f.id == ClipboardFormatId::CF_UNICODETEXT);
    }
    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        let mut state = self.0.lock().unwrap();
        if state.requests.len() >= 8 {
            state.failure = Some(failure("Remote clipboard request queue overflow"));
            return;
        }
        state
            .requests
            .push(request.format == ClipboardFormatId::CF_UNICODETEXT);
    }
    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        let mut state = self.0.lock().unwrap();
        if let Some((id, _, reported)) = state.inflight.take()
            && !reported
        {
            state.completed = Some((id, decode(response)));
        }
    }
    fn on_format_list_response(&mut self, ok: bool) {
        if !ok {
            self.0.lock().unwrap().failure =
                Some(failure("Remote clipboard rejected the offered text"));
        }
    }
    fn on_file_contents_request(&mut self, _: FileContentsRequest) {}
    fn on_file_contents_response(&mut self, _: FileContentsResponse<'_>) {}
    fn on_lock(&mut self, _: LockDataId) {}
    fn on_unlock(&mut self, _: LockDataId) {}
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_multiline_limits_and_stale_responses() {
        assert_eq!(normalize_send("ñ\nA\r\nB").unwrap(), "ñ\r\nA\r\nB");
        assert!(normalize_send(&"a".repeat(MAX_CLIPBOARD_BYTES)).is_err());
        assert!(normalize_send("a\0b").is_err());
        assert!(decode(FormatDataResponse::new_data(vec![1u8])).is_err());
        let shared = SharedClipboard::default();
        let mut backend = Backend(shared.clone());
        backend.on_ready();
        backend.on_remote_copy(&[ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)]);
        shared.lock().unwrap().request(41).unwrap();
        assert!(shared.lock().unwrap().request(42).is_err());
        backend.on_format_data_response(FormatDataResponse::new_unicode_string("ñ\r\n🦀"));
        let (id, result) = shared.lock().unwrap().completed.take().unwrap();
        assert_eq!(id, 41);
        assert_eq!(&*result.unwrap(), "ñ\n🦀");
        backend.on_format_data_response(FormatDataResponse::new_unicode_string("unsolicited"));
        assert!(shared.lock().unwrap().completed.is_none());
        assert!(
            !shared.lock().unwrap().initialize,
            "Receiving text must not announce local clipboard formats"
        );
    }
}
