//! Lane framing: `u32 BE length ‖ u8 kind ‖ body`. Control messages are
//! JSON ([`Msg`]); data frames are [`Block`]s. The same frames travel over a
//! P2P mux stream or a relay pipe — both are plain ordered byte streams.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::blocks::{BLOCK_SIZE, RangeSet};
use crate::manifest::Entry;

pub const PROTOCOL_VERSION: u32 = 1;
const KIND_MSG: u8 = 1;
const KIND_BLOCK: u8 = 2;
/// Control frames stay small; manifests are chunked below this.
const MAX_MSG: usize = 8 * 1024 * 1024;
const BLOCK_HEADER: usize = 4 + 8 + 32;
/// Manifest entries per `Manifest` frame.
pub const MANIFEST_CHUNK: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Lane {
    Control,
    Data,
}

/// Per-file progress a receiver reports when it accepts (resume state).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHave {
    /// Entry index in the manifest.
    pub file: u64,
    /// Verified blocks.
    pub blocks: RangeSet,
    /// Verified, digest-checked and renamed into place.
    #[serde(default)]
    pub done: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Msg {
    /// First frame on every lane.
    #[serde(rename_all = "camelCase")]
    Hello {
        v: u32,
        transfer_id: String,
        /// One per sender connection attempt; data lanes join it.
        session_id: String,
        lane: Lane,
        sender_name: String,
    },
    /// Sender → receiver, then the manifest in `Manifest` chunks.
    #[serde(rename_all = "camelCase")]
    Offer {
        entry_count: u64,
        file_count: u64,
        total_bytes: u64,
        digest: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        destination: Option<String>,
        #[serde(default)]
        skipped: u64,
    },
    Manifest {
        entries: Vec<Entry>,
    },
    ManifestEnd,
    /// Receiver: waiting for its user to accept.
    Pending,
    /// Receiver: go ahead; skip what `have` lists.
    Accept {
        have: Vec<FileHave>,
    },
    Decline {
        reason: String,
    },
    /// Sender: the whole-file SHA-256 of entry `file` (hex).
    Digest {
        file: u64,
        sha256: String,
    },
    /// Receiver: send these blocks of `file` again (`None` = all of them).
    Resend {
        file: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<u64>>,
    },
    #[serde(rename_all = "camelCase")]
    Progress {
        done_bytes: u64,
    },
    Complete,
    Cancel {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Error {
        message: String,
    },
}

/// One verified unit of file data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub file: u32,
    pub offset: u64,
    pub sha256: [u8; 32],
    pub data: Vec<u8>,
}

impl Block {
    pub fn new(file: u32, offset: u64, data: Vec<u8>) -> Self {
        let sha256 = Sha256::digest(&data).into();
        Self {
            file,
            offset,
            sha256,
            data,
        }
    }

    pub fn verify(&self) -> bool {
        <[u8; 32]>::from(Sha256::digest(&self.data)) == self.sha256
    }
}

pub enum Frame {
    Msg(Msg),
    Block(Block),
}

pub async fn write_msg<W: AsyncWrite + Unpin>(io: &mut W, msg: &Msg) -> anyhow::Result<()> {
    let body = serde_json::to_vec(msg)?;
    anyhow::ensure!(body.len() < MAX_MSG, "control message too large");
    io.write_u32((body.len() + 1) as u32).await?;
    io.write_u8(KIND_MSG).await?;
    io.write_all(&body).await?;
    io.flush().await?;
    Ok(())
}

pub async fn write_block<W: AsyncWrite + Unpin>(io: &mut W, block: &Block) -> anyhow::Result<()> {
    let length = 1 + BLOCK_HEADER + block.data.len();
    io.write_u32(length as u32).await?;
    io.write_u8(KIND_BLOCK).await?;
    let mut header = [0u8; BLOCK_HEADER];
    header[..4].copy_from_slice(&block.file.to_be_bytes());
    header[4..12].copy_from_slice(&block.offset.to_be_bytes());
    header[12..].copy_from_slice(&block.sha256);
    io.write_all(&header).await?;
    io.write_all(&block.data).await?;
    Ok(())
}

/// `None` on a clean end of stream between frames.
pub async fn read_frame<R: AsyncRead + Unpin>(io: &mut R) -> anyhow::Result<Option<Frame>> {
    let length = match io.read_u32().await {
        Ok(length) => length as usize,
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    anyhow::ensure!(length >= 1, "empty transfer frame");
    let kind = io.read_u8().await?;
    match kind {
        KIND_MSG => {
            anyhow::ensure!(length <= MAX_MSG, "oversized control frame");
            let mut body = vec![0; length - 1];
            io.read_exact(&mut body).await?;
            Ok(Some(Frame::Msg(serde_json::from_slice(&body)?)))
        }
        KIND_BLOCK => {
            anyhow::ensure!(
                length > 1 + BLOCK_HEADER && length <= 1 + BLOCK_HEADER + BLOCK_SIZE as usize,
                "invalid block frame"
            );
            let mut header = [0u8; BLOCK_HEADER];
            io.read_exact(&mut header).await?;
            let mut data = vec![0; length - 1 - BLOCK_HEADER];
            io.read_exact(&mut data).await?;
            Ok(Some(Frame::Block(Block {
                file: u32::from_be_bytes(header[..4].try_into()?),
                offset: u64::from_be_bytes(header[4..12].try_into()?),
                sha256: header[12..].try_into()?,
                data,
            })))
        }
        other => anyhow::bail!("unknown transfer frame kind {other}"),
    }
}

/// The next frame, which must be a control message.
pub async fn read_msg<R: AsyncRead + Unpin>(io: &mut R) -> anyhow::Result<Msg> {
    match read_frame(io).await? {
        Some(Frame::Msg(msg)) => Ok(msg),
        Some(Frame::Block(_)) => anyhow::bail!("unexpected data on the control lane"),
        None => anyhow::bail!("the other device closed the transfer"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_round_trip_and_blocks_verify() {
        let (mut a, mut b) = tokio::io::duplex(4 * 1024 * 1024);
        let msg = Msg::Hello {
            v: PROTOCOL_VERSION,
            transfer_id: "t".into(),
            session_id: "s".into(),
            lane: Lane::Data,
            sender_name: "Laptop".into(),
        };
        write_msg(&mut a, &msg).await.unwrap();
        let block = Block::new(3, BLOCK_SIZE * 2, vec![9; BLOCK_SIZE as usize]);
        write_block(&mut a, &block).await.unwrap();
        drop(a);
        match read_frame(&mut b).await.unwrap() {
            Some(Frame::Msg(got)) => assert_eq!(got, msg),
            _ => panic!("expected a message"),
        }
        match read_frame(&mut b).await.unwrap() {
            Some(Frame::Block(got)) => {
                assert!(got.verify());
                assert_eq!(got, block);
                let mut tampered = got.clone();
                tampered.data[17] ^= 1;
                assert!(!tampered.verify(), "a flipped bit fails the block hash");
            }
            _ => panic!("expected a block"),
        }
        assert!(read_frame(&mut b).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn oversized_and_unknown_frames_are_rejected() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        a.write_u32(u32::MAX).await.unwrap();
        a.write_u8(KIND_BLOCK).await.unwrap();
        assert!(read_frame(&mut b).await.is_err());
        let (mut a, mut b) = tokio::io::duplex(1024);
        a.write_u32(2).await.unwrap();
        a.write_u8(9).await.unwrap();
        a.write_u8(0).await.unwrap();
        assert!(read_frame(&mut b).await.is_err());
    }
}
