//! A viewer that answers terminal queries itself (zgx's terminal, through
//! `zeron-zgx terminal-bridge`) writes each answer as its own WriteTerminal
//! call, in the order the program asked. The program must read them in that
//! order: termenv (gh, glamour, lipgloss) asks for the background (OSC 11)
//! and then the cursor (CSI 6n), and takes a cursor report first as "no
//! OSC 11": it stops reading, and the colour answer that follows is read as
//! typed keys (gh's prompt: "unexpected escape sequence from terminal:
//! ['\x1b' ']']", and `;22R` left in the shell).
//!
//! Over the engine's own RPC dispatch (the memory transport is the same loop
//! as the WebSocket's), a program in the PTY asks as termenv does, a viewer
//! answers as Ghostty does (two writes, back to back, the moment it sees the
//! query), and the program records which answer came first and how long each
//! took. (A Perl script, as Perl is on every Unix runner.)
#![cfg(unix)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};
use zeron_engine::EngineCore;
use zeron_rpc::methods;

const ROUNDS: usize = 300;
const QUERY: &[u8] = b"\x1b]11;?\x1b\\\x1b[6n";
const COLOR: &[u8] = b"\x1b]11;rgb:1e1e/1e1e/1e1e\x1b\\";
const CURSOR: &[u8] = b"\x1b[22;50R";

/// termenv's query, `ROUNDS` times: each answer read a byte at a time, the
/// first required to be the colour's (as `termStatusReport`), each timed.
const PROBE: &str = r#"
use strict;
use warnings;
use Time::HiRes qw(time sleep);
system("stty raw -echo") == 0 or die "stty";
sub byte {
    my ($deadline) = @_;
    my $left = $deadline - time();
    return undef if $left <= 0;
    my $rin = '';
    vec($rin, fileno(STDIN), 1) = 1;
    return undef unless select(my $rout = $rin, undef, undef, $left) > 0;
    my $b;
    return undef unless sysread(STDIN, $b, 1) == 1;
    return $b;
}
sub answer {
    my ($deadline) = @_;
    while (1) {
        my $b = byte($deadline);
        return undef unless defined $b;
        last if $b eq "\e";
    }
    my $kind = byte($deadline);
    return undef unless defined $kind;
    my $got = "\e$kind";
    while (length($got) < 64) {
        my $b = byte($deadline);
        return undef unless defined $b;
        $got .= $b;
        return $got if $kind eq ']' && ($b eq "\a" || substr($got, -2) eq "\e\\");
        return $got if $kind eq '[' && $b eq 'R';
    }
    return $got;
}
my ($ordered, $misordered, $lost) = (0, 0, 0);
my (@color, @cursor);
for (1 .. $ARGV[0]) {
    my $start = time();
    syswrite(STDOUT, "\e]11;?\e\\\e[6n");
    my $deadline = $start + 2;
    my $first = answer($deadline);
    my $t1 = time();
    my $second = defined $first ? answer($deadline) : undef;
    my $t2 = time();
    if (!defined $first || !defined $second) {
        $lost++;
    } elsif (substr($first, 0, 2) eq "\e]") {
        $ordered++; push @color, ($t1 - $start) * 1e3; push @cursor, ($t2 - $start) * 1e3;
    } else {
        $misordered++; push @cursor, ($t1 - $start) * 1e3; push @color, ($t2 - $start) * 1e3;
    }
    sleep(0.002);
}
system("stty sane");
sub pct { my ($q, @x) = @_; return -1 unless @x; @x = sort { $a <=> $b } @x; my $i = int($q * @x); $i = $#x if $i > $#x; return $x[$i]; }
sub most { my @x = @_; return -1 unless @x; my $m = $x[0]; for (@x) { $m = $_ if $_ > $m } return $m; }
printf "\r\nPROBE ordered=%d misordered=%d lost=%d color_p50=%.2f color_p99=%.2f color_max=%.2f cursor_p50=%.2f cursor_max=%.2f\r\n",
    $ordered, $misordered, $lost, pct(.5, @color), pct(.99, @color), most(@color), pct(.5, @cursor), most(@cursor);
"#;

fn frame(id: u64, method: &str, params: Value) -> String {
    json!({ "id": id, "method": method, "params": params }).to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_self_answering_viewer_s_answers_reach_the_program_in_order() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).expect("data dir");
    let core = EngineCore::assemble(
        &data,
        Arc::new(zeron_engine::HarnessRegistry::new()),
        zeron_proto::HarnessId::Mock,
        None,
    )
    .expect("engine assembles");
    let probe = tmp.path().join("probe.pl");
    std::fs::write(&probe, PROBE).expect("probe");
    let session = core
        .terminals
        .open_with_shell(&tmp.path().to_string_lossy(), 80, 24, Some("/bin/sh"))
        .expect("open sh");
    // Closed however the test ends: its reader would keep the test running.
    struct Close<'a>(&'a zeron_engine::Terminals, String);
    impl Drop for Close<'_> {
        fn drop(&mut self) {
            let _ = self.0.close(&self.1);
        }
    }
    let _close = Close(&core.terminals, session.id.clone());

    // The bridge's connection: raw frames, as it sends them.
    let (to_server, server_in) = tokio::sync::mpsc::channel::<String>(1024);
    let (server_out, mut from_server) = tokio::sync::mpsc::channel::<String>(1024);
    tokio::spawn(zeron_rpc::serve_connection(
        core.rpc_service(),
        server_out,
        server_in,
    ));
    to_server
        .send(frame(
            1,
            methods::SUBSCRIBE_TERMINAL,
            json!({ "terminalId": session.id, "hostReplies": false }),
        ))
        .await
        .unwrap();
    let mut next = 2u64;
    let write = |next: &mut u64, bytes: &[u8]| {
        let line = frame(
            *next,
            methods::WRITE_TERMINAL,
            json!({ "terminalId": session.id, "data": BASE64.encode(bytes) }),
        );
        *next += 1;
        line
    };
    let command = format!("perl {} {ROUNDS}\n", probe.display());
    to_server
        .send(write(&mut next, command.as_bytes()))
        .await
        .unwrap();

    // The viewer: each query answered as it's seen, as two writes.
    let mut output: Vec<u8> = Vec::new();
    let mut scanned = 0usize;
    let deadline = Instant::now() + Duration::from_secs(120);
    let summary = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let line = tokio::time::timeout(left, from_server.recv())
            .await
            .expect("the probe finishes in time")
            .expect("the connection stays up");
        for line in line.lines().filter(|l| !l.trim().is_empty()) {
            let frame: Value = serde_json::from_str(line).expect("a frame");
            if frame["id"].as_u64() != Some(1) || frame["item"]["type"] != "data" {
                continue;
            }
            let bytes = BASE64
                .decode(frame["item"]["data"].as_str().unwrap())
                .unwrap();
            output.extend_from_slice(&bytes);
        }
        while let Some(at) = output[scanned..]
            .windows(QUERY.len())
            .position(|w| w == QUERY)
        {
            scanned += at + QUERY.len();
            to_server.send(write(&mut next, COLOR)).await.unwrap();
            to_server.send(write(&mut next, CURSOR)).await.unwrap();
        }
        let text = String::from_utf8_lossy(&output);
        if let Some(at) = text.find("PROBE ordered=")
            && let Some(end) = text[at..].find('\n')
        {
            break text[at..at + end].trim().to_owned();
        }
    };
    eprintln!("{summary}");
    let field = |name: &str| -> usize {
        summary
            .split_whitespace()
            .find_map(|kv| kv.strip_prefix(&format!("{name}=")))
            .and_then(|v| v.parse().ok())
            .unwrap_or(usize::MAX)
    };
    assert_eq!(field("lost"), 0, "every query answered: {summary}");
    assert_eq!(
        field("misordered"),
        0,
        "the colour answer read before the cursor's every time: {summary}"
    );
    assert_eq!(field("ordered"), ROUNDS, "{summary}");
}
