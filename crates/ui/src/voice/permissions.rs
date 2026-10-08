//! Ask for microphone permission in the viewport's macOS process identity
//! before its engine opens native devices. CoreAudio can otherwise yield silence
//! without displaying a privacy prompt.
use zeron_proto::voice::VoiceRejection;

pub(super) async fn microphone() -> Result<(), VoiceRejection> {
    #[cfg(target_os = "macos")]
    if let Some(reply) = macos::request()? {
        if !reply.await.map_err(|_| VoiceRejection::DeviceUnavailable)? {
            return Err(VoiceRejection::MicrophonePermissionDenied);
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use block2::RcBlock;
    use objc2::{
        msg_send,
        runtime::{AnyClass, AnyObject, Bool},
    };
    use objc2_foundation::{NSBundle, ns_string};
    use std::sync::Mutex;
    use tokio::sync::oneshot;

    #[link(name = "AVFoundation", kind = "framework")]
    unsafe extern "C" {
        static AVMediaTypeAudio: *const AnyObject;
    }

    // Keep Objective-C objects and the block out of the async future. AVFoundation
    // copies the completion block and may invoke it on any queue.
    pub(super) fn request() -> Result<Option<oneshot::Receiver<bool>>, VoiceRejection> {
        let device = AnyClass::get(c"AVCaptureDevice").ok_or(VoiceRejection::DeviceUnavailable)?;
        let status: isize =
            unsafe { msg_send![device, authorizationStatusForMediaType: AVMediaTypeAudio] };
        match status {
            3 => return Ok(None), // authorized
            1 | 2 => return Err(VoiceRejection::MicrophonePermissionDenied),
            0 => {} // not determined
            _ => return Err(VoiceRejection::DeviceUnavailable),
        }
        // Requesting capture permission without a purpose string terminates the
        // process. Both app bundles and direct Cargo builds supply this string.
        if NSBundle::mainBundle()
            .objectForInfoDictionaryKey(ns_string!("NSMicrophoneUsageDescription"))
            .is_none()
        {
            return Err(VoiceRejection::MicrophoneMetadataMissing);
        }
        let (tx, rx) = oneshot::channel();
        let reply = Mutex::new(Some(tx));
        let completion = RcBlock::new(move |granted: Bool| {
            if let Some(tx) = reply.lock().unwrap().take() {
                let _ = tx.send(granted.as_bool());
            }
        });
        unsafe {
            let _: () = msg_send![device,
                requestAccessForMediaType: AVMediaTypeAudio,
                completionHandler: &*completion
            ];
        }
        Ok(Some(rx))
    }
}
