//! Best-effort, nonblocking Unix datagrams. All timestamps are monotonic ns.
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct McpServer {
    pub name: String,
    pub status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum ProbeEvent {
    Spawn {
        prog: String,
        argv: Vec<String>,
        ppid: u32,
        t: u64,
    },
    Init {
        session: String,
        resumed: bool,
        t: u64,
    },
    Mark {
        chat: String,
        seq: u64,
        t: u64,
    },
    Exec {
        argv: Vec<String>,
        t0: u64,
        t1: u64,
        status: i32,
        makeflags_present: bool,
    },
    McpReady {
        t_spawn: u64,
        t_init: u64,
        t_ready: u64,
        servers: Vec<McpServer>,
        fallback: bool,
    },
}

pub struct ProbeSock {
    socket: UnixDatagram,
    path: PathBuf,
}
impl ProbeSock {
    /// Refuses to replace an existing socket (another trial may own it).
    pub fn bind(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_owned();
        let socket = UnixDatagram::bind(&path)?;
        Ok(Self { socket, path })
    }
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> Result<()> {
        Ok(self.socket.set_read_timeout(timeout)?)
    }
    pub fn set_nonblocking(&self, nonblocking: bool) -> Result<()> {
        Ok(self.socket.set_nonblocking(nonblocking)?)
    }
    pub fn recv(&self) -> Result<ProbeEvent> {
        let mut bytes = [0; 65_536];
        let len = self.socket.recv(&mut bytes)?;
        Ok(serde_json::from_slice(&bytes[..len])?)
    }
}
impl Drop for ProbeSock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub struct ProbeSender(Option<UnixDatagram>);
impl ProbeSender {
    pub fn from_env() -> Self {
        Self(std::env::var_os("ZB_PROBE_SOCK").and_then(|path| {
            let socket = UnixDatagram::unbound().ok()?;
            socket.set_nonblocking(true).ok()?;
            socket.connect(path).ok()?;
            Some(socket)
        }))
    }
    /// A full receive queue must never stall the workload being measured.
    pub fn send(&self, event: &ProbeEvent) {
        if let (Some(socket), Ok(bytes)) = (&self.0, serde_json::to_vec(event)) {
            let _ = socket.send(&bytes);
        }
    }
}
