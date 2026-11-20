//! A `.zeron-plugin` file sent here from another of your devices
//! (`StagePluginPackage`), piece by piece, then installed by `InstallPlugin
//! {upload}` as a file on this device would be: verified, then waiting for
//! a consent window like any other.
//!
//! The file is on the sending device, so this one can't read it by path;
//! and a request crosses the relay as one message (replies come in pieces,
//! requests don't), so a package travels as pieces of at most [`PIECE`].
//! An upload is good only to whoever started it (`reviews::Origin`), for
//! [`TTL`], up to [`package::MAX_PACKAGE_BYTES`].

use std::collections::HashMap;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use zeron_rpc::RpcError;

use super::package;
use super::reviews::Origin;

/// How long an upload waits for its next piece or its install.
pub const TTL: Duration = Duration::from_secs(10 * 60);
/// The most one piece may carry (decoded): well under the relay's limits.
pub const PIECE: usize = 384 << 10;
/// At most this many outstanding at once; the oldest goes first.
const MAX: usize = 8;

struct Upload {
    origin: Origin,
    bytes: Vec<u8>,
    at: Instant,
}

#[derive(Default)]
pub struct Uploads {
    open: HashMap<String, Upload>,
}

fn failed(message: impl Into<String>) -> RpcError {
    RpcError::Failed(message.into())
}

impl Uploads {
    /// Adds `data` (base64, at most [`PIECE`] decoded) to `upload`, or to a
    /// new one when none is named: its id, and how many bytes it holds now.
    pub fn stage(&mut self, origin: Origin, upload: Option<&str>, data: &str) -> Result<(String, usize), RpcError> {
        self.prune();
        let piece = B64.decode(data.trim()).map_err(|_| failed("That piece of the plugin file didn't arrive whole; try again"))?;
        if piece.len() > PIECE {
            return Err(failed(format!("A piece of a plugin file is at most {PIECE} bytes")));
        }
        let id = match upload.filter(|u| !u.is_empty()) {
            Some(id) => {
                let up = self
                    .open
                    .get(id)
                    .filter(|up| up.origin == origin)
                    .ok_or_else(|| failed("That upload is gone (it waited too long); install the file again"))?;
                if (up.bytes.len() + piece.len()) as u64 > package::MAX_PACKAGE_BYTES {
                    self.open.remove(id);
                    return Err(failed("That plugin file is too big"));
                }
                id.to_owned()
            }
            None => {
                while self.open.len() >= MAX {
                    let Some(oldest) = self.open.iter().min_by_key(|(_, u)| u.at).map(|(id, _)| id.clone()) else {
                        break;
                    };
                    self.open.remove(&oldest);
                }
                let id = zeron_rpc::ipc_auth::generate_token().map_err(|e| failed(format!("Couldn't take the file: {e}")))?;
                self.open.insert(id.clone(), Upload { origin, bytes: Vec::new(), at: Instant::now() });
                id
            }
        };
        let up = self.open.get_mut(&id).expect("staged above");
        up.bytes.extend_from_slice(&piece);
        up.at = Instant::now();
        Ok((id, up.bytes.len()))
    }

    /// The whole file of `upload`, taken (only by whoever sent it).
    pub fn take(&mut self, origin: &Origin, upload: &str) -> Result<Vec<u8>, RpcError> {
        self.prune();
        match self.open.remove(upload) {
            Some(up) if up.origin == *origin => Ok(up.bytes),
            _ => Err(failed("That upload is gone (it waited too long); install the file again")),
        }
    }

    fn prune(&mut self) {
        self.open.retain(|_, u| u.at.elapsed() < TTL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_add_up_for_whoever_sent_them() {
        let mut uploads = Uploads::default();
        let laptop = Origin::Peer("laptop".into());
        let (id, held) = uploads.stage(laptop.clone(), None, &B64.encode(b"hello ")).unwrap();
        assert_eq!(held, 6);
        // Another device can't add to it or take it.
        assert!(uploads.stage(Origin::Peer("desk".into()), Some(&id), &B64.encode(b"x")).is_err());
        let (_, held) = uploads.stage(laptop.clone(), Some(&id), &B64.encode(b"world")).unwrap();
        assert_eq!(held, 11);
        assert!(uploads.take(&Origin::Local, &id).is_err(), "someone else's, and now gone");
        let (id, _) = uploads.stage(laptop.clone(), None, &B64.encode(b"again")).unwrap();
        assert_eq!(uploads.take(&laptop, &id).unwrap(), b"again");
        assert!(uploads.take(&laptop, &id).is_err(), "taken once");
    }

    #[test]
    fn a_piece_is_bounded() {
        let mut uploads = Uploads::default();
        assert!(uploads.stage(Origin::Local, None, &B64.encode(vec![0u8; PIECE + 1])).is_err());
        assert!(uploads.stage(Origin::Local, None, "not base64!").is_err());
        assert!(uploads.stage(Origin::Local, None, &B64.encode(vec![0u8; PIECE])).is_ok());
    }
}
