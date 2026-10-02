//! Opt-in interoperability probe. Credentials come from environment, never argv.
use std::time::{Duration, Instant};
use zeron_rdp::*;
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("zeron_rdp=debug,ironrdp_displaycontrol=debug")
        .with_writer(std::io::stderr)
        .init();
    let cycles = std::env::var("ZERON_RDP_PROBE_CYCLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    for generation in 1..=cycles {
        eprintln!("cycle={generation}/{cycles}");
        run(generation).await?;
    }
    Ok(())
}
async fn run(generation: u64) -> Result<(), Box<dyn std::error::Error>> {
    let duration = Duration::from_secs(
        std::env::var("ZERON_RDP_PROBE_SECONDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(15),
    );
    let host = std::env::var("ZERON_RDP_HOST")?;
    let port = std::env::var("ZERON_RDP_PORT")
        .unwrap_or_else(|_| "3389".into())
        .parse()?;
    let username = std::env::var("ZERON_RDP_USER")?;
    let password = Password::new(std::env::var("ZERON_RDP_PASSWORD")?);
    let pin = std::env::var("ZERON_RDP_CERT_SHA256").ok();
    let mut session = connect(
        ConnectConfig {
            host,
            port,
            username,
            keyboard_layout: std::env::var("ZERON_RDP_KEYBOARD_LAYOUT")
                .ok()
                .and_then(|s| u32::from_str_radix(&s, 16).ok())
                .unwrap_or(0x409),
            password,
            domain: None,
            width: std::env::var("ZERON_RDP_WIDTH")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1280),
            height: std::env::var("ZERON_RDP_HEIGHT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(800),
            trusted_certificate_sha256: pin,
            timeout: CONNECT_TIMEOUT,
        },
        generation,
    )?;
    let start = Instant::now();
    let mut frames = 0;
    let mut last = 0;
    let mut prior = SessionState::Idle;
    let resize_enabled = std::env::var("ZERON_RDP_PROBE_RESIZE").as_deref() != Ok("off");
    let mut resized = false;
    let mut dimensions = (0, 0);
    let lab = std::env::var("ZERON_RDP_LAB_INPUT").as_deref() == Ok("1");
    let mut input_sent = false;
    let mut input_sent_at: Option<Instant> = None;
    let mut copy_requested = false;
    let mut local_sent = false;
    let mut pasted = false;
    let mut remote_received = false;
    let background = std::env::var("ZERON_RDP_PROBE_BACKGROUND").as_deref() == Ok("1");
    let mut visible = true;
    let mut hidden_since = None;
    let mut hidden_sequence = None;
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    loop {
        let snapshot = session.snapshots.borrow_and_update().clone();
        if snapshot.state != prior {
            eprintln!("state={:?}", snapshot.state);
            prior = snapshot.state.clone();
        }
        if let Some(challenge) = snapshot.certificate {
            if std::env::var("ZERON_RDP_LAB_TRUST").as_deref() == Ok("1") {
                session.send(Command::Certificate(CertificateDecision::TrustOnce))?;
            } else {
                eprintln!("Untrusted certificate SHA-256: {}", challenge.sha256);
                return Err("Set the expected certificate pin before connecting".into());
            }
        }
        if let Some(frame) = snapshot.frame
            && frame.sequence != last
        {
            last = frame.sequence;
            frames += 1;
            if dimensions != (frame.width, frame.height) {
                dimensions = (frame.width, frame.height);
                eprintln!(
                    "frame={}x{} bytes={} elapsed={:?}",
                    frame.width,
                    frame.height,
                    frame.bgra.len(),
                    start.elapsed()
                );
            }
        }
        let elapsed = start.elapsed();
        if background {
            let wanted = elapsed < duration / 3 || elapsed > duration * 2 / 3;
            if wanted != visible {
                visible = wanted;
                session.set_visible(visible);
                hidden_since = (!visible).then_some(Instant::now());
                hidden_sequence = None;
                eprintln!("visible={visible}");
            }
            if hidden_since.is_some_and(|t| t.elapsed() > Duration::from_millis(500)) {
                if let Some(previous) = hidden_sequence {
                    if previous != last {
                        return Err("Hidden session continued publishing frames".into());
                    }
                }
                hidden_sequence = Some(last);
            }
        }
        if resize_enabled
            && snapshot.capabilities.resize
            && frames > 0
            && elapsed > Duration::from_secs(2)
            && !resized
        {
            resized = true;
            session.resize(1024, 768)?;
        }
        if lab
            && frames > 0
            && !snapshot.reactivating
            && (!resize_enabled || dimensions == (1024, 768))
            && elapsed > Duration::from_millis(4500)
            && !input_sent
        {
            input_sent = true;
            input_sent_at = Some(Instant::now());
            type_command(
                &session,
                "printf 'niño con acento á\\n' > /tmp/zeron-rdp-input.txt; printf 'remote ñ\\n' | xclip -selection clipboard",
            ).await?;
        }
        if lab
            && snapshot.capabilities.clipboard_text
            && input_sent_at.is_some_and(|t| t.elapsed() > Duration::from_secs(2))
            && !copy_requested
        {
            copy_requested = true;
            session.send(Command::RequestClipboard(1))?;
        }
        if let Some((1, result)) = snapshot.clipboard {
            let text = result?;
            if &*text != "remote ñ\n" {
                return Err("Remote clipboard text did not match fixture".into());
            }
            if !remote_received {
                remote_received = true;
                eprintln!("remote-unicode-clipboard=ok");
            }
        }
        if lab
            && snapshot.capabilities.clipboard_text
            && input_sent_at.is_some_and(|t| t.elapsed() > Duration::from_secs(3))
            && !local_sent
        {
            local_sent = true;
            session.send(Command::SendClipboard("local ñ\nline two".into()))?;
        }
        if lab && input_sent_at.is_some_and(|t| t.elapsed() > Duration::from_secs(4)) && !pasted {
            pasted = true;
            type_command(
                &session,
                "xclip -selection clipboard -o > /tmp/zeron-rdp-clipboard.txt",
            )
            .await?;
        }
        if matches!(
            snapshot.state,
            SessionState::Failed(_) | SessionState::Disconnected
        ) {
            return Err("Session ended before probe completed".into());
        }
        if elapsed > duration {
            if frames == 0 {
                return Err("No desktop frames received".into());
            }
            if resize_enabled && snapshot.capabilities.resize && dimensions != (1024, 768) {
                return Err("Display Control did not produce the confirmed target size".into());
            }
            if lab && !remote_received {
                return Err("Remote clipboard did not complete".into());
            }
            eprintln!(
                "frames={frames}, clipboard={}, dimensions={dimensions:?}",
                snapshot.capabilities.clipboard_text
            );
            session.disconnect();
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    if session.snapshots.borrow_and_update().state == SessionState::Disconnected {
                        break;
                    }
                    session.snapshots.changed().await?;
                }
                Ok::<_, Box<dyn std::error::Error>>(())
            })
            .await??;
            break;
        }
        tokio::select! {_=tick.tick()=>{},result=session.snapshots.changed()=>{result?;}}
    }
    Ok(())
}
async fn type_command(session: &SessionHandle, text: &str) -> Result<(), SessionError> {
    // Resizing can clear the server window manager's active window. Focus the
    // lab terminal exactly as the interactive user does before typing.
    for down in [true, false] {
        session.send(Command::Input(InputEvent::Button {
            button: PointerButton::Left,
            down,
            x: 100,
            y: 100,
        }))?;
    }
    // The window manager handles focus asynchronously; allow the click to be
    // processed before sending keyboard events to the newly focused terminal.
    tokio::time::sleep(Duration::from_millis(200)).await;
    session.send(Command::Input(InputEvent::Text(text.into())))?;
    session.send(Command::Input(InputEvent::ScanCode {
        code: 28,
        down: true,
    }))?;
    session.send(Command::Input(InputEvent::ScanCode {
        code: 28,
        down: false,
    }))
}
